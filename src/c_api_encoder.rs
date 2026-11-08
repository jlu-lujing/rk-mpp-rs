//! C ABI for the stable synchronous encoder SDK.

use std::ffi::c_int;
use std::mem::size_of;

use crate::encoder;
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
pub const RK_MPP_ERR_TOO_LARGE: c_int = -8;

pub const RK_MPP_ENCODED_PACKET_FLAG_KEYFRAME: u32 = 1 << 0;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RkMppEncoderConfig {
    pub struct_size: u32,
    pub codec: u32,
    pub input_format: u32,
    pub width: u32,
    pub height: u32,
    pub profile: u32,
    pub level: u32,
    pub gop: u32,
    pub qp: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub jpeg_quality: u32,
    pub jpeg_restart_mcus: u32,
    pub flags: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
pub struct RkMppEncodedPacket {
    pub struct_size: u32,
    pub flags: u32,
    pub reserved0: u32,
    pub reserved1: u32,
    pub pts: i64,
    pub dts: i64,
    pub frame_num: i32,
    pub poc: i32,
    pub data: *const u8,
    pub size: u64,
    pub reserved: [u64; 2],
}

pub struct RkMppEncoder {
    inner: encoder::Encoder,
    last_packet: Option<encoder::Packet>,
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

fn clear_packet(packet: &mut RkMppEncodedPacket) {
    packet.flags = 0;
    packet.reserved0 = 0;
    packet.reserved1 = 0;
    packet.pts = 0;
    packet.dts = 0;
    packet.frame_num = 0;
    packet.poc = 0;
    packet.data = std::ptr::null();
    packet.size = 0;
    packet.reserved = [0; 2];
}

/// # Safety
/// `config` must point to writable storage for one encoder config.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_config_init(
    config: *mut RkMppEncoderConfig,
    codec: u32,
    width: u32,
    height: u32,
    input_format: u32,
) -> c_int {
    if config.is_null()
        || sdk::Codec::from_c_value(codec).is_none()
        || sdk::PixelFormat::from_c_value(input_format).is_none()
        || width == 0
        || height == 0
    {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: the caller guarantees writable config storage.
    unsafe {
        config.write(RkMppEncoderConfig {
            struct_size: size_of::<RkMppEncoderConfig>() as u32,
            codec,
            input_format,
            width,
            height,
            profile: 0,
            level: 30,
            gop: 1,
            qp: 26,
            fps_num: 30,
            fps_den: 1,
            jpeg_quality: 10,
            jpeg_restart_mcus: 0,
            flags: 0,
            reserved: [0; 3],
        });
    }
    RK_MPP_OK
}

/// # Safety
/// `packet` must point to writable storage for one packet descriptor.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoded_packet_init(packet: *mut RkMppEncodedPacket) -> c_int {
    if packet.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: the caller guarantees writable packet storage.
    unsafe {
        packet.write(RkMppEncodedPacket {
            struct_size: size_of::<RkMppEncodedPacket>() as u32,
            flags: 0,
            reserved0: 0,
            reserved1: 0,
            pts: 0,
            dts: 0,
            frame_num: 0,
            poc: 0,
            data: std::ptr::null(),
            size: 0,
            reserved: [0; 2],
        });
    }
    RK_MPP_OK
}

/// # Safety
/// `config` must point to a valid config and `out_encoder` to writable pointer
/// storage.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_create(
    config: *const RkMppEncoderConfig,
    out_encoder: *mut *mut RkMppEncoder,
) -> c_int {
    if config.is_null() || out_encoder.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: both pointers were validated and the caller owns their storage.
    unsafe {
        *out_encoder = std::ptr::null_mut();
        let config = &*config;
        if config.struct_size != size_of::<RkMppEncoderConfig>() as u32 {
            return RK_MPP_ERR_INVALID;
        }
        if config.flags != 0 || config.reserved != [0; 3] {
            return RK_MPP_ERR_INVALID;
        }
        let Some(codec) = sdk::Codec::from_c_value(config.codec) else {
            return RK_MPP_ERR_INVALID;
        };
        let Some(input_format) = sdk::PixelFormat::from_c_value(config.input_format) else {
            return RK_MPP_ERR_INVALID;
        };
        let encoder_config =
            encoder::EncoderConfig::new(codec, config.width as usize, config.height as usize)
                .with_input_format(input_format)
                .with_profile(u8::try_from(config.profile).unwrap_or(u8::MAX))
                .with_level(u8::try_from(config.level).unwrap_or(u8::MAX))
                .with_gop(config.gop)
                .with_qp(u8::try_from(config.qp).unwrap_or(u8::MAX))
                .with_fps(config.fps_num, config.fps_den)
                .with_jpeg_quality(config.jpeg_quality)
                .with_jpeg_restart_mcus(config.jpeg_restart_mcus);
        match encoder::Encoder::new(encoder_config) {
            Ok(inner) => {
                *out_encoder = Box::into_raw(Box::new(RkMppEncoder {
                    inner,
                    last_packet: None,
                }));
                RK_MPP_OK
            }
            Err(error) => error_code(&error),
        }
    }
}

/// Submit one tightly packed visible-size NV12/I420 frame.
///
/// # Safety
/// `encoder` must be live. `data` must address `size` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_submit_frame(
    encoder: *mut RkMppEncoder,
    data: *const u8,
    size: u64,
    pts: i64,
    dts: i64,
) -> c_int {
    if encoder.is_null() || data.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    let Ok(size) = usize::try_from(size) else {
        return RK_MPP_ERR_TOO_LARGE;
    };
    // SAFETY: the caller guarantees `size` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data, size) };
    // SAFETY: encoder is live and exclusively used for this call.
    match unsafe { &mut *encoder }
        .inner
        .submit_frame(encoder::InputFrame::new(bytes).with_pts(pts).with_dts(dts))
    {
        Ok(_) => RK_MPP_OK,
        Err(error) => error_code(&error),
    }
}

/// # Safety
/// `encoder` must be live and `packet` writable initialized storage.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_receive_packet(
    encoder: *mut RkMppEncoder,
    packet: *mut RkMppEncodedPacket,
) -> c_int {
    if encoder.is_null() || packet.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: both pointers are validated.
    let encoder = unsafe { &mut *encoder };
    let packet = unsafe { &mut *packet };
    if packet.struct_size != size_of::<RkMppEncodedPacket>() as u32 {
        return RK_MPP_ERR_INVALID;
    }
    if encoder.last_packet.is_some() {
        return RK_MPP_ERR_STATE;
    }
    clear_packet(packet);
    let Some(encoded) = encoder.inner.receive_packet() else {
        return if encoder.inner.is_finished() {
            RK_MPP_EOS
        } else {
            RK_MPP_AGAIN
        };
    };
    packet.flags = if encoded.is_keyframe() {
        RK_MPP_ENCODED_PACKET_FLAG_KEYFRAME
    } else {
        0
    };
    packet.pts = encoded.pts();
    packet.dts = encoded.dts();
    packet.frame_num = encoded.frame_num() as i32;
    packet.poc = encoded.poc();
    packet.data = encoded.data().as_ptr();
    packet.size = encoded.data().len() as u64;
    encoder.last_packet = Some(encoded);
    RK_MPP_OK
}

/// # Safety
/// `encoder` must be live; `packet` may be null.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoded_packet_release(
    encoder: *mut RkMppEncoder,
    packet: *mut RkMppEncodedPacket,
) {
    if encoder.is_null() {
        return;
    }
    // SAFETY: encoder is live under the call contract.
    let encoder = unsafe { &mut *encoder };
    encoder.last_packet = None;
    if !packet.is_null() {
        // SAFETY: caller guarantees writable packet storage.
        clear_packet(unsafe { &mut *packet });
    }
}

/// # Safety
/// `encoder` must be live.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_flush(encoder: *mut RkMppEncoder) -> c_int {
    if encoder.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: encoder is live.
    unsafe { &mut *encoder }.inner.flush();
    RK_MPP_OK
}

/// # Safety
/// `encoder` must be live.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_reset(encoder: *mut RkMppEncoder) -> c_int {
    if encoder.is_null() {
        return RK_MPP_ERR_INVALID;
    }
    // SAFETY: encoder is live.
    let encoder = unsafe { &mut *encoder };
    encoder.last_packet = None;
    match encoder.inner.reset() {
        Ok(()) => RK_MPP_OK,
        Err(error) => error_code(&error),
    }
}

/// # Safety
/// `encoder` must come from create and not be used after this call.
#[no_mangle]
pub unsafe extern "C" fn rk_mpp_encoder_close(encoder: *mut RkMppEncoder) {
    if !encoder.is_null() {
        // SAFETY: caller transfers ownership.
        drop(unsafe { Box::from_raw(encoder) });
    }
}

#[cfg(test)]
mod tests {
    use super::{RkMppEncodedPacket, RkMppEncoderConfig};
    use std::mem::size_of;

    #[test]
    fn encoder_structs_match_the_v1_header_layout() {
        assert_eq!(size_of::<RkMppEncoderConfig>(), 68);
        assert_eq!(size_of::<RkMppEncodedPacket>(), 72);
    }
}
