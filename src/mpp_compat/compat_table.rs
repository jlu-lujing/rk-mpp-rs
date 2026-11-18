//! `mpp_compat.h` drop-in: the vendor's three runtime compatibility toggles.
//!
//! The table mirrors `osal/mpp_compat.c` so a caller that reads or updates
//! `MppCompat::value_usr` links and behaves the same way. The toggles affect
//! internal buffer sizing / async-input policy; this crate accepts and stores
//! them, and `MPP_COMPAT_INC_FBC_BUF_SIZE` is already the behaviour used by the
//! decoder pools.

use std::ffi::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicI32, Ordering};

/// `MppCompatId`.
const MPP_COMPAT_INC_FBC_BUF_SIZE: c_int = 0;
const MPP_COMPAT_ENC_ASYNC_INPUT: c_int = 1;
const MPP_COMPAT_DEC_FBC_HDR_256_ODD: c_int = 2;

/// `MppCompatType`.
const MPP_COMPAT_BOOL: c_int = 0;

#[repr(C)]
pub struct MppCompat {
    feature_id: c_int,
    feature_type: c_int,
    value_mpp: i32,
    /// `AtomicI32` has the layout of `RK_S32`; the vendor API mutates this
    /// field through a stable pointer, so interior mutability is required.
    value_usr: AtomicI32,
    name: *const c_char,
    next: *const MppCompat,
}

// SAFETY: the only mutable state is an atomic, and the string/next pointers
// refer to static data.
unsafe impl Sync for MppCompat {}

static INC_FBC_NAME: &[u8] = b"increase decoder fbc buffer size\0";
static ASYNC_INPUT_NAME: &[u8] = b"support encoder async input mode\0";
static FBC_HDR_256_NAME: &[u8] = b"set decoder fbc header stride to 256 odd align\0";

static COMPATS: [MppCompat; 3] = [
    MppCompat {
        feature_id: MPP_COMPAT_INC_FBC_BUF_SIZE,
        feature_type: MPP_COMPAT_BOOL,
        value_mpp: 1,
        value_usr: AtomicI32::new(1),
        name: INC_FBC_NAME.as_ptr().cast(),
        next: &COMPATS[1],
    },
    MppCompat {
        feature_id: MPP_COMPAT_ENC_ASYNC_INPUT,
        feature_type: MPP_COMPAT_BOOL,
        value_mpp: 1,
        value_usr: AtomicI32::new(0),
        name: ASYNC_INPUT_NAME.as_ptr().cast(),
        next: &COMPATS[2],
    },
    MppCompat {
        feature_id: MPP_COMPAT_DEC_FBC_HDR_256_ODD,
        feature_type: MPP_COMPAT_BOOL,
        value_mpp: 0,
        value_usr: AtomicI32::new(0),
        name: FBC_HDR_256_NAME.as_ptr().cast(),
        next: std::ptr::null(),
    },
];

#[no_mangle]
pub extern "C" fn mpp_compat_query() -> *mut MppCompat {
    COMPATS.as_ptr() as *mut MppCompat
}

#[no_mangle]
pub extern "C" fn mpp_compat_query_by_id(id: c_int) -> *mut MppCompat {
    COMPATS
        .iter()
        .find(|entry| entry.feature_id == id)
        .map(|entry| entry as *const MppCompat as *mut MppCompat)
        .unwrap_or(std::ptr::null_mut())
}

/// Set `value_usr`. Unknown ids return `MPP_NOK`; `BOOL` values are clamped to
/// 0/1 like the vendor runtime.
#[no_mangle]
pub unsafe extern "C" fn mpp_compat_update(compat: *mut MppCompat, value: i32) -> c_int {
    let Some(entry) = (unsafe { compat.as_ref() }) else {
        return -3; // MPP_ERR_NULL_PTR
    };
    if !COMPATS
        .iter()
        .any(|known| std::ptr::eq(known, entry as *const MppCompat))
    {
        return -6; // MPP_ERR_VALUE
    }
    entry.value_usr.store(value.clamp(0, 1), Ordering::Relaxed);
    0
}

#[no_mangle]
pub extern "C" fn mpp_compat_show() {
    for entry in &COMPATS {
        let name = if entry.name.is_null() {
            "<unnamed>".to_string()
        } else {
            unsafe { std::ffi::CStr::from_ptr(entry.name) }
                .to_string_lossy()
                .into_owned()
        };
        eprintln!(
            "mpp_compat[{}] {}: mpp={} user={}",
            entry.feature_id,
            name,
            entry.value_mpp,
            entry.value_usr.load(Ordering::Relaxed)
        );
    }
}

/// Current user value, used by the decoder pool policy.
pub fn compat_inc_fbc_buf_size() -> bool {
    COMPATS[MPP_COMPAT_INC_FBC_BUF_SIZE as usize]
        .value_usr
        .load(Ordering::Relaxed)
        != 0
}

const _: () = {
    // The fields the vendor struct exposes are `int`, `int`, `int`, `int`,
    // pointer, pointer on 32/64-bit ABIs.
    assert!(std::mem::size_of::<c_int>() == 4);
    let _ = std::mem::size_of::<*const c_void>();
};
