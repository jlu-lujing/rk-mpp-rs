//! Incremental drop-in compatibility surface for the official Rockchip MPI.

use std::ffi::{c_int, c_void};
use std::mem::size_of;

use crate::{encoder, sdk};

mod cfg;
mod objects;
mod standalone;

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

const MPP_VIDEO_CODING_MPEG2: c_int = 2;
const MPP_VIDEO_CODING_H263: c_int = 3;
const MPP_VIDEO_CODING_MPEG4: c_int = 4;
const MPP_VIDEO_CODING_AVC: c_int = 7;
const MPP_VIDEO_CODING_MJPEG: c_int = 8;
const MPP_VIDEO_CODING_VP8: c_int = 9;
const MPP_VIDEO_CODING_VP9: c_int = 10;
const MPP_VIDEO_CODING_HEVC: c_int = 0x0100_0004;
const MPP_VIDEO_CODING_AV1: c_int = 0x0100_0008;

const MPP_DEC_SET_OUTPUT_FORMAT: c_int = 3_211_274;
const MPP_DEC_SET_DISABLE_ERROR: c_int = 3_211_275;
const MPP_DEC_SET_IMMEDIATE_OUT: c_int = 3_211_276;
const MPP_DEC_SET_CFG: c_int = 3_211_777;
const MPP_DEC_GET_CFG: c_int = 3_211_778;

const MPP_ENC_SET_CFG: c_int = 3_276_801;
const MPP_ENC_GET_CFG: c_int = 3_276_802;
const MPP_ENC_SET_IDR_FRAME: c_int = 3_276_809;
const MPP_ENC_GET_HDR_SYNC: c_int = 3_276_813;
const MPP_ENC_SET_SEI_CFG: c_int = 3_276_815;
const MPP_ENC_SET_HEADER_MODE: c_int = 3_309_569;
const MPP_ENC_SET_OSD_PLT_CFG: c_int = 3_310_593;
const MPP_ENC_SET_RC_API: c_int = 3_277_317;
const MPP_SET_OUTPUT_TIMEOUT: c_int = 2_097_159;

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
    decoder: Option<sdk::Decoder>,
    encoder: Option<encoder::Encoder>,
    eos_submitted: bool,
    eos_delivered: bool,
    pending_output_frame: MppFrame,
    pending_input_packet: MppPacket,
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

fn nv12_input(frame: &objects::FrameInputView) -> Result<Vec<u8>, c_int> {
    let luma_size = frame
        .hor_stride
        .checked_mul(frame.ver_stride)
        .ok_or(MPP_ERR_VALUE)?;
    let output_size = frame
        .width
        .checked_mul(frame.height)
        .and_then(|luma| luma.checked_mul(3).map(|bytes| bytes / 2))
        .ok_or(MPP_ERR_VALUE)?;
    if frame.data.is_null()
        || frame.len < luma_size
        || frame.hor_stride < frame.width
        || frame.height % 2 != 0
    {
        return Err(MPP_ERR_VALUE);
    }
    let bytes = unsafe { std::slice::from_raw_parts(frame.data, frame.len) };
    let mut output = vec![0u8; output_size];
    for row in 0..frame.height {
        let source = row * frame.hor_stride;
        let destination = row * frame.width;
        output[destination..destination + frame.width]
            .copy_from_slice(&bytes[source..source + frame.width]);
    }
    let uv_source = luma_size;
    let uv_destination = frame.width * frame.height;
    for row in 0..frame.height / 2 {
        let source = uv_source + row * frame.hor_stride;
        let destination = uv_destination + row * frame.width;
        output[destination..destination + frame.width]
            .copy_from_slice(&bytes[source..source + frame.width]);
    }
    Ok(output)
}

unsafe extern "C" fn compat_decode(ctx: MppCtx, packet: MppPacket, frame: *mut MppFrame) -> c_int {
    let put = unsafe { compat_decode_put_packet(ctx, packet) };
    if put != MPP_OK {
        return put;
    }
    unsafe { compat_decode_get_frame(ctx, frame) }
}

unsafe extern "C" fn compat_decode_put_packet(ctx: MppCtx, packet: MppPacket) -> c_int {
    let Some(context) = (unsafe { context_mut(ctx) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if context.ctx_type != MPP_CTX_DEC {
        return MPP_ERR_VALUE;
    }
    let Some(decoder) = context.decoder.as_mut() else {
        return MPP_ERR_VALUE;
    };
    let Some(bytes) = (unsafe { objects::packet_bytes(packet) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let Some((pts, dts, eos)) = (unsafe { objects::packet_metadata(packet) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let output_frame =
        unsafe { objects::packet_output_frame(packet) }.unwrap_or(std::ptr::null_mut());
    if !output_frame.is_null() {
        if let Some(buffer) = unsafe { objects::frame_output_dmabuf(output_frame) } {
            if matches!(
                context.coding,
                MPP_VIDEO_CODING_AVC
                    | MPP_VIDEO_CODING_MJPEG
                    | MPP_VIDEO_CODING_VP9
                    | MPP_VIDEO_CODING_AV1
                    | MPP_VIDEO_CODING_HEVC
            ) {
                if let Err(error) = decoder.import_output_dmabuf(buffer) {
                    eprintln!("rk-mpp-rs mpp: output buffer import failed: {error}");
                    return MPP_NOK;
                }
            }
        }
    }
    let flags = if eos {
        sdk::PacketFlags::empty().with_eos()
    } else {
        sdk::PacketFlags::empty()
    };
    let sdk_packet = sdk::Packet::new(bytes)
        .with_pts(pts)
        .with_dts(dts)
        .with_flags(flags);
    if decoder.submit_packet(sdk_packet).is_err() {
        return MPP_NOK;
    }
    if !unsafe { objects::packet_consume(packet) } {
        return MPP_ERR_VALUE;
    }
    context.eos_submitted |= eos;
    context.pending_output_frame = output_frame;
    context.pending_input_packet = packet;
    MPP_OK
}

unsafe extern "C" fn compat_decode_get_frame(ctx: MppCtx, frame: *mut MppFrame) -> c_int {
    if frame.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(context) = (unsafe { context_mut(ctx) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if context.ctx_type != MPP_CTX_DEC {
        return MPP_ERR_VALUE;
    }
    let Some(decoder) = context.decoder.as_mut() else {
        return MPP_ERR_VALUE;
    };
    unsafe { *frame = std::ptr::null_mut() };
    if let Some(decoded) = decoder.receive_frame() {
        let frame_object = if context.pending_output_frame.is_null() {
            let Ok(frame) = (unsafe { objects::frame_from_sdk(decoded) }) else {
                return MPP_NOK;
            };
            frame
        } else {
            let frame = context.pending_output_frame;
            if unsafe { objects::frame_fill_from_sdk(frame, decoded) }.is_err() {
                return MPP_NOK;
            }
            context.pending_output_frame = std::ptr::null_mut();
            frame
        };
        if !context.pending_input_packet.is_null() {
            let _ = unsafe {
                objects::frame_set_input_packet(frame_object, context.pending_input_packet)
            };
        }
        unsafe { *frame = frame_object };
        return MPP_OK;
    }
    if context.eos_submitted && decoder.is_finished() && !context.eos_delivered {
        context.eos_delivered = true;
        unsafe { *frame = objects::frame_eos() };
    }
    MPP_OK
}

unsafe extern "C" fn compat_encode(ctx: MppCtx, frame: MppFrame, packet: *mut MppPacket) -> c_int {
    let put = unsafe { compat_encode_put_frame(ctx, frame) };
    if put != MPP_OK {
        return put;
    }
    unsafe { compat_encode_get_packet(ctx, packet) }
}

unsafe extern "C" fn compat_encode_put_frame(ctx: MppCtx, frame: MppFrame) -> c_int {
    let Some(context) = (unsafe { context_mut(ctx) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if context.ctx_type != MPP_CTX_ENC {
        return MPP_ERR_VALUE;
    }
    let Some(encoder) = context.encoder.as_mut() else {
        return MPP_ERR_VALUE;
    };
    let Some(frame) = (unsafe { objects::frame_input_view(frame) }) else {
        eprintln!("rk-mpp-rs mpp: encode frame has no readable buffer");
        return MPP_ERR_NULL_PTR;
    };
    let motion_info = frame.motion_info;
    if frame.eos {
        return MPP_OK;
    }
    if frame.width == 0 || frame.height == 0 || frame.format != 0 {
        eprintln!(
            "rk-mpp-rs mpp: encode frame has invalid geometry/format {}x{} fmt {}",
            frame.width, frame.height, frame.format
        );
        return MPP_ERR_VALUE;
    }
    let input = match nv12_input(&frame) {
        Ok(input) => input,
        Err(ret) => {
            eprintln!("rk-mpp-rs mpp: encode frame is not a complete NV12 picture");
            return ret;
        }
    };
    let input = encoder::InputFrame::new(&input)
        .with_pts(frame.pts)
        .with_dts(frame.dts);
    match encoder.submit_frame(input) {
        Ok(_) => {
            if !motion_info.is_null() {
                if let Some(data) = encoder.take_motion_info() {
                    if !unsafe { objects::buffer_replace(motion_info, &data) } {
                        eprintln!(
                            "rk-mpp-rs mpp: KEY_MOTION_INFO buffer is smaller than the encoder output"
                        );
                        return MPP_NOK;
                    }
                }
            }
            MPP_OK
        }
        Err(error) => {
            eprintln!("rk-mpp-rs mpp: encode frame failed: {error}");
            MPP_NOK
        }
    }
}

unsafe extern "C" fn compat_encode_get_packet(ctx: MppCtx, packet: *mut MppPacket) -> c_int {
    if packet.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(context) = (unsafe { context_mut(ctx) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if context.ctx_type != MPP_CTX_ENC {
        return MPP_ERR_VALUE;
    }
    let Some(encoder) = context.encoder.as_mut() else {
        return MPP_ERR_VALUE;
    };
    unsafe { *packet = std::ptr::null_mut() };
    if let Some(encoded) = encoder.receive_packet() {
        let pts = encoded.pts();
        let dts = encoded.dts();
        unsafe {
            *packet = objects::packet_from_bytes(encoded.into_data(), pts, dts);
        }
    }
    MPP_OK
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
    let Some(context) = (unsafe { context_mut(ctx) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if let Some(decoder) = context.decoder.as_mut() {
        if decoder.reset().is_err() {
            return MPP_NOK;
        }
    }
    context.encoder = None;
    context.eos_submitted = false;
    context.eos_delivered = false;
    context.pending_output_frame = std::ptr::null_mut();
    context.pending_input_packet = std::ptr::null_mut();
    MPP_OK
}

unsafe extern "C" fn compat_control(ctx: MppCtx, command: c_int, param: MppParam) -> c_int {
    let Some(context) = (unsafe { context_mut(ctx) }) else {
        return MPP_ERR_NULL_PTR;
    };
    match command {
        // Configuration is consumed by the surrounding SDK session at
        // mpp_init. Keep the vendor MppDecCfg object valid and accept the
        // values the official simple decode loop sets.
        MPP_DEC_SET_CFG
        | MPP_DEC_GET_CFG
        | MPP_DEC_SET_DISABLE_ERROR
        | MPP_DEC_SET_IMMEDIATE_OUT => MPP_OK,
        MPP_DEC_SET_OUTPUT_FORMAT => {
            if param.is_null() {
                return MPP_ERR_NULL_PTR;
            }
            // The compatible session currently exposes NV12.
            let format = unsafe { *param.cast::<u32>() };
            if format == 0 {
                MPP_OK
            } else {
                MPP_NOK
            }
        }
        MPP_ENC_SET_CFG => {
            if context.ctx_type != MPP_CTX_ENC || param.is_null() {
                return MPP_ERR_VALUE;
            }
            let config = match unsafe { cfg::encoder_config(param) } {
                Ok(config) => config,
                Err(error) => {
                    eprintln!("rk-mpp-rs mpp: encoder config rejected: {error}");
                    return MPP_NOK;
                }
            };
            match encoder::Encoder::new_mpi_compat(config) {
                Ok(mut encoder) => {
                    if context.coding == MPP_VIDEO_CODING_AVC {
                        if matches!(config.profile(), 0 | 66) {
                            if encoder.force_reference_slot(Some(0)).is_err() {
                                eprintln!(
                                    "rk-mpp-rs mpp: default MPP H.264 reference schedule is unavailable"
                                );
                                return MPP_NOK;
                            }
                        } else if config.profile() != 100 {
                            eprintln!(
                                "rk-mpp-rs mpp: H.264 profile {} is not supported",
                                config.profile()
                            );
                            return MPP_NOK;
                        }
                    }
                    context.encoder = Some(encoder);
                    MPP_OK
                }
                Err(error) => {
                    eprintln!("rk-mpp-rs mpp: encoder open failed: {error}");
                    MPP_NOK
                }
            }
        }
        MPP_ENC_GET_CFG
        | MPP_ENC_SET_IDR_FRAME
        | MPP_ENC_SET_SEI_CFG
        | MPP_ENC_SET_HEADER_MODE
        | MPP_ENC_SET_OSD_PLT_CFG
        | MPP_ENC_SET_RC_API
        | MPP_SET_OUTPUT_TIMEOUT => MPP_OK,
        MPP_ENC_GET_HDR_SYNC => {
            if context.ctx_type != MPP_CTX_ENC {
                return MPP_ERR_VALUE;
            }
            MPP_OK
        }
        _ => {
            eprintln!("rk-mpp-rs mpp: unsupported control command {command}");
            MPP_NOK
        }
    }
}

fn coding_supported(ctx_type: c_int, coding: c_int) -> bool {
    match ctx_type {
        MPP_CTX_DEC => matches!(
            coding,
            MPP_VIDEO_CODING_MPEG2
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
            decoder: None,
            encoder: None,
            eos_submitted: false,
            eos_delivered: false,
            pending_output_frame: std::ptr::null_mut(),
            pending_input_packet: std::ptr::null_mut(),
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
    let decoder = if ctx_type == MPP_CTX_DEC {
        let Some(codec) = sdk_codec(coding) else {
            return MPP_NOK;
        };
        let config = sdk::DecoderConfig::new(codec)
            .with_output_format(sdk::PixelFormat::Nv12)
            .with_dmabuf_output(matches!(
                coding,
                MPP_VIDEO_CODING_AVC
                    | MPP_VIDEO_CODING_MJPEG
                    | MPP_VIDEO_CODING_VP9
                    | MPP_VIDEO_CODING_HEVC
                    | MPP_VIDEO_CODING_AV1
            ));
        match sdk::Decoder::new(config) {
            Ok(decoder) => Some(decoder),
            Err(_) => return MPP_NOK,
        }
    } else {
        None
    };
    context.ctx_type = ctx_type;
    context.coding = coding;
    context.decoder = decoder;
    context.encoder = None;
    context.eos_submitted = false;
    context.eos_delivered = false;
    context.pending_output_frame = std::ptr::null_mut();
    context.pending_input_packet = std::ptr::null_mut();
    MPP_OK
}

fn sdk_codec(coding: c_int) -> Option<sdk::Codec> {
    match coding {
        MPP_VIDEO_CODING_MPEG2 => Some(sdk::Codec::Mpeg2),
        MPP_VIDEO_CODING_H263 => Some(sdk::Codec::H263),
        MPP_VIDEO_CODING_MPEG4 => Some(sdk::Codec::Mpeg4),
        MPP_VIDEO_CODING_AVC => Some(sdk::Codec::H264),
        MPP_VIDEO_CODING_MJPEG => Some(sdk::Codec::Mjpeg),
        MPP_VIDEO_CODING_VP8 => Some(sdk::Codec::Vp8),
        MPP_VIDEO_CODING_VP9 => Some(sdk::Codec::Vp9),
        MPP_VIDEO_CODING_HEVC => Some(sdk::Codec::H265),
        MPP_VIDEO_CODING_AV1 => Some(sdk::Codec::Av1),
        _ => None,
    }
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
