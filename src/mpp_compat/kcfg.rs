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

pub(super) struct KcfgStore {
    magic: u64,
    kind: &'static str,
    type_id: u32,
    name: Option<String>,
    values: HashMap<String, Val>,
    flex: Flex,
}

fn store_mut<'a>(cfg: *mut c_void) -> Option<&'a mut KcfgStore> {
    let store = unsafe { cfg.cast::<KcfgStore>().as_mut() }?;
    (store.magic == KCFG_MAGIC).then_some(store)
}

fn new_store(kind: &'static str, type_id: u32, name: Option<String>) -> *mut c_void {
    Box::into_raw(Box::new(KcfgStore {
        magic: KCFG_MAGIC,
        kind,
        type_id,
        name,
        values: HashMap::new(),
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
    unsafe { *out = new_store("vdec", type_id, None) };
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
    unsafe { *out = new_store("vdec", 0, Some(name)) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_vdec_kcfg_deinit(cfg: *mut c_void) -> c_int {
    if cfg.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    if unsafe { store_mut(cfg) }.is_none() {
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
    show_store("vdec", cfg);
}

// ---- venc ------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mpp_venc_kcfg_init(out: *mut *mut c_void, type_id: u32) -> c_int {
    if out.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    unsafe { *out = new_store("venc", type_id, None) };
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
    unsafe { *out = new_store("venc", 0, Some(name)) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_venc_kcfg_deinit(cfg: *mut c_void) -> c_int {
    if cfg.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    if unsafe { store_mut(cfg) }.is_none() {
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
    show_store("venc", cfg);
}

/// Base of the flex area for a CTRL_CFG handle (4 KiB, zeroed).
#[no_mangle]
pub unsafe extern "C" fn mpp_venc_ctrl_flex_base(cfg: *mut c_void) -> *mut c_void {
    match store_mut(cfg) {
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
    let Some(store) = store_mut(cfg) else {
        return MPP_NOK;
    };
    let size = size as usize;
    if size > store.flex.buffer.len() {
        store.flex.buffer.resize(size, 0);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(data.cast::<u8>(), store.flex.buffer.as_mut_ptr(), size);
    }
    MPP_OK
}

fn show_store(kind: &str, cfg: *mut c_void) {
    let Some(store) = store_mut(cfg) else {
        eprintln!("rk-mpp-rs: {kind}_kcfg: invalid handle");
        return;
    };
    eprintln!(
        "rk-mpp-rs: {kind}_kcfg type={} name={:?} entries={} (storage only; not sent to the kernel)",
        store.type_id,
        store.name,
        store.values.len()
    );
}

const _: () = {
    let _ = MPP_NOK;
};
