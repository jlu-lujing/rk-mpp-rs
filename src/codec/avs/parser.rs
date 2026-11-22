//! AVS sequence and I-picture syntax used by the RK3588 AVS+ decoder.
//!
//! The client accepts both the base AVS `profile_id=0x20` syntax and the AVS+
//! `profile_id=0x48` extensions. This module starts with the fields the first
//! I picture needs: sequence state, picture state, access-unit splitting and
//! the exact byte containing the first slice start code. PB pictures remain a
//! later slice.
//!
//! Cross-checked against `mpp/codec/dec/avs/avsd_parse.c` and the public
//! FFmpeg sample `AVS/issue767/stream01.avs`.

use std::fmt;

pub const SLICE_MIN_START_CODE: u8 = 0x00;
pub const SLICE_MAX_START_CODE: u8 = 0xaf;
pub const VIDEO_SEQUENCE_START_CODE: u8 = 0xb0;
pub const VIDEO_SEQUENCE_END_CODE: u8 = 0xb1;
pub const USER_DATA_CODE: u8 = 0xb2;
pub const I_PICTURE_START_CODE: u8 = 0xb3;
pub const EXTENSION_START_CODE: u8 = 0xb5;
pub const PB_PICTURE_START_CODE: u8 = 0xb6;
pub const VIDEO_EDIT_CODE: u8 = 0xb7;

pub const I_PICTURE: u32 = 0;
pub const P_PICTURE: u32 = 1;
pub const B_PICTURE: u32 = 2;

pub const FRAME_PICTURE: u32 = 1;
pub const FIELD_PICTURE: u32 = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvsError {
    Truncated,
    Unsupported(&'static str),
    Invalid(&'static str),
}

impl fmt::Display for AvsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("AVS bitstream truncated"),
            Self::Unsupported(feature) => write!(f, "unsupported AVS feature: {feature}"),
            Self::Invalid(reason) => write!(f, "invalid AVS bitstream: {reason}"),
        }
    }
}

impl std::error::Error for AvsError {}

/// MSB-first bit reader. AVS start codes are byte aligned; the picture header
/// itself is not.
pub struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    pub fn bits(&mut self, count: u32) -> Result<u32, AvsError> {
        if count == 0 {
            return Ok(0);
        }
        if count > 32 || self.remaining_bits() < count as usize {
            return Err(AvsError::Truncated);
        }
        let mut value = 0u32;
        for _ in 0..count {
            let byte = self.data[self.position / 8];
            value = (value << 1) | u32::from((byte >> (7 - self.position % 8)) & 1);
            self.position += 1;
        }
        Ok(value)
    }

    pub fn flag(&mut self) -> Result<bool, AvsError> {
        Ok(self.bits(1)? != 0)
    }

    pub fn skip(&mut self, count: u32) -> Result<(), AvsError> {
        if self.remaining_bits() < count as usize {
            return Err(AvsError::Truncated);
        }
        self.position += count as usize;
        Ok(())
    }

    pub fn peek(&self, count: u32) -> Result<u32, AvsError> {
        let mut copy = Self {
            data: self.data,
            position: self.position,
        };
        copy.bits(count)
    }

    pub fn ue(&mut self) -> Result<u32, AvsError> {
        let mut zeros = 0u32;
        while !self.flag()? {
            zeros = zeros.checked_add(1).ok_or(AvsError::Truncated)?;
            if zeros > 31 {
                return Err(AvsError::Invalid("Exp-Golomb code is too long"));
            }
        }
        let suffix = if zeros == 0 { 0 } else { self.bits(zeros)? };
        ((1u32 << zeros) - 1)
            .checked_add(suffix)
            .ok_or(AvsError::Truncated)
    }

    pub fn se(&mut self) -> Result<i32, AvsError> {
        let code = self.ue()?;
        let magnitude = i64::from(code.div_ceil(2));
        let value = if code & 1 != 0 { magnitude } else { -magnitude };
        i32::try_from(value).map_err(|_| AvsError::Invalid("signed Exp-Golomb overflow"))
    }

    pub fn bit_pos(&self) -> usize {
        self.position
    }

    fn remaining_bits(&self) -> usize {
        self.data
            .len()
            .saturating_mul(8)
            .saturating_sub(self.position)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequenceHeader {
    pub profile_id: u8,
    pub level_id: u8,
    pub progressive_sequence: bool,
    pub horizontal_size: u32,
    pub vertical_size: u32,
    pub chroma_format: u32,
    pub sample_precision: u32,
    pub aspect_ratio: u32,
    pub frame_rate_code: u32,
    pub bit_rate: u32,
    pub low_delay: bool,
    pub bbv_buffer_size: u32,
}

impl SequenceHeader {
    pub fn parse(data: &[u8]) -> Result<Self, AvsError> {
        let mut reader = Reader::new(data);
        let profile_id = reader.bits(8)? as u8;
        if profile_id != 0x20 && profile_id != 0x48 {
            return Err(AvsError::Unsupported("sequence profile"));
        }
        let level_id = reader.bits(8)? as u8;
        if level_id > 0x42 {
            return Err(AvsError::Unsupported("sequence level"));
        }
        let progressive_sequence = reader.flag()?;
        let horizontal_size = reader.bits(14)?;
        let vertical_size = reader.bits(14)?;
        let chroma_format = reader.bits(2)?;
        let sample_precision = reader.bits(3)?;
        let aspect_ratio = reader.bits(4)?;
        let frame_rate_code = reader.bits(4)?;
        let bit_rate_high = reader.bits(18)?;
        reader.skip(1)?;
        let bit_rate_low = reader.bits(12)?;
        let low_delay = reader.flag()?;
        reader.skip(1)?;
        let bbv_buffer_size = reader.bits(18)?;
        reader.skip(3)?;
        Ok(Self {
            profile_id,
            level_id,
            progressive_sequence,
            horizontal_size,
            vertical_size,
            chroma_format,
            sample_precision,
            aspect_ratio,
            frame_rate_code,
            bit_rate: (bit_rate_high << 12) | bit_rate_low,
            low_delay,
            bbv_buffer_size,
        })
    }

    pub fn is_avs_plus(self) -> bool {
        self.profile_id == 0x48
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureHeader {
    pub picture_coding_type: u32,
    pub bbv_delay: u32,
    pub bbv_delay_extension: u32,
    pub time_code_flag: bool,
    pub time_code: u32,
    pub picture_distance: u32,
    pub bbv_check_times: u32,
    pub progressive_frame: bool,
    pub picture_structure: u32,
    pub top_field_first: bool,
    pub repeat_first_field: bool,
    pub fixed_picture_qp: bool,
    pub picture_qp: u32,
    pub picture_reference_flag: bool,
    pub skip_mode_flag: bool,
    pub no_forward_reference_flag: bool,
    pub pb_field_enhanced_flag: bool,
    pub loop_filter_disable: bool,
    pub loop_filter_parameter_flag: bool,
    pub alpha_offset: i32,
    pub beta_offset: i32,
    pub weighting_quant_flag: bool,
    pub chroma_quant_param_disable: bool,
    pub chroma_quant_param_delta_cb: i32,
    pub chroma_quant_param_delta_cr: i32,
    pub weighting_quant_param_index: u32,
    pub weighting_quant_model: u32,
    pub weighting_quant_param: [u32; 6],
    pub aec_enable: bool,
    /// Bits consumed from the start of the picture payload.
    pub header_bits: usize,
}

fn weight_params(flag: bool, index: u32, delta1: &[i32; 6], delta2: &[i32; 6]) -> [u32; 6] {
    if !flag {
        return [128; 6];
    }
    const DEFAULT: [i32; 6] = [128, 98, 106, 116, 116, 128];
    const BASE1: [i32; 6] = [135, 143, 143, 160, 160, 213];
    const BASE2: [i32; 6] = [128, 98, 106, 116, 116, 128];
    let (base, delta) = match index {
        0 => (&DEFAULT, &[0; 6]),
        1 => (&BASE1, delta1),
        2 => (&BASE2, delta2),
        _ => (&DEFAULT, &[0; 6]),
    };
    std::array::from_fn(|index| (base[index] + delta[index]).max(0) as u32)
}

fn parse_extensions(
    reader: &mut Reader<'_>,
    sequence: &SequenceHeader,
    picture: &mut PictureHeader,
) -> Result<(), AvsError> {
    picture.loop_filter_disable = reader.flag()?;
    if !picture.loop_filter_disable {
        picture.loop_filter_parameter_flag = reader.flag()?;
        if picture.loop_filter_parameter_flag {
            picture.alpha_offset = reader.se()?;
            picture.beta_offset = reader.se()?;
        }
    }

    if sequence.is_avs_plus() {
        picture.weighting_quant_flag = reader.flag()?;
        if picture.weighting_quant_flag {
            reader.skip(1)?;
            picture.chroma_quant_param_disable = reader.flag()?;
            if !picture.chroma_quant_param_disable {
                picture.chroma_quant_param_delta_cb = reader.se()?;
                picture.chroma_quant_param_delta_cr = reader.se()?;
            }
            picture.weighting_quant_param_index = reader.bits(2)?;
            picture.weighting_quant_model = reader.bits(2)?;
            match picture.weighting_quant_param_index {
                1 => {
                    let mut deltas = [0; 6];
                    for delta in &mut deltas {
                        *delta = reader.se()?;
                    }
                    picture.weighting_quant_param =
                        weight_params(true, picture.weighting_quant_param_index, &deltas, &[0; 6]);
                }
                2 => {
                    let mut deltas = [0; 6];
                    for delta in &mut deltas {
                        *delta = reader.se()?;
                    }
                    picture.weighting_quant_param =
                        weight_params(true, picture.weighting_quant_param_index, &[0; 6], &deltas);
                }
                _ => {
                    picture.weighting_quant_param =
                        weight_params(true, picture.weighting_quant_param_index, &[0; 6], &[0; 6]);
                }
            }
        } else {
            picture.weighting_quant_param = weight_params(false, 0, &[0; 6], &[0; 6]);
        }
        picture.aec_enable = reader.flag()?;
    }
    Ok(())
}

impl PictureHeader {
    /// Parse the I-picture header that follows `00 00 01 B3`.
    ///
    /// `version_checked` mirrors the C parser's per-sequence latch. The first
    /// I picture peeks at the next 9/11 bits to choose whether the optional
    /// marker bit is present; later I pictures reuse that choice.
    pub fn parse_i(
        data: &[u8],
        sequence: &SequenceHeader,
        version_checked: bool,
    ) -> Result<(Self, bool), AvsError> {
        let mut reader = Reader::new(data);
        let mut picture = PictureHeader {
            picture_coding_type: I_PICTURE,
            bbv_delay: reader.bits(16)?,
            bbv_delay_extension: 0,
            time_code_flag: false,
            time_code: 0,
            picture_distance: 0,
            bbv_check_times: 0,
            progressive_frame: false,
            picture_structure: FRAME_PICTURE,
            top_field_first: false,
            repeat_first_field: false,
            fixed_picture_qp: false,
            picture_qp: 0,
            picture_reference_flag: false,
            skip_mode_flag: false,
            no_forward_reference_flag: false,
            pb_field_enhanced_flag: false,
            loop_filter_disable: false,
            loop_filter_parameter_flag: false,
            alpha_offset: 0,
            beta_offset: 0,
            weighting_quant_flag: false,
            chroma_quant_param_disable: false,
            chroma_quant_param_delta_cb: 0,
            chroma_quant_param_delta_cr: 0,
            weighting_quant_param_index: 0,
            weighting_quant_model: 0,
            weighting_quant_param: [128; 6],
            aec_enable: false,
            header_bits: 0,
        };

        if sequence.is_avs_plus() {
            reader.skip(1)?;
            picture.bbv_delay_extension = reader.bits(7)?;
        }
        picture.time_code_flag = reader.flag()?;
        if picture.time_code_flag {
            picture.time_code = reader.bits(24)?;
        }

        let detected_version = if version_checked {
            false
        } else if sequence.low_delay {
            true
        } else if reader.peek(9)? & 1 == 0 {
            true
        } else {
            reader.peek(11)? & 3 != 0
        };
        if detected_version {
            reader.skip(1)?;
        }

        picture.picture_distance = reader.bits(8)?;
        if sequence.low_delay {
            picture.bbv_check_times = reader.ue()?;
        }
        picture.progressive_frame = reader.flag()?;
        picture.picture_structure = FRAME_PICTURE;
        if !picture.progressive_frame {
            picture.picture_structure = reader.flag()? as u32;
        }
        picture.top_field_first = reader.flag()?;
        picture.repeat_first_field = reader.flag()?;
        picture.fixed_picture_qp = reader.flag()?;
        picture.picture_qp = reader.bits(6)?;
        if !picture.progressive_frame && picture.picture_structure == FIELD_PICTURE {
            picture.skip_mode_flag = reader.flag()?;
        }
        reader.skip(4)?;
        parse_extensions(&mut reader, sequence, &mut picture)?;
        picture.header_bits = reader.bit_pos();
        Ok((picture, detected_version || version_checked))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unit {
    /// Byte index of the first zero in `00 00 01`.
    pub start: usize,
    /// Byte immediately after the start-code byte.
    pub payload: usize,
    pub code: u8,
}

pub fn start_codes(data: &[u8]) -> Vec<Unit> {
    let mut units = Vec::new();
    let mut index = 0usize;
    while index + 3 < data.len() {
        if data[index] == 0 && data[index + 1] == 0 && data[index + 2] == 1 {
            units.push(Unit {
                start: index,
                payload: index + 4,
                code: data[index + 3],
            });
            index += 4;
        } else {
            index += 1;
        }
    }
    units
}

/// Split elementary stream data at the same boundaries as
/// `avsd_parser_split`: after a picture has started, the next non-slice start
/// code other than user data begins the following access unit.
pub fn split_access_units(data: &[u8]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut current_start = 0usize;
    let mut picture_found = false;
    for unit in start_codes(data) {
        if !picture_found {
            picture_found = matches!(unit.code, I_PICTURE_START_CODE | PB_PICTURE_START_CODE);
            continue;
        }
        if unit.code > SLICE_MAX_START_CODE && unit.code != USER_DATA_CODE {
            if picture_found {
                ranges.push((current_start, unit.start));
            }
            current_start = unit.start;
            picture_found = matches!(unit.code, I_PICTURE_START_CODE | PB_PICTURE_START_CODE);
        }
    }
    if picture_found && current_start < data.len() {
        ranges.push((current_start, data.len()));
    }
    ranges
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessUnit<'a> {
    pub sequence: SequenceHeader,
    pub picture: PictureHeader,
    /// Byte of the first slice's `00 00 01`, relative to this access unit.
    pub slice_start: usize,
    /// Bytes between the I-picture start code and the picture payload.
    pub picture_payload: &'a [u8],
}

impl<'a> AccessUnit<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, AvsError> {
        let units = start_codes(data);
        let sequence = units
            .iter()
            .find(|unit| unit.code == VIDEO_SEQUENCE_START_CODE)
            .ok_or(AvsError::Invalid("access unit has no sequence header"))
            .and_then(|unit| SequenceHeader::parse(&data[unit.payload..]))?;
        let picture_unit = units
            .iter()
            .find(|unit| unit.code == I_PICTURE_START_CODE)
            .ok_or(AvsError::Unsupported("PB picture header"))?;
        let picture_payload = &data[picture_unit.payload..];
        let (picture, _) = PictureHeader::parse_i(picture_payload, &sequence, false)?;
        let slice_start = units
            .iter()
            .skip_while(|unit| unit.start <= picture_unit.start)
            .find(|unit| unit.code >= SLICE_MIN_START_CODE && unit.code <= SLICE_MAX_START_CODE)
            .map(|unit| unit.start)
            .ok_or(AvsError::Invalid("I picture has no slice"))?;
        Ok(Self {
            sequence,
            picture,
            slice_start,
            picture_payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// First 135 bytes of FFmpeg's public AVS `issue767/stream01.avs`: sequence
    /// header, three extension headers, user data, the complete I-picture
    /// header and the first slice start code.
    const PREFIX: &[u8] = &[
        0x80, 0x00, 0x00, 0x01, 0xb0, 0x20, 0x10, 0x82, 0xc0, 0x09, 0x02, 0x44, 0xc0, 0x9c, 0x48,
        0x00, 0x20, 0x00, 0x38, 0x80, 0x00, 0x00, 0x01, 0xb5, 0x23, 0x0f, 0x01, 0x14, 0x00, 0x80,
        0x00, 0x00, 0x01, 0xb5, 0x40, 0x00, 0x04, 0x00, 0x00, 0x20, 0x00, 0x00, 0x40, 0x00, 0x00,
        0x80, 0x00, 0x00, 0x01, 0xb5, 0xb0, 0x08, 0x00, 0x00, 0x10, 0x00, 0x00, 0x20, 0x00, 0x00,
        0x40, 0x00, 0x00, 0x80, 0x00, 0x40, 0x00, 0x20, 0x00, 0x10, 0x00, 0x08, 0x00, 0x04, 0x00,
        0x02, 0x00, 0x00, 0x04, 0x00, 0x00, 0x08, 0x00, 0x00, 0x10, 0x00, 0x00, 0x20, 0x00, 0x00,
        0x40, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x01, 0xb2, 0x41, 0x56, 0x53,
        0x20, 0x74, 0x65, 0x73, 0x74, 0x20, 0x73, 0x74, 0x72, 0x65, 0x61, 0x6d, 0x80, 0x00, 0x00,
        0x01, 0xb3, 0xff, 0xff, 0x80, 0x00, 0x00, 0x40, 0x26, 0x80, 0x20, 0x00, 0x00, 0x01, 0x00,
    ];

    #[test]
    fn public_avs_i_header_matches_vendor_fields() {
        let access = AccessUnit::parse(PREFIX).expect("parse public AVS prefix");
        assert_eq!(access.sequence.profile_id, 0x20);
        assert_eq!(access.sequence.level_id, 0x10);
        assert!(access.sequence.progressive_sequence);
        assert_eq!(
            (
                access.sequence.horizontal_size,
                access.sequence.vertical_size
            ),
            (352, 288)
        );
        assert_eq!(access.sequence.chroma_format, 1);
        assert_eq!(access.sequence.sample_precision, 1);
        assert_eq!(access.sequence.frame_rate_code, 3);
        assert!(!access.sequence.low_delay);
        assert_eq!(access.sequence.bbv_buffer_size, 7);

        let picture = access.picture;
        assert_eq!(picture.picture_coding_type, I_PICTURE);
        assert_eq!(picture.bbv_delay, 0xffff);
        assert!(picture.time_code_flag);
        assert_eq!(picture.time_code, 0);
        assert_eq!(picture.picture_distance, 0);
        assert!(picture.progressive_frame);
        assert!(picture.fixed_picture_qp);
        assert_eq!(picture.picture_qp, 40);
        assert!(!picture.loop_filter_disable);
        assert!(!picture.loop_filter_parameter_flag);
        assert_eq!(picture.header_bits, 66);
        assert_eq!(access.slice_start, 0x83);
    }

    #[test]
    fn start_code_split_keeps_headers_with_their_picture() {
        assert_eq!(
            split_access_units(PREFIX),
            vec![(0, PREFIX.len())],
            "an incomplete final picture remains one access unit"
        );
        let units = start_codes(PREFIX);
        assert_eq!(units[0].code, VIDEO_SEQUENCE_START_CODE);
        assert_eq!(units.last().unwrap().code, SLICE_MIN_START_CODE);
    }
}
