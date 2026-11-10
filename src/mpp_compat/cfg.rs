use std::ffi::{c_char, c_int, c_void, CStr, CString};

use crate::encoder::{EncoderConfig, RateControlConfig, RateControlMode};
use crate::sdk::Codec;

const MPP_OK: c_int = 0;
const MPP_VIDEO_CODING_MPEG2: c_int = 2;
const MPP_VIDEO_CODING_H263: c_int = 3;
const MPP_VIDEO_CODING_MPEG4: c_int = 4;
const MPP_VIDEO_CODING_AVC: c_int = 7;
const MPP_VIDEO_CODING_MJPEG: c_int = 8;
const MPP_VIDEO_CODING_VP8: c_int = 9;
const MPP_VIDEO_CODING_VP9: c_int = 10;
const MPP_VIDEO_CODING_HEVC: c_int = 0x0100_0004;
const MPP_VIDEO_CODING_AV1: c_int = 0x0100_0008;

type GetS32 = unsafe extern "C" fn(*mut c_void, *const c_char, *mut i32) -> c_int;

struct VendorEncCfg {
    config: *mut c_void,
    get_s32: GetS32,
}

impl VendorEncCfg {
    unsafe fn new(config: *mut c_void) -> Option<Self> {
        let symbol = CStr::from_bytes_with_nul(b"mpp_enc_cfg_get_s32\0")
            .ok()?
            .as_ptr();
        // SAFETY: RTLD_DEFAULT searches the process symbol scope. The vendor
        // test library supplies this helper when cfg is a vendor MppEncCfg.
        let pointer = unsafe { libc::dlsym(libc::RTLD_DEFAULT, symbol) };
        if pointer.is_null() {
            return None;
        }
        let get_s32 = unsafe { std::mem::transmute::<*mut c_void, GetS32>(pointer) };
        Some(Self { config, get_s32 })
    }

    unsafe fn get(&self, name: &str) -> Option<i32> {
        let name = CString::new(name).ok()?;
        let mut value = 0i32;
        let result = unsafe { (self.get_s32)(self.config, name.as_ptr(), &mut value) };
        (result == MPP_OK).then_some(value)
    }

    unsafe fn get_or(&self, name: &str, default: i32) -> i32 {
        unsafe { self.get(name) }.unwrap_or(default)
    }
}

fn codec(coding: i32) -> Option<Codec> {
    match coding {
        MPP_VIDEO_CODING_MPEG2 => Some(Codec::Mpeg2),
        MPP_VIDEO_CODING_H263 => Some(Codec::H263),
        MPP_VIDEO_CODING_MPEG4 => Some(Codec::Mpeg4),
        MPP_VIDEO_CODING_AVC => Some(Codec::H264),
        MPP_VIDEO_CODING_MJPEG => Some(Codec::Mjpeg),
        MPP_VIDEO_CODING_VP8 => Some(Codec::Vp8),
        MPP_VIDEO_CODING_VP9 => Some(Codec::Vp9),
        MPP_VIDEO_CODING_HEVC => Some(Codec::H265),
        MPP_VIDEO_CODING_AV1 => Some(Codec::Av1),
        _ => None,
    }
}

pub(super) unsafe fn encoder_config(config: *mut c_void) -> Result<EncoderConfig, String> {
    let cfg = unsafe { VendorEncCfg::new(config) }
        .ok_or_else(|| "vendor MppEncCfg accessors are unavailable".to_string())?;
    let coding = unsafe { cfg.get("codec:type") }
        .ok_or_else(|| "MppEncCfg has no codec:type".to_string())?;
    let codec = codec(coding).ok_or_else(|| format!("unsupported encoder coding {coding}"))?;
    let width = unsafe { cfg.get("prep:width") }
        .filter(|value| *value > 0)
        .ok_or_else(|| "MppEncCfg has no positive prep:width".to_string())?;
    let height = unsafe { cfg.get("prep:height") }
        .filter(|value| *value > 0)
        .ok_or_else(|| "MppEncCfg has no positive prep:height".to_string())?;
    let format = unsafe { cfg.get_or("prep:format", 0) };
    if format != 0 {
        return Err(format!(
            "compatible encoder object path currently accepts NV12 only, got {format}"
        ));
    }

    let gop = unsafe { cfg.get_or("rc:gop", 60) }.max(1) as u32;
    let fps_num = unsafe { cfg.get_or("rc:fps_in_num", 30) }.max(1) as u32;
    let fps_den = unsafe { cfg.get_or("rc:fps_in_denom", 1) }.max(1) as u32;
    let qp = unsafe { cfg.get_or("rc:qp_init", 26) }.clamp(0, 51) as u8;
    let profile = unsafe { cfg.get_or("h264:profile", 0) }.clamp(0, u8::MAX as i32) as u8;
    let level = unsafe { cfg.get_or("h264:level", 30) }.clamp(0, u8::MAX as i32) as u8;
    let jpeg_qfactor = unsafe { cfg.get_or("jpeg:q_factor", 80) }.clamp(1, 100) as u32;

    let mut result = EncoderConfig::new(codec, width as usize, height as usize)
        .with_gop(gop)
        .with_qp(qp)
        .with_fps(fps_num, fps_den)
        .with_profile(profile)
        .with_level(level);
    if codec == Codec::Mjpeg {
        result = result.with_jpeg_qfactor(jpeg_qfactor);
    }

    if codec == Codec::H264 || codec == Codec::H265 {
        if codec == Codec::H264 {
            result = result.with_num_ref_frames(2);
        }
        let mode = match unsafe { cfg.get_or("rc:mode", 2) } {
            0 => RateControlMode::Vbr,
            1 => RateControlMode::Cbr,
            2 => RateControlMode::FixedQp,
            3 => RateControlMode::Avbr,
            value => return Err(format!("unsupported encoder RC mode {value}")),
        };
        if mode != RateControlMode::FixedQp {
            let target = unsafe { cfg.get_or("rc:bps_target", 4_000_000) }.max(1) as i64;
            let min = unsafe { cfg.get_or("rc:bps_min", target as i32 / 2) }.max(1) as i64;
            let max =
                unsafe { cfg.get_or("rc:bps_max", target as i32 * 3 / 2) }.max(min as i32) as i64;
            let default_qp_min = if codec == Codec::H264 { 8 } else { 10 };
            let default_qp_max = if codec == Codec::H264 { 48 } else { 51 };
            let qp_min = unsafe { cfg.get("rc:qp_min") }
                .filter(|value| *value > 0)
                .unwrap_or(default_qp_min)
                .clamp(0, 51) as u8;
            let qp_max = unsafe { cfg.get("rc:qp_max") }
                .filter(|value| *value > 0)
                .unwrap_or(default_qp_max)
                .clamp(qp_min as i32, default_qp_max) as u8;
            let qp_init = unsafe { cfg.get("rc:qp_init") }
                .filter(|value| *value != 0)
                .unwrap_or(26);
            result = result.with_rate_control(
                RateControlConfig::new(mode, target, min, max)
                    .with_initial_qp(qp_init.clamp(qp_min as i32, qp_max as i32) as u8)
                    .with_qp_range(qp_min, qp_max),
            );
        }
    }
    Ok(result)
}
