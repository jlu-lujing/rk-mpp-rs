//! `MppEncH265Cfg`'s encoder-side parameter sets: the VPS, SPS and PPS the
//! software writes ahead of every IDR.
//!
//! This is a bit-exact transliteration of the develop-vintage
//! `mpp/codec/enc/h265/h265e_ps.c` (`h265e_set_vps`/`set_sps`/`set_pps`, which fix
//! every field) feeding `h265e_header_gen.c` (`h265e_vps_write`/`sps_write`/
//! `pps_write`, which serialise them) through `h265e_stream.c`. The defaults here
//! are the ones `h265e_init` sets, i.e. what `mpi_enc_test -t 9` inherits unless it
//! overrides a field - which the fixed-QP comparison does not.
//!
//! The two-byte NAL header packs `nal_unit_type(6) | nuh_layer_id(6) |
//! nuh_temporal_id_plus1(3)` across the byte boundary, so with layer id 0 and
//! temporal id 0 it is `[type << 1, 0x01]` for every unit. The payload is then
//! Annex-B escaped. Both facts came off the board rather than out of the header
//! generator: the oracle's VPS carries `01 60 00 00 03 00 80 ...` where this
//! writer had `01 60 00 00 00 80 ...`, and removing the `0x03` gives the RBSP back
//! byte for byte; its SPS NAL header is `42 01` where this writer had `42 81`.
//!
//! The escape is *not* `h265e_nal_escape_c`'s work - that function leaves the
//! insertion commented out in both vintages (`h265e_header_gen.c:42`). VEPU580
//! writes its parameter sets from `reg0237_synt_sps` / `reg0238_synt_pps` and the
//! hardware formatter escapes them. That formatter is the layer every board golden
//! was produced by, so it is the one this writer matches, whatever the software
//! fallback in the C source does.
//!
//! What this module backs with bytes: the VPS, SPS and PPS are byte-identical to
//! the oracle at 320x240, 640x480 and 1280x720 (`scripts/board_h265_qp_vs_mpp.sh`,
//! three cells), and the same comparison runs on the host against the recorded
//! streams in `tests/data/h265e_c_fixqp_qp26_*.265`. The IDR payload is still not
//! compared - that needs the captured register-job template, see `hal::h265e`.

use std::fmt;

use super::sei;

pub const PROFILE_HEVC_MAIN: u8 = 1;

/// VPS NAL unit type (`NAL_VPS`).
pub const NAL_VPS: u8 = 32;
/// SPS NAL unit type (`NAL_SPS`).
pub const NAL_SPS: u8 = 33;
/// PPS NAL unit type (`NAL_PPS`).
pub const NAL_PPS: u8 = 34;
/// An IDR picture with leading pictures (`NAL_IDR_W_RADL`), the NAL type the
/// hardware stamps on a fixed-QP IDR access unit.
pub const NAL_IDR_W_RADL: u8 = 19;

/// `MPP_ENC_SPLIT_OUT_LOWDELAY`: publish one output packet per completed slice.
pub const MPP_ENC_SPLIT_OUT_LOWDELAY: u32 = 1 << 0;

/// The largest reference count the DPB caps `max_dec_pic_buffering` at. The C
/// `MAX_REFS` is 16; with `num_ref` defaulting to 1 it never binds, but the clip
/// is what the reference applies, so it is what we apply.
const MAX_REFS: u32 = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Encoder configuration for the H.265 parameter sets.
///
/// Every field defaults to what `h265e_init` writes into `MppEncH265Cfg`, i.e. the
/// library default a caller gets for not asking. The geometry (`log2_min_cb`, the
/// transform block range, `log2_diff_max_min_coding_block_size`) is *derived* in
/// [`EncoderConfig::geometry`] exactly as `h265e_set_sps` derives it from
/// `max_cu_size`, rather than stored, because storing it separately would let it
/// drift from the rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncoderConfig {
    pub width: usize,
    pub height: usize,
    /// `general_profile_idc`: 1 = Main.
    pub profile: u8,
    /// `general_level_idc` (4.0 = 120). MPP raises it to what the coded size needs,
    /// exactly as `h265e_set_vps` does with its level table.
    pub level: u8,
    /// `general_tier_flag` (0 = Main tier, 1 = High tier).
    pub tier: bool,
    /// `codec->num_ref`, clipped into `max_dec_pic_buffering` by the C formula.
    pub num_ref: u32,
    /// `codec->max_cu_size` (64 on RK3588, 32 on the parts that list it).
    pub max_cu_size: usize,
    /// Sample bit depth. VEPU580 accepts the 10-bit prep/frame format but its
    /// H.265 hardware job returns `int_sta=0x100` without a bitstream on t6, so
    /// this port only advertises the supported 8-bit path.
    pub bit_depth: u8,
    pub sao_enable: bool,
    pub amp_enable: bool,
    pub tmvp_enable: bool,
    pub const_intra_pred: bool,
    pub strong_intra_smoothing: bool,
    pub scaling_list_enabled: bool,
    pub transform_skip_enabled: bool,
    pub transquant_bypass_enabled: bool,
    /// `rc->rc_mode`. `h265e_set_pps` writes `m_useDQP = (rc_mode != FIXQP)`, so a
    /// fixed-QP picture carries no `diff_cu_qp_delta_depth` at all. The board
    /// settles it: the oracle's PPS is `44 01 c0 f1 80 9b 20` - `f1` has that bit
    /// clear - where a hard-wired 1 produced `c0 f3 ...` and every following field
    /// shifted by one bit.
    pub rc_mode: RateControl,
    /// `pps->m_maxCuDQPDepth` = `trans_cfg.diff_cu_qp_delta_depth`.
    pub diff_cu_qp_delta_depth: u32,
    /// `codec->intra_qp`; the PPS writes `init_qp_minus26 = intra_qp - 26`.
    pub intra_qp: u8,
    pub cb_qp_offset: i32,
    pub cabac_init_present: bool,
    /// `pps->m_LFCrossSliceBoundaryFlag` = `codec->lpf_acs_sli_en`.
    pub loop_filter_across_slices: bool,
    /// `!codec->dblk_cfg.slice_deblocking_filter_disabled_flag`.
    pub deblocking_filter_control_present: bool,
    pub deblock_beta_offset_div2: i32,
    pub deblock_tc_offset_div2: i32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub vui: bool,
    /// `long_term_ref_pics_present_flag` plus one `(poc_lsb 0, used 0)` entry. The
    /// harness enables this when it is given an explicit `-g` GOP mode, which is
    /// also what makes it emit P pictures; without it the SPS is two bytes shorter.
    pub long_term_refs: bool,
}

/// The derived coding-tree geometry, the numbers `h265e_set_sps` computes once from
/// `max_cu_size` and reuses for both the SPS fields and the CU maps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    /// `log2_min_coding_block_size` (3 for a 64-sized max CU).
    pub log2_min_cb: u32,
    /// `log2_diff_max_min_coding_block_size`.
    pub log2_diff_max_min_cb: u32,
    /// `log2_min_transform_block_size` (2).
    pub log2_min_tb: u32,
    /// `log2_diff_max_min_transform_block_size`.
    pub log2_diff_max_min_tb: u32,
}

impl EncoderConfig {
    /// The RKVENC H.265 library defaults `h265e_init` leaves in place, at the
    /// caller's picture size.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            profile: PROFILE_HEVC_MAIN,
            // h265e_init sets level = 120 (4.0); it is raised only if the size needs
            // a higher level than the request, and 4.0 already covers 320x240.
            level: 120,
            tier: false,
            num_ref: 1,
            max_cu_size: 64,
            bit_depth: 8,
            sao_enable: true,
            amp_enable: false,
            tmvp_enable: false,
            const_intra_pred: false,
            strong_intra_smoothing: true,
            scaling_list_enabled: false,
            transform_skip_enabled: false,
            transquant_bypass_enabled: false,
            // `h265-enc <qp>` is the fixed-QP caller; CBR/VBR are not wired for
            // H.265 yet, but the PPS branch for them is.
            rc_mode: RateControl::FixQp,
            diff_cu_qp_delta_depth: 0,
            intra_qp: 26,
            cb_qp_offset: 0,
            cabac_init_present: true,
            loop_filter_across_slices: false,
            deblocking_filter_control_present: true,
            deblock_beta_offset_div2: 0,
            deblock_tc_offset_div2: 0,
            fps_num: 30,
            fps_den: 1,
            // h265_init sets codec->vui.vui_en = 1.
            vui: true,
            long_term_refs: false,
        }
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.width == 0 || self.height == 0 {
            return Err(Error("H.265 dimensions must be non-zero".into()));
        }
        if self.bit_depth != 8 {
            return Err(Error(format!(
                "VEPU580 H.265 does not support {}-bit encoding on t6",
                self.bit_depth
            )));
        }
        if self.width > 8192 || self.height > 8192 {
            return Err(Error("H.265 dimensions must fit in 13 bits".into()));
        }
        if self.max_cu_size != 32 && self.max_cu_size != 64 {
            return Err(Error(format!(
                "max_cu_size {} is neither 32 nor 64",
                self.max_cu_size
            )));
        }
        if self.intra_qp > 51 {
            return Err(Error("intra_qp must be in 0..=51".into()));
        }
        if !(-12..=12).contains(&self.cb_qp_offset) {
            return Err(Error("cb_qp_offset must be in -12..=12".into()));
        }
        if self.fps_num == 0 || self.fps_den == 0 {
            return Err(Error(
                "frame-rate numerator and denominator must be non-zero".into(),
            ));
        }
        Ok(())
    }

    /// `h265e_set_sps`' derivation of the coding-tree geometry from `max_cu_size`.
    ///
    /// The C walks `convertToBit[max_cu]` down to the minimum block and a parallel
    /// `addCUDepth` loop; for a 64-sized max CU that resolves to a 8-sized min CB
    /// (`log2_min_cb = 3`), `log2_diff_max_min_cb = 3`, and a transform range of 4
    /// through 32 (`log2_min_tb = 2`, `log2_diff_max_min_tb = 3`). `tuQTMaxLog2Size`
    /// gets a +1 only on the RK3576/RV1126B/RK3572 parts, which the byte oracle is
    /// not (t6 is an RK3588), so that branch is out of scope here and asserted as
    /// such.
    pub fn geometry(&self) -> Geometry {
        let max_cud = log2(self.max_cu_size) - 2; // convertToBit[64] = 4, [32] = 3
        let mut add_cu_depth = 0u32;
        while (self.max_cu_size >> max_cud) > (1usize << (2 + add_cu_depth)) {
            add_cu_depth += 1;
        }
        // maxCUDepth += addCUDepth; addCUDepth++;
        let max_cu_depth = max_cud + add_cu_depth;
        let add_cu_depth = add_cu_depth + 1;
        Geometry {
            log2_min_cb: 3,
            log2_diff_max_min_cb: max_cu_depth - add_cu_depth,
            log2_min_tb: 2,
            log2_diff_max_min_tb: (max_cud + 2 - 1) - 2, // quadtreeTULog2Max - Min
        }
    }

    /// `h265e_set_sps`' conformance-window padding: the coded picture is the source
    /// size rounded up to the minimum CU, and the window carries the pad as a
    /// right/bottom offset. `min_cu_size` is `1 << log2_min_cb`.
    pub fn coded_size(&self) -> (usize, usize) {
        let min_cu = 1usize << self.geometry().log2_min_cb;
        (align(self.width, min_cu), align(self.height, min_cu))
    }

    /// `h265e_set_vps`'s level table: the lowest level whose `maxLumaSamples` fits
    /// the picture. MPP then uses `max(codec->level, this)`.
    pub fn effective_level(&self) -> u8 {
        const LEVELS: &[(u32, u8)] = &[
            (36864, 30),
            (122880, 60),
            (245760, 63),
            (552960, 90),
            (983040, 93),
            (2228224, 120),
            (2228224, 123),
            (8912896, 150),
            (8912896, 153),
            (8912896, 156),
            (35651584, 180),
            (35651584, 183),
            (35651584, 186),
        ];
        let luma = (self.width * self.height) as u32;
        let size_level = LEVELS
            .iter()
            .find(|(max, _)| *max >= luma)
            .map(|(_, level)| *level)
            .unwrap_or(255);
        self.level.max(size_level)
    }

    /// `vps->m_maxDecPicBuffering[0]` = `min(MAX_REFS, max(num_reorder+3, num_ref))`
    /// + `num_reorder`, with `num_reorder` fixed at 0 by `h265e_set_vps`.
    fn max_dec_pic_buffering(&self) -> u32 {
        let clipped = MAX_REFS.min((0 + 3).max(self.num_ref)) + 0;
        clipped.max(1)
    }

    /// `pps->m_useDQP`: on for every rate-control mode except fixed QP.
    pub fn cu_qp_delta_enabled(&self) -> bool {
        self.rc_mode != RateControl::FixQp
    }
}

/// The rate-control modes `h265e_set_pps` distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateControl {
    /// `MPP_ENC_RC_MODE_FIXQP`: no in-loop CU QP delta.
    FixQp,
    Cbr,
    Vbr,
}

/// `MppEncSplitMode`: what cuts an H.265 picture into more than one slice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplitMode {
    /// `MPP_ENC_SPLIT_NONE`, one slice per picture.
    #[default]
    None,
    /// `MPP_ENC_SPLIT_BY_BYTE`, the `sli_splt_byte` cap.
    ByByte,
    /// `MPP_ENC_SPLIT_BY_CTU`, where the unit is a 64x64 CTU on VEPU580.
    ByCtu,
}

/// The `MppEncSliceSplit` fields used by VEPU580.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SliceSplit {
    pub mode: SplitMode,
    /// Byte cap for [`SplitMode::ByByte`], CTU count for [`SplitMode::ByCtu`].
    pub arg: u32,
    /// `MppEncSliceSplit::split_out`; bit 0 is low-delay packet output.
    pub out: u32,
}

impl SliceSplit {
    pub fn validate(&self) -> Result<(), Error> {
        if self.out & !MPP_ENC_SPLIT_OUT_LOWDELAY != 0 {
            return Err(Error(format!(
                "unsupported H.265 slice output bits 0x{:x}; only LOWDELAY is implemented",
                self.out
            )));
        }
        if self.out != 0 && self.mode == SplitMode::None {
            return Err(Error(
                "H.265 split_out requires BY_CTU or BY_BYTE slice split".into(),
            ));
        }
        match self.mode {
            SplitMode::None => {}
            SplitMode::ByByte if self.arg == 0 || self.arg > 0x000f_ffff => {
                return Err(Error(format!(
                    "H.265 slice byte target {} is outside 1..=1048575",
                    self.arg
                )));
            }
            SplitMode::ByCtu if self.arg == 0 || self.arg > 0x0010_0000 => {
                return Err(Error(format!(
                    "H.265 CTUs per slice {} is outside 1..=1048576",
                    self.arg
                )));
            }
            _ => {}
        }
        Ok(())
    }

    /// `vepu580_setup_split`'s CTU count. Tiles are disabled on the paths this
    /// crate implements, so the C's `title_en` halving is not applied.
    pub fn slice_count(&self, width: usize, height: usize) -> u32 {
        if self.mode != SplitMode::ByCtu || self.arg == 0 {
            return 1;
        }
        let ctu_w = width.div_ceil(64) as u32;
        let ctu_h = height.div_ceil(64) as u32;
        ctu_w.saturating_mul(ctu_h).div_ceil(self.arg).max(1)
    }
}

/// Serialise the VPS, SPS and PPS, in that order, each with a four-byte Annex-B
/// start code. This is the whole `h265e_set_extra_info` + `h265e_get_extra_info`
/// prefix the hardware prepends to an IDR access unit.
pub fn build_headers(config: &EncoderConfig) -> Result<Vec<u8>, Error> {
    config.validate()?;
    let mut out = build_vps(config)?;
    out.extend_from_slice(&build_sps(config)?);
    out.extend_from_slice(&build_pps(config)?);
    Ok(out)
}

/// The whole access-unit prefix MPP writes ahead of an H.265 IDR: VPS, SPS, PPS,
/// then the two prefix SEIs (version and rate-control line). This is the software
/// half of the packet; the hardware writes the slice NAL.
pub fn build_prefix(config: &EncoderConfig, qp: u32, igop: u32) -> Result<Vec<u8>, Error> {
    // The harness's default target rate, which the SEI prints: 30 fps at
    // `w * h * 30 / 8` bits per second. Its min/max stay at the harness values.
    let bps = sei::default_bps_target(config.width, config.height, 30);
    build_prefix_line(config, &sei::RcCfgLine::fixed_qp(qp, igop, bps))
}

/// The prefix with a caller-chosen rate-control line and PPS variant. `config`'s
/// `rc_mode` picks the PPS bytes (`cu_qp_delta_enabled` follows it), `line` is the
/// `uuid_rc_cfg` text.
pub fn build_prefix_line(config: &EncoderConfig, line: &sei::RcCfgLine) -> Result<Vec<u8>, Error> {
    let mut out = build_headers(config)?;
    out.extend(sei::user_data_unregistered(
        &sei::VERSION_UUID,
        sei::VERSION_TEXT.as_bytes(),
    ));
    let text = sei::rc_cfg_line(line);
    out.extend(sei::user_data_unregistered(&sei::RC_UUID, text.as_bytes()));
    Ok(out)
}

/// `h265e_vps_write`.
pub fn build_vps(config: &EncoderConfig) -> Result<Vec<u8>, Error> {
    config.validate()?;
    let mut w = BitWriter::new();
    w.put_bits(0, 4); // vps_video_parameter_set_id
    w.put_bits(3, 2); // vps_reserved_three_2bits
    w.put_bits(0, 6); // vps_reserved_zero_6bits
    w.put_bits(0, 3); // vps_max_sub_layers_minus1 (m_maxTLayers == 1)
    w.put_bits(1, 1); // vps_temporal_id_nesting_flag
    w.put_bits(0xffff, 16); // vps_reserved_ffff_16bits
    code_ptl(&mut w, config);
    w.put_bits(1, 1); // vps_sub_layer_ordering_info_present_flag
    w.put_ue(config.max_dec_pic_buffering() - 1); // vps_max_dec_pic_buffering_minus1[0]
    w.put_ue(0); // vps_num_reorder_pics[0]
    w.put_ue(0); // vps_max_latency_increase_plus1[0]
    w.put_bits(0, 6); // vps_max_nuh_reserved_zero_layer_id
    w.put_ue(0); // vps_max_op_sets_minus1 (m_numOpSets forced to 1)
                 // vps_timing_info_present_flag is 0: h265e_set_vps never fills
                 // m_timingInfo, and the ctx was memset.
    w.put_bits(0, 1);
    w.put_bits(0, 1); // vps_extension_flag
    w.put_stop_bit();
    Ok(annex_b(NAL_VPS, w.finish_rbsp()))
}

/// `h265e_sps_write`.
pub fn build_sps(config: &EncoderConfig) -> Result<Vec<u8>, Error> {
    config.validate()?;
    let geom = config.geometry();
    let (coded_w, coded_h) = config.coded_size();
    let conf_right = coded_w - config.width;
    let conf_bottom = coded_h - config.height;
    let conformance = conf_right != 0 || conf_bottom != 0;
    let mut w = BitWriter::new();
    w.put_bits(0, 4); // sps_video_parameter_set_id
    w.put_bits(0, 3); // sps_max_sub_layers_minus1
    w.put_bits(1, 1); // sps_temporal_id_nesting_flag
    code_ptl(&mut w, config);
    w.put_ue(0); // sps_seq_parameter_set_id
    w.put_ue(1); // chroma_format_idc (4:2:0)
                 // (separate_colour_plane_flag only under 4:4:4)
    w.put_ue(coded_w as u32); // pic_width_in_luma_samples
    w.put_ue(coded_h as u32); // pic_height_in_luma_samples
    w.put_bits(u32::from(conformance), 1); // conformance_window_flag
    if conformance {
        // winUnitX/Y for 4:2:0 are both 2; the C writes offsets / winUnit.
        w.put_ue(0); // conf_win_left_offset
        w.put_ue((conf_right / 2) as u32);
        w.put_ue(0); // conf_win_top_offset
        w.put_ue((conf_bottom / 2) as u32);
    }
    w.put_ue(u32::from(config.bit_depth - 8)); // bit_depth_luma_minus8
    w.put_ue(u32::from(config.bit_depth - 8)); // bit_depth_chroma_minus8
    w.put_ue(12); // log2_max_pic_order_cnt_lsb_minus4 (m_bitsForPOC == 16)
    w.put_bits(1, 1); // sps_sub_layer_ordering_info_present_flag
    w.put_ue(config.max_dec_pic_buffering() - 1); // sps_max_dec_pic_buffering_minus1[0]
    w.put_ue(0); // sps_num_reorder_pics[0]
    w.put_ue(0); // sps_max_latency_increase_plus1[0]
    w.put_ue(geom.log2_min_cb - 3); // log2_min_coding_block_size_minus3
    w.put_ue(geom.log2_diff_max_min_cb); // log2_diff_max_min_coding_block_size
    w.put_ue(geom.log2_min_tb - 2); // log2_min_transform_block_size_minus2
    w.put_ue(geom.log2_diff_max_min_tb); // log2_diff_max_min_transform_block_size
    w.put_ue(0); // max_transform_hierarchy_depth_inter (m_quadtreeTUMaxDepthInter - 1)
    w.put_ue(0); // max_transform_hierarchy_depth_intra
    w.put_bits(u32::from(config.scaling_list_enabled), 1); // scaling_list_enabled_flag
    if config.scaling_list_enabled {
        // h265e_sps_write writes sps_scaling_list_data_present_flag = 0 for the
        // enabled==1 case, which is the only enabled value the reference supports.
        w.put_bits(0, 1);
    }
    w.put_bits(u32::from(config.amp_enable), 1); // amp_enabled_flag
    w.put_bits(u32::from(config.sao_enable), 1); // sample_adaptive_offset_enabled_flag
    w.put_bits(0, 1); // pcm_enabled_flag
    w.put_ue(0); // num_short_term_ref_pic_sets (m_numberOfReferencePictureSets)
    w.put_bits(u32::from(config.long_term_refs), 1); // long_term_ref_pics_present_flag
    if config.long_term_refs {
        w.put_ue(1); // num_long_term_ref_pics_sps
                     // The traced GOP SPS carries one entry: poc_lsb 0, not used by the current
                     // picture (the P marks it through `synt_refm` instead).
        w.put_bits(0, 16); // lt_ref_pic_poc_lsb_sps[0]
        w.put_bits(0, 1); // used_by_curr_pic_lt_sps_flag[0]
    }
    w.put_bits(u32::from(config.tmvp_enable), 1); // sps_temporal_mvp_enable_flag
    w.put_bits(u32::from(config.strong_intra_smoothing), 1); // sps_strong_intra_smoothing_enable_flag
    w.put_bits(u32::from(config.vui), 1); // vui_parameters_present_flag
    if config.vui {
        code_vui(&mut w, config);
    }
    w.put_bits(0, 1); // sps_extension_flag
    w.put_stop_bit();
    Ok(annex_b(NAL_SPS, w.finish_rbsp()))
}

/// `h265e_pps_write`.
pub fn build_pps(config: &EncoderConfig) -> Result<Vec<u8>, Error> {
    config.validate()?;
    let mut w = BitWriter::new();
    w.put_ue(0); // pps_pic_parameter_set_id
    w.put_ue(0); // pps_seq_parameter_set_id
    w.put_bits(0, 1); // dependent_slice_segments_enabled_flag
    w.put_bits(0, 1); // output_flag_present_flag (m_outputFlagPresentFlag)
    w.put_bits(0, 3); // num_extra_slice_header_bits
    w.put_bits(0, 1); // sign_data_hiding_flag
    w.put_bits(u32::from(config.cabac_init_present), 1); // cabac_init_present_flag
    w.put_ue(0); // num_ref_idx_l0_default_active_minus1
    w.put_ue(0); // num_ref_idx_l1_default_active_minus1
    w.put_se(i32::from(config.intra_qp) - 26); // init_qp_minus26
    w.put_bits(u32::from(config.const_intra_pred), 1); // constrained_intra_pred_flag
    w.put_bits(u32::from(config.transform_skip_enabled), 1); // transform_skip_enabled_flag
    w.put_bits(u32::from(config.cu_qp_delta_enabled()), 1); // cu_qp_delta_enabled_flag
    if config.cu_qp_delta_enabled() {
        w.put_ue(config.diff_cu_qp_delta_depth); // diff_cu_qp_delta_depth
    }
    w.put_se(config.cb_qp_offset); // pps_cb_qp_offset
    w.put_se(config.cb_qp_offset); // pps_cr_qp_offset (rkvenc copies cb onto cr)
    w.put_bits(0, 1); // pps_slice_chroma_qp_offsets_present_flag
    w.put_bits(0, 1); // weighted_pred_flag
    w.put_bits(0, 1); // weighted_bipred_flag
    w.put_bits(u32::from(config.transquant_bypass_enabled), 1); // transquant_bypass_enable_flag
    w.put_bits(0, 1); // tiles_enabled_flag (auto_tile default is 0)
    w.put_bits(0, 1); // entropy_coding_sync_enabled_flag (WPP)
                      // (tiles block skipped: tiles_enabled_flag is 0)
    w.put_bits(u32::from(config.loop_filter_across_slices), 1); // loop_filter_across_slices_enabled_flag
    w.put_bits(u32::from(config.deblocking_filter_control_present), 1);
    if config.deblocking_filter_control_present {
        w.put_bits(0, 1); // deblocking_filter_override_enabled_flag
        w.put_bits(0, 1); // pps_disable_deblocking_filter_flag
        w.put_se(config.deblock_beta_offset_div2); // pps_beta_offset_div2
        w.put_se(config.deblock_tc_offset_div2); // pps_tc_offset_div2
    }
    w.put_bits(0, 1); // pps_scaling_list_data_present_flag
    w.put_bits(1, 1); // lists_modification_present_flag (h265e_set_pps sets it to 1)
    w.put_ue(0); // log2_parallel_merge_level_minus2
    w.put_bits(0, 1); // slice_segment_header_extension_present_flag
    w.put_bits(0, 1); // pps_extension_flag
    w.put_stop_bit();
    Ok(annex_b(NAL_PPS, w.finish_rbsp()))
}

/// `codePTL(s, &vps->m_ptl, 1, m_maxTLayers - 1)` for the single-layer default:
/// `profilePresentFlag` is 1, `maxNumSubLayersMinus1` is 0, so the body reduces to
/// `codeProfileTier` followed by `general_level_idc` with no sub-layer flags or
/// reserved padding after it.
fn code_ptl(w: &mut BitWriter, config: &EncoderConfig) {
    code_profile_tier(w, config);
    w.put_bits(u32::from(config.effective_level()), 8); // general_level_idc
}

/// `codeProfileTier` for the non-range-extension (Main) profile, the else branch.
fn code_profile_tier(w: &mut BitWriter, config: &EncoderConfig) {
    w.put_bits(0, 2); // profile_space
    w.put_bits(u32::from(config.tier), 1); // tier_flag
    w.put_bits(u32::from(config.profile), 5); // profile_idc
    for j in 0..32 {
        // h265e_set_vps sets profileCompatibilityFlag[2] = 1 (the non-400 default)
        // and profileCompatibilityFlag[profile] = 1; for Main that is index 1.
        let set = j == 1 || j == 2;
        w.put_bits(u32::from(set), 1); // profile_compatibility_flag[j]
    }
    w.put_bits(1, 1); // general_progressive_source_flag
    w.put_bits(0, 1); // general_interlaced_source_flag
    w.put_bits(0, 1); // general_non_packed_constraint_flag
    w.put_bits(0, 1); // general_frame_only_constraint_flag
                      // profile_idc != FORMAT_RANGE_EXTENDIONS: reserved_zero_44bits.
    w.put_bits(0, 16);
    w.put_bits(0, 16);
    w.put_bits(0, 12);
}

/// `codeVUI`. Every flag `h265e_set_sps` leaves at its default except the timing
/// block, which it fills with the frame rate.
fn code_vui(w: &mut BitWriter, config: &EncoderConfig) {
    w.put_bits(0, 1); // aspect_ratio_info_present_flag
    w.put_bits(0, 1); // overscan_info_present_flag
    w.put_bits(0, 1); // video_signal_type_present_flag (range not JPEG)
    w.put_bits(0, 1); // chroma_loc_info_present_flag
    w.put_bits(0, 1); // neutral_chroma_indication_flag
    w.put_bits(0, 1); // field_seq_flag
    w.put_bits(0, 1); // frame_field_info_present_flag
    w.put_bits(0, 1); // default_display_window_flag
    w.put_bits(1, 1); // vui_timing_info_present_flag
    w.put_bits(config.fps_den, 32); // vui_num_units_in_tick
    w.put_bits(config.fps_num, 32); // vui_time_scale
    w.put_bits(0, 1); // vui_poc_proportional_to_timing_flag
    w.put_bits(0, 1); // hrd_parameters_present_flag
    w.put_bits(0, 1); // bitstream_restriction_flag
}

fn align(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

fn log2(value: usize) -> u32 {
    (usize::BITS - value.leading_zeros() - 1) as u32
}

fn nal_header(unit_type: u8) -> [u8; 2] {
    // forbidden_zero_bit(1) | nal_unit_type(6) | nuh_layer_id bit 5 in the first
    // byte; nuh_layer_id bits 4..0 | nuh_temporal_id_plus1(3) in the second. Layer
    // id and temporal id are both 0. Putting `unit_type & 0x01` in the second
    // byte's top bit - which this function did - writes layer_id bit 5, so an odd
    // unit type claimed nuh_layer_id 2: the SPS (33) came out `42 81` against the
    // oracle's `42 01`.
    const LAYER_ID: u8 = 0;
    const TEMPORAL_ID: u8 = 0;
    [unit_type << 1, (LAYER_ID << 3) | (TEMPORAL_ID + 1)]
}

/// Annex-B framing: a four-byte start code, the two-byte NAL header, then the RBSP
/// with emulation-prevention bytes inserted.
pub(crate) fn annex_b(unit_type: u8, rbsp: Vec<u8>) -> Vec<u8> {
    let escaped = escape_rbsp(&rbsp);
    let mut out = Vec::with_capacity(4 + 2 + escaped.len());
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(&nal_header(unit_type));
    out.extend_from_slice(&escaped);
    out
}

/// Insert an emulation-prevention byte after any `00 00` that is followed by a
/// byte in `00..=03`, so no `00 00 0x` can be mistaken for a start code. This is
/// `parser::ebsp_to_rbsp` in reverse; `the_escape_round_trips_through_the_parser`
/// checks that the two agree on every pattern that matters.
fn escape_rbsp(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + rbsp.len() / 8);
    let mut zeros = 0usize;
    for &b in rbsp {
        if zeros >= 2 && b <= 0x03 {
            out.push(0x03);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    current: u8,
    used: u8,
}

impl BitWriter {
    fn new() -> Self {
        Self::default()
    }

    fn put_bits(&mut self, value: u32, count: u8) {
        assert!(count <= 32);
        if count == 0 {
            return;
        }
        for bit in (0..count).rev() {
            self.current = (self.current << 1) | ((value >> bit) as u8 & 1);
            self.used += 1;
            if self.used == 8 {
                self.bytes.push(self.current);
                self.current = 0;
                self.used = 0;
            }
        }
    }

    fn put_ue(&mut self, value: u32) {
        let code_num = u64::from(value) + 1;
        let bits = 64 - code_num.leading_zeros() as u8;
        self.put_bits(0, bits - 1);
        self.put_bits(code_num as u32, bits);
    }

    fn put_se(&mut self, value: i32) {
        let code_num = if value <= 0 {
            (value as i64).unsigned_abs() as u64 * 2
        } else {
            (value as u64) * 2 - 1
        };
        self.put_ue(u32::try_from(code_num).expect("Exp-Golomb value overflow"));
    }

    /// `h265e_stream_rbsp_trailing`: a single 1 bit then zeros to the byte edge.
    fn put_stop_bit(&mut self) {
        self.put_bits(1, 1);
        if self.used != 0 {
            self.current <<= 8 - self.used;
            self.bytes.push(self.current);
            self.current = 0;
            self.used = 0;
        }
    }

    fn finish_rbsp(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::hevc::parser;

    #[test]
    fn geometry_of_64_sized_cu_matches_the_c_derivation() {
        let config = EncoderConfig::new(320, 240);
        let geom = config.geometry();
        assert_eq!(geom.log2_min_cb, 3);
        assert_eq!(geom.log2_diff_max_min_cb, 3);
        assert_eq!(geom.log2_min_tb, 2);
        assert_eq!(geom.log2_diff_max_min_tb, 3);
    }

    #[test]
    fn max_dec_pic_buffering_is_three_at_default_num_ref() {
        // min(MAX_REFS, max(0+3, num_ref)) + 0 == 3 for any num_ref in 1..=3.
        assert_eq!(EncoderConfig::new(320, 240).max_dec_pic_buffering(), 3);
    }

    #[test]
    fn level_is_not_raised_below_four_point_zero() {
        let mut config = EncoderConfig::new(320, 240);
        config.level = 120;
        assert_eq!(config.effective_level(), 120);
    }

    #[test]
    fn coded_size_pads_to_the_minimum_cu() {
        assert_eq!(EncoderConfig::new(320, 240).coded_size(), (320, 240));
        assert_eq!(EncoderConfig::new(322, 242).coded_size(), (328, 248));
    }

    #[test]
    fn slice_split_validation_matches_the_vepu580_fields() {
        assert!(SliceSplit::default().validate().is_ok());
        assert_eq!(
            SliceSplit {
                mode: SplitMode::ByCtu,
                arg: 96,
                out: MPP_ENC_SPLIT_OUT_LOWDELAY,
            }
            .slice_count(320, 240),
            1
        );
        assert_eq!(
            SliceSplit {
                mode: SplitMode::ByCtu,
                arg: 8,
                out: MPP_ENC_SPLIT_OUT_LOWDELAY,
            }
            .slice_count(320, 240),
            3
        );
        assert!(SliceSplit {
            mode: SplitMode::ByByte,
            arg: 0,
            out: 0,
        }
        .validate()
        .is_err());
        assert!(SliceSplit {
            mode: SplitMode::None,
            arg: 0,
            out: MPP_ENC_SPLIT_OUT_LOWDELAY,
        }
        .validate()
        .is_err());
    }

    /// Round-trip the VPS and SPS through the independent decoder parser and
    /// check every field it can see came out as the C source sets it. The parser
    /// is a separate implementation, so agreement is a genuine cross-check.
    ///
    /// The start code and two-byte NAL header are sliced off explicitly, then the
    /// escaped payload goes through `parser::ebsp_to_rbsp` because the parser reads
    /// RBSP. Both operations are load-bearing: the default VUI carries
    /// `vui_num_units_in_tick = 1`, four bytes that begin `00 00 00 01`, and the
    /// formatter escapes that to `00 00 03 00 01` - a parser fed the EBSP directly
    /// would read the `0x03` as VUI data.
    #[test]
    fn sps_and_vps_round_trip_through_the_decoder_parser() {
        let config = EncoderConfig::new(320, 240);
        let vps = build_vps(&config).unwrap();
        let sps = build_sps(&config).unwrap();

        assert_eq!(&vps[4..6], &[NAL_VPS << 1, 0x01]);
        let vps_rbsp = parser::ebsp_to_rbsp(&vps[6..]);
        let parsed_vps = parser::parse_vps(&vps_rbsp).unwrap();
        assert_eq!(parsed_vps.vps_id, 0);
        assert_eq!(parsed_vps.max_sub_layers, 1);
        assert!(parsed_vps.temporal_id_nesting);
        assert_eq!(parsed_vps.profile.general_profile_idc, PROFILE_HEVC_MAIN);
        assert_eq!(parsed_vps.profile.general_level_idc, 120);

        let sps_rbsp = parser::ebsp_to_rbsp(&sps[6..]);
        let parsed_sps = parser::parse_sps(&sps_rbsp).unwrap();
        assert_eq!(parsed_sps.sps_id, 0);
        assert_eq!(parsed_sps.vps_id, 0);
        assert_eq!(parsed_sps.chroma_format_idc, 1);
        assert_eq!(parsed_sps.width, 320);
        assert_eq!(parsed_sps.height, 240);
        assert_eq!(parsed_sps.bit_depth_luma, 8);
        assert_eq!(parsed_sps.log2_max_pic_order_cnt_lsb, 16);
        assert_eq!(parsed_sps.max_dec_pic_buffering, 3);
        assert_eq!(parsed_sps.log2_min_cb, 3);
        assert_eq!(parsed_sps.log2_diff_max_min_cb, 3);
        assert_eq!(parsed_sps.log2_min_tb, 2);
        assert_eq!(parsed_sps.log2_diff_max_min_tb, 3);
        assert!(!parsed_sps.amp_enabled);
        assert!(parsed_sps.sao_enabled);
        assert!(!parsed_sps.pcm_enabled);
        assert!(!parsed_sps.long_term_ref_pics_present);
        assert!(!parsed_sps.temporal_mvp_enabled);
        assert!(parsed_sps.strong_intra_smoothing);
    }

    #[test]
    fn cropped_sps_round_trips_to_the_visible_size() {
        let config = EncoderConfig::new(322, 242);
        let sps = build_sps(&config).unwrap();
        let parsed = parser::parse_sps(&parser::ebsp_to_rbsp(&sps[6..])).unwrap();
        assert_eq!((parsed.width, parsed.height), (322, 242));
    }

    /// Round-trip the PPS through the parser. `loop_filter_across_tiles` is not
    /// written when tiles are off, so the parser keeps its MPP default there; the
    /// fields the writer does emit must all read back.
    #[test]
    fn pps_round_trips_through_the_decoder_parser() {
        let config = EncoderConfig::new(320, 240);
        let pps = build_pps(&config).unwrap();
        assert_eq!(&pps[4..6], &[NAL_PPS << 1, 0x01][..]);
        let parsed = parser::parse_pps(&pps[6..]).unwrap();
        assert_eq!(parsed.pps_id, 0);
        assert_eq!(parsed.sps_id, 0);
        assert!(!parsed.dependent_slice_segments);
        assert!(!parsed.output_flag_present);
        assert_eq!(parsed.num_extra_slice_header_bits, 0);
        assert!(!parsed.sign_data_hiding);
        assert!(parsed.cabac_init_present);
        assert_eq!(parsed.num_ref_idx_l0_default_active, 1);
        assert_eq!(parsed.num_ref_idx_l1_default_active, 1);
        assert_eq!(parsed.init_qp, 26);
        assert!(!parsed.constrained_intra_pred);
        assert!(!parsed.transform_skip_enabled);
        // Fixed QP: m_useDQP stays 0 and no depth is written.
        assert!(!parsed.cu_qp_delta_enabled);
        assert_eq!(parsed.diff_cu_qp_delta_depth, 0);
        assert_eq!(parsed.cb_qp_offset, 0);
        assert_eq!(parsed.cr_qp_offset, 0);
        assert!(!parsed.weighted_pred);
        assert!(!parsed.weighted_bipred);
        assert!(!parsed.tiles_enabled);
        assert!(!parsed.entropy_coding_sync);
        assert!(parsed.deblocking_filter_control_present);
        assert!(!parsed.deblocking_filter_override_enabled);
        assert!(!parsed.pps_deblocking_filter_disabled);
        assert!(parsed.lists_modification_present);
        assert_eq!(parsed.log2_parallel_merge_level, 2);
        assert!(!parsed.slice_segment_header_extension_present);
    }

    #[test]
    fn init_qp_minus_26_follows_intra_qp() {
        let mut config = EncoderConfig::new(320, 240);
        config.intra_qp = 30;
        let pps = build_pps(&config).unwrap();
        let parsed = parser::parse_pps(&pps[6..]).unwrap();
        assert_eq!(parsed.init_qp, 30);
    }

    #[test]
    fn cb_qp_offset_is_mirrored_onto_cr() {
        let mut config = EncoderConfig::new(320, 240);
        config.cb_qp_offset = -3;
        let pps = build_pps(&config).unwrap();
        let parsed = parser::parse_pps(&pps[6..]).unwrap();
        assert_eq!((parsed.cb_qp_offset, parsed.cr_qp_offset), (-3, -3));
    }

    /// The first six VPS RBSP bytes, hand-derived from `h265e_set_vps` +
    /// `h265e_vps_write` for the 320x240 Main/4.0 defaults and independent of the
    /// PTL payload: `vps_id 0 | reserved_three 11 | reserved_zero six 0 |` gives
    /// `0x0c`; `max_sub_layers_minus1 0 | temporal_id_nesting 1` gives `0x01`; the
    /// two `vps_reserved_ffff_16bits` bytes are `ff ff`; the PTL opens `space 00 |
    /// tier 0 | idc 00001` = `0x01`; the first eight compatibility flags
    /// (`flag[1]` and `flag[2]` set) are `0110 0000` = `0x60`. The parser
    /// round-trip above checks the rest, field by field.
    #[test]
    fn vps_leading_bytes_match_the_hand_derived_c_bytes() {
        let vps = build_vps(&EncoderConfig::new(320, 240)).unwrap();
        assert_eq!(&vps[..4], &[0, 0, 0, 1]);
        assert_eq!(&vps[4..6], &[NAL_VPS << 1, 0x01]);
        assert_eq!(&vps[6..12], &[0x0c, 0x01, 0xff, 0xff, 0x01, 0x60]);
        // The tail is the oracle's own bytes, from `mpi_enc_test -t 16777220 -n 1
        // -rc 2 -qc 26:26:26:26:26` on t6 at 320x240: the 44 reserved PTL bits are
        // zero, so the formatter inserts `03` before the `00`s and the `80`, and
        // level 120 (`0x78`) lands right after them.
        assert_eq!(
            &vps[6..27],
            &[
                0x0c, 0x01, 0xff, 0xff, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x80, 0x00, 0x00, 0x03,
                0x00, 0x00, 0x03, 0x00, 0x78, 0xbc, 0x09,
            ]
        );
    }

    /// Annex B reserves `00 00 0x` for start codes, `x` in `00..=03`. The escape
    /// has to be the exact inverse of the parser's strip, including on long zero
    /// runs where a pair gets a byte but the run does not.
    #[test]
    fn the_escape_round_trips_through_the_parser() {
        assert_eq!(escape_rbsp(&[0, 0, 0]), [0, 0, 3, 0]);
        assert_eq!(escape_rbsp(&[0, 0, 3]), [0, 0, 3, 3]);
        assert_eq!(escape_rbsp(&[0, 0, 4]), [0, 0, 4]);
        // One byte per offending pair: `00 00 03 00 00` leaves no run of two
        // unescaped zeroes followed by a low byte, which is all the rule asks.
        assert_eq!(escape_rbsp(&[0, 0, 0, 0]), [0, 0, 3, 0, 0]);
        assert_eq!(escape_rbsp(&[0, 0, 1, 0, 0, 2]), [0, 0, 3, 1, 0, 0, 3, 2]);
        assert_eq!(escape_rbsp(&[0; 6]), [0, 0, 3, 0, 0, 3, 0, 0]);

        let rbsp: Vec<u8> = (0u16..=255).map(|b| b as u8).collect();
        assert_eq!(
            parser::ebsp_to_rbsp(&escape_rbsp(&rbsp)).as_ref(),
            &rbsp[..]
        );
    }

    #[test]
    fn headers_are_the_three_parameter_sets_in_order() {
        let headers = build_headers(&EncoderConfig::new(320, 240)).unwrap();
        let vps = build_vps(&EncoderConfig::new(320, 240)).unwrap();
        let sps = build_sps(&EncoderConfig::new(320, 240)).unwrap();
        let pps = build_pps(&EncoderConfig::new(320, 240)).unwrap();
        assert_eq!(headers.len(), vps.len() + sps.len() + pps.len());
        assert!(headers.starts_with(&vps));
        assert!(headers.ends_with(&pps));
    }

    #[test]
    fn pps_matches_the_hand_derived_c_bytes() {
        // The bit string, in order: pps_id ue(0), sps_id ue(0), dependent 0, output
        // 0, num_extra 000, sign 0, cabac_init 1, refIdxL0 ue(0), refIdxL1 ue(0),
        // init_qp se(0), const_intra 0, transform_skip 0, dqp_en 0 (fixed QP),
        // cb se(0), cr se(0), slice_chroma 0, weighted_pred 0, weighted_bipred 0,
        // transquant 0, tiles 0, entropy_sync 0, lfas 0, dbcp 1, override 0,
        // disable 0, beta se(0), tc se(0), scaling 0, lists_mod 1, merge ue(0),
        // hdr_ext 0, pps_ext 0, then the rbsp trailing 1+N zeros.
        //
        //   1 1 0 0 0 0 0 0 | 1 1 1 1 0 0 0 1 | 1 0 0 0 0 0 0 0 | 1 0 0 1 1 0 1 1
        //   | 0 0 1 0 0 0 0 0
        // = c0 f1 80 9b 20, behind a 44 01 NAL header (PPS = 34). This is the
        // oracle's own PPS from the 320x240 board cell; the hand-derived golden
        // this test used to carry had dqp_en 1 and therefore `c0 f3 c0 4d 90`.
        let pps = build_pps(&EncoderConfig::new(320, 240)).unwrap();
        assert_eq!(
            pps,
            vec![0, 0, 0, 1, 0x44, 0x01, 0xc0, 0xf1, 0x80, 0x9b, 0x20]
        );
    }

    /// `h265e_set_pps` turns `m_useDQP` on for CBR and VBR and writes
    /// `diff_cu_qp_delta_depth` after the flag, which inserts the very `ue(0)` bits
    /// the fixed-QP stream omits. Both modes take the same branch.
    #[test]
    fn rate_controlled_pps_enables_cu_qp_delta() {
        for mode in [RateControl::Cbr, RateControl::Vbr] {
            let mut config = EncoderConfig::new(320, 240);
            config.rc_mode = mode;
            let pps = build_pps(&config).unwrap();
            let parsed = parser::parse_pps(&parser::ebsp_to_rbsp(&pps[6..])).unwrap();
            assert!(parsed.cu_qp_delta_enabled, "{mode:?}");
            assert_eq!(parsed.diff_cu_qp_delta_depth, 0, "{mode:?}");
        }
    }

    #[test]
    fn rejects_zero_size_before_writing_a_header() {
        assert!(build_headers(&EncoderConfig::new(0, 0)).is_err());
    }

    #[test]
    fn rejects_ten_bit_without_claiming_unsupported_output() {
        let mut config = EncoderConfig::new(320, 240);
        config.bit_depth = 10;
        let error = build_headers(&config).unwrap_err().to_string();
        assert!(error.contains("does not support 10-bit"));
    }
}
