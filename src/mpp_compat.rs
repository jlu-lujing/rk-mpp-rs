//! Incremental drop-in compatibility surface for the official Rockchip MPI.

use std::ffi::{c_int, c_void};
use std::mem::size_of;

type MppCtx = *mut c_void;
type MppFrame = *mut c_void;
type MppPacket = *mut c_void;
type MppTask = *mut c_void;
type MppParam = *mut c_void;

const MPP_OK: c_int = 0;
const MPP_NOK: c_int = -1;
const MPP_ERR_NULL_PTR: c_int = -3;
const MPP_ERR_VALUE: c_int = -6;

const MPP_CTX_DEC: c_int = 0;
const MPP_CTX_ENC: c_int = 1;
const MPP_CTX_ISP: c_int = 2;
const MPP_CTX_BUTT: c_int = 3;

const MPP_VIDEO_CODING_AUTO_DETECT: c_int = 1;
const MPP_VIDEO_CODING_MPEG2: c_int = 2;
const MPP_VIDEO_CODING_H263: c_int = 3;
const MPP_VIDEO_CODING_MPEG4: c_int = 4;
const MPP_VIDEO_CODING_AVC: c_int = 7;
const MPP_VIDEO_CODING_MJPEG: c_int = 8;
const MPP_VIDEO_CODING_VP8: c_int = 9;
const MPP_VIDEO_CODING_VP9: c_int = 10;
const MPP_VIDEO_CODING_HEVC: c_int = 0x0100_0004;
const MPP_VIDEO_CODING_AV1: c_int = 0x0100_0008;

const CONTEXT_MAGIC: u64 = 0x4d50_5052_5343_5458;

type DecodeFn = unsafe extern "C" fn(MppCtx, MppPacket, *mut MppFrame) -> c_int;
type DecodePutFn = unsafe extern "C" fn(MppCtx, MppPacket) -> c_int;
type DecodeGetFn = unsafe extern "C" fn(MppCtx, *mut MppFrame) -> c_int;
type EncodeFn = unsafe extern "C" fn(MppCtx, MppFrame, *mut MppPacket) -> c_int;
type EncodePutFn = unsafe extern "C" fn(MppCtx, MppFrame) -> c_int;
type EncodeGetFn = unsafe extern "C" fn(MppCtx, *mut MppPacket) -> c_int;
type IspFn = unsafe extern "C" fn(MppCtx, MppFrame, MppFrame) -> c_int;
type IspPutFn = unsafe extern "C" fn(MppCtx, MppFrame) -> c_int;
type IspGetFn = unsafe extern "C" fn(MppCtx, *mut MppFrame) -> c_int;
type PollFn = unsafe extern "C" fn(MppCtx, c_int, c_int) -> c_int;
type TaskFn = unsafe extern "C" fn(MppCtx, c_int, *mut MppTask) -> c_int;
type EnqueueFn = unsafe extern "C" fn(MppCtx, c_int, MppTask) -> c_int;
type ResetFn = unsafe extern "C" fn(MppCtx) -> c_int;
type ControlFn = unsafe extern "C" fn(MppCtx, c_int, MppParam) -> c_int;

#[repr(C)]
pub struct MppApi {
    size: u32,
    version: u32,
    decode: Option<DecodeFn>,
    decode_put_packet: Option<DecodePutFn>,
    decode_get_frame: Option<DecodeGetFn>,
    encode: Option<EncodeFn>,
    encode_put_frame: Option<EncodePutFn>,
    encode_get_packet: Option<EncodeGetFn>,
    isp: Option<IspFn>,
    isp_put_frame: Option<IspPutFn>,
    isp_get_frame: Option<IspGetFn>,
    poll: Option<PollFn>,
    dequeue: Option<TaskFn>,
    enqueue: Option<EnqueueFn>,
    reset: Option<ResetFn>,
    control: Option<ControlFn>,
    reserv: [u32; 16],
}

static MPP_API: MppApi = MppApi {
    size: size_of::<MppApi>() as u32,
    version: 0,
    decode: Some(compat_decode),
    decode_put_packet: Some(compat_decode_put_packet),
    decode_get_frame: Some(compat_decode_get_frame),
    encode: Some(compat_encode),
    encode_put_frame: Some(compat_encode_put_frame),
    encode_get_packet: Some(compat_encode_get_packet),
    isp: Some(compat_isp),
    isp_put_frame: Some(compat_isp_put_frame),
    isp_get_frame: Some(compat_isp_get_frame),
    poll: Some(compat_poll),
    dequeue: Some(compat_dequeue),
    enqueue: Some(compat_enqueue),
    reset: Some(compat_reset),
    control: Some(compat_control),
    reserv: [0; 16],
};

#[repr(C)]
struct CompatContext {
    magic: u64,
    ctx_type: c_int,
    coding: c_int,
}

unsafe fn context_mut<'a>(ctx: MppCtx) -> Option<&'a mut CompatContext> {
    if ctx.is_null() {
        return None;
    }
    // SAFETY: the public C contract requires a live MppCtx returned by
    // mpp_create(). The magic check catches stale or foreign pointers that are
    // still readable, without pretending arbitrary pointers are valid.
    let context = unsafe { &mut *(ctx.cast::<CompatContext>()) };
    (context.magic == CONTEXT_MAGIC).then_some(context)
}

unsafe extern "C" fn compat_decode(
    _ctx: MppCtx,
    _packet: MppPacket,
    _frame: *mut MppFrame,
) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_decode_put_packet(_ctx: MppCtx, _packet: MppPacket) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_decode_get_frame(_ctx: MppCtx, _frame: *mut MppFrame) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_encode(
    _ctx: MppCtx,
    _frame: MppFrame,
    _packet: *mut MppPacket,
) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_encode_put_frame(_ctx: MppCtx, _frame: MppFrame) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_encode_get_packet(_ctx: MppCtx, _packet: *mut MppPacket) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_isp(_ctx: MppCtx, _dst: MppFrame, _src: MppFrame) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_isp_put_frame(_ctx: MppCtx, _frame: MppFrame) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_isp_get_frame(_ctx: MppCtx, _frame: *mut MppFrame) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_poll(_ctx: MppCtx, _port: c_int, _timeout: c_int) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_dequeue(_ctx: MppCtx, _port: c_int, _task: *mut MppTask) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_enqueue(_ctx: MppCtx, _port: c_int, _task: MppTask) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_reset(ctx: MppCtx) -> c_int {
    if unsafe { context_mut(ctx) }.is_some() {
        MPP_OK
    } else {
        MPP_ERR_NULL_PTR
    }
}

unsafe extern "C" fn compat_control(ctx: MppCtx, _command: c_int, _param: MppParam) -> c_int {
    if unsafe { context_mut(ctx) }.is_some() {
        MPP_NOK
    } else {
        MPP_ERR_NULL_PTR
    }
}

fn coding_supported(ctx_type: c_int, coding: c_int) -> bool {
    match ctx_type {
        MPP_CTX_DEC => matches!(
            coding,
            MPP_VIDEO_CODING_AUTO_DETECT
                | MPP_VIDEO_CODING_MPEG2
                | MPP_VIDEO_CODING_H263
                | MPP_VIDEO_CODING_MPEG4
                | MPP_VIDEO_CODING_AVC
                | MPP_VIDEO_CODING_MJPEG
                | MPP_VIDEO_CODING_VP8
                | MPP_VIDEO_CODING_VP9
                | MPP_VIDEO_CODING_HEVC
                | MPP_VIDEO_CODING_AV1
        ),
        MPP_CTX_ENC => matches!(
            coding,
            MPP_VIDEO_CODING_AVC | MPP_VIDEO_CODING_MJPEG | MPP_VIDEO_CODING_HEVC
        ),
        _ => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_create(ctx: *mut MppCtx, mpi: *mut *mut MppApi) -> c_int {
    if ctx.is_null() || mpi.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: the two out pointers were checked non-null.
    unsafe {
        *ctx = std::ptr::null_mut();
        *mpi = std::ptr::null_mut();
        let context = Box::new(CompatContext {
            magic: CONTEXT_MAGIC,
            ctx_type: MPP_CTX_BUTT,
            coding: 0,
        });
        *ctx = Box::into_raw(context).cast();
        *mpi = std::ptr::addr_of!(MPP_API).cast_mut();
    }
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_init(ctx: MppCtx, ctx_type: c_int, coding: c_int) -> c_int {
    let Some(context) = (unsafe { context_mut(ctx) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if ctx_type >= MPP_CTX_BUTT || !coding_supported(ctx_type, coding) {
        return MPP_ERR_VALUE;
    }
    if ctx_type == MPP_CTX_ISP {
        return MPP_NOK;
    }
    context.ctx_type = ctx_type;
    context.coding = coding;
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_destroy(ctx: MppCtx) -> c_int {
    if ctx.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: the magic check validates a context allocated by mpp_create().
    let context = unsafe { &mut *(ctx.cast::<CompatContext>()) };
    if context.magic != CONTEXT_MAGIC {
        return MPP_ERR_VALUE;
    }
    context.magic = 0;
    // SAFETY: ownership is returned to Box exactly once after the magic was
    // consumed by this destroy call.
    drop(unsafe { Box::from_raw(ctx.cast::<CompatContext>()) });
    MPP_OK
}

#[no_mangle]
pub extern "C" fn mpp_check_support_format(ctx_type: c_int, coding: c_int) -> c_int {
    if coding_supported(ctx_type, coding) {
        MPP_OK
    } else {
        MPP_NOK
    }
}

#[no_mangle]
pub extern "C" fn mpp_show_support_format() {
    println!("rk-mpp-rs compatibility formats:");
    println!("  dec: MPEG2 H263 MPEG4 AVC MJPEG VP8 VP9 HEVC AV1");
    println!("  enc: AVC MJPEG HEVC");
}

#[no_mangle]
pub extern "C" fn mpp_show_color_format() {
    println!("rk-mpp-rs color formats: NV12 I420 I420P10");
}

const _: () = {
    assert!(size_of::<MppApi>() == 184);
};
