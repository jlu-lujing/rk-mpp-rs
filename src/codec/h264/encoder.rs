//! H.264 encoder high-level syntax.
//!
//! This is a bit-exact port of MPP's `h264e_sps.c` and `h264e_pps.c`. The
//! hardware emits slice payloads; the encoder layer owns Annex-B headers,
//! VUI, emulation prevention and escaping.

use std::fmt;

pub const PROFILE_BASELINE: u8 = 66;
pub const PROFILE_MAIN: u8 = 77;
pub const PROFILE_HIGH: u8 = 100;

const NAL_SPS: u8 = 7;
const NAL_PPS: u8 = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Encoder configuration used by the high-level syntax writer.
///
/// The defaults match MPP's RKVENC H.264 defaults after `mpi_enc_test` sets
/// its common 30 fps values. `gop == 1` intentionally selects the same
/// 16-bit `frame_num` and POC counters used by the board reference stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncoderConfig {
    pub width: usize,
    pub height: usize,
    pub profile: u8,
    pub level_idc: u8,
    pub gop: u32,
    pub gaps_in_frame_num_allowed: bool,
    pub poc_type: u8,
    pub log2_max_frame_num_minus4: u8,
    pub log2_max_poc_lsb_minus4: u8,
    pub num_ref_frames: u32,
    pub qp_init: u8,
    pub entropy_coding: bool,
    pub transform_8x8: bool,
    pub chroma_qp_index_offset: i8,
    pub second_chroma_qp_index_offset: i8,
    /// Bit rate figures, carried into the `uuid_rc_cfg` SEI today and into the
    /// rate-control registers once M7's RC model lands. MPP writes them into that
    /// SEI whatever the mode, so they are carried from the start rather than
    /// filled with an invented constant later.
    ///
    /// The three defaults are `h264e_api_v2.c`'s "2Mbps +-25 %": the H.264
    /// encoder API initialises `bps_target` to 2 000 000 and derives the bounds
    /// as `* 5 / 4` and `* 3 / 4`. They are the values a caller gets for not
    /// asking, and they are what MPP's own SEI shows when the harness never set
    /// bounds - which the fixed-QP comparison does.
    pub rc_mode: RcMode,
    pub bps_target: i64,
    pub bps_min: i64,
    pub bps_max: i64,
    pub fps_num: u32,
    pub fps_den: u32,
    pub vui: bool,
}

/// The bit rate target a caller that says nothing gets.
///
/// `mpi_enc_test` fills an unset `-bps` with
/// `p->width * p->height / 8 * (fps_out_num / fps_out_den)` and pushes it into
/// `rc:bps_target` for every rate-control mode, fixed QP included (`test/
/// mpi_enc_test.c`). Fixed QP ignores the target for quantisation but still
/// prints it into the `uuid_rc_cfg` SEI, so a stream compared against that
/// harness has to carry the same derived number.
///
/// The division comes before the multiplication, as it does there: at 64x64/30
/// that order yields 15360, and `(64 * 64 * 30) / 8` would yield the same only
/// until a size where truncation differs.
pub fn default_bps_target(width: usize, height: usize, fps: u32) -> i64 {
    (width * height / 8 * usize::try_from(fps).unwrap()) as i64
}

/// Which rate control drives the encoder.
///
/// `FixedQp` is the path the whole comparison matrix was built on. `Cbr` runs
/// `codec::rc::model` per picture and writes the rate-control register block; a
/// stream from one is not comparable to a stream from the other by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RcMode {
    FixedQp,
    Cbr,
    Vbr,
    Avbr,
}

impl EncoderConfig {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            profile: PROFILE_BASELINE,
            level_idc: 30,
            gop: 1,
            gaps_in_frame_num_allowed: true,
            poc_type: 0,
            log2_max_frame_num_minus4: 12,
            log2_max_poc_lsb_minus4: 12,
            num_ref_frames: 1,
            qp_init: 26,
            entropy_coding: false,
            transform_8x8: false,
            chroma_qp_index_offset: -6,
            second_chroma_qp_index_offset: -6,
            rc_mode: RcMode::FixedQp,
            bps_target: 2_000_000,
            bps_min: 1_500_000,
            bps_max: 2_500_000,
            fps_num: 30,
            fps_den: 1,
            vui: true,
        }
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.width == 0 || self.height == 0 {
            return Err(Error("H.264 dimensions must be non-zero".into()));
        }
        if self.width > u16::MAX as usize || self.height > u16::MAX as usize {
            return Err(Error("H.264 dimensions must fit in 16 bits".into()));
        }
        match self.profile {
            PROFILE_BASELINE | PROFILE_MAIN | PROFILE_HIGH => {}
            other => {
                return Err(Error(format!(
                    "unsupported H.264 profile {other}; expected 66, 77 or 100"
                )));
            }
        }
        if self.poc_type > 2 {
            return Err(Error("pic_order_cnt_type must be 0, 1 or 2".into()));
        }
        if self.poc_type == 1 {
            return Err(Error(
                "MPP RKVENC does not implement pic_order_cnt_type 1".into(),
            ));
        }
        if self.log2_max_frame_num_minus4 > 12 {
            return Err(Error(
                "log2_max_frame_num_minus4 exceeds H.264 limit".into(),
            ));
        }
        if self.poc_type == 0 && self.log2_max_poc_lsb_minus4 > 12 {
            return Err(Error(
                "log2_max_pic_order_cnt_lsb_minus4 exceeds H.264 limit".into(),
            ));
        }
        if self.num_ref_frames > 16 {
            return Err(Error("num_ref_frames must not exceed 16".into()));
        }
        if self.qp_init > 51 {
            return Err(Error("pic_init_qp must be in 0..=51".into()));
        }
        if self.fps_num == 0 || self.fps_den == 0 {
            return Err(Error(
                "frame-rate numerator and denominator must be non-zero".into(),
            ));
        }
        if self.profile <= PROFILE_BASELINE && self.entropy_coding {
            return Err(Error("baseline profile cannot use CABAC".into()));
        }
        if self.profile < PROFILE_HIGH && self.transform_8x8 {
            return Err(Error(
                "transform_8x8 is only valid for High profile or above".into(),
            ));
        }
        if !(-12..=12).contains(&self.chroma_qp_index_offset)
            || !(-12..=12).contains(&self.second_chroma_qp_index_offset)
        {
            return Err(Error("chroma QP offsets must be in -12..=12".into()));
        }
        Ok(())
    }

    pub fn coded_width(&self) -> usize {
        align(self.width, 16)
    }

    pub fn coded_height(&self) -> usize {
        align(self.height, 16)
    }

    pub fn frame_num_bits(&self) -> u8 {
        self.log2_max_frame_num_minus4 + 4
    }

    pub fn poc_lsb_bits(&self) -> u8 {
        self.log2_max_poc_lsb_minus4 + 4
    }

    /// MPP raises a requested level when the coded picture size needs it.
    /// This intentionally mirrors `h264e_sps_update()`'s frame-size check.
    pub fn effective_level_idc(&self) -> u8 {
        let mbs = self.coded_width() * self.coded_height() / 256;
        const LEVELS: &[(u8, usize)] = &[
            (10, 99),
            (11, 396),
            (12, 396),
            (13, 396),
            (20, 396),
            (21, 792),
            (22, 1_620),
            (30, 1_620),
            (31, 3_600),
            (32, 5_120),
            (40, 8_192),
            (41, 8_192),
            (42, 8_704),
            (50, 22_080),
            (51, 36_864),
            (52, 36_864),
            (60, 139_264),
            (61, 139_264),
            (62, 139_264),
        ];
        for &(level, max_mbs) in LEVELS {
            if max_mbs >= mbs {
                return self.level_idc.max(level);
            }
        }
        self.level_idc
    }

    /// `h264e_sps_init` picks the constraint flags per profile: Baseline sets
    /// set0 and set1, Main sets set1, High sets set3. Nothing else in this crate
    /// forces them (`constraint_set` stays zero, so the force path in the
    /// reference never fires).
    fn constraints(&self) -> u8 {
        match self.profile {
            PROFILE_BASELINE => 0b1100_0000,
            PROFILE_MAIN => 0b0100_0000,
            PROFILE_HIGH => 0b0001_0000,
            _ => 0,
        }
    }
}

/// Serialize SPS and PPS with four-byte Annex-B start codes.
pub fn build_headers(config: &EncoderConfig) -> Result<Vec<u8>, Error> {
    config.validate()?;
    let mut out = build_sps(config)?;
    out.extend_from_slice(&build_pps(config)?);
    Ok(out)
}

pub fn build_sps(config: &EncoderConfig) -> Result<Vec<u8>, Error> {
    config.validate()?;
    let mut w = BitWriter::new();
    w.put_raw(u32::from(nal_header(3, NAL_SPS)), 8);
    w.put_bits(u32::from(config.profile), 8);
    w.put_bits(u32::from(config.constraints()), 8);
    w.put_bits(u32::from(config.effective_level_idc()), 8);
    w.put_ue(0); // seq_parameter_set_id

    if config.profile >= PROFILE_HIGH {
        w.put_ue(1); // chroma_format_idc: 4:2:0
        w.put_ue(0); // bit_depth_luma_minus8
        w.put_ue(0); // bit_depth_chroma_minus8
        w.put_bits(0, 1); // qpprime_y_zero_transform_bypass_flag
        w.put_bits(0, 1); // seq_scaling_matrix_present_flag
    }

    w.put_ue(u32::from(config.log2_max_frame_num_minus4));
    w.put_ue(u32::from(config.poc_type));
    if config.poc_type == 0 {
        w.put_ue(u32::from(config.log2_max_poc_lsb_minus4));
    }
    w.put_ue(config.num_ref_frames);
    w.put_bits(u32::from(config.gaps_in_frame_num_allowed), 1);

    let width_mbs = (config.coded_width() >> 4) as u32;
    let height_mbs = (config.coded_height() >> 4) as u32;
    w.put_ue(width_mbs - 1);
    w.put_ue(height_mbs - 1);
    w.put_bits(1, 1); // frame_mbs_only_flag
    w.put_bits(u32::from(config.transform_8x8), 1); // direct_8x8_inference_flag

    let crop_right = config.coded_width() - config.width;
    let crop_bottom = config.coded_height() - config.height;
    let cropping = crop_right != 0 || crop_bottom != 0;
    w.put_bits(u32::from(cropping), 1);
    if cropping {
        // 4:2:0 ChromaArrayType: SubWidthC=2, SubHeightC=2.
        w.put_ue(0); // frame_crop_left_offset
        w.put_ue((crop_right / 2) as u32);
        w.put_ue(0); // frame_crop_top_offset
        w.put_ue((crop_bottom / 2) as u32);
    }

    w.put_bits(u32::from(config.vui), 1);
    if config.vui {
        write_vui(&mut w, config);
    }
    w.put_stop_bit();
    Ok(annex_b(NAL_SPS, w.finish_rbsp()))
}

pub fn build_pps(config: &EncoderConfig) -> Result<Vec<u8>, Error> {
    config.validate()?;
    let mut w = BitWriter::new();
    w.put_raw(u32::from(nal_header(3, NAL_PPS)), 8);
    w.put_ue(0); // pic_parameter_set_id
    w.put_ue(0); // seq_parameter_set_id
    w.put_bits(u32::from(config.entropy_coding), 1);
    w.put_bits(0, 1); // bottom_field_pic_order_in_frame_present_flag
    w.put_ue(0); // num_slice_groups_minus1
    w.put_ue(0); // num_ref_idx_l0_default_active_minus1
    w.put_ue(0); // num_ref_idx_l1_default_active_minus1
    w.put_bits(0, 1); // weighted_pred_flag
    w.put_bits(0, 2); // weighted_bipred_idc
                      // Not `config.qp_init`. `h264e_pps_init` sets `pic_init_qp = 26` and
                      // `pic_init_qs = pic_init_qp` unconditionally, and nothing in either the 2024-04
                      // vintage or master writes the requested QP there - the QP reaches the encode as
                      // `enc_pic.pic_qp`, and the hardware emits `slice_qp_delta` as the difference
                      // against this PPS value. Putting qp_init in both places applies it twice: on the
                      // board that made a PPS of 6 bytes where the reference writes 4, and a slice the
                      // decoder would resolve eight QP steps away from the reference's.
    w.put_se(0); // pic_init_qp_minus26
    w.put_se(0); // pic_init_qs_minus26
    w.put_se(i32::from(config.chroma_qp_index_offset));
    w.put_bits(1, 1); // deblocking_filter_control_present_flag
    w.put_bits(0, 1); // constrained_intra_pred_flag
    w.put_bits(0, 1); // redundant_pic_cnt_present_flag

    let high_profile_tail = config.transform_8x8
        || config.profile >= PROFILE_HIGH
        || config.second_chroma_qp_index_offset != config.chroma_qp_index_offset;
    if high_profile_tail {
        w.put_bits(u32::from(config.transform_8x8), 1);
        w.put_bits(0, 1); // pic_scaling_matrix_present_flag
        w.put_se(i32::from(config.second_chroma_qp_index_offset));
    }
    w.put_stop_bit();
    Ok(annex_b(NAL_PPS, w.finish_rbsp()))
}

fn write_vui(w: &mut BitWriter, config: &EncoderConfig) {
    w.put_bits(0, 1); // aspect_ratio_info_present_flag
    w.put_bits(0, 1); // overscan_info_present_flag
    w.put_bits(0, 1); // video_signal_type_present_flag
    w.put_bits(0, 1); // chroma_loc_info_present_flag
    w.put_bits(1, 1); // timing_info_present_flag
    w.put_bits(config.fps_den, 32); // num_units_in_tick
    w.put_bits(config.fps_num.saturating_mul(2), 32); // time_scale
    w.put_bits(1, 1); // fixed_frame_rate_flag
    w.put_bits(0, 1); // nal_hrd_parameters_present_flag
    w.put_bits(0, 1); // vcl_hrd_parameters_present_flag
    w.put_bits(0, 1); // pic_struct_present_flag
    w.put_bits(1, 1); // bitstream_restriction_flag
    w.put_bits(1, 1); // motion_vectors_over_pic_boundaries_flag
    w.put_ue(0); // max_bytes_per_pic_denom
    w.put_ue(0); // max_bits_per_mb_denom
                 // The 2016 text allows 16. The installed t6 MPP library emits 16 even
                 // though this repository's older `refs/mpp` tree still says 15.
    w.put_ue(16); // log2_max_mv_length_horizontal
    w.put_ue(16); // log2_max_mv_length_vertical
    w.put_ue(0); // num_reorder_frames
    w.put_ue(config.num_ref_frames); // max_dec_frame_buffering
}

fn nal_header(ref_idc: u8, unit_type: u8) -> u8 {
    (ref_idc << 5) | unit_type
}

fn annex_b(unit_type: u8, mut rbsp: Vec<u8>) -> Vec<u8> {
    debug_assert_eq!(rbsp[0], nal_header(3, unit_type));
    let header = rbsp.remove(0);
    let mut out = Vec::with_capacity(4 + 1 + rbsp.len() + rbsp.len() / 2);
    out.extend_from_slice(&[0, 0, 0, 1, header]);
    let mut zeros = 0usize;
    for byte in rbsp {
        if zeros >= 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        if byte == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
    }
    out
}

fn align(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
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

    fn put_raw(&mut self, value: u32, count: u8) {
        self.put_bits(value, count);
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

    fn put_stop_bit(&mut self) {
        self.put_bits(1, 1);
        if self.used != 0 {
            self.current <<= 8 - self.used;
            self.bytes.push(self.current);
            self.current = 0;
            self.used = 0;
        }
    }

    fn finish_rbsp(mut self) -> Vec<u8> {
        if self.used != 0 {
            self.current <<= 8 - self.used;
            self.bytes.push(self.current);
        }
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD_DEFAULT: &[u8] = &[
        0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xc0, 0x1e, 0x8d, 0x8d, 0x50, 0xa0, 0xf9, 0x08, 0x00,
        0x00, 0x03, 0x00, 0x08, 0x00, 0x00, 0x03, 0x01, 0xe4, 0x78, 0x44, 0x23, 0x50, 0x00, 0x00,
        0x00, 0x01, 0x68, 0xce, 0x31, 0xb2,
    ];

    /// The PPS never carries the requested QP. `h264e_pps_init` pins
    /// `pic_init_qp` at 26 and the hardware derives `slice_qp_delta` from that, so
    /// writing qp_init here would apply it twice. On the board this showed up as a
    /// 6-byte PPS against the reference's 4 at every QP but 26.
    /// SPS and PPS bytes taken from the C encoder on t6 at 320x240 QP 26 for each
    /// profile it can be pinned to (`mpi_enc_test` has to be patched, see
    /// tools/patches). These are the headers our own writer must reproduce, byte
    /// for byte, before the hardware ever sees a picture.
    #[test]
    fn sps_and_pps_match_the_c_encoder_per_profile() {
        let cases: [(&str, u8, bool, bool, &[u8], &[u8]); 3] = [
            (
                "baseline",
                PROFILE_BASELINE,
                false,
                false,
                &[
                    0x67, 0x42, 0xc0, 0x1e, 0x8d, 0x8d, 0x50, 0xa0, 0xf9, 0x08, 0x00, 0x00, 0x03,
                    0x00, 0x08, 0x00, 0x00, 0x03, 0x01, 0xe4, 0x78, 0x44, 0x23, 0x50,
                ],
                &[0x68, 0xce, 0x31, 0xb2],
            ),
            (
                "main",
                PROFILE_MAIN,
                true,
                false,
                &[
                    0x67, 0x4d, 0x40, 0x1e, 0x8d, 0x8d, 0x50, 0xa0, 0xf9, 0x08, 0x00, 0x00, 0x03,
                    0x00, 0x08, 0x00, 0x00, 0x03, 0x01, 0xe4, 0x78, 0x44, 0x23, 0x50,
                ],
                &[0x68, 0xee, 0x31, 0xb2],
            ),
            (
                "high",
                PROFILE_HIGH,
                true,
                true,
                &[
                    0x67, 0x64, 0x10, 0x1e, 0xac, 0x1b, 0x1a, 0xa1, 0x41, 0xfa, 0x10, 0x00, 0x00,
                    0x03, 0x00, 0x10, 0x00, 0x00, 0x03, 0x03, 0xc8, 0xf0, 0x88, 0x46, 0xa0,
                ],
                &[0x68, 0xee, 0x31, 0xb2, 0x1b],
            ),
        ];
        for (name, profile, cabac, t8x8, sps, pps) in cases {
            let mut config = EncoderConfig::new(320, 240);
            config.profile = profile;
            config.entropy_coding = cabac;
            config.transform_8x8 = t8x8;
            let got_sps = build_sps(&config).unwrap();
            let got_pps = build_pps(&config).unwrap();
            // build_sps / build_pps include the four-byte start code.
            assert_eq!(&got_sps[4..], sps, "{name} SPS");
            assert_eq!(&got_pps[4..], pps, "{name} PPS");
        }
    }

    #[test]
    fn pps_qp_fields_ignore_qp_init() {
        for qp in [0u8, 10, 18, 26, 32, 42, 51] {
            let mut config = EncoderConfig::new(320, 240);
            config.qp_init = qp;
            assert_eq!(
                build_pps(&config).unwrap(),
                build_pps(&EncoderConfig::new(320, 240)).unwrap(),
                "PPS must not move with QP {qp}"
            );
        }
    }

    #[test]
    fn board_default_headers_are_byte_exact() {
        let got = build_headers(&EncoderConfig::new(320, 240)).unwrap();
        assert_eq!(got, BOARD_DEFAULT);
        assert_eq!(got.len(), 36);
    }

    #[test]
    fn sps_and_pps_split_at_the_expected_offsets() {
        let config = EncoderConfig::new(320, 240);
        let sps = build_sps(&config).unwrap();
        let pps = build_pps(&config).unwrap();
        assert_eq!(sps.len(), 28);
        assert_eq!(pps.len(), 8);
        assert_eq!(sps[4] & 0x1f, NAL_SPS);
        assert_eq!(pps[4] & 0x1f, NAL_PPS);
        assert_eq!(&sps[..], &BOARD_DEFAULT[..28]);
        assert_eq!(&pps[..], &BOARD_DEFAULT[28..]);
    }

    #[test]
    fn cropped_dimensions_round_trip() {
        let config = EncoderConfig::new(322, 242);
        let sps = build_sps(&config).unwrap();
        let nals = crate::codec::h264::split_annex_b(&sps);
        let parsed = crate::codec::h264::parse_sps(&nals[0].rbsp).unwrap();
        assert_eq!(parsed.frame_crop, Some((0, 7, 0, 7)));
        assert_eq!((parsed.width, parsed.height), (322, 242));
        assert_eq!((parsed.coded_width, parsed.coded_height), (336, 256));
    }

    #[test]
    fn level_is_raised_for_the_coded_picture_size() {
        assert_eq!(EncoderConfig::new(640, 480).effective_level_idc(), 30);
        assert_eq!(EncoderConfig::new(1280, 720).effective_level_idc(), 31);
        assert_eq!(EncoderConfig::new(1920, 1080).effective_level_idc(), 40);

        let sps = build_sps(&EncoderConfig::new(1280, 720)).unwrap();
        let nals = crate::codec::h264::split_annex_b(&sps);
        let parsed = crate::codec::h264::parse_sps(&nals[0].rbsp).unwrap();
        assert_eq!(parsed.level_idc, 31);
    }

    #[test]
    fn exp_golomb_and_unsigned_values() {
        let mut w = BitWriter::new();
        w.put_ue(0);
        w.put_ue(1);
        w.put_ue(2);
        w.put_se(0);
        w.put_se(1);
        w.put_se(-1);
        w.put_stop_bit();
        assert_eq!(w.finish_rbsp(), [0b1010_0111, 0b0100_1110]);
    }

    #[test]
    fn rejects_baseline_cabac_before_writing_a_mismatched_header() {
        let mut config = EncoderConfig::new(320, 240);
        config.entropy_coding = true;
        assert!(build_headers(&config).is_err());
    }
}
