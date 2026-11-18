//! `rk_mpp_cfg.h`: a storage + readback implementation of `mpp_sys_cfg_*`.
//!
//! The vendor implementation is a kmpp singleton with a large `dec_buf_chk`
//! entry table and an ioctl that recomputes FBC/stride/size fields. This crate
//! implements the handle and the typed name/value store so applications that
//! set and read configuration keep linking and round-tripping; the derived
//! buffer-check computation (`mpp_sys_cfg_ioctl`, `mpp_sys_dec_buf_chk_proc`,
//! `mpp_sys_cfg_st_get_*`) returns `MPP_NOK` instead of reporting numbers this
//! HAL did not compute. See `docs/MIGRATION.md`.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr};

const SYS_CFG_MAGIC: u64 = 0x4d50_5052_5359_5343; // "MPPRSYSC"
const MPP_OK: c_int = 0;
const MPP_NOK: c_int = -1;
const MPP_ERR_NULL_PTR: c_int = -3;

#[derive(Clone, Copy)]
enum CfgValue {
    S32(i32),
    U32(u32),
    S64(i64),
    U64(u64),
    Ptr(*mut c_void),
}

pub(super) struct SysCfgStore {
    magic: u64,
    values: HashMap<String, CfgValue>,
}

fn store_mut<'a>(cfg: *mut c_void) -> Option<&'a mut SysCfgStore> {
    let store = unsafe { cfg.cast::<SysCfgStore>().as_mut() }?;
    (store.magic == SYS_CFG_MAGIC).then_some(store)
}

unsafe fn key_name(name: *const c_char) -> Option<String> {
    if name.is_null() {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned(),
    )
}

#[no_mangle]
pub unsafe extern "C" fn mpp_sys_cfg_get(out: *mut *mut c_void) -> c_int {
    if out.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let store = Box::into_raw(Box::new(SysCfgStore {
        magic: SYS_CFG_MAGIC,
        values: HashMap::new(),
    }));
    unsafe { *out = store.cast() };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_sys_cfg_put(cfg: *mut c_void) -> c_int {
    if cfg.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let valid = unsafe { store_mut(cfg) }.is_some();
    if !valid {
        return MPP_NOK;
    }
    unsafe { drop(Box::from_raw(cfg.cast::<SysCfgStore>())) };
    MPP_OK
}

fn set(cfg: *mut c_void, name: *const c_char, value: CfgValue) -> c_int {
    let Some(name) = (unsafe { key_name(name) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let Some(store) = store_mut(cfg) else {
        return MPP_NOK;
    };
    store.values.insert(name, value);
    MPP_OK
}

fn get(
    cfg: *mut c_void,
    name: *const c_char,
    want: fn(CfgValue) -> Option<CfgValue>,
) -> Option<CfgValue> {
    let name = unsafe { key_name(name) }?;
    let store = store_mut(cfg)?;
    store.values.get(&name).copied().and_then(want)
}

macro_rules! set_fn {
    ($name:ident, $variant:ident, $ty:ty) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(cfg: *mut c_void, name: *const c_char, value: $ty) -> c_int {
            set(cfg, name, CfgValue::$variant(value))
        }
    };
}

macro_rules! get_fn {
    ($name:ident, $variant:ident, $ty:ty) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(
            cfg: *mut c_void,
            name: *const c_char,
            value: *mut $ty,
        ) -> c_int {
            if value.is_null() {
                return MPP_ERR_NULL_PTR;
            }
            match get(cfg, name, |stored| match stored {
                CfgValue::$variant(v) => Some(CfgValue::$variant(v)),
                _ => None,
            }) {
                Some(CfgValue::$variant(v)) => {
                    unsafe { *value = v };
                    MPP_OK
                }
                _ => MPP_NOK,
            }
        }
    };
}

set_fn!(mpp_sys_cfg_set_s32, S32, i32);
set_fn!(mpp_sys_cfg_set_u32, U32, u32);
set_fn!(mpp_sys_cfg_set_s64, S64, i64);
set_fn!(mpp_sys_cfg_set_u64, U64, u64);
set_fn!(mpp_sys_cfg_set_ptr, Ptr, *mut c_void);

get_fn!(mpp_sys_cfg_get_s32, S32, i32);
get_fn!(mpp_sys_cfg_get_u32, U32, u32);
get_fn!(mpp_sys_cfg_get_s64, S64, i64);
get_fn!(mpp_sys_cfg_get_u64, U64, u64);
get_fn!(mpp_sys_cfg_get_ptr, Ptr, *mut c_void);

/// The vendor version recomputes the `dec_buf_chk` derived fields. This crate
/// does not claim that computation, so it refuses loudly.
#[no_mangle]
pub unsafe extern "C" fn mpp_sys_cfg_ioctl(_cfg: *mut c_void) -> c_int {
    eprintln!(
        "rk-mpp-rs: mpp_sys_cfg_ioctl is not supported (dec_buf_chk is not computed); \
         use the decoder/SDK geometry helpers instead"
    );
    MPP_NOK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_sys_dec_buf_chk_proc(_cfg: *mut c_void) -> c_int {
    MPP_NOK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_sys_cfg_st_get_h_stride(_cfg: *mut c_void) -> c_int {
    MPP_NOK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_sys_cfg_st_get_byte_stride(_cfg: *mut c_void) -> c_int {
    MPP_NOK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_sys_cfg_st_get_v_stride(_cfg: *mut c_void) -> c_int {
    MPP_NOK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_sys_cfg_st_get_size(_cfg: *mut c_void) -> c_int {
    MPP_NOK
}

#[no_mangle]
pub extern "C" fn mpp_sys_cfg_show() {
    eprintln!(
        "rk-mpp-rs: mpp_sys_cfg supports named typed storage; derived dec_buf_chk fields \
         (h_stride/size/fbc) are not computed"
    );
}
