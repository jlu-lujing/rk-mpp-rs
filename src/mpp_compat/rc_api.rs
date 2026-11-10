use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::mem::size_of;
use std::ptr;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::encoder::{EncoderConfig, RateControlMode};

use super::{
    MPP_ERR_NULL_PTR, MPP_NOK, MPP_OK, MPP_VIDEO_CODING_AVC, MPP_VIDEO_CODING_HEVC,
    MPP_VIDEO_CODING_MJPEG, MPP_VIDEO_CODING_VP8,
};

type RcInitFn = unsafe extern "C" fn(*mut c_void, *mut RcCfg) -> c_int;
type RcDeinitFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type RcTaskFn = unsafe extern "C" fn(*mut c_void, *mut EncRcTask) -> c_int;

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct RcImplApi {
    pub name: *const c_char,
    pub coding: c_int,
    pub ctx_size: u32,
    pub init: Option<RcInitFn>,
    pub deinit: Option<RcDeinitFn>,
    pub check_drop: Option<RcTaskFn>,
    pub check_reenc: Option<RcTaskFn>,
    pub frame_start: Option<RcTaskFn>,
    pub frame_end: Option<RcTaskFn>,
    pub hal_start: Option<RcTaskFn>,
    pub hal_end: Option<RcTaskFn>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct RcFpsCfg {
    pub fps_in_flex: i32,
    pub fps_in_num: i32,
    pub fps_in_denom: i32,
    pub fps_out_flex: i32,
    pub fps_out_num: i32,
    pub fps_out_denom: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct RcSuperframeCfg {
    pub super_mode: i32,
    pub super_i_thd: u32,
    pub super_p_thd: u32,
    pub rc_priority: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct RcDebreathCfg {
    pub enable: u32,
    pub strength: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct RcHierQpCfg {
    pub hier_qp_en: i32,
    pub hier_qp_delta: [i32; 4],
    pub hier_frame_num: [i32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct RcCfg {
    pub width: i32,
    pub height: i32,
    pub mode: i32,
    pub fps: RcFpsCfg,
    pub gop_mode: i32,
    pub igop: i32,
    pub vgop: i32,
    pub bps_min: i32,
    pub bps_target: i32,
    pub bps_max: i32,
    pub stats_time: i32,
    pub max_i_bit_prop: i32,
    pub min_i_bit_prop: i32,
    pub init_ip_ratio: i32,
    pub layer_bit_prop: [i32; 4],
    pub init_quality: i32,
    pub max_quality: i32,
    pub min_quality: i32,
    pub max_i_quality: i32,
    pub min_i_quality: i32,
    pub i_quality_delta: i32,
    pub vi_quality_delta: i32,
    pub fqp_min_i: i32,
    pub fqp_min_p: i32,
    pub fqp_max_i: i32,
    pub fqp_max_p: i32,
    pub layer_quality_delta: [i32; 4],
    pub max_reencode_times: i32,
    pub min_still_prop: i32,
    pub max_still_quality: i32,
    pub vbr_hi_prop: i32,
    pub vbr_lo_prop: i32,
    pub drop_mode: i32,
    pub drop_thd: u32,
    pub drop_gap: u32,
    pub super_cfg: RcSuperframeCfg,
    pub debreath_cfg: RcDebreathCfg,
    pub hier_qp_cfg: RcHierQpCfg,
    pub refresh_len: u32,
    pub scene_mode: i32,
    pub fps_chg_prop: u32,
}

impl RcCfg {
    pub(super) fn from_encoder(config: EncoderConfig) -> Self {
        let (fps_num, fps_den) = config.fps();
        let rate_control = config.rate_control();
        let mode = rate_control.map_or(2, |rate| match rate.mode() {
            RateControlMode::Vbr => 0,
            RateControlMode::Cbr => 1,
            RateControlMode::FixedQp => 2,
            RateControlMode::Avbr => 3,
        });
        let default_target =
            (config.width() * config.height() / 8 * fps_num as usize / fps_den as usize) as i32;
        let bps_target = rate_control.map_or(default_target, |rate| rate.bps_target() as i32);
        let bps_min = rate_control.map_or(0, |rate| rate.bps_min() as i32);
        let bps_max = rate_control.map_or(0, |rate| rate.bps_max() as i32);
        let init_quality = rate_control.map_or(config.qp() as i32, |rate| rate.initial_qp() as i32);
        let min_quality = rate_control.map_or(config.qp() as i32, |rate| rate.min_qp() as i32);
        let max_quality = rate_control.map_or(config.qp() as i32, |rate| rate.max_qp() as i32);
        Self {
            width: config.width() as i32,
            height: config.height() as i32,
            mode,
            fps: RcFpsCfg {
                fps_in_num: fps_num as i32,
                fps_in_denom: fps_den as i32,
                fps_out_num: fps_num as i32,
                fps_out_denom: fps_den as i32,
                ..RcFpsCfg::default()
            },
            igop: config.gop() as i32,
            bps_min,
            bps_target,
            bps_max,
            stats_time: 1,
            init_quality,
            max_quality,
            min_quality,
            max_i_quality: max_quality,
            min_i_quality: min_quality,
            fqp_min_i: min_quality,
            fqp_min_p: min_quality,
            fqp_max_i: max_quality,
            fqp_max_p: max_quality,
            ..Self::default()
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct EncFrmStatus {
    pub val: u64,
}

impl EncFrmStatus {
    fn set_bit(&mut self, bit: u32, enabled: bool) {
        if enabled {
            self.val |= 1u64 << bit;
        } else {
            self.val &= !(1u64 << bit);
        }
    }

    pub(super) fn is_intra(self) -> bool {
        self.val & (1 << 4) != 0
    }

    pub(super) fn is_idr(self) -> bool {
        self.val & (1 << 5) != 0
    }

    pub(super) fn is_drop(self) -> bool {
        self.val & (1u64 << 32) != 0
    }

    pub(super) fn is_reencode(self) -> bool {
        self.val & (1u64 << 34) != 0
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct EncCpbStatus {
    pub seq_idx: i32,
    pub curr: EncFrmStatus,
    pub refr: EncFrmStatus,
    pub init: [EncFrmStatus; 8],
    pub final_: [EncFrmStatus; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct EncRcTaskInfo {
    pub frame_type: i32,
    pub bit_target: i32,
    pub bit_max: i32,
    pub bit_min: i32,
    pub quality_target: i32,
    pub quality_max: i32,
    pub quality_min: i32,
    pub bit_real: i32,
    pub quality_real: i32,
    pub madi: i32,
    pub madp: i32,
    pub iblk4_prop: u32,
    pub sse: i64,
    pub lvl64_inter_num: u32,
    pub lvl32_inter_num: u32,
    pub lvl16_inter_num: u32,
    pub lvl8_inter_num: u32,
    pub lvl32_intra_num: u32,
    pub lvl16_intra_num: u32,
    pub lvl8_intra_num: u32,
    pub lvl4_intra_num: u32,
    pub reserve: [i32; 5],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct EncRcForceCfg {
    pub force_flag: u32,
    pub force_qp: i32,
    pub reserve: [u32; 6],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct EncRcTask {
    pub cpb: EncCpbStatus,
    pub frm: EncFrmStatus,
    pub info: EncRcTaskInfo,
    pub force: EncRcForceCfg,
    pub frame: *mut c_void,
}

impl EncRcTask {
    pub(super) fn new(sequence: u32, intra: bool, idr: bool) -> Self {
        let mut task = Self::default();
        task.cpb.seq_idx = sequence as i32;
        task.frm.set_bit(0, true);
        task.frm.set_bit(4, intra);
        task.frm.set_bit(5, idr);
        task.frm.val |= (sequence as u64 & 0xffff) << 48;
        task.cpb.curr = task.frm;
        task.info.frame_type = if intra { 2 } else { 0 };
        task
    }

    pub(super) fn rc_target(&self) -> (i32, i32, i32, i32) {
        if self.force.force_flag & 1 != 0 {
            let qp = self.force.force_qp.clamp(0, 51);
            (self.info.bit_target.max(0), qp, qp, qp)
        } else {
            (
                self.info.bit_target.max(0),
                self.info.quality_target.clamp(0, 51),
                self.info.quality_min.clamp(0, 51),
                self.info.quality_max.clamp(0, 51),
            )
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(super) struct RcApiBrief {
    pub name: *const c_char,
    pub coding: c_int,
}

#[repr(C)]
pub(super) struct RcApiQueryAll {
    pub brief: *mut RcApiBrief,
    pub max_count: c_int,
    pub count: c_int,
}

#[repr(C)]
pub(super) struct RcApiQueryType {
    pub brief: *mut RcApiBrief,
    pub max_count: c_int,
    pub coding: c_int,
    pub count: c_int,
}

struct Registration {
    name: CString,
    coding: c_int,
    callbacks: RcCallbacks,
}

#[derive(Clone, Copy)]
struct RcCallbacks {
    ctx_size: u32,
    init: Option<RcInitFn>,
    deinit: Option<RcDeinitFn>,
    check_drop: Option<RcTaskFn>,
    check_reenc: Option<RcTaskFn>,
    frame_start: Option<RcTaskFn>,
    frame_end: Option<RcTaskFn>,
    hal_start: Option<RcTaskFn>,
    hal_end: Option<RcTaskFn>,
}

static REGISTRY: OnceLock<Mutex<Vec<Registration>>> = OnceLock::new();

fn lock_registry() -> MutexGuard<'static, Vec<Registration>> {
    REGISTRY
        .get_or_init(|| Mutex::new(builtins()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn builtins() -> Vec<Registration> {
    [
        ("default", MPP_VIDEO_CODING_AVC),
        ("default", MPP_VIDEO_CODING_HEVC),
        ("default", MPP_VIDEO_CODING_MJPEG),
        ("default", MPP_VIDEO_CODING_VP8),
        ("smart", MPP_VIDEO_CODING_AVC),
        ("smart", MPP_VIDEO_CODING_HEVC),
    ]
    .into_iter()
    .map(|(name, coding)| {
        new_registration(
            CString::new(name).expect("built-in RC API names contain no NUL"),
            coding,
            None,
        )
    })
    .collect()
}

fn new_registration(name: CString, coding: c_int, api: Option<&RcImplApi>) -> Registration {
    let callbacks = api.map_or(
        RcCallbacks {
            ctx_size: 0,
            init: None,
            deinit: None,
            check_drop: None,
            check_reenc: None,
            frame_start: None,
            frame_end: None,
            hal_start: None,
            hal_end: None,
        },
        |api| RcCallbacks {
            ctx_size: api.ctx_size,
            init: api.init,
            deinit: api.deinit,
            check_drop: api.check_drop,
            check_reenc: api.check_reenc,
            frame_start: api.frame_start,
            frame_end: api.frame_end,
            hal_start: api.hal_start,
            hal_end: api.hal_end,
        },
    );
    Registration {
        name,
        coding,
        callbacks,
    }
}

fn same_name(left: &CStr, right: &CStr) -> bool {
    let left = left.to_bytes();
    let right = right.to_bytes();
    let len = left.len().min(right.len()).min(31);
    left[..len] == right[..len] && (left.len().min(31) == right.len().min(31))
}

fn add_unlocked(registry: &mut Vec<Registration>, api: &RcImplApi) -> c_int {
    if api.name.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: the public ABI requires a NUL-terminated strategy name.
    let requested = unsafe { CStr::from_ptr(api.name) };
    let registration = new_registration(requested.to_owned(), api.coding, Some(api));
    if let Some(existing) = registry
        .iter_mut()
        .find(|existing| existing.coding == api.coding && same_name(&existing.name, requested))
    {
        *existing = registration;
    } else {
        registry.push(registration);
    }
    MPP_OK
}

pub(super) unsafe fn add(api: *const RcImplApi) -> c_int {
    if api.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: the caller passed the official RcImplApi pointer for this
    // command. The registry copies the struct before the control returns.
    let api = unsafe { &*api };
    add_unlocked(&mut lock_registry(), api)
}

unsafe fn write_briefs(
    output: *mut RcApiBrief,
    max_count: c_int,
    count: *mut c_int,
    registry: &[Registration],
    coding: Option<c_int>,
) -> c_int {
    if output.is_null() || count.is_null() || max_count <= 0 {
        return MPP_NOK;
    }
    let mut written = 0usize;
    for registration in registry {
        if coding.is_some_and(|coding| coding != registration.coding) {
            continue;
        }
        if written >= max_count as usize {
            break;
        }
        // SAFETY: output points to max_count writable RcApiBrief entries.
        unsafe {
            output.add(written).write(RcApiBrief {
                name: registration.name.as_ptr(),
                coding: registration.coding,
            });
            *count = written as c_int + 1;
        }
        written += 1;
    }
    if written == 0 {
        // SAFETY: count was checked non-null above.
        unsafe { *count = 0 };
    }
    MPP_OK
}

pub(super) unsafe fn query_all(query: *mut RcApiQueryAll) -> c_int {
    if query.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: command arguments use the official RcApiQueryAll layout.
    let query = unsafe { &mut *query };
    let registry = lock_registry();
    // SAFETY: all pointers originate from the caller's query structure.
    unsafe {
        write_briefs(
            query.brief,
            query.max_count,
            &mut query.count,
            &registry,
            None,
        )
    }
}

pub(super) unsafe fn query_by_type(query: *mut RcApiQueryType) -> c_int {
    if query.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: command arguments use the official RcApiQueryType layout.
    let query = unsafe { &mut *query };
    let registry = lock_registry();
    // SAFETY: all pointers originate from the caller's query structure.
    unsafe {
        write_briefs(
            query.brief,
            query.max_count,
            &mut query.count,
            &registry,
            Some(query.coding),
        )
    }
}

/// Register or replace a user-defined RC strategy.
///
/// # Safety
/// `api` must point to a valid official `RcImplApi` whose `name` is a
/// NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn rc_api_add(api: *const RcImplApi) -> c_int {
    unsafe { add(api) }
}

/// Query every registered RC strategy.
///
/// # Safety
/// `query` must point to a valid `RcApiQueryAll`; `brief` must address
/// `max_count` writable entries.
#[no_mangle]
pub unsafe extern "C" fn rc_brief_get_all(query: *mut RcApiQueryAll) -> c_int {
    unsafe { query_all(query) }
}

/// Query registered RC strategies for one coding type.
///
/// # Safety
/// `query` must point to a valid `RcApiQueryType`; `brief` must address
/// `max_count` writable entries.
#[no_mangle]
pub unsafe extern "C" fn rc_brief_get_by_type(query: *mut RcApiQueryType) -> c_int {
    unsafe { query_by_type(query) }
}

pub(super) struct Selection {
    name: Option<CString>,
    coding: c_int,
}

impl Selection {
    pub(super) fn new() -> Self {
        Self {
            name: Some(CString::new("default").expect("literal")),
            coding: 0,
        }
    }

    pub(super) fn get(&self, output: *mut RcApiBrief) -> c_int {
        if output.is_null() {
            return MPP_ERR_NULL_PTR;
        }
        // SAFETY: output points to the official writable RcApiBrief.
        unsafe {
            *output = RcApiBrief {
                name: self.name.as_ref().map_or(ptr::null(), |name| name.as_ptr()),
                coding: self.coding,
            };
        }
        MPP_OK
    }

    pub(super) fn set(&mut self, input: *const RcApiBrief, coding: c_int) -> c_int {
        if input.is_null() {
            return MPP_ERR_NULL_PTR;
        }
        // SAFETY: input points to the official RcApiBrief for this command.
        let input = unsafe { &*input };
        if input.coding != coding {
            return MPP_NOK;
        }
        self.name = if input.name.is_null() {
            None
        } else {
            // SAFETY: the public ABI requires a NUL-terminated strategy name.
            Some(unsafe { CStr::from_ptr(input.name) }.to_owned())
        };
        self.coding = input.coding;
        MPP_OK
    }

    pub(super) fn is_native_default(&self) -> bool {
        self.name
            .as_deref()
            .is_none_or(|name| name.to_bytes() == b"default")
    }

    pub(super) fn name(&self) -> Option<&CStr> {
        self.name.as_deref()
    }
}

fn resolve(name: &CStr, coding: c_int) -> Option<RcCallbacks> {
    lock_registry()
        .iter()
        .find(|registration| registration.coding == coding && same_name(&registration.name, name))
        .map(|registration| registration.callbacks)
        .filter(|callbacks| callbacks.frame_start.is_some())
}

pub(super) struct Session {
    callbacks: RcCallbacks,
    context: Box<[u8]>,
}

impl Session {
    pub(super) fn new(name: &CStr, coding: c_int, config: &mut RcCfg) -> Result<Self, String> {
        let callbacks = resolve(name, coding).ok_or_else(|| {
            format!("RC strategy {name:?} has no executable frame_start callback")
        })?;
        let mut context = vec![0u8; callbacks.ctx_size as usize].into_boxed_slice();
        if let Some(init) = callbacks.init {
            let result = unsafe { init(context.as_mut_ptr().cast(), config) };
            if result != MPP_OK {
                if let Some(deinit) = callbacks.deinit {
                    let _ = unsafe { deinit(context.as_mut_ptr().cast()) };
                }
                return Err(format!("RC strategy {name:?} init failed with {result}"));
            }
        }
        Ok(Self { callbacks, context })
    }

    fn call(&mut self, callback: Option<RcTaskFn>, task: &mut EncRcTask) -> Result<(), String> {
        if let Some(callback) = callback {
            let result = unsafe { callback(self.context.as_mut_ptr().cast(), task) };
            if result != MPP_OK {
                return Err(format!("RC callback failed with {result}"));
            }
        }
        Ok(())
    }

    pub(super) fn check_drop(&mut self, task: &mut EncRcTask) -> Result<(), String> {
        self.call(self.callbacks.check_drop, task)
    }

    pub(super) fn check_reencode(&mut self, task: &mut EncRcTask) -> Result<(), String> {
        self.call(self.callbacks.check_reenc, task)
    }

    pub(super) fn frame_start(&mut self, task: &mut EncRcTask) -> Result<(), String> {
        self.call(self.callbacks.frame_start, task)?;
        self.call(self.callbacks.hal_start, task)
    }

    pub(super) fn frame_end(&mut self, task: &mut EncRcTask) -> Result<(), String> {
        self.call(self.callbacks.hal_end, task)?;
        self.call(self.callbacks.frame_end, task)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(deinit) = self.callbacks.deinit {
            let _ = unsafe { deinit(self.context.as_mut_ptr().cast()) };
        }
    }
}

const _: () = {
    assert!(size_of::<RcApiBrief>() == 16);
    assert!(size_of::<RcApiQueryAll>() == 16);
    assert!(size_of::<RcApiQueryType>() == 24);
    assert!(size_of::<RcImplApi>() == 80);
    assert!(size_of::<RcCfg>() == 256);
    assert!(size_of::<EncFrmStatus>() == 8);
    assert!(size_of::<EncCpbStatus>() == 152);
    assert!(size_of::<EncRcTaskInfo>() == 112);
    assert!(size_of::<EncRcForceCfg>() == 32);
    assert!(size_of::<EncRcTask>() == 312);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_matches_the_vendor_query_and_replacement_rules() {
        let mut registry = builtins();
        assert_eq!(registry.len(), 6);
        assert_eq!(registry[0].name.to_str().unwrap(), "default");
        assert_eq!(registry[0].coding, MPP_VIDEO_CODING_AVC);
        assert_eq!(registry[4].name.to_str().unwrap(), "smart");

        let api = RcImplApi {
            name: c"codex_rc_test".as_ptr(),
            coding: MPP_VIDEO_CODING_AVC,
            ctx_size: 0,
            init: None,
            deinit: None,
            check_drop: None,
            check_reenc: None,
            frame_start: None,
            frame_end: None,
            hal_start: None,
            hal_end: None,
        };
        assert_eq!(add_unlocked(&mut registry, &api), MPP_OK);
        assert_eq!(registry.len(), 7);
        assert_eq!(add_unlocked(&mut registry, &api), MPP_OK);
        assert_eq!(registry.len(), 7);

        let mut briefs = [RcApiBrief {
            name: ptr::null(),
            coding: 0,
        }; 16];
        let mut count = -1;
        let result = unsafe {
            write_briefs(
                briefs.as_mut_ptr(),
                briefs.len() as c_int,
                &mut count,
                &registry,
                Some(MPP_VIDEO_CODING_AVC),
            )
        };
        assert_eq!(result, MPP_OK);
        assert_eq!(count, 3);
        assert_eq!(
            unsafe { CStr::from_ptr(briefs[2].name) }.to_bytes(),
            b"codex_rc_test"
        );
    }

    #[test]
    fn selection_tracks_the_default_and_rejects_a_mismatched_coding() {
        let mut selection = Selection::new();
        assert!(selection.is_native_default());
        let input = RcApiBrief {
            name: c"smart".as_ptr(),
            coding: MPP_VIDEO_CODING_HEVC,
        };
        assert_eq!(selection.set(&input, MPP_VIDEO_CODING_HEVC), MPP_OK);
        assert!(!selection.is_native_default());
        assert_eq!(selection.name().unwrap().to_bytes(), b"smart");
        assert_eq!(selection.set(&input, MPP_VIDEO_CODING_AVC), MPP_NOK);
    }
}
