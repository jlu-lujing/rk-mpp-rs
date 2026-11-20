use std::ffi::{c_char, c_int, c_void, CStr, CString};

use super::standalone;
use crate::codec::h265::encoder::SyntaxOptions as H265SyntaxOptions;
use crate::encoder::{
    EncoderConfig, GdrConfig, GdrMode, H264DeblockConfig, H264RcTuning, H264SyntaxConfig,
    RateControlConfig, RateControlMode,
};
use crate::sdk::{Codec, PixelFormat};

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
    let input_format = match format {
        0 | 5 => PixelFormat::Nv12,
        4 => PixelFormat::I420,
        _ => {
            return Err(format!(
                "compatible encoder object path accepts NV12(0), I420(4) or NV21(5), got {format}"
            ))
        }
    };

    let gop = unsafe { cfg.get_or("rc:gop", 60) }.max(1) as u32;
    let fps_num = unsafe { cfg.get_or("rc:fps_in_num", 30) }.max(1) as u32;
    let fps_den = unsafe { cfg.get_or("rc:fps_in_denom", 1) }.max(1) as u32;
    let qp = unsafe { cfg.get_or("rc:qp_init", 26) }.clamp(0, 51) as u8;
    let profile_key = if codec == Codec::H265 {
        "h265:profile"
    } else {
        "h264:profile"
    };
    let level_key = if codec == Codec::H265 {
        "h265:level"
    } else {
        "h264:level"
    };
    let profile = unsafe { cfg.get_or(profile_key, 0) }.clamp(0, u8::MAX as i32) as u8;
    // MPP uses one for HEVC Main; the typed encoder uses zero for its
    // currently supported Main-only backend.
    let profile = if codec == Codec::H265 && profile == 1 {
        0
    } else {
        profile
    };
    let level = unsafe { cfg.get_or(level_key, 0) }.clamp(0, u8::MAX as i32) as u8;
    // FFmpeg uses zero for "let MPP choose". H.264's SPS builder raises this
    // initial 1.0 level to the lowest level that fits the coded picture.
    let level = match (codec, level) {
        (Codec::H264, 0) => 10,
        (Codec::H265, 0) => 30,
        (_, 0) => 0,
        (_, value) => value,
    };
    let jpeg_qfactor = unsafe { cfg.get_or("jpeg:q_factor", 80) }.clamp(1, 100) as u32;

    let mut result = EncoderConfig::new(codec, width as usize, height as usize)
        .with_input_format(input_format)
        .with_gop(gop)
        .with_qp(qp)
        .with_fps(fps_num, fps_den)
        .with_profile(profile)
        .with_level(level);
    if codec == Codec::Mjpeg {
        result = result.with_jpeg_qfactor(jpeg_qfactor);
    }

    if codec == Codec::H264 {
        if let Some(values) = standalone::encoder_cfg_i32_array::<16>(config, "hw:aq_step_i") {
            let mut aq_step_i = [0i8; 16];
            for (output, input) in aq_step_i.iter_mut().zip(values) {
                *output = i8::try_from(input)
                    .map_err(|_| format!("hw:aq_step_i value {input} is outside i8"))?;
            }
            result = result.with_h264_aq_step_i(aq_step_i);
        }
        let disable = unsafe { cfg.get("h264:dblk_disable") }
            .or_else(|| unsafe { cfg.get("h264:deblock_disable") });
        let alpha = unsafe { cfg.get("h264:dblk_alpha") }
            .or_else(|| unsafe { cfg.get("h264:deblock_offset_alpha") });
        let beta = unsafe { cfg.get("h264:dblk_beta") }
            .or_else(|| unsafe { cfg.get("h264:deblock_offset_beta") });
        if disable.is_some() || alpha.is_some() || beta.is_some() {
            result = result.with_h264_deblock(H264DeblockConfig::new(
                disable.unwrap_or(0).max(0) as u32,
                alpha.unwrap_or(0),
                beta.unwrap_or(0),
            ));
        }
        let cabac = unsafe { cfg.get("h264:cabac_en") };
        let transform_8x8 = unsafe { cfg.get("h264:trans8x8") };
        let cabac_init_idc = unsafe { cfg.get("h264:cabac_idc") };
        let constraint_set = unsafe { cfg.get("h264:constraint_set") };
        let base_layer_pid = unsafe { cfg.get("h264:base_layer_pid") };
        if cabac.is_some()
            || transform_8x8.is_some()
            || cabac_init_idc.is_some()
            || constraint_set.is_some()
            || base_layer_pid.is_some()
        {
            let mut syntax =
                H264SyntaxConfig::new(cabac.unwrap_or(0) != 0, transform_8x8.unwrap_or(0) != 0);
            syntax = match cabac_init_idc {
                Some(value) => syntax.with_cabac_init_idc(
                    u8::try_from(value)
                        .map_err(|_| format!("h264:cabac_idc {value} is outside 0..=2"))?,
                ),
                None => syntax,
            };
            if let Some(value) = constraint_set {
                syntax = syntax.with_constraint_set(u32::try_from(value).map_err(|_| {
                    format!("h264:constraint_set {value} is outside the 32-bit mask")
                })?);
            }
            if let Some(value) = base_layer_pid {
                syntax = syntax.with_base_layer_pid(
                    u8::try_from(value)
                        .map_err(|_| format!("h264:base_layer_pid {value} is outside 0..=56"))?,
                );
            }
            result = result.with_h264_syntax(syntax);
        }
    }

    if codec == Codec::H265 {
        let mut syntax = H265SyntaxOptions::default();
        let mut any = false;
        if let Some(value) = unsafe { cfg.get("h265:dblk_disable") } {
            syntax.deblock_disable = value != 0;
            any = true;
        }
        if let Some(value) = unsafe { cfg.get("h265:dblk_alpha") } {
            syntax.deblock_beta_offset_div2 = value;
            any = true;
        }
        if let Some(value) = unsafe { cfg.get("h265:dblk_beta") } {
            syntax.deblock_tc_offset_div2 = value;
            any = true;
        }
        if let Some(value) = unsafe { cfg.get("h265:sao_luma_disable") } {
            syntax.sao_luma_disable = value != 0;
            any = true;
        }
        if let Some(value) = unsafe { cfg.get("h265:sao_chroma_disable") } {
            syntax.sao_chroma_disable = value != 0;
            any = true;
        }
        if let Some(value) = unsafe { cfg.get("h265:lpf_acs_sli_en") } {
            syntax.loop_filter_across_slices = value != 0;
            any = true;
        }
        if let Some(value) = unsafe { cfg.get("h265:lpf_acs_tile_disable") } {
            syntax.loop_filter_across_tiles = value == 0;
            any = true;
        }
        if let Some(value) = unsafe { cfg.get("h265:auto_tile") } {
            syntax.auto_tile = value != 0;
            any = true;
        }
        if any {
            result = result.with_h265_syntax(syntax);
        }
    }

    if matches!(codec, Codec::H264 | Codec::H265) {
        if let Some(gdr) = gdr_config(
            unsafe { cfg.get_or("rc:refresh_en", 0) },
            unsafe { cfg.get_or("rc:refresh_mode", 0) },
            unsafe { cfg.get_or("rc:refresh_num", 1) },
        )? {
            result = result.with_gdr(gdr);
        }
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
                .unwrap_or(default_qp_min)
                .clamp(0, 51) as u8;
            let qp_max = unsafe { cfg.get("rc:qp_max") }
                .unwrap_or(default_qp_max)
                .clamp(qp_min as i32, default_qp_max) as u8;
            let qp_init = unsafe { cfg.get("rc:qp_init") }
                .filter(|value| *value != 0)
                .unwrap_or(26);
            let mut rate_control = RateControlConfig::new(mode, target, min, max)
                .with_initial_qp(qp_init.clamp(qp_min as i32, qp_max as i32) as u8)
                .with_qp_range(qp_min, qp_max);
            if codec == Codec::H264 && unsafe { cfg.get_or("rc:hier_qp_en", 0) } != 0 {
                rate_control = rate_control.with_hierarchical_qp(
                    standalone::encoder_cfg_i32_array::<4>(config, "rc:hier_qp_delta")
                        .unwrap_or([0; 4]),
                    standalone::encoder_cfg_i32_array::<4>(config, "rc:hier_frame_num")
                        .unwrap_or([0; 4]),
                );
            }
            result = result.with_rate_control(rate_control);

            if codec == Codec::H264 {
                let max_reencode = unsafe { cfg.get("rc:max_reenc_times") };
                let super_mode = unsafe { cfg.get("rc:super_mode") };
                let super_i = unsafe { cfg.get("rc:super_i_thd") };
                let super_p = unsafe { cfg.get("rc:super_p_thd") };
                let priority = unsafe { cfg.get("rc:priority") };
                let debreath_en = unsafe { cfg.get("rc:debreath_en") };
                let debreath_strength = unsafe { cfg.get("rc:debreath_strength") };
                let drop_mode = unsafe { cfg.get("rc:drop_mode") };
                let drop_threshold = unsafe { cfg.get("rc:drop_thd") };
                let drop_gap = unsafe { cfg.get("rc:drop_gap") };
                let any_tuning = [
                    max_reencode,
                    super_mode,
                    super_i,
                    super_p,
                    priority,
                    debreath_en,
                    debreath_strength,
                    drop_mode,
                    drop_threshold,
                    drop_gap,
                ]
                .iter()
                .any(Option::is_some);
                if any_tuning {
                    let mut tuning = H264RcTuning::new();
                    if let Some(value) = max_reencode {
                        tuning = tuning.with_max_reencode_times(value.max(0) as u32);
                    }
                    tuning = tuning.with_super_frame(
                        super_mode.unwrap_or(i32::from(tuning.super_mode())).max(0) as u8,
                        super_i.unwrap_or(tuning.super_i_thd() as i32).max(0) as u32,
                        super_p.unwrap_or(tuning.super_p_thd() as i32).max(0) as u32,
                        priority.unwrap_or(i32::from(tuning.rc_priority())).max(0) as u8,
                    );
                    tuning = tuning.with_debreath(
                        debreath_en.unwrap_or(0) != 0,
                        debreath_strength
                            .unwrap_or(i32::from(tuning.debreath().1))
                            .max(0) as u8,
                    );
                    tuning = tuning.with_drop_policy(
                        drop_mode
                            .unwrap_or(i32::from(tuning.drop_policy().0))
                            .max(0) as u8,
                        drop_threshold
                            .unwrap_or(tuning.drop_policy().1 as i32)
                            .max(0) as u32,
                        drop_gap.unwrap_or(tuning.drop_policy().2 as i32).max(0) as u32,
                    );
                    result = result.with_h264_rc_tuning(tuning);
                }
            }
        }
    }
    Ok(result)
}

fn gdr_config(enabled: i32, mode: i32, refresh_blocks: i32) -> Result<Option<GdrConfig>, String> {
    if enabled == 0 {
        return Ok(None);
    }
    let mode = match mode {
        0 => GdrMode::Row,
        1 => GdrMode::Col,
        value => return Err(format!("unsupported GDR refresh mode {value}")),
    };
    if refresh_blocks <= 0 {
        return Err(format!(
            "GDR refresh_num must be positive, got {refresh_blocks}"
        ));
    }
    Ok(Some(GdrConfig::new(mode, refresh_blocks as u32)))
}

#[cfg(test)]
mod tests {
    use super::gdr_config;
    use crate::encoder::GdrMode;

    #[test]
    fn gdr_keys_map_to_the_public_refresh_axis() {
        let row = gdr_config(1, 0, 4).unwrap().unwrap();
        assert_eq!(row.mode(), GdrMode::Row);
        assert_eq!(row.refresh_blocks(), 4);
        let col = gdr_config(1, 1, 8).unwrap().unwrap();
        assert_eq!(col.mode(), GdrMode::Col);
        assert!(gdr_config(0, 1, 8).unwrap().is_none());
    }

    #[test]
    fn invalid_gdr_keys_are_named() {
        assert!(gdr_config(1, 2, 4).unwrap_err().contains("mode 2"));
        assert!(gdr_config(1, 0, 0)
            .unwrap_err()
            .contains("refresh_num must be positive"));
    }
}
