//! `rk_vdec_kcfg.h` / `rk_venc_kcfg.h`: typed name/value kernel-config stores.
//!
//! The vendor headers expose opaque handles with `init(type)` /
//! `init_by_name()` / `deinit()` and typed `set_*`/`get_*` access by name,
//! plus a few `st` and flex helpers. This module implements the handle and the
//! store so applications that configure MPP through these APIs link, set and
//! read back. The stored values are not pushed into a kernel `kcfg` ioctl:
//! this crate programs the HAL directly, so advanced kernel-side knobs have no
//! effect beyond storage. See `docs/MIGRATION.md`.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr};

const KCFG_MAGIC: u64 = 0x4d50_5052_4b43_4647; // "MPPRKCFG"
const MPP_OK: c_int = 0;
const MPP_NOK: c_int = -1;
const MPP_ERR_NULL_PTR: c_int = -3;
const MPP_ERR_VALUE: c_int = -6;

const VDEC_KCFG_TYPE_BUTT: u32 = 5;
const VENC_KCFG_TYPE_BUTT: u32 = 8;
const VENC_KCFG_TYPE_CTRL_CFG: u32 = 7;
const KMPP_CTRL_FLAG_FLEX: u32 = 2;

const VDEC_KCFG_NAMES: [&str; VDEC_KCFG_TYPE_BUTT as usize] = [
    "KmppVdecInitCfg",
    "KmppVdecDeinitCfg",
    "KmppVdecResetCfg",
    "KmppVdecStartCfg",
    "KmppVdecStopCfg",
];

const VENC_KCFG_NAMES: [&str; VENC_KCFG_TYPE_BUTT as usize] = [
    "KmppVencInitCfg",
    "KmppVencDeinitCfg",
    "KmppVencResetCfg",
    "KmppVencStartCfg",
    "KmppVencStopCfg",
    "KmppVencStCfg",
    "KmppVencRefCfg",
    "KmppVencCtrlCfg",
];

#[derive(Clone, Copy)]
enum Val {
    S8(i8),
    U8(u8),
    S16(i16),
    U16(u16),
    S32(i32),
    U32(u32),
    S64(i64),
    U64(u64),
    Ptr(*mut c_void),
}

struct Flex {
    buffer: Vec<u8>,
}

pub(super) struct VencInitConfig {
    pub ctx_type: c_int,
    pub coding: c_int,
    pub max_width: u32,
    pub max_height: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KcfgKind {
    Vdec,
    Venc,
}

pub(super) struct KcfgStore {
    magic: u64,
    kind: KcfgKind,
    type_id: u32,
    name: Option<String>,
    values: HashMap<String, Val>,
    flex: Flex,
}

fn store_mut<'a>(cfg: *mut c_void) -> Option<&'a mut KcfgStore> {
    let store = unsafe { cfg.cast::<KcfgStore>().as_mut() }?;
    (store.magic == KCFG_MAGIC).then_some(store)
}

fn new_store(kind: KcfgKind, type_id: u32, name: Option<String>) -> *mut c_void {
    let mut values = HashMap::new();
    if kind == KcfgKind::Venc && type_id == VENC_KCFG_TYPE_CTRL_CFG {
        values.insert("cmd".to_string(), Val::S32(0));
        values.insert("val".to_string(), Val::S64(0));
        values.insert("arg".to_string(), Val::Ptr(std::ptr::null_mut()));
        values.insert("ret".to_string(), Val::Ptr(std::ptr::null_mut()));
    }
    Box::into_raw(Box::new(KcfgStore {
        magic: KCFG_MAGIC,
        kind,
        type_id,
        name,
        values,
        flex: Flex {
            buffer: vec![0u8; 4096],
        },
    }))
    .cast()
}

unsafe fn key(name: *const c_char) -> Option<String> {
    if name.is_null() {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned(),
    )
}

fn type_by_name(name: &str, names: &[&str]) -> Option<u32> {
    names
        .iter()
        .position(|canonical| name.starts_with(canonical))
        .map(|index| index as u32)
}

fn set_value(cfg: *mut c_void, name: *const c_char, value: Val) -> c_int {
    let Some(name) = (unsafe { key(name) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let Some(store) = store_mut(cfg) else {
        return MPP_NOK;
    };
    store.values.insert(name, value);
    MPP_OK
}

fn get_value(cfg: *mut c_void, name: *const c_char, want: fn(Val) -> Option<Val>) -> Option<Val> {
    let name = unsafe { key(name) }?;
    let store = store_mut(cfg)?;
    store.values.get(&name).copied().and_then(want)
}

macro_rules! ptr_only {
    ($set:ident, $get:ident) => {
        #[no_mangle]
        pub unsafe extern "C" fn $set(
            cfg: *mut c_void,
            name: *const c_char,
            value: *mut c_void,
        ) -> c_int {
            set_value(cfg, name, Val::Ptr(value))
        }

        #[no_mangle]
        pub unsafe extern "C" fn $get(
            cfg: *mut c_void,
            name: *const c_char,
            value: *mut *mut c_void,
        ) -> c_int {
            if value.is_null() {
                return MPP_ERR_NULL_PTR;
            }
            match get_value(cfg, name, |v| match v {
                Val::Ptr(x) => Some(Val::Ptr(x)),
                _ => None,
            }) {
                Some(Val::Ptr(x)) => {
                    unsafe { *value = x };
                    MPP_OK
                }
                _ => MPP_NOK,
            }
        }
    };
}

// ---- vdec ------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mpp_vdec_kcfg_init(out: *mut *mut c_void, type_id: u32) -> c_int {
    if out.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    if type_id >= VDEC_KCFG_TYPE_BUTT {
        return MPP_ERR_VALUE;
    }
    unsafe { *out = new_store(KcfgKind::Vdec, type_id, None) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_vdec_kcfg_init_by_name(
    out: *mut *mut c_void,
    name: *const c_char,
) -> c_int {
    if out.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(name) = (unsafe { key(name) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let Some(type_id) = type_by_name(&name, &VDEC_KCFG_NAMES) else {
        return MPP_ERR_VALUE;
    };
    unsafe { *out = new_store(KcfgKind::Vdec, type_id, Some(name)) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_vdec_kcfg_deinit(cfg: *mut c_void) -> c_int {
    if cfg.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    if store_mut(cfg).is_none() {
        return MPP_NOK;
    }
    unsafe { drop(Box::from_raw(cfg.cast::<KcfgStore>())) };
    MPP_OK
}

// Typed families: one macro generates both prefixes from a shared body.
macro_rules! kcfg_typed {
    ($set:ident, $get:ident, $variant:ident, $ty:ty) => {
        #[no_mangle]
        pub unsafe extern "C" fn $set(cfg: *mut c_void, name: *const c_char, value: $ty) -> c_int {
            set_value(cfg, name, Val::$variant(value))
        }

        #[no_mangle]
        pub unsafe extern "C" fn $get(
            cfg: *mut c_void,
            name: *const c_char,
            value: *mut $ty,
        ) -> c_int {
            if value.is_null() {
                return MPP_ERR_NULL_PTR;
            }
            match get_value(cfg, name, |v| match v {
                Val::$variant(x) => Some(Val::$variant(x)),
                _ => None,
            }) {
                Some(Val::$variant(x)) => {
                    unsafe { *value = x };
                    MPP_OK
                }
                _ => MPP_NOK,
            }
        }
    };
}

kcfg_typed!(mpp_vdec_kcfg_set_s32, mpp_vdec_kcfg_get_s32, S32, i32);
kcfg_typed!(mpp_vdec_kcfg_set_u32, mpp_vdec_kcfg_get_u32, U32, u32);
kcfg_typed!(mpp_vdec_kcfg_set_s64, mpp_vdec_kcfg_get_s64, S64, i64);
kcfg_typed!(mpp_vdec_kcfg_set_u64, mpp_vdec_kcfg_get_u64, U64, u64);
ptr_only!(mpp_vdec_kcfg_set_ptr, mpp_vdec_kcfg_get_ptr);
ptr_only!(mpp_vdec_kcfg_set_st, mpp_vdec_kcfg_get_st);

#[no_mangle]
pub unsafe extern "C" fn mpp_vdec_kcfg_show(cfg: *mut c_void) {
    show_store(KcfgKind::Vdec, cfg);
}

// ---- venc ------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mpp_venc_kcfg_init(out: *mut *mut c_void, type_id: u32) -> c_int {
    if out.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    if type_id >= VENC_KCFG_TYPE_BUTT {
        return MPP_ERR_VALUE;
    }
    unsafe { *out = new_store(KcfgKind::Venc, type_id, None) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_venc_kcfg_init_by_name(
    out: *mut *mut c_void,
    name: *const c_char,
) -> c_int {
    if out.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(name) = (unsafe { key(name) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let Some(type_id) = type_by_name(&name, &VENC_KCFG_NAMES) else {
        return MPP_ERR_VALUE;
    };
    unsafe { *out = new_store(KcfgKind::Venc, type_id, Some(name)) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_venc_kcfg_deinit(cfg: *mut c_void) -> c_int {
    if cfg.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    if store_mut(cfg).is_none() {
        return MPP_NOK;
    }
    unsafe { drop(Box::from_raw(cfg.cast::<KcfgStore>())) };
    MPP_OK
}

kcfg_typed!(mpp_venc_kcfg_set_s8, mpp_venc_kcfg_get_s8, S8, i8);
kcfg_typed!(mpp_venc_kcfg_set_u8, mpp_venc_kcfg_get_u8, U8, u8);
kcfg_typed!(mpp_venc_kcfg_set_s16, mpp_venc_kcfg_get_s16, S16, i16);
kcfg_typed!(mpp_venc_kcfg_set_u16, mpp_venc_kcfg_get_u16, U16, u16);
kcfg_typed!(mpp_venc_kcfg_set_s32, mpp_venc_kcfg_get_s32, S32, i32);
kcfg_typed!(mpp_venc_kcfg_set_u32, mpp_venc_kcfg_get_u32, U32, u32);
kcfg_typed!(mpp_venc_kcfg_set_s64, mpp_venc_kcfg_get_s64, S64, i64);
kcfg_typed!(mpp_venc_kcfg_set_u64, mpp_venc_kcfg_get_u64, U64, u64);
ptr_only!(mpp_venc_kcfg_set_ptr, mpp_venc_kcfg_get_ptr);
ptr_only!(mpp_venc_kcfg_set_st, mpp_venc_kcfg_get_st);

#[no_mangle]
pub unsafe extern "C" fn mpp_venc_kcfg_show(cfg: *mut c_void) {
    show_store(KcfgKind::Venc, cfg);
}

/// Base of the flex area for a CTRL_CFG handle (4 KiB, zeroed).
#[no_mangle]
pub unsafe extern "C" fn mpp_venc_ctrl_flex_base(cfg: *mut c_void) -> *mut c_void {
    match ctrl_store_mut(cfg) {
        Some(store) => store.flex.buffer.as_mut_ptr().cast(),
        None => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_venc_ctrl_set_flex(
    cfg: *mut c_void,
    data: *const c_void,
    size: u32,
) -> c_int {
    if data.is_null() || size == 0 {
        return MPP_ERR_NULL_PTR;
    }
    let Some(store) = ctrl_store_mut(cfg) else {
        return MPP_NOK;
    };
    let size = size as usize;
    if size > store.flex.buffer.len() {
        store.flex.buffer.resize(size, 0);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(data.cast::<u8>(), store.flex.buffer.as_mut_ptr(), size);
    }
    store
        .values
        .insert("flags".to_string(), Val::U32(KMPP_CTRL_FLAG_FLEX));
    store
        .values
        .insert("size".to_string(), Val::U32(size as u32));
    MPP_OK
}

fn ctrl_store_mut<'a>(cfg: *mut c_void) -> Option<&'a mut KcfgStore> {
    let store = store_mut(cfg)?;
    (store.kind == KcfgKind::Venc && store.type_id == VENC_KCFG_TYPE_CTRL_CFG).then_some(store)
}

fn show_store(kind: KcfgKind, cfg: *mut c_void) {
    let Some(store) = store_mut(cfg) else {
        eprintln!("rk-mpp-rs: kcfg: invalid handle");
        return;
    };
    if store.kind != kind {
        eprintln!("rk-mpp-rs: kcfg: handle kind does not match show function");
        return;
    }
    let kind = if kind == KcfgKind::Vdec {
        "vdec"
    } else {
        "venc"
    };
    eprintln!(
        "rk-mpp-rs: {kind}_kcfg type={} name={:?} entries={} (storage only; not sent to the kernel)",
        store.type_id,
        store.name,
        store.values.len()
    );
}

pub(super) fn venc_init_config(cfg: *mut c_void) -> Result<VencInitConfig, String> {
    let store = store_mut(cfg).ok_or("invalid venc init kcfg handle")?;
    const KNOWN_KEYS: &[&str] = &[
        "type",
        "coding",
        "chan_id",
        "online",
        "buf_size",
        "max_strm_cnt",
        "shared_buf_en",
        "smart_en",
        "max_width",
        "max_height",
        "max_lt_cnt",
        "qpmap_en",
        "chan_dup",
        "tmvp_enable",
        "only_smartp",
        "ntfy_mode",
        "input_timeout",
    ];
    if let Some(unknown) = store
        .values
        .keys()
        .find(|key| !KNOWN_KEYS.contains(&key.as_str()))
    {
        return Err(format!("unsupported venc init kcfg key {unknown:?}"));
    }
    Ok(VencInitConfig {
        ctx_type: required_u32(store, "type")? as c_int,
        coding: required_u32(store, "coding")? as c_int,
        max_width: required_u32(store, "max_width")?,
        max_height: required_u32(store, "max_height")?,
    })
}

fn required_u32(store: &KcfgStore, name: &str) -> Result<u32, String> {
    match store.values.get(name) {
        Some(Val::U32(value)) => Ok(*value),
        Some(Val::S32(value)) if *value >= 0 => Ok(*value as u32),
        Some(_) => Err(format!(
            "venc init kcfg {name:?} must be an unsigned integer"
        )),
        None => Err(format!("venc init kcfg is missing {name:?}")),
    }
}

const _: () = {
    let _ = MPP_NOK;
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn init_validation_and_names_match_the_vendor_handles() {
        let mut vdec = std::ptr::null_mut();
        assert_eq!(
            unsafe { mpp_vdec_kcfg_init(&mut vdec, VDEC_KCFG_TYPE_BUTT) },
            MPP_ERR_VALUE
        );
        let mut venc = std::ptr::null_mut();
        assert_eq!(
            unsafe { mpp_venc_kcfg_init(&mut venc, VENC_KCFG_TYPE_BUTT) },
            MPP_ERR_VALUE
        );

        let name = CString::new("KmppVdecStartCfg:test").unwrap();
        assert_eq!(
            unsafe { mpp_vdec_kcfg_init_by_name(&mut vdec, name.as_ptr()) },
            MPP_OK
        );
        assert_eq!(unsafe { (*vdec.cast::<KcfgStore>()).type_id }, 3);
        assert_eq!(unsafe { mpp_vdec_kcfg_deinit(vdec) }, MPP_OK);

        let name = CString::new("not-a-vendor-config").unwrap();
        assert_eq!(
            unsafe { mpp_venc_kcfg_init_by_name(&mut venc, name.as_ptr()) },
            MPP_ERR_VALUE
        );
    }

    #[test]
    fn ctrl_cfg_initializes_and_records_flex_metadata() {
        let mut ctrl = std::ptr::null_mut();
        assert_eq!(
            unsafe { mpp_venc_kcfg_init(&mut ctrl, VENC_KCFG_TYPE_CTRL_CFG) },
            MPP_OK
        );
        let store = unsafe { &*ctrl.cast::<KcfgStore>() };
        assert!(matches!(store.values.get("cmd"), Some(Val::S32(0))));
        assert!(matches!(store.values.get("val"), Some(Val::S64(0))));
        assert!(matches!(
            store.values.get("arg"),
            Some(Val::Ptr(ptr)) if ptr.is_null()
        ));

        let data = [1u8, 2, 3, 4];
        assert_eq!(
            unsafe {
                mpp_venc_ctrl_set_flex(ctrl, data.as_ptr().cast(), data.len().try_into().unwrap())
            },
            MPP_OK
        );
        let mut flags = 0u32;
        let mut size = 0u32;
        let flags_key = CString::new("flags").unwrap();
        let size_key = CString::new("size").unwrap();
        assert_eq!(
            unsafe { mpp_venc_kcfg_get_u32(ctrl, flags_key.as_ptr(), &mut flags) },
            MPP_OK
        );
        assert_eq!(
            unsafe { mpp_venc_kcfg_get_u32(ctrl, size_key.as_ptr(), &mut size) },
            MPP_OK
        );
        assert_eq!(flags, KMPP_CTRL_FLAG_FLEX);
        assert_eq!(size, 4);
        let base = unsafe { mpp_venc_ctrl_flex_base(ctrl) }.cast::<u8>();
        assert_eq!(
            unsafe { std::slice::from_raw_parts(base, data.len()) },
            data
        );
        assert_eq!(unsafe { mpp_venc_kcfg_deinit(ctrl) }, MPP_OK);
    }

    #[test]
    fn ctrl_flex_rejects_non_ctrl_handles() {
        let mut init = std::ptr::null_mut();
        assert_eq!(unsafe { mpp_venc_kcfg_init(&mut init, 0) }, MPP_OK);
        assert!(unsafe { mpp_venc_ctrl_flex_base(init) }.is_null());
        let data = [1u8];
        assert_eq!(
            unsafe { mpp_venc_ctrl_set_flex(init, data.as_ptr().cast(), 1) },
            MPP_NOK
        );
        assert_eq!(unsafe { mpp_venc_kcfg_deinit(init) }, MPP_OK);
    }
}
