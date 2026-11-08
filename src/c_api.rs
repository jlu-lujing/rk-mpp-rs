//! C ABI for the stable decoder SDK.

use std::ffi::{c_char, c_int};
use std::mem::size_of;

use crate::sdk;

pub const RK_MPP_OK: c_int = 0;
pub const RK_MPP_AGAIN: c_int = 1;
pub const RK_MPP_EOS: c_int = 2;
pub const RK_MPP_ERR_INVALID: c_int = -1;
pub const RK_MPP_ERR_UNSUPPORTED: c_int = -2;
pub const RK_MPP_ERR_PARSE: c_int = -3;
pub const RK_MPP_ERR_HARDWARE: c_int = -4;
pub const RK_MPP_ERR_RESOURCE: c_int = -5;
pub const RK_MPP_ERR_STATE: c_int = -6;
pub const RK_MPP_ERR_IO: c_int = -7;
pub const RK_MPP_ERR_TOO_LARGE: c_int = -8;

static VERSION: &[u8] = b"1.3\0";
static OK: &[u8] = b"ok\0";
static INVALID: &[u8] = b"invalid argument\0";
static UNSUPPORTED: &[u8] = b"unsupported operation\0";
static PARSE: &[u8] = b"bitstream parse error\0";
static HARDWARE: &[u8] = b"hardware or service error\0";
static RESOURCE: &[u8] = b"resource allocation error\0";
static STATE: &[u8] = b"invalid decoder state\0";
static IO: &[u8] = b"I/O error\0";
static TOO_LARGE: &[u8] = b"packet exceeds internal limit\0";
static AGAIN: &[u8] = b"no frame is ready\0";
static EOS: &[u8] = b"end of stream\0";
static UNKNOWN: &[u8] = b"unknown error\0";

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RkMppDecoderConfig {
    pub struct_size: u32,
    pub codec: u32,
    pub output_format: u32,
    pub flags: u32,
    pub max_reorder_frames: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
pub struct RkMppFrame {
    pub struct_size: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub flags: u32,
    pub reserved0: u32,
    pub pts: i64,
    pub dts: i64,
    pub poc: i32,
    pub err_info: u32,
    pub plane_data: [*const u8; 3],
    pub plane_sizes: [u64; 3],
    pub plane_strides: [u32; 3],
}

pub struct RkMppDecoder {
    inner: sdk::Decoder,
    last_frame: Option<sdk::Frame>,
}

fn error_code(error: &sdk::Error) -> c_int {
    match error {
        sdk::Error::InvalidInput(_) => RK_MPP_ERR_INVALID,
        sdk::Error::Unsupported(_) => RK_MPP_ERR_UNSUPPORTED,
        sdk::Error::Parse(_) => RK_MPP_ERR_PARSE,
        sdk::Error::Hardware(_) => RK_MPP_ERR_HARDWARE,
        sdk::Error::Resource(_) => RK_MPP_ERR_RESOURCE,
        sdk::Error::State(_) => RK_MPP_ERR_STATE,
        sdk::Error::TooLarge => RK_MPP_ERR_TOO_LARGE,
    }
}

fn clear_frame(frame: &mut RkMppFrame) {
    frame.width = 0;
    frame.height = 0;
    frame.format = 0;
    frame.flags = 0;
    frame.reserved0 = 0;
    frame.pts = 0;
    frame.dts = 0;
    frame.poc = 0;
    frame.err_info = 0;
    frame.plane_data = [std::ptr::null(); 3];
    frame.plane_sizes = [0; 3];
    frame.plane_strides = [0; 3];
}

/// Return the checked-in API version as `(major << 16) | minor`.
#[no_mangle]
pub extern "C" fn rk_mpp_api_version() -> u32 {
    sdk::API_VERSION
}

#[no_mangle]
pub extern "C" fn rk_mpp_api_version_string() -> *const c_char {
    VERSION.as_ptr().cast()
}

/// Initialize a configuration with explicit codec and output layout.
///
/// # Safety
/// `config` must point to writable storage for one config structure.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_decoder_config_init(
    config: *mut RkMppDecoderConfig,
    codec: u32,
    output_format: u32,
) -> c_int {
    if config.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    if sdk::Codec::from_c_value(codec).is_none()
        || sdk::PixelFormat::from_c_value(output_format).is_none()
    {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: the caller guarantees writable storage for one config.
    unsafe {
        config.write(RkMppDecoderConfig {
            struct_size: size_of::<RkMppDecoderConfig>() as u32,
            codec,
            output_format,
            flags: 0,
            max_reorder_frames: 0,
            reserved: [0; 3],
        });
    }
    RK_MPP_OK
}

/// Initialize a frame descriptor before it is passed to receive.
///
/// # Safety
/// `frame` must point to writable storage for one frame descriptor.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_frame_init(frame: *mut RkMppFrame) -> c_int {
    if frame.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: the caller guarantees writable storage for one frame.
    unsafe {
        frame.write(RkMppFrame {
            struct_size: size_of::<RkMppFrame>() as u32,
            width: 0,
            height: 0,
            format: 0,
            flags: 0,
            reserved0: 0,
            pts: 0,
            dts: 0,
            poc: 0,
            err_info: 0,
            plane_data: [std::ptr::null(); 3],
            plane_sizes: [0; 3],
            plane_strides: [0; 3],
        });
    }
    RK_MPP_OK
}

/// Create a persistent decoder session.
///
/// # Safety
/// `config` must point to a valid configuration and `out_decoder` must point
/// to writable pointer storage.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_decoder_create(
    config: *const RkMppDecoderConfig,
    out_decoder: *mut *mut RkMppDecoder,
) -> c_int {
    if config.is_null() || out_decoder.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: both pointers are validated and caller guarantees their storage.
    unsafe {
        *out_decoder = std::ptr::null_mut();
        let config = &*config;
        if config.struct_size != size_of::<RkMppDecoderConfig>() as u32 {
            return RK_MPP_ERR_INVALID;
        }
        if config.flags != 0 {
            return RK_MPP_ERR_UNSUPPORTED;
        }
        let Some(codec) = sdk::Codec::from_c_value(config.codec) else {
            return RK_MPP_ERR_INVALID;
        };
        let Some(output_format) = sdk::PixelFormat::from_c_value(config.output_format) else {
            return RK_MPP_ERR_INVALID;
        };
        let decoder_config = sdk::DecoderConfig::new(codec)
            .with_output_format(output_format)
            .with_max_reorder_frames(config.max_reorder_frames);
        match sdk::Decoder::new(decoder_config) {
            Ok(inner) => {
                *out_decoder = Box::into_raw(Box::new(RkMppDecoder {
                    inner,
                    last_frame: None,
                }));
                RK_MPP_OK
            }
            Err(error) => error_code(&error),
        }
    }
}

/// Submit one encoded packet. Input bytes are copied before this call returns.
///
/// # Safety
/// `decoder` must be a live handle. `data` must point to `size` readable bytes
/// unless `size` is zero.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_decoder_submit_packet(
    decoder: *mut RkMppDecoder,
    data: *const u8,
    size: u64,
    pts: i64,
    dts: i64,
    flags: u32,
) -> c_int {
    if decoder.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    let Ok(size) = usize::try_from(size) else {
        return RK_MPP_ERR_TOO_LARGE;
    };
    if size != 0 && data.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: the length conversion and null check above enforce the borrow.
    let bytes = if size == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, size) }
    };
    let Some(flags) = sdk::PacketFlags::from_bits(flags) else {
        return RK_MPP_ERR_INVALID;
    };
    let packet = sdk::Packet::new(bytes)
        .with_pts(pts)
        .with_dts(dts)
        .with_flags(flags);
    // SAFETY: decoder is a live handle from create and has exclusive use for
    // this call under the documented C contract.
    match unsafe { &mut *decoder }.inner.submit_packet(packet) {
        Ok(_) => RK_MPP_OK,
        Err(error) => error_code(&error),
    }
}

/// Receive the next decoded frame.
///
/// The frame data remains valid until `rk_mpp_frame_release` or decoder close.
///
/// # Safety
/// `decoder` must be a live handle and `frame` must point to an initialized
/// writable frame descriptor.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_decoder_receive_frame(
    decoder: *mut RkMppDecoder,
    frame: *mut RkMppFrame,
) -> c_int {
    if decoder.is_null() || frame.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: caller guarantees both live pointers.
    let decoder = unsafe { &mut *decoder };
    let frame = unsafe { &mut *frame };
    if frame.struct_size != size_of::<RkMppFrame>() as u32 {
        return RK_MPP_ERR_INVALID;
    }
    if decoder.last_frame.is_some() {
        return RK_MPP_ERR_STATE;
    }
    let Some(decoded) = decoder.inner.receive_frame() else {
        clear_frame(frame);
        return if decoder.inner.is_finished() {
            RK_MPP_EOS
        } else {
            RK_MPP_AGAIN
        };
    };
    clear_frame(frame);
    frame.width = decoded.width() as u32;
    frame.height = decoded.height() as u32;
    frame.format = decoded.format().c_value();
    frame.pts = decoded.pts();
    frame.dts = decoded.dts();
    frame.poc = decoded.poc();
    frame.err_info = decoded.err_info();
    for index in 0..3 {
        if let Some(plane) = decoded.plane(index) {
            frame.plane_data[index] = plane.data.as_ptr();
            frame.plane_sizes[index] = plane.data.len() as u64;
            frame.plane_strides[index] = plane.stride as u32;
        }
    }
    decoder.last_frame = Some(decoded);
    RK_MPP_OK
}

/// Release the frame returned by the last receive call.
///
/// # Safety
/// `decoder` and `frame` follow the same lifetime rules as receive.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_frame_release(decoder: *mut RkMppDecoder, frame: *mut RkMppFrame) {
    if decoder.is_null() {
        return;
    }
    // SAFETY: decoder is live under the call contract.
    let decoder = unsafe { &mut *decoder };
    decoder.last_frame = None;
    if !frame.is_null() {
        // SAFETY: caller guarantees writable frame storage.
        let frame = unsafe { &mut *frame };
        clear_frame(frame);
    }
}

/// Flush buffered packets/reordering and make display-ready frames available.
///
/// # Safety
/// `decoder` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_decoder_flush(decoder: *mut RkMppDecoder) -> c_int {
    if decoder.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: decoder is live under the call contract.
    match unsafe { &mut *decoder }.inner.flush() {
        Ok(_) => RK_MPP_OK,
        Err(error) => error_code(&error),
    }
}

/// Recreate the codec session and discard queued packets/frames.
///
/// # Safety
/// `decoder` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_decoder_reset(decoder: *mut RkMppDecoder) -> c_int {
    if decoder.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: decoder is live under the call contract.
    let decoder = unsafe { &mut *decoder };
    decoder.last_frame = None;
    match decoder.inner.reset() {
        Ok(()) => RK_MPP_OK,
        Err(error) => error_code(&error),
    }
}

/// Recover the hardware session without discarding codec state where possible.
///
/// # Safety
/// `decoder` must be a live handle.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_decoder_recover(decoder: *mut RkMppDecoder) -> c_int {
    if decoder.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: decoder is live under the call contract.
    match unsafe { &mut *decoder }.inner.recover() {
        Ok(()) => RK_MPP_OK,
        Err(error) => error_code(&error),
    }
}

/// Read the last H.264/H.265/VP9/AV1 job status words. MJPEG returns unsupported.
///
/// # Safety
/// `decoder` must be a live handle; non-null output pointers must be writable.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_decoder_status(
    decoder: *mut RkMppDecoder,
    status_a: *mut u32,
    status_b: *mut u32,
    status_c: *mut u32,
) -> c_int {
    if decoder.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: decoder is live under the call contract.
    let Some(status) = (unsafe { &*decoder }).inner.status() else {
        return RK_MPP_ERR_UNSUPPORTED;
    };
    // SAFETY: non-null out pointers are writable per the C contract.
    unsafe {
        if !status_a.is_null() {
            *status_a = status.0;
        }
        if !status_b.is_null() {
            *status_b = status.1;
        }
        if !status_c.is_null() {
            *status_c = status.2;
        }
    }
    RK_MPP_OK
}

/// Destroy a decoder handle. Null is ignored.
///
/// # Safety
/// `decoder` must come from create and must not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_decoder_close(decoder: *mut RkMppDecoder) {
    if !decoder.is_null() {
        // SAFETY: caller transfers ownership and guarantees no later use.
        drop(unsafe { Box::from_raw(decoder) });
    }
}

#[no_mangle]
pub extern "C" fn rk_mpp_strerror(code: c_int) -> *const c_char {
    let bytes = match code {
        RK_MPP_OK => OK,
        RK_MPP_AGAIN => AGAIN,
        RK_MPP_EOS => EOS,
        RK_MPP_ERR_INVALID => INVALID,
        RK_MPP_ERR_UNSUPPORTED => UNSUPPORTED,
        RK_MPP_ERR_PARSE => PARSE,
        RK_MPP_ERR_HARDWARE => HARDWARE,
        RK_MPP_ERR_RESOURCE => RESOURCE,
        RK_MPP_ERR_STATE => STATE,
        RK_MPP_ERR_IO => IO,
        RK_MPP_ERR_TOO_LARGE => TOO_LARGE,
        _ => UNKNOWN,
    };
    bytes.as_ptr().cast()
}

#[cfg(test)]
mod tests {
    use super::{RkMppDecoderConfig, RkMppFrame};
    use std::mem::size_of;

    #[test]
    fn public_structs_match_the_v1_header_layout() {
        assert_eq!(size_of::<RkMppDecoderConfig>(), 32);
        assert_eq!(size_of::<RkMppFrame>(), 112);
    }
}
