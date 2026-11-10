use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::mem::size_of;
use std::ptr;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::encoder::{EncoderConfig, RateControlMode};
use crate::service::soc::{SocProfile, VideoEncodeHal};

use super::{
    MPP_ERR_NULL_PTR, MPP_NOK, MPP_OK, MPP_VIDEO_CODING_AVC, MPP_VIDEO_CODING_HEVC,
    MPP_VIDEO_CODING_MJPEG, MPP_VIDEO_CODING_VP8,
};

type RcInitFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;
type RcDeinitFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type RcTaskFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RcAbi {
    Legacy,
    Current,
}

impl RcAbi {
    fn detect() -> Self {
        match std::env::var("RK_MPP_RC_ABI").ok().as_deref() {
            Some("legacy") => return Self::Legacy,
            Some("current") => return Self::Current,
            _ => {}
        }
        if SocProfile::detect().is_ok_and(|profile| profile.video_encode == VideoEncodeHal::Vepu510)
        {
            Self::Current
        } else {
            Self::Legacy
        }
    }

    const fn rc_cfg_size(self) -> usize {
        match self {
            Self::Legacy => 256,
            Self::Current => 260,
        }
    }

    const fn task_size(self) -> usize {
        match self {
            Self::Legacy => 312,
            Self::Current => 336,
        }
    }

    const fn force_offset(self) -> usize {
        match self {
            Self::Legacy => 272,
            Self::Current => 296,
        }
    }

    const fn frame_offset(self) -> usize {
        match self {
            Self::Legacy => 304,
            Self::Current => 328,
        }
    }
}

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
    pub rc_container: i32,
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
            max_reencode_times: 1,
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

    pub(super) fn set_reencode(&mut self, enabled: bool) {
        self.set_bit(34, enabled);
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

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct EncRcTaskInfo {
    pub frame_type: i32,
    pub bit_target: i32,
    pub bit_target_fix: i32,
    pub bit_max: i32,
    pub bit_min: i32,
    pub quality_target: i32,
    pub quality_max: i32,
    pub quality_min: i32,
    pub bit_real: i32,
    pub quality_real: i32,
    pub madi: i32,
    pub madp: i32,
    pub dsp_y_avg: u32,
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
    pub motion_level: u32,
    pub complex_level: u32,
    pub complex_scene: i32,
    pub scene_mode: i32,
    pub last_scene_mode: i32,
    pub rt_bits: i32,
    pub madi_b16: i32,
    pub madp_ctu: i32,
    pub reserve: [i32; 6],
}

impl EncRcTaskInfo {
    pub(super) fn clear_measurements(&mut self) {
        let frame_type = self.frame_type;
        let bit_target = self.bit_target;
        let bit_max = self.bit_max;
        let bit_min = self.bit_min;
        let quality_target = self.quality_target;
        let quality_max = self.quality_max;
        let quality_min = self.quality_min;
        *self = Self::default();
        self.frame_type = frame_type;
        self.bit_target = bit_target;
        self.bit_max = bit_max;
        self.bit_min = bit_min;
        self.quality_target = quality_target;
        self.quality_max = quality_max;
        self.quality_min = quality_min;
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct EncRcForceCfg {
    pub force_flag: u32,
    pub force_qp: i32,
    pub reserve: [u32; 6],
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct EncRcTask {
    pub cpb: EncCpbStatus,
    pub frm: EncFrmStatus,
    pub info: EncRcTaskInfo,
    pub force: EncRcForceCfg,
    pub frame: *mut c_void,
}

impl EncRcTask {
    #[cfg(test)]
    pub(super) fn new(sequence: u32, intra: bool, idr: bool) -> Self {
        let mut task = Self::default();
        task.set_frame(sequence, intra, idr);
        task
    }

    pub(super) fn set_frame(&mut self, sequence: u32, intra: bool, idr: bool) {
        self.frm = EncFrmStatus::default();
        self.cpb.curr = EncFrmStatus::default();
        self.info = EncRcTaskInfo::default();
        self.frm.set_bit(0, true);
        self.frm.set_bit(4, intra);
        self.frm.set_bit(5, idr);
        self.frm.val |= (sequence as u64 & 0xffff) << 48;
        self.cpb.curr = self.frm;
        self.info.frame_type = if intra { 2 } else { 0 };
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

unsafe fn write_task_info(abi: RcAbi, base: *mut u8, info: &EncRcTaskInfo) {
    unsafe {
        let write_i32 = |offset: usize, value: i32| {
            ptr::write_unaligned(base.add(offset).cast::<i32>(), value);
        };
        let write_i64 = |offset: usize, value: i64| {
            ptr::write_unaligned(base.add(offset).cast::<i64>(), value);
        };

        write_i32(0, info.frame_type);
        write_i32(4, info.bit_target);
        match abi {
            RcAbi::Legacy => {
                write_i32(8, info.bit_max);
                write_i32(12, info.bit_min);
                write_i32(16, info.quality_target);
                write_i32(20, info.quality_max);
                write_i32(24, info.quality_min);
                write_i32(28, info.bit_real);
                write_i32(32, info.quality_real);
                write_i32(36, info.madi);
                write_i32(40, info.madp);
                write_i32(44, info.iblk4_prop as i32);
                write_i64(48, info.sse);
                for (index, value) in [
                    info.lvl64_inter_num,
                    info.lvl32_inter_num,
                    info.lvl16_inter_num,
                    info.lvl8_inter_num,
                    info.lvl32_intra_num,
                    info.lvl16_intra_num,
                    info.lvl8_intra_num,
                    info.lvl4_intra_num,
                ]
                .into_iter()
                .enumerate()
                {
                    write_i32(56 + index * 4, value as i32);
                }
                for (index, value) in info.reserve[..5].iter().copied().enumerate() {
                    write_i32(88 + index * 4, value);
                }
            }
            RcAbi::Current => {
                write_i32(8, info.bit_target_fix);
                write_i32(12, info.bit_max);
                write_i32(16, info.bit_min);
                write_i32(20, info.quality_target);
                write_i32(24, info.quality_max);
                write_i32(28, info.quality_min);
                write_i32(32, info.bit_real);
                write_i32(36, info.quality_real);
                write_i32(40, info.madi);
                write_i32(44, info.madp);
                write_i32(48, info.dsp_y_avg as i32);
                write_i32(52, info.iblk4_prop as i32);
                write_i64(56, info.sse);
                for (index, value) in [
                    info.lvl64_inter_num,
                    info.lvl32_inter_num,
                    info.lvl16_inter_num,
                    info.lvl8_inter_num,
                    info.lvl32_intra_num,
                    info.lvl16_intra_num,
                    info.lvl8_intra_num,
                    info.lvl4_intra_num,
                ]
                .into_iter()
                .enumerate()
                {
                    write_i32(64 + index * 4, value as i32);
                }
                write_i32(96, info.motion_level as i32);
                write_i32(100, info.complex_level as i32);
                write_i32(104, info.complex_scene);
                write_i32(108, info.scene_mode);
                write_i32(112, info.last_scene_mode);
                write_i32(116, info.rt_bits);
                write_i32(120, info.madi_b16);
                write_i32(124, info.madp_ctu);
                write_i32(128, info.reserve[0]);
                write_i32(132, info.reserve[1]);
            }
        }
    }
}

unsafe fn read_task_info(abi: RcAbi, base: *const u8, info: &mut EncRcTaskInfo) {
    unsafe {
        let read_i32 = |offset: usize| ptr::read_unaligned(base.add(offset).cast::<i32>());
        let read_i64 = |offset: usize| ptr::read_unaligned(base.add(offset).cast::<i64>());

        info.frame_type = read_i32(0);
        info.bit_target = read_i32(4);
        match abi {
            RcAbi::Legacy => {
                info.bit_target_fix = 0;
                info.bit_max = read_i32(8);
                info.bit_min = read_i32(12);
                info.quality_target = read_i32(16);
                info.quality_max = read_i32(20);
                info.quality_min = read_i32(24);
                info.bit_real = read_i32(28);
                info.quality_real = read_i32(32);
                info.madi = read_i32(36);
                info.madp = read_i32(40);
                info.dsp_y_avg = 0;
                info.iblk4_prop = read_i32(44) as u32;
                info.sse = read_i64(48);
                for (index, target) in [
                    &mut info.lvl64_inter_num,
                    &mut info.lvl32_inter_num,
                    &mut info.lvl16_inter_num,
                    &mut info.lvl8_inter_num,
                    &mut info.lvl32_intra_num,
                    &mut info.lvl16_intra_num,
                    &mut info.lvl8_intra_num,
                    &mut info.lvl4_intra_num,
                ]
                .into_iter()
                .enumerate()
                {
                    *target = read_i32(56 + index * 4) as u32;
                }
                for (index, target) in info.reserve[..5].iter_mut().enumerate() {
                    *target = read_i32(88 + index * 4);
                }
            }
            RcAbi::Current => {
                info.bit_target_fix = read_i32(8);
                info.bit_max = read_i32(12);
                info.bit_min = read_i32(16);
                info.quality_target = read_i32(20);
                info.quality_max = read_i32(24);
                info.quality_min = read_i32(28);
                info.bit_real = read_i32(32);
                info.quality_real = read_i32(36);
                info.madi = read_i32(40);
                info.madp = read_i32(44);
                info.dsp_y_avg = read_i32(48) as u32;
                info.iblk4_prop = read_i32(52) as u32;
                info.sse = read_i64(56);
                for (index, target) in [
                    &mut info.lvl64_inter_num,
                    &mut info.lvl32_inter_num,
                    &mut info.lvl16_inter_num,
                    &mut info.lvl8_inter_num,
                    &mut info.lvl32_intra_num,
                    &mut info.lvl16_intra_num,
                    &mut info.lvl8_intra_num,
                    &mut info.lvl4_intra_num,
                ]
                .into_iter()
                .enumerate()
                {
                    *target = read_i32(64 + index * 4) as u32;
                }
                info.motion_level = read_i32(96) as u32;
                info.complex_level = read_i32(100) as u32;
                info.complex_scene = read_i32(104);
                info.scene_mode = read_i32(108);
                info.last_scene_mode = read_i32(112);
                info.rt_bits = read_i32(116);
                info.madi_b16 = read_i32(120);
                info.madp_ctu = read_i32(124);
                info.reserve[0] = read_i32(128);
                info.reserve[1] = read_i32(132);
            }
        }
    }
}

unsafe fn write_task(abi: RcAbi, raw: &mut [u8], task: &EncRcTask) {
    let base = raw.as_mut_ptr();
    unsafe {
        ptr::copy_nonoverlapping(
            (&task.cpb as *const EncCpbStatus).cast::<u8>(),
            base,
            size_of::<EncCpbStatus>(),
        );
        ptr::write_unaligned(base.add(152).cast::<u64>(), task.frm.val);
        write_task_info(abi, base.add(160), &task.info);
        let force = abi.force_offset();
        ptr::write_unaligned(base.add(force).cast::<u32>(), task.force.force_flag);
        ptr::write_unaligned(base.add(force + 4).cast::<i32>(), task.force.force_qp);
        for (index, value) in task.force.reserve.into_iter().enumerate() {
            ptr::write_unaligned(base.add(force + 8 + index * 4).cast::<u32>(), value);
        }
        ptr::write_unaligned(
            base.add(abi.frame_offset()).cast::<*mut c_void>(),
            task.frame,
        );
    }
}

unsafe fn read_task(abi: RcAbi, raw: &[u8], task: &mut EncRcTask) {
    let base = raw.as_ptr();
    unsafe {
        ptr::copy_nonoverlapping(
            base,
            (&mut task.cpb as *mut EncCpbStatus).cast::<u8>(),
            size_of::<EncCpbStatus>(),
        );
        task.frm.val = ptr::read_unaligned(base.add(152).cast::<u64>());
        read_task_info(abi, base.add(160), &mut task.info);
        let force = abi.force_offset();
        task.force.force_flag = ptr::read_unaligned(base.add(force).cast::<u32>());
        task.force.force_qp = ptr::read_unaligned(base.add(force + 4).cast::<i32>());
        for (index, value) in task.force.reserve.iter_mut().enumerate() {
            *value = ptr::read_unaligned(base.add(force + 8 + index * 4).cast::<u32>());
        }
        task.frame = ptr::read_unaligned(base.add(abi.frame_offset()).cast::<*mut c_void>());
    }
}

pub(super) struct Session {
    abi: RcAbi,
    callbacks: RcCallbacks,
    context: Box<[u8]>,
}

impl Session {
    pub(super) fn new(name: &CStr, coding: c_int, config: &mut RcCfg) -> Result<Self, String> {
        let callbacks = resolve(name, coding).ok_or_else(|| {
            format!("RC strategy {name:?} has no executable frame_start callback")
        })?;
        let abi = RcAbi::detect();
        let mut context = vec![0u8; callbacks.ctx_size as usize].into_boxed_slice();
        if let Some(init) = callbacks.init {
            let size = abi.rc_cfg_size();
            let mut raw = vec![0u8; size];
            let copy = size_of::<RcCfg>().min(size);
            unsafe {
                ptr::copy_nonoverlapping(
                    (config as *const RcCfg).cast::<u8>(),
                    raw.as_mut_ptr(),
                    copy,
                );
            }
            let result = unsafe { init(context.as_mut_ptr().cast(), raw.as_mut_ptr().cast()) };
            if result != MPP_OK {
                if let Some(deinit) = callbacks.deinit {
                    let _ = unsafe { deinit(context.as_mut_ptr().cast()) };
                }
                return Err(format!("RC strategy {name:?} init failed with {result}"));
            }
            unsafe {
                ptr::copy_nonoverlapping(raw.as_ptr(), (config as *mut RcCfg).cast::<u8>(), copy);
            }
        }
        Ok(Self {
            abi,
            callbacks,
            context,
        })
    }

    fn call(&mut self, callback: Option<RcTaskFn>, task: &mut EncRcTask) -> Result<(), String> {
        if let Some(callback) = callback {
            let mut raw = vec![0u8; self.abi.task_size()];
            unsafe { write_task(self.abi, &mut raw[..], task) };
            let result =
                unsafe { callback(self.context.as_mut_ptr().cast(), raw.as_mut_ptr().cast()) };
            if result != MPP_OK {
                return Err(format!("RC callback failed with {result}"));
            }
            unsafe { read_task(self.abi, &raw, task) };
        }
        Ok(())
    }

    pub(super) fn check_drop(&mut self, task: &mut EncRcTask) -> Result<(), String> {
        self.call(self.callbacks.check_drop, task)
    }

    fn check_reencode(&mut self, task: &mut EncRcTask) -> Result<(), String> {
        self.call(self.callbacks.check_reenc, task)
    }

    pub(super) fn frame_start(&mut self, task: &mut EncRcTask) -> Result<(), String> {
        self.call(self.callbacks.frame_start, task)?;
        self.call(self.callbacks.hal_start, task)
    }

    pub(super) fn hal_start(&mut self, task: &mut EncRcTask) -> Result<(), String> {
        self.call(self.callbacks.hal_start, task)
    }

    pub(super) fn finish_attempt(
        &mut self,
        task: &mut EncRcTask,
        fill_metrics: impl FnOnce(&mut EncRcTask),
    ) -> Result<bool, String> {
        self.call(self.callbacks.hal_end, task)?;
        fill_metrics(task);
        self.check_reencode(task)?;
        Ok(task.frm.is_reencode())
    }

    pub(super) fn frame_end(&mut self, task: &mut EncRcTask) -> Result<(), String> {
        self.call(self.callbacks.frame_end, task)
    }

    #[cfg(test)]
    pub(super) fn finish_frame(
        &mut self,
        task: &mut EncRcTask,
        fill_metrics: impl FnOnce(&mut EncRcTask),
    ) -> Result<bool, String> {
        if self.finish_attempt(task, fill_metrics)? {
            return Ok(false);
        }
        self.frame_end(task)?;
        Ok(true)
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
    assert!(size_of::<RcCfg>() == 260);
    assert!(size_of::<EncFrmStatus>() == 8);
    assert!(size_of::<EncCpbStatus>() == 152);
};

#[cfg(test)]
mod tests {
    use super::*;

    static CALLBACK_STAGES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

    unsafe extern "C" fn record_frame_start(_: *mut c_void, _: *mut c_void) -> c_int {
        CALLBACK_STAGES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push("frame-start");
        MPP_OK
    }

    unsafe extern "C" fn record_hal_end(_: *mut c_void, _: *mut c_void) -> c_int {
        CALLBACK_STAGES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push("hal-end");
        MPP_OK
    }

    unsafe extern "C" fn record_check_reenc(_: *mut c_void, _: *mut c_void) -> c_int {
        CALLBACK_STAGES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push("reenc");
        MPP_OK
    }

    unsafe extern "C" fn record_frame_end(_: *mut c_void, _: *mut c_void) -> c_int {
        CALLBACK_STAGES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push("frame-end");
        MPP_OK
    }

    #[test]
    fn both_rc_task_abis_round_trip_the_callback_fields() {
        for abi in [RcAbi::Legacy, RcAbi::Current] {
            let mut source = EncRcTask::new(3, true, true);
            source.info.frame_type = 2;
            source.info.bit_target = 33_333;
            source.info.bit_target_fix = 31_111;
            source.info.bit_max = 40_000;
            source.info.bit_min = 20_000;
            source.info.quality_target = 26;
            source.info.quality_max = 51;
            source.info.quality_min = 10;
            source.info.bit_real = 29_208;
            source.info.quality_real = 27;
            source.info.dsp_y_avg = 42;
            source.info.iblk4_prop = 128;
            source.info.sse = 0x1234_5678;
            source.info.motion_level = 100;
            source.info.complex_level = 2;
            source.info.complex_scene = 3;
            source.info.scene_mode = 4;
            source.info.last_scene_mode = 5;
            source.info.rt_bits = 28_000;
            source.info.madi_b16 = 19;
            source.info.madp_ctu = 23;
            source.force.force_flag = 1;
            source.force.force_qp = 24;
            source.frame = 0x1234usize as *mut c_void;

            let mut raw = vec![0u8; abi.task_size()];
            unsafe { write_task(abi, &mut raw, &source) };
            let raw_bit_fix =
                unsafe { ptr::read_unaligned(raw.as_ptr().add(160 + 8).cast::<i32>()) };
            let mut decoded = EncRcTask::default();
            unsafe { read_task(abi, &raw, &mut decoded) };

            assert_eq!(decoded.cpb.seq_idx, source.cpb.seq_idx);
            assert_eq!(decoded.frm.val, source.frm.val);
            assert_eq!(decoded.info.frame_type, source.info.frame_type);
            assert_eq!(decoded.info.bit_target, source.info.bit_target);
            if abi == RcAbi::Current {
                assert_eq!(decoded.info.bit_target_fix, source.info.bit_target_fix);
                assert_eq!(raw_bit_fix, source.info.bit_target_fix);
            } else {
                assert_eq!(decoded.info.bit_target_fix, 0);
                assert_eq!(raw_bit_fix, source.info.bit_max);
            }
            assert_eq!(decoded.info.bit_max, source.info.bit_max);
            assert_eq!(decoded.info.bit_min, source.info.bit_min);
            assert_eq!(decoded.info.quality_target, source.info.quality_target);
            assert_eq!(decoded.info.quality_max, source.info.quality_max);
            assert_eq!(decoded.info.quality_min, source.info.quality_min);
            assert_eq!(decoded.info.bit_real, source.info.bit_real);
            assert_eq!(decoded.info.quality_real, source.info.quality_real);
            assert_eq!(decoded.info.iblk4_prop, source.info.iblk4_prop);
            assert_eq!(decoded.info.sse, source.info.sse);
            if abi == RcAbi::Current {
                assert_eq!(decoded.info.dsp_y_avg, source.info.dsp_y_avg);
                assert_eq!(decoded.info.motion_level, source.info.motion_level);
                assert_eq!(decoded.info.complex_level, source.info.complex_level);
                assert_eq!(decoded.info.complex_scene, source.info.complex_scene);
                assert_eq!(decoded.info.scene_mode, source.info.scene_mode);
                assert_eq!(decoded.info.last_scene_mode, source.info.last_scene_mode);
                assert_eq!(decoded.info.rt_bits, source.info.rt_bits);
                assert_eq!(decoded.info.madi_b16, source.info.madi_b16);
                assert_eq!(decoded.info.madp_ctu, source.info.madp_ctu);
            } else {
                assert_eq!(decoded.info.dsp_y_avg, 0);
            }
            assert_eq!(decoded.force.force_flag, source.force.force_flag);
            assert_eq!(decoded.force.force_qp, source.force.force_qp);
            assert_eq!(decoded.frame, source.frame);
        }
    }

    #[test]
    fn cpb_sequence_stays_unloaded_while_frame_sequence_is_available() {
        let task = EncRcTask::new(7, false, false);
        assert_eq!(task.cpb.seq_idx, 0);
        assert_eq!((task.frm.val >> 48) & 0xffff, 7);
    }

    #[test]
    fn finish_frame_uses_the_vendor_hal_reenc_frame_order() {
        let api = RcImplApi {
            name: c"codex_callback_order_test".as_ptr(),
            coding: MPP_VIDEO_CODING_AVC,
            ctx_size: 1,
            init: None,
            deinit: None,
            check_drop: None,
            check_reenc: Some(record_check_reenc),
            frame_start: Some(record_frame_start),
            frame_end: Some(record_frame_end),
            hal_start: None,
            hal_end: Some(record_hal_end),
        };
        assert_eq!(unsafe { add(&api) }, MPP_OK);
        CALLBACK_STAGES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();

        let mut config = RcCfg::default();
        let mut session = Session::new(
            c"codex_callback_order_test",
            MPP_VIDEO_CODING_AVC,
            &mut config,
        )
        .unwrap();
        let mut task = EncRcTask::new(0, true, true);
        assert!(session
            .finish_frame(&mut task, |_| {
                CALLBACK_STAGES
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push("metrics");
            })
            .unwrap());

        assert_eq!(
            *CALLBACK_STAGES
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            ["hal-end", "metrics", "reenc", "frame-end"]
        );
    }

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
