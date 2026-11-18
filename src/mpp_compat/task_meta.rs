//! `mpp_task.h`'s `mpp_task_meta_*` helpers.
//!
//! The vendor runtime reaches a task's `MppMeta` through the task object. This
//! crate has no public task objects (the task queue is internal), so the same
//! helpers accept an `MppMeta` handle and delegate to `mpp_meta_*`. An
//! application that keeps its own meta handle keeps working after the switch;
//! passing a task pointer from another runtime returns `MPP_NOK`.

use std::ffi::c_int;

use super::objects::{
    mpp_meta_get_buffer, mpp_meta_get_frame, mpp_meta_get_packet, mpp_meta_get_ptr,
    mpp_meta_get_s32, mpp_meta_get_s64, mpp_meta_set_buffer, mpp_meta_set_frame,
    mpp_meta_set_packet, mpp_meta_set_ptr, mpp_meta_set_s32, mpp_meta_set_s64,
};
use std::ffi::c_void;

type Handle = *mut c_void;

const MPP_OK: c_int = 0;
const MPP_NOK: c_int = -1;
const MPP_ERR_NULL_PTR: c_int = -3;

macro_rules! set_delegate {
    ($name:ident, $target:ident, $ty:ty) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(task: Handle, key: c_int, value: $ty) -> c_int {
            if task.is_null() {
                return MPP_ERR_NULL_PTR;
            }
            unsafe { $target(task.cast::<std::ffi::c_void>() as Handle, key, value) }
        }
    };
}

set_delegate!(mpp_task_meta_set_s32, mpp_meta_set_s32, i32);
set_delegate!(mpp_task_meta_set_s64, mpp_meta_set_s64, i64);
set_delegate!(
    mpp_task_meta_set_ptr,
    mpp_meta_set_ptr,
    *mut std::ffi::c_void
);
set_delegate!(mpp_task_meta_set_frame, mpp_meta_set_frame, Handle);
set_delegate!(mpp_task_meta_set_packet, mpp_meta_set_packet, Handle);
set_delegate!(mpp_task_meta_set_buffer, mpp_meta_set_buffer, Handle);

macro_rules! get_delegate_default {
    ($name:ident, $target:ident, $ty:ty) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(
            task: Handle,
            key: c_int,
            value: *mut $ty,
            default_value: $ty,
        ) -> c_int {
            if task.is_null() || value.is_null() {
                return MPP_ERR_NULL_PTR;
            }
            let ret = unsafe { $target(task.cast::<std::ffi::c_void>() as Handle, key, value) };
            if ret != MPP_OK {
                unsafe { *value = default_value };
            }
            ret
        }
    };
}

get_delegate_default!(mpp_task_meta_get_s32, mpp_meta_get_s32, i32);
get_delegate_default!(mpp_task_meta_get_s64, mpp_meta_get_s64, i64);
get_delegate_default!(
    mpp_task_meta_get_ptr,
    mpp_meta_get_ptr,
    *mut std::ffi::c_void
);

macro_rules! get_delegate {
    ($name:ident, $target:ident, $ty:ty) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(task: Handle, key: c_int, value: *mut $ty) -> c_int {
            if task.is_null() || value.is_null() {
                return MPP_ERR_NULL_PTR;
            }
            unsafe { $target(task.cast::<std::ffi::c_void>() as Handle, key, value) }
        }
    };
}

get_delegate!(mpp_task_meta_get_frame, mpp_meta_get_frame, Handle);
get_delegate!(mpp_task_meta_get_packet, mpp_meta_get_packet, Handle);
get_delegate!(mpp_task_meta_get_buffer, mpp_meta_get_buffer, Handle);

const _: () = {
    let _ = MPP_NOK;
};
