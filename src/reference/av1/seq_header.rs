//! AV1 `sequence_header_obu()` (specification §5.5).
//!
//! This is the first bitstream-driven layer in the reference decoder: every
//! frame header syntax element (frame size, superblock size, feature flags,
//! bit depth, subsampling, operating point) ultimately depends on the fields
//! parsed here. Nothing is inferred from the container; all values come from
//! the OBU payload bits.
//!
//! `choose_operating_point()` is implementation-defined in the specification,
//! so callers may pass an explicit index; the default is 0.

use crate::reference::av1::bitstream::{BitReader, BitstreamError};
use crate::reference::av1::obu::{Obu, ObuError, ObuType};

pub const SELECT_SCREEN_CONTENT_TOOLS: u8 = 2;
pub const SELECT_INTEGER_MV: u8 = 2;

pub const CP_UNSPECIFIED: u8 = 2;
pub const CP_BT_709: u8 = 1;
pub const TC_UNSPECIFIED: u8 = 2;
pub const TC_SRGB: u8 = 13;
pub const MC_UNSPECIFIED: u8 = 2;
pub const MC_IDENTITY: u8 = 0;
pub const CSP_UNKNOWN: u8 = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeqHeaderError {
    Bitstream(BitstreamError),
    Obu(ObuError),
    WrongObuType(ObuType),
    BadOperatingPoint { requested: usize, available: usize },
}

impl From<BitstreamError> for SeqHeaderError {
    fn from(value: BitstreamError) -> Self {
        Self::Bitstream(value)
    }
}

impl From<ObuError> for SeqHeaderError {
    fn from(value: ObuError) -> Self {
        Self::Obu(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingInfo {
    pub num_units_in_display_tick: u32,
    pub time_scale: u32,
    pub equal_picture_interval: bool,
    /// Only transmitted when `equal_picture_interval` is set.
    pub num_ticks_per_picture_minus_1: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecoderModelInfo {
    pub buffer_delay_length_minus_1: u8,
    pub num_units_in_decoding_tick: u32,
    pub buffer_removal_time_length_minus_1: u8,
    pub frame_presentation_time_length_minus_1: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatingPoint {
    pub idc: u16,
    pub level_idx: u8,
    pub tier: u8,
    pub decoder_model_present: bool,
    pub decoder_buffer_delay: u32,
    pub encoder_buffer_delay: u32,
    pub low_delay_mode_flag: bool,
    pub initial_display_delay_present: bool,
    pub initial_display_delay_minus_1: u8,
}

impl OperatingPoint {
    fn bare(idc: u16, level_idx: u8, tier: u8) -> Self {
        Self {
            idc,
            level_idx,
            tier,
            decoder_model_present: false,
            decoder_buffer_delay: 0,
            encoder_buffer_delay: 0,
            low_delay_mode_flag: false,
            initial_display_delay_present: false,
            initial_display_delay_minus_1: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorConfig {
    pub high_bitdepth: bool,
    /// Only transmitted for `seq_profile == 2 && high_bitdepth`.
    pub twelve_bit: bool,
    pub bit_depth: u8,
    pub mono_chrome: bool,
    pub num_planes: usize,
    pub color_description_present_flag: bool,
    pub color_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
    pub color_range: bool,
    pub subsampling_x: bool,
    pub subsampling_y: bool,
    /// `CSP_UNKNOWN` when the syntax element is not transmitted.
    pub chroma_sample_position: u8,
    pub separate_uv_delta_q: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceHeader {
    pub seq_profile: u8,
    pub still_picture: bool,
    pub reduced_still_picture_header: bool,
    pub timing_info: Option<TimingInfo>,
    pub decoder_model_info: Option<DecoderModelInfo>,
    pub initial_display_delay_present_flag: bool,
    pub operating_points: Vec<OperatingPoint>,
    /// Index returned by `choose_operating_point()` (default 0).
    pub operating_point: usize,
    pub operating_point_idc: u16,
    pub frame_width_bits_minus_1: u8,
    pub frame_height_bits_minus_1: u8,
    pub max_frame_width: u32,
    pub max_frame_height: u32,
    pub frame_id_numbers_present_flag: bool,
    /// `delta_frame_id_length_minus_2 + 2`, zero when frame ids are absent.
    pub delta_frame_id_length: u8,
    /// `additional_frame_id_length_minus_1 + 1`, zero when absent.
    pub additional_frame_id_length: u8,
    pub use_128x128_superblock: bool,
    pub enable_filter_intra: bool,
    pub enable_intra_edge_filter: bool,
    pub enable_interintra_compound: bool,
    pub enable_masked_compound: bool,
    pub enable_warped_motion: bool,
    pub enable_dual_filter: bool,
    pub enable_order_hint: bool,
    pub enable_jnt_comp: bool,
    pub enable_ref_frame_mvs: bool,
    pub seq_force_screen_content_tools: u8,
    pub seq_force_integer_mv: u8,
    pub order_hint_bits: u8,
    pub enable_superres: bool,
    pub enable_cdef: bool,
    pub enable_restoration: bool,
    pub color: ColorConfig,
    pub film_grain_params_present: bool,
    /// Bit position after the last syntax element; feed to
    /// [`Obu::trailing_bits_needed`] when parsing a whole OBU.
    pub bits_consumed: usize,
}

fn read_timing_info(r: &mut BitReader<'_>) -> Result<TimingInfo, SeqHeaderError> {
    let num_units_in_display_tick = r.read_f(32)?;
    let time_scale = r.read_f(32)?;
    let equal_picture_interval = r.read_f(1)? != 0;
    let num_ticks_per_picture_minus_1 = if equal_picture_interval {
        r.read_uvlc()?
    } else {
        0
    };
    Ok(TimingInfo {
        num_units_in_display_tick,
        time_scale,
        equal_picture_interval,
        num_ticks_per_picture_minus_1,
    })
}

fn read_decoder_model_info(r: &mut BitReader<'_>) -> Result<DecoderModelInfo, SeqHeaderError> {
    Ok(DecoderModelInfo {
        buffer_delay_length_minus_1: r.read_f(5)? as u8,
        num_units_in_decoding_tick: r.read_f(32)?,
        buffer_removal_time_length_minus_1: r.read_f(5)? as u8,
        frame_presentation_time_length_minus_1: r.read_f(5)? as u8,
    })
}

fn read_color_config(
    r: &mut BitReader<'_>,
    seq_profile: u8,
) -> Result<ColorConfig, SeqHeaderError> {
    let high_bitdepth = r.read_f(1)? != 0;
    let twelve_bit = seq_profile == 2 && high_bitdepth && r.read_f(1)? != 0;
    // The specification only assigns BitDepth in the `seq_profile == 2` and
    // `seq_profile <= 2` branches; for profile 3 we follow libaom's effective
    // 8/10 rule. Profiles 0..2 are exactly the normative expression.
    let bit_depth = if seq_profile == 2 && high_bitdepth {
        if twelve_bit {
            12
        } else {
            10
        }
    } else if high_bitdepth {
        10
    } else {
        8
    };
    let mono_chrome = if seq_profile == 1 {
        false
    } else {
        r.read_f(1)? != 0
    };
    let num_planes = if mono_chrome { 1 } else { 3 };
    let color_description_present_flag = r.read_f(1)? != 0;
    let (color_primaries, transfer_characteristics, matrix_coefficients) =
        if color_description_present_flag {
            (r.read_f(8)? as u8, r.read_f(8)? as u8, r.read_f(8)? as u8)
        } else {
            (CP_UNSPECIFIED, TC_UNSPECIFIED, MC_UNSPECIFIED)
        };

    // Three normative branches: monochrome (returns early), the
    // BT.709/sRGB/identity shortcut, and the general case. Keeping them as a
    // tuple expression makes every value read in exactly one branch.
    let (color_range, subsampling_x, subsampling_y, chroma_sample_position, separate_uv_delta_q) =
        if mono_chrome {
            // Returns before separate_uv_delta_q; subsampling is forced 4:2:0.
            (r.read_f(1)? != 0, true, true, CSP_UNKNOWN, false)
        } else if color_primaries == CP_BT_709
            && transfer_characteristics == TC_SRGB
            && matrix_coefficients == MC_IDENTITY
        {
            // color_range and subsampling are implied, not transmitted.
            let sep = r.read_f(1)? != 0;
            (true, false, false, CSP_UNKNOWN, sep)
        } else {
            let color_range = r.read_f(1)? != 0;
            let (sx, sy) = match seq_profile {
                0 => (true, true),
                1 => (false, false),
                _ => {
                    if bit_depth == 12 {
                        let sx = r.read_f(1)? != 0;
                        let sy = if sx { r.read_f(1)? != 0 } else { false };
                        (sx, sy)
                    } else {
                        (true, false)
                    }
                }
            };
            let csp = if sx && sy {
                r.read_f(2)? as u8
            } else {
                CSP_UNKNOWN
            };
            let sep = r.read_f(1)? != 0;
            (color_range, sx, sy, csp, sep)
        };

    Ok(ColorConfig {
        high_bitdepth,
        twelve_bit,
        bit_depth,
        mono_chrome,
        num_planes,
        color_description_present_flag,
        color_primaries,
        transfer_characteristics,
        matrix_coefficients,
        color_range,
        subsampling_x,
        subsampling_y,
        chroma_sample_position,
        separate_uv_delta_q,
    })
}

/// Parse a sequence header from the raw OBU payload bits.
pub fn parse_sequence_header(
    payload: &[u8],
    operating_point: Option<usize>,
) -> Result<SequenceHeader, SeqHeaderError> {
    let mut r = BitReader::new(payload);
    let seq_profile = r.read_f(3)? as u8;
    let still_picture = r.read_f(1)? != 0;
    let reduced_still_picture_header = r.read_f(1)? != 0;

    let mut timing_info = None;
    let mut decoder_model_info = None;
    let mut initial_display_delay_present_flag = false;
    let mut operating_points: Vec<OperatingPoint> = Vec::new();

    if reduced_still_picture_header {
        // All model/timing/display-delay fields are fixed, and only
        // seq_level_idx[0] is transmitted.
        operating_points.push(OperatingPoint::bare(0, r.read_f(5)? as u8, 0));
    } else {
        let timing_info_present_flag = r.read_f(1)? != 0;
        let decoder_model_info_present_flag = if timing_info_present_flag {
            timing_info = Some(read_timing_info(&mut r)?);
            r.read_f(1)? != 0
        } else {
            false
        };
        if decoder_model_info_present_flag {
            decoder_model_info = Some(read_decoder_model_info(&mut r)?);
        }
        initial_display_delay_present_flag = r.read_f(1)? != 0;
        let operating_points_cnt_minus_1 = r.read_f(5)? as usize;
        let buffer_delay_bits = decoder_model_info
            .as_ref()
            .map(|m| (m.buffer_delay_length_minus_1 + 1) as usize)
            .unwrap_or(0);

        for _ in 0..=operating_points_cnt_minus_1 {
            let idc = r.read_f(12)? as u16;
            let level_idx = r.read_f(5)? as u8;
            let tier = if level_idx > 7 { r.read_f(1)? as u8 } else { 0 };
            let mut op = OperatingPoint::bare(idc, level_idx, tier);

            if decoder_model_info.is_some() {
                op.decoder_model_present = r.read_f(1)? != 0;
                if op.decoder_model_present {
                    op.decoder_buffer_delay = r.read_f(buffer_delay_bits)?;
                    op.encoder_buffer_delay = r.read_f(buffer_delay_bits)?;
                    op.low_delay_mode_flag = r.read_f(1)? != 0;
                }
            }
            if initial_display_delay_present_flag {
                op.initial_display_delay_present = r.read_f(1)? != 0;
                if op.initial_display_delay_present {
                    op.initial_display_delay_minus_1 = r.read_f(4)? as u8;
                }
            }
            operating_points.push(op);
        }
    }

    let chosen = operating_point.unwrap_or(0);
    if chosen >= operating_points.len() {
        return Err(SeqHeaderError::BadOperatingPoint {
            requested: chosen,
            available: operating_points.len(),
        });
    }
    let operating_point_idc = operating_points[chosen].idc;

    let frame_width_bits_minus_1 = r.read_f(4)? as u8;
    let frame_height_bits_minus_1 = r.read_f(4)? as u8;
    let max_frame_width = r.read_f(frame_width_bits_minus_1 as usize + 1)? + 1;
    let max_frame_height = r.read_f(frame_height_bits_minus_1 as usize + 1)? + 1;

    let frame_id_numbers_present_flag = if reduced_still_picture_header {
        false
    } else {
        r.read_f(1)? != 0
    };
    let (delta_frame_id_length, additional_frame_id_length) = if frame_id_numbers_present_flag {
        (r.read_f(4)? as u8 + 2, r.read_f(3)? as u8 + 1)
    } else {
        (0, 0)
    };

    let use_128x128_superblock = r.read_f(1)? != 0;
    let enable_filter_intra = r.read_f(1)? != 0;
    let enable_intra_edge_filter = r.read_f(1)? != 0;

    let (
        enable_interintra_compound,
        enable_masked_compound,
        enable_warped_motion,
        enable_dual_filter,
        enable_order_hint,
        enable_jnt_comp,
        enable_ref_frame_mvs,
        seq_force_screen_content_tools,
        seq_force_integer_mv,
        order_hint_bits,
    ) = if reduced_still_picture_header {
        (
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            SELECT_SCREEN_CONTENT_TOOLS,
            SELECT_INTEGER_MV,
            0,
        )
    } else {
        let interintra = r.read_f(1)? != 0;
        let masked = r.read_f(1)? != 0;
        let warped = r.read_f(1)? != 0;
        let dual = r.read_f(1)? != 0;
        let order_hint = r.read_f(1)? != 0;
        let (jnt, ref_mvs) = if order_hint {
            (r.read_f(1)? != 0, r.read_f(1)? != 0)
        } else {
            (false, false)
        };
        let screen = if r.read_f(1)? != 0 {
            SELECT_SCREEN_CONTENT_TOOLS
        } else {
            r.read_f(1)? as u8
        };
        let integer = if screen > 0 {
            if r.read_f(1)? != 0 {
                SELECT_INTEGER_MV
            } else {
                r.read_f(1)? as u8
            }
        } else {
            SELECT_INTEGER_MV
        };
        let order_hint_bits = if order_hint {
            r.read_f(3)? as u8 + 1
        } else {
            0
        };
        (
            interintra,
            masked,
            warped,
            dual,
            order_hint,
            jnt,
            ref_mvs,
            screen,
            integer,
            order_hint_bits,
        )
    };

    let enable_superres = r.read_f(1)? != 0;
    let enable_cdef = r.read_f(1)? != 0;
    let enable_restoration = r.read_f(1)? != 0;
    let color = read_color_config(&mut r, seq_profile)?;
    let film_grain_params_present = r.read_f(1)? != 0;
    let bits_consumed = r.position();

    Ok(SequenceHeader {
        seq_profile,
        still_picture,
        reduced_still_picture_header,
        timing_info,
        decoder_model_info,
        initial_display_delay_present_flag,
        operating_points,
        operating_point: chosen,
        operating_point_idc,
        frame_width_bits_minus_1,
        frame_height_bits_minus_1,
        max_frame_width,
        max_frame_height,
        frame_id_numbers_present_flag,
        delta_frame_id_length,
        additional_frame_id_length,
        use_128x128_superblock,
        enable_filter_intra,
        enable_intra_edge_filter,
        enable_interintra_compound,
        enable_masked_compound,
        enable_warped_motion,
        enable_dual_filter,
        enable_order_hint,
        enable_jnt_comp,
        enable_ref_frame_mvs,
        seq_force_screen_content_tools,
        seq_force_integer_mv,
        order_hint_bits,
        enable_superres,
        enable_cdef,
        enable_restoration,
        color,
        film_grain_params_present,
        bits_consumed,
    })
}

/// Parse a whole `OBU_SEQUENCE_HEADER`, including the `trailing_bits()`
/// conformance check from `open_bitstream_unit()`.
pub fn parse_sequence_header_obu(
    obu: &Obu<'_>,
    operating_point: Option<usize>,
) -> Result<SequenceHeader, SeqHeaderError> {
    if obu.header.obu_type != ObuType::SequenceHeader {
        return Err(SeqHeaderError::WrongObuType(obu.header.obu_type));
    }
    let header = parse_sequence_header(obu.payload, operating_point)?;
    let trailing = obu.trailing_bits_needed(header.bits_consumed)?;
    let mut reader = BitReader::new(obu.payload);
    reader.skip_bits(header.bits_consumed)?;
    reader.read_trailing_bits(trailing)?;
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::obu::{parse_ivf, parse_obus};

    /// MSB-first bit writer for constructing normative syntax vectors.
    struct BitWriter {
        bytes: Vec<u8>,
        bits: usize,
    }

    impl BitWriter {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                bits: 0,
            }
        }
        fn push(&mut self, value: u32, n: usize) {
            for i in (0..n).rev() {
                if self.bits % 8 == 0 {
                    self.bytes.push(0);
                }
                let bit = ((value >> i) & 1) as u8;
                let last = self.bytes.len() - 1;
                self.bytes[last] |= bit << (7 - (self.bits % 8));
                self.bits += 1;
            }
        }
        fn push_uvlc(&mut self, value: u32) {
            let leading_zeros = 31 - (value + 1).leading_zeros() as usize;
            let v = value + 1 - (1u32 << leading_zeros);
            self.push(0, leading_zeros);
            self.push(1, 1);
            self.push(v, leading_zeros);
        }
        fn trailing_bits(&mut self) {
            let pad = (8 - self.bits % 8) % 8;
            let count = if pad == 0 { 0 } else { pad };
            if count > 0 {
                self.push(1, 1);
                self.push(0, count - 1);
            }
        }
        fn finish(self) -> Vec<u8> {
            self.bytes
        }
    }

    /// §5.5 reduced_still_picture_header path: 48 bits, 6 bytes, no trailing
    /// bits. Profile 0, 64x64, 8-bit 4:2:0.
    fn reduced_vector() -> Vec<u8> {
        let mut w = BitWriter::new();
        w.push(0, 3); // seq_profile
        w.push(0, 1); // still_picture
        w.push(1, 1); // reduced_still_picture_header
        w.push(0, 5); // seq_level_idx[0]
        w.push(7, 4); // frame_width_bits_minus_1  -> n = 8
        w.push(7, 4); // frame_height_bits_minus_1 -> n = 8
        w.push(63, 8); // max_frame_width_minus_1  -> 64
        w.push(63, 8); // max_frame_height_minus_1 -> 64
        w.push(0, 1); // use_128x128_superblock
        w.push(0, 1); // enable_filter_intra
        w.push(0, 1); // enable_intra_edge_filter
        w.push(0, 1); // enable_superres
        w.push(1, 1); // enable_cdef
        w.push(1, 1); // enable_restoration
        w.push(0, 1); // high_bitdepth -> 8 bit
        w.push(0, 1); // mono_chrome
        w.push(0, 1); // color_description_present_flag
        w.push(0, 1); // color_range
        w.push(0, 2); // chroma_sample_position (4:2:0)
        w.push(0, 1); // separate_uv_delta_q
        w.push(0, 1); // film_grain_params_present
        assert_eq!(w.bits, 48);
        w.finish()
    }

    #[test]
    fn reduced_still_picture_header_vector() {
        let payload = reduced_vector();
        assert_eq!(payload.len(), 6);
        let sh = parse_sequence_header(&payload, None).unwrap();
        assert_eq!(sh.seq_profile, 0);
        assert!(sh.reduced_still_picture_header);
        assert_eq!(sh.bits_consumed, 48);
        assert_eq!(sh.operating_points.len(), 1);
        assert_eq!(sh.operating_point_idc, 0);
        assert_eq!(sh.max_frame_width, 64);
        assert_eq!(sh.max_frame_height, 64);
        assert!(!sh.frame_id_numbers_present_flag);
        assert!(!sh.enable_order_hint);
        assert_eq!(
            sh.seq_force_screen_content_tools,
            SELECT_SCREEN_CONTENT_TOOLS
        );
        assert_eq!(sh.seq_force_integer_mv, SELECT_INTEGER_MV);
        assert_eq!(sh.order_hint_bits, 0);
        assert!(sh.enable_cdef && sh.enable_restoration);
        assert_eq!(sh.color.bit_depth, 8);
        assert_eq!(sh.color.num_planes, 3);
        assert!(sh.color.subsampling_x && sh.color.subsampling_y);
        assert!(!sh.film_grain_params_present);
    }

    /// A non-reduced header exercising timing_info, decoder_model_info,
    /// two operating points, frame ids, order hint and profile 2 12-bit
    /// chroma subsampling.
    fn full_vector() -> Vec<u8> {
        let mut w = BitWriter::new();
        w.push(2, 3); // seq_profile = 2
        w.push(0, 1); // still_picture
        w.push(0, 1); // reduced_still_picture_header
        w.push(1, 1); // timing_info_present_flag
        w.push(1000, 32); // num_units_in_display_tick
        w.push(30000, 32); // time_scale
        w.push(1, 1); // equal_picture_interval
        w.push_uvlc(1); // num_ticks_per_picture_minus_1 = 1
        w.push(1, 1); // decoder_model_info_present_flag
        w.push(9, 5); // buffer_delay_length_minus_1 -> n = 10
        w.push(1000, 32); // num_units_in_decoding_tick
        w.push(10, 5); // buffer_removal_time_length_minus_1
        w.push(11, 5); // frame_presentation_time_length_minus_1
        w.push(1, 1); // initial_display_delay_present_flag
        w.push(1, 5); // operating_points_cnt_minus_1 -> 2 ops
                      // op 0
        w.push(0, 12); // operating_point_idc
        w.push(8, 5); // seq_level_idx > 7
        w.push(1, 1); // seq_tier
        w.push(1, 1); // decoder_model_present_for_this_op
        w.push(5, 10); // decoder_buffer_delay
        w.push(7, 10); // encoder_buffer_delay
        w.push(1, 1); // low_delay_mode_flag
        w.push(1, 1); // initial_display_delay_present_for_this_op
        w.push(3, 4); // initial_display_delay_minus_1
                      // op 1
        w.push(0x101, 12); // operating_point_idc
        w.push(3, 5); // seq_level_idx <= 7 (no tier bit)
        w.push(0, 1); // decoder_model_present_for_this_op
        w.push(0, 1); // initial_display_delay_present_for_this_op
                      // frame geometry
        w.push(9, 4); // frame_width_bits_minus_1  -> n = 10
        w.push(9, 4); // frame_height_bits_minus_1 -> n = 10
        w.push(319, 10); // max_frame_width_minus_1  -> 320
        w.push(239, 10); // max_frame_height_minus_1 -> 240
        w.push(1, 1); // frame_id_numbers_present_flag
        w.push(5, 4); // delta_frame_id_length_minus_2  -> 7
        w.push(2, 3); // additional_frame_id_length_minus_1 -> 3
        w.push(1, 1); // use_128x128_superblock
        w.push(0, 1); // enable_filter_intra
        w.push(1, 1); // enable_intra_edge_filter
        w.push(1, 1); // enable_interintra_compound
        w.push(0, 1); // enable_masked_compound
        w.push(1, 1); // enable_warped_motion
        w.push(0, 1); // enable_dual_filter
        w.push(1, 1); // enable_order_hint
        w.push(1, 1); // enable_jnt_comp
        w.push(0, 1); // enable_ref_frame_mvs
        w.push(0, 1); // seq_choose_screen_content_tools -> read explicit value
        w.push(1, 1); // seq_force_screen_content_tools = 1
        w.push(0, 1); // seq_choose_integer_mv -> read explicit value
        w.push(0, 1); // seq_force_integer_mv = 0
        w.push(4, 3); // order_hint_bits_minus_1 -> 5
        w.push(1, 1); // enable_superres
        w.push(0, 1); // enable_cdef
        w.push(1, 1); // enable_restoration
                      // color_config, profile 2 / 12-bit / explicit description
        w.push(1, 1); // high_bitdepth
        w.push(1, 1); // twelve_bit -> BitDepth 12
        w.push(0, 1); // mono_chrome
        w.push(1, 1); // color_description_present_flag
        w.push(9, 8); // color_primaries (BT.2020)
        w.push(16, 8); // transfer_characteristics (PQ)
        w.push(9, 8); // matrix_coefficients (BT.2020)
        w.push(1, 1); // color_range
        w.push(1, 1); // subsampling_x (only for 12-bit)
        w.push(0, 1); // subsampling_y
        w.push(1, 1); // separate_uv_delta_q
        w.push(1, 1); // film_grain_params_present
        w.trailing_bits();
        w.finish()
    }

    #[test]
    fn full_header_covers_optional_syntax_branches() {
        let payload = full_vector();
        let sh = parse_sequence_header(&payload, None).unwrap();
        assert_eq!(sh.seq_profile, 2);
        assert!(!sh.reduced_still_picture_header);
        let timing = sh.timing_info.unwrap();
        assert_eq!(timing.num_units_in_display_tick, 1000);
        assert_eq!(timing.time_scale, 30000);
        assert!(timing.equal_picture_interval);
        assert_eq!(timing.num_ticks_per_picture_minus_1, 1);
        let model = sh.decoder_model_info.unwrap();
        assert_eq!(model.buffer_delay_length_minus_1, 9);
        assert_eq!(model.num_units_in_decoding_tick, 1000);
        assert_eq!(model.frame_presentation_time_length_minus_1, 11);
        assert!(sh.initial_display_delay_present_flag);
        assert_eq!(sh.operating_points.len(), 2);
        let op0 = sh.operating_points[0];
        assert_eq!((op0.level_idx, op0.tier), (8, 1));
        assert!(op0.decoder_model_present && op0.low_delay_mode_flag);
        assert_eq!((op0.decoder_buffer_delay, op0.encoder_buffer_delay), (5, 7));
        assert_eq!(op0.initial_display_delay_minus_1, 3);
        let op1 = sh.operating_points[1];
        assert_eq!((op1.idc, op1.level_idx, op1.tier), (0x101, 3, 0));
        assert!(!op1.decoder_model_present);
        assert_eq!(sh.max_frame_width, 320);
        assert_eq!(sh.max_frame_height, 240);
        assert_eq!(sh.delta_frame_id_length, 7);
        assert_eq!(sh.additional_frame_id_length, 3);
        assert!(sh.use_128x128_superblock && sh.enable_intra_edge_filter);
        assert!(sh.enable_order_hint && sh.enable_jnt_comp && !sh.enable_ref_frame_mvs);
        assert_eq!(sh.seq_force_screen_content_tools, 1);
        assert_eq!(sh.seq_force_integer_mv, 0);
        assert_eq!(sh.order_hint_bits, 5);
        assert!(sh.enable_superres && sh.enable_restoration && !sh.enable_cdef);
        assert_eq!(sh.color.bit_depth, 12);
        assert!(sh.color.twelve_bit);
        assert_eq!(sh.color.color_primaries, 9);
        assert!(sh.color.subsampling_x && !sh.color.subsampling_y);
        assert_eq!(sh.color.chroma_sample_position, CSP_UNKNOWN);
        assert!(sh.film_grain_params_present);

        // Selecting the second operating point changes OperatingPointIdc.
        let alt = parse_sequence_header(&payload, Some(1)).unwrap();
        assert_eq!(alt.operating_point, 1);
        assert_eq!(alt.operating_point_idc, 0x101);
        assert_eq!(
            parse_sequence_header(&payload, Some(2)),
            Err(SeqHeaderError::BadOperatingPoint {
                requested: 2,
                available: 2
            })
        );
    }

    #[test]
    fn whole_obu_checks_trailing_bits() {
        let payload = full_vector();
        let mut obu_bytes = vec![0x0a];
        // Sequence header OBU with a size field; payload is 30-ish bytes so a
        // single-byte leb128 is enough.
        assert!(payload.len() < 128);
        obu_bytes.push(payload.len() as u8);
        obu_bytes.extend_from_slice(&payload);
        let obus = parse_obus(&obu_bytes).unwrap();
        assert_eq!(obus.len(), 1);
        let sh = parse_sequence_header_obu(&obus[0], None).unwrap();
        assert_eq!(sh.seq_profile, 2);
        // Corrupting the trailing one bit must be detected.
        let mut bad = obu_bytes.clone();
        let last = bad.len() - 1;
        bad[last] = 0x00;
        let bad_obus = parse_obus(&bad).unwrap();
        assert!(matches!(
            parse_sequence_header_obu(&bad_obus[0], None),
            Err(SeqHeaderError::Bitstream(
                BitstreamError::MissingTrailingOneBit
            ))
        ));
    }

    #[test]
    fn wrong_obu_type_and_truncated_payload() {
        let obus = parse_obus(&[0x12, 0x00]).unwrap(); // temporal delimiter
        assert_eq!(
            parse_sequence_header_obu(&obus[0], None),
            Err(SeqHeaderError::WrongObuType(ObuType::TemporalDelimiter))
        );
        assert!(matches!(
            parse_sequence_header(&[0x00], None),
            Err(SeqHeaderError::Bitstream(BitstreamError::OutOfBits { .. }))
        ));
    }

    /// Common prefix of a 64x64 reduced header (up to and including the
    /// three post-processing enable flags).
    fn reduced_prefix(profile: u32) -> BitWriter {
        let mut w = BitWriter::new();
        w.push(profile, 3);
        w.push(0, 1); // still_picture
        w.push(1, 1); // reduced_still_picture_header
        w.push(0, 5); // seq_level_idx[0]
        w.push(7, 4);
        w.push(7, 4);
        w.push(63, 8);
        w.push(63, 8);
        for _ in 0..6 {
            w.push(0, 1); // three block flags + superres/cdef/restoration
        }
        w
    }

    #[test]
    fn color_config_profile_one_is_444_and_skips_mono_chrome() {
        // Profile 1 forces mono_chrome = 0 (no bit) and 4:4:4 subsampling
        // (no subsampling or chroma_sample_position bits).
        let mut w = reduced_prefix(1);
        w.push(1, 1); // high_bitdepth -> BitDepth 10
        w.push(0, 1); // color_description_present_flag
        w.push(1, 1); // color_range
        w.push(1, 1); // separate_uv_delta_q
        w.push(0, 1); // film_grain_params_present
        w.trailing_bits();
        let sh = parse_sequence_header(&w.finish(), None).unwrap();
        assert_eq!(sh.seq_profile, 1);
        assert_eq!(sh.color.bit_depth, 10);
        assert!(!sh.color.mono_chrome);
        assert_eq!(sh.color.num_planes, 3);
        assert!(!sh.color.subsampling_x && !sh.color.subsampling_y);
        assert_eq!(sh.color.chroma_sample_position, CSP_UNKNOWN);
        assert!(sh.color.color_range && sh.color.separate_uv_delta_q);
    }

    #[test]
    fn color_config_monochrome_returns_before_separate_uv_delta_q() {
        // The monochrome branch reads only color_range, then returns: no
        // subsampling, no chroma_sample_position, no separate_uv_delta_q.
        let mut w = reduced_prefix(0);
        w.push(0, 1); // high_bitdepth -> BitDepth 8
        w.push(1, 1); // mono_chrome
        w.push(0, 1); // color_description_present_flag
        w.push(1, 1); // color_range
        w.push(0, 1); // film_grain_params_present
        w.trailing_bits();
        let sh = parse_sequence_header(&w.finish(), None).unwrap();
        assert_eq!(sh.color.num_planes, 1);
        assert_eq!(sh.color.bit_depth, 8);
        assert!(sh.color.subsampling_x && sh.color.subsampling_y);
        assert_eq!(sh.color.chroma_sample_position, CSP_UNKNOWN);
        assert!(!sh.color.separate_uv_delta_q);
        assert!(sh.color.color_range);
    }

    #[test]
    fn color_config_identity_matrix_shortcut() {
        // CP_BT_709 / TC_SRGB / MC_IDENTITY forces color_range = 1 and 4:4:4
        // without transmitting color_range or subsampling bits.
        let mut w = reduced_prefix(0);
        w.push(0, 1); // high_bitdepth
        w.push(0, 1); // mono_chrome
        w.push(1, 1); // color_description_present_flag
        w.push(CP_BT_709 as u32, 8);
        w.push(TC_SRGB as u32, 8);
        w.push(MC_IDENTITY as u32, 8);
        w.push(1, 1); // separate_uv_delta_q
        w.push(1, 1); // film_grain_params_present
        w.trailing_bits();
        let sh = parse_sequence_header(&w.finish(), None).unwrap();
        assert!(sh.color.color_range);
        assert!(!sh.color.subsampling_x && !sh.color.subsampling_y);
        assert!(sh.color.separate_uv_delta_q);
        assert!(sh.film_grain_params_present);
    }

    fn fixture(name: &str) -> &'static [u8] {
        match name {
            "64x64" => include_bytes!("../../../tests/data/av1_64x64_10.ivf"),
            "128x128" => include_bytes!("../../../tests/data/av1_128x128_10.ivf"),
            "320x240" => include_bytes!("../../../tests/data/av1_320x240_10.ivf"),
            "96x96" => include_bytes!("../../../tests/data/av1_96x96_10.ivf"),
            "192x192" => include_bytes!("../../../tests/data/av1_192x192_10.ivf"),
            _ => panic!("unknown fixture"),
        }
    }

    #[test]
    fn real_ivf_fixtures_match_container_metadata() {
        for (name, width, height) in [
            ("64x64", 64u32, 64u32),
            ("128x128", 128, 128),
            ("320x240", 320, 240),
            ("96x96", 96, 96),
            ("192x192", 192, 192),
        ] {
            let ivf = parse_ivf(fixture(name)).unwrap();
            assert_eq!(ivf.header.width as u32, width, "{name}");
            assert_eq!(ivf.header.height as u32, height, "{name}");
            assert_eq!(ivf.frames.len(), ivf.header.frame_count as usize, "{name}");

            let mut sequence_headers = 0;
            for frame in &ivf.frames {
                let obus = parse_obus(frame.payload).unwrap();
                assert!(!obus.is_empty(), "{name}: empty frame");
                // A key frame starts with a temporal delimiter.
                assert_eq!(
                    obus[0].header.obu_type,
                    ObuType::TemporalDelimiter,
                    "{name}"
                );
                for obu in &obus {
                    if obu.header.obu_type == ObuType::SequenceHeader {
                        let sh = parse_sequence_header_obu(obu, None).unwrap();
                        assert_eq!(sh.seq_profile, 0, "{name}");
                        assert_eq!(sh.max_frame_width, width, "{name}");
                        assert_eq!(sh.max_frame_height, height, "{name}");
                        assert_eq!(sh.color.bit_depth, 8, "{name}");
                        assert_eq!(sh.color.num_planes, 3, "{name}");
                        assert!(
                            sh.color.subsampling_x && sh.color.subsampling_y,
                            "{name}: expected 4:2:0"
                        );
                        assert_eq!(sh.operating_point_idc, 0, "{name}");
                        sequence_headers += 1;
                    }
                }
            }
            assert!(sequence_headers >= 1, "{name}: no sequence header");
        }
    }

    #[test]
    fn real_fixture_sequence_header_field_snapshot() {
        // Values independently derived from the bitstream with a separate
        // Python parser and cross-checked against ffprobe (Main / yuv420p).
        let ivf = parse_ivf(fixture("320x240")).unwrap();
        let obus = parse_obus(ivf.frames[0].payload).unwrap();
        assert_eq!(
            obus.iter().map(|o| o.header.obu_type).collect::<Vec<_>>(),
            vec![
                ObuType::TemporalDelimiter,
                ObuType::SequenceHeader,
                ObuType::Frame
            ]
        );
        let sh = parse_sequence_header_obu(&obus[1], None).unwrap();
        assert_eq!(sh.bits_consumed, 81);
        assert_eq!(sh.operating_points.len(), 1);
        assert_eq!(sh.operating_points[0].level_idx, 0);
        assert!(sh.timing_info.is_none());
        assert!(!sh.frame_id_numbers_present_flag);
        assert!(sh.enable_filter_intra && sh.enable_intra_edge_filter);
        assert!(sh.enable_order_hint);
        assert_eq!(sh.order_hint_bits, 7);
        assert_eq!(
            sh.seq_force_screen_content_tools,
            SELECT_SCREEN_CONTENT_TOOLS
        );
        assert_eq!(sh.seq_force_integer_mv, SELECT_INTEGER_MV);
        assert!(sh.enable_cdef && !sh.enable_restoration && !sh.enable_superres);
        assert_eq!(sh.color.chroma_sample_position, CSP_UNKNOWN);
        assert!(!sh.film_grain_params_present);
        // trailing_bits(88 - 81 = 7): the last payload byte is 0x40.
        assert_eq!(obus[1].size, 11);
        assert_eq!(obus[1].payload.last().copied(), Some(0x40));
    }
}
