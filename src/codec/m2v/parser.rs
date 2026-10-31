//! MPEG-2 (ISO/IEC 13818-2) start-code splitting and the sequence and picture
//! headers the vdpu2 register file is built from.
//!
//! MPEG-2 has no emulation-prevention bytes, so a unit runs from its start
//! code to the next one. Every MPEG-2 extension shares the `0x000001B5` code
//! and is named by the four-bit identifier that follows it, which is what makes
//! the extensions unambiguous without tracking parser state.
//!
//! Reference: ISO/IEC 13818-2 §6.2 (start codes), §6.3.3 (sequence
//! extension), §6.3.10 (picture coding extension), §6.3.14 (quant matrix
//! extension). Cross-checked against the FFmpeg encoder bit order and
//! `mpp/codec/dec/m2v/m2vd_parser.c`.

use std::fmt;

pub const CODE_PICTURE: u8 = 0x00;
pub const CODE_SLICE_MIN: u8 = 0x01;
pub const CODE_SLICE_MAX: u8 = 0xAF;
pub const CODE_USER_DATA: u8 = 0xB2;
pub const CODE_SEQUENCE: u8 = 0xB3;
pub const CODE_EXTENSION: u8 = 0xB5;
pub const CODE_EXTENSION_ALT: u8 = 0xB7;
pub const CODE_GOP: u8 = 0xB8;

/// `extension_start_code_identifier` values (13818-2 Table 6.3).
pub const EXT_SEQUENCE: u32 = 1;
pub const EXT_SEQUENCE_DISPLAY: u32 = 2;
pub const EXT_QUANT_MATRIX: u32 = 3;
pub const EXT_COPYRIGHT: u32 = 4;
pub const EXT_SEQUENCE_SCALABLE: u32 = 5;
pub const EXT_PICTURE_DISPLAY: u32 = 7;
pub const EXT_PICTURE_CODING: u32 = 8;

pub const PIC_I: u32 = 1;
pub const PIC_P: u32 = 2;
pub const PIC_B: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum M2vError {
    Truncated,
    Unsupported(&'static str),
}

impl fmt::Display for M2vError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            M2vError::Truncated => write!(f, "bitstream truncated"),
            M2vError::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

impl std::error::Error for M2vError {}

/// MSB-first bit reader; MPEG-2 needs no de-escaping step.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    fn bit(&mut self) -> Result<u32, M2vError> {
        if self.pos >= self.data.len() * 8 {
            return Err(M2vError::Truncated);
        }
        let b = (self.data[self.pos / 8] >> (7 - (self.pos % 8))) & 1;
        self.pos += 1;
        Ok(u32::from(b))
    }

    pub fn bits(&mut self, n: u32) -> Result<u32, M2vError> {
        if n == 0 {
            return Ok(0);
        }
        if n > 32 || self.data.len() * 8 - self.pos < n as usize {
            return Err(M2vError::Truncated);
        }
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Ok(v)
    }

    pub fn flag(&mut self) -> Result<bool, M2vError> {
        Ok(self.bits(1)? == 1)
    }

    pub fn skip(&mut self, n: u32) -> Result<(), M2vError> {
        if self.data.len() * 8 - self.pos < n as usize {
            return Err(M2vError::Truncated);
        }
        self.pos += n as usize;
        Ok(())
    }

    /// `marker_bit` is fixed at one; streams that lose it are corrupt.
    pub fn marker(&mut self) -> Result<(), M2vError> {
        match self.bits(1)? {
            1 => Ok(()),
            _ => Err(M2vError::Truncated),
        }
    }

    pub fn bit_pos(&self) -> usize {
        self.pos
    }
}

fn read_matrix(r: &mut Reader<'_>) -> Result<[u8; 64], M2vError> {
    let mut m = [0u8; 64];
    for byte in m.iter_mut() {
        *byte = r.bits(8)? as u8;
    }
    Ok(m)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceHeader {
    pub horizontal_size_value: u32,
    pub vertical_size_value: u32,
    pub aspect_ratio_information: u32,
    pub frame_rate_code: u32,
    pub bit_rate_value: u32,
    pub vbv_buffer_size_value: u32,
    pub constrained_parameters_flag: bool,
    pub load_intra_quant_matrix: bool,
    pub intra_quant_matrix: [u8; 64],
    pub load_non_intra_quant_matrix: bool,
    pub non_intra_quant_matrix: [u8; 64],
}

impl SequenceHeader {
    pub fn parse(data: &[u8]) -> Result<Self, M2vError> {
        let mut r = Reader::new(data);
        let horizontal_size_value = r.bits(12)?;
        let vertical_size_value = r.bits(12)?;
        if horizontal_size_value == 0 || vertical_size_value == 0 {
            // A zero low field means the extension carries the high bits, which
            // only an MPEG-2 sequence extension can supply.
            if r.bit_pos() > data.len() * 8 {
                return Err(M2vError::Truncated);
            }
        }
        let aspect_ratio_information = r.bits(4)?;
        let frame_rate_code = r.bits(4)?;
        let bit_rate_value = r.bits(18)?;
        r.marker()?;
        let vbv_buffer_size_value = r.bits(10)?;
        // MPEG-1 reads a one_bit here; ISO/IEC 13818-2 removed it, and the C
        // parser follows MPEG-2, so the next bit is already the flag.
        let constrained_parameters_flag = r.flag()?;
        let load_intra_quant_matrix = r.flag()?;
        let intra_quant_matrix = if load_intra_quant_matrix {
            read_matrix(&mut r)?
        } else {
            [0; 64]
        };
        let load_non_intra_quant_matrix = r.flag()?;
        let non_intra_quant_matrix = if load_non_intra_quant_matrix {
            read_matrix(&mut r)?
        } else {
            [0; 64]
        };
        Ok(SequenceHeader {
            horizontal_size_value,
            vertical_size_value,
            aspect_ratio_information,
            frame_rate_code,
            bit_rate_value,
            vbv_buffer_size_value,
            constrained_parameters_flag,
            load_intra_quant_matrix,
            intra_quant_matrix,
            load_non_intra_quant_matrix,
            non_intra_quant_matrix,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequenceExtension {
    /// `profile_and_level_indication`: escape bit, 3-bit profile, 4-bit level.
    pub profile_and_level_indication: u32,
    pub progressive_sequence: bool,
    /// 1 = 4:2:0. The rkvdec/vdpu2 path takes nothing else.
    pub chroma_format: u32,
    pub horizontal_size_extension: u32,
    pub vertical_size_extension: u32,
    pub bit_rate_extension: u32,
    pub vbv_buffer_size_extension: u32,
    pub low_delay: bool,
    pub frame_rate_extension_n: u32,
    pub frame_rate_extension_d: u32,
}

impl SequenceExtension {
    /// Continue an existing bit stream; the caller has read the identifier.
    pub fn read(r: &mut Reader<'_>) -> Result<Self, M2vError> {
        let profile_and_level_indication = r.bits(8)?;
        let progressive_sequence = r.flag()?;
        let chroma_format = r.bits(2)?;
        let horizontal_size_extension = r.bits(2)?;
        let vertical_size_extension = r.bits(2)?;
        let bit_rate_extension = r.bits(12)?;
        r.marker()?;
        let vbv_buffer_size_extension = r.bits(8)?;
        let low_delay = r.flag()?;
        let frame_rate_extension_n = r.bits(2)?;
        let frame_rate_extension_d = r.bits(5)?;
        Ok(SequenceExtension {
            profile_and_level_indication,
            progressive_sequence,
            chroma_format,
            horizontal_size_extension,
            vertical_size_extension,
            bit_rate_extension,
            vbv_buffer_size_extension,
            low_delay,
            frame_rate_extension_n,
            frame_rate_extension_d,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequenceDisplayExtension {
    /// `video_signal_type`; the C HAL calls this field `video_format`.
    pub video_signal_type: u32,
    pub colour_description: bool,
    pub colour_primaries: u32,
    pub transfer_characteristics: u32,
    pub matrix_coefficients: u32,
    pub display_horizontal_size: u32,
    pub display_vertical_size: u32,
}

impl SequenceDisplayExtension {
    pub fn read(r: &mut Reader<'_>) -> Result<Self, M2vError> {
        let video_signal_type = r.bits(3)?;
        let colour_description = r.flag()?;
        let mut colour_primaries = 0;
        let mut transfer_characteristics = 0;
        let mut matrix_coefficients = 0;
        if colour_description {
            colour_primaries = r.bits(8)?;
            transfer_characteristics = r.bits(8)?;
            matrix_coefficients = r.bits(8)?;
        }
        let display_horizontal_size = r.bits(14)?;
        r.marker()?;
        let display_vertical_size = r.bits(14)?;
        Ok(SequenceDisplayExtension {
            video_signal_type,
            colour_description,
            colour_primaries,
            transfer_characteristics,
            matrix_coefficients,
            display_horizontal_size,
            display_vertical_size,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureHeader {
    pub temporal_reference: u32,
    pub picture_coding_type: u32,
    pub vbv_delay: u32,
    /// `full_pel_forward_vector` and `forward_f_code`, present for P and B
    /// pictures only. MPEG-2 moves the real motion vector range into the
    /// picture coding extension, so these are the MPEG-1 form of the same
    /// information and the hardware path uses them only when the stream has no
    /// sequence extension at all.
    pub full_pel_forward_vector: bool,
    pub forward_f_code: u32,
    pub full_pel_backward_vector: bool,
    pub backward_f_code: u32,
}

impl PictureHeader {
    pub fn parse(data: &[u8]) -> Result<Self, M2vError> {
        let mut r = Reader::new(data);
        let temporal_reference = r.bits(10)?;
        let picture_coding_type = r.bits(3)?;
        let vbv_delay = r.bits(16)?;
        let mut full_pel_forward_vector = false;
        let mut forward_f_code = 0;
        let mut full_pel_backward_vector = false;
        let mut backward_f_code = 0;
        if matches!(picture_coding_type, PIC_P | PIC_B) {
            full_pel_forward_vector = r.flag()?;
            forward_f_code = r.bits(3)?;
        }
        if picture_coding_type == PIC_B {
            full_pel_backward_vector = r.flag()?;
            backward_f_code = r.bits(3)?;
        }
        Ok(PictureHeader {
            temporal_reference,
            picture_coding_type,
            vbv_delay,
            full_pel_forward_vector,
            forward_f_code,
            full_pel_backward_vector,
            backward_f_code,
        })
    }

    /// The picture header f_code as `(full_pel, f_code)` per direction, which
    /// is the shape the register file wants.
    pub fn f_code(&self) -> [[u32; 2]; 2] {
        [
            [u32::from(self.full_pel_forward_vector), self.forward_f_code],
            [
                u32::from(self.full_pel_backward_vector),
                self.backward_f_code,
            ],
        ]
    }

    /// One letter for the coding type, as the test expectations read.
    pub fn coding_letter(&self) -> char {
        match self.picture_coding_type {
            PIC_I => 'I',
            PIC_P => 'P',
            PIC_B => 'B',
            _ => '?',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureCodingExtension {
    /// `[forward, backward][horizontal, vertical]`, 4 bits each.
    pub f_code: [[u32; 2]; 2],
    pub intra_dc_precision: u32,
    /// 1 = top field, 2 = bottom field, 3 = frame.
    pub picture_structure: u32,
    pub top_field_first: bool,
    pub frame_pred_frame_dct: bool,
    pub concealment_motion_vectors: bool,
    pub q_scale_type: bool,
    pub intra_vlc_format: bool,
    pub alternate_scan: bool,
    pub repeat_first_field: bool,
    pub chroma_420_type: bool,
    pub progressive_frame: bool,
    pub composite_display_flag: bool,
    pub v_axis: u32,
    pub field_sequence: u32,
    pub sub_carrier: bool,
    pub burst_amplitude: u32,
    pub sub_carrier_phase: u32,
}

impl PictureCodingExtension {
    pub fn read(r: &mut Reader<'_>) -> Result<Self, M2vError> {
        let f_code = [[r.bits(4)?, r.bits(4)?], [r.bits(4)?, r.bits(4)?]];
        let intra_dc_precision = r.bits(2)?;
        let picture_structure = r.bits(2)?;
        let top_field_first = r.flag()?;
        let frame_pred_frame_dct = r.flag()?;
        let concealment_motion_vectors = r.flag()?;
        let q_scale_type = r.flag()?;
        let intra_vlc_format = r.flag()?;
        let alternate_scan = r.flag()?;
        let repeat_first_field = r.flag()?;
        let chroma_420_type = r.flag()?;
        let progressive_frame = r.flag()?;
        let composite_display_flag = r.flag()?;
        let mut v_axis = 0;
        let mut field_sequence = 0;
        let mut sub_carrier = false;
        let mut burst_amplitude = 0;
        let mut sub_carrier_phase = 0;
        if composite_display_flag {
            v_axis = r.bits(1)?;
            field_sequence = r.bits(3)?;
            sub_carrier = r.flag()?;
            burst_amplitude = r.bits(7)?;
            sub_carrier_phase = r.bits(8)?;
        }
        Ok(PictureCodingExtension {
            f_code,
            intra_dc_precision,
            picture_structure,
            top_field_first,
            frame_pred_frame_dct,
            concealment_motion_vectors,
            q_scale_type,
            intra_vlc_format,
            alternate_scan,
            repeat_first_field,
            chroma_420_type,
            progressive_frame,
            composite_display_flag,
            v_axis,
            field_sequence,
            sub_carrier,
            burst_amplitude,
            sub_carrier_phase,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuantMatrixExtension {
    pub load_intra_quant_matrix: bool,
    pub intra_quant_matrix: [u8; 64],
    pub load_non_intra_quant_matrix: bool,
    pub non_intra_quant_matrix: [u8; 64],
    pub load_intra_chroma_quant_matrix: bool,
    pub intra_chroma_quant_matrix: [u8; 64],
    pub load_non_intra_chroma_quant_matrix: bool,
    pub non_intra_chroma_quant_matrix: [u8; 64],
}

impl QuantMatrixExtension {
    pub fn read(mut r: &mut Reader<'_>) -> Result<Self, M2vError> {
        let parse_one = |r: &mut Reader<'_>| -> Result<(bool, [u8; 64]), M2vError> {
            let load = r.flag()?;
            let matrix = if load { read_matrix(r)? } else { [0; 64] };
            Ok((load, matrix))
        };
        let (load_intra_quant_matrix, intra_quant_matrix) = parse_one(&mut r)?;
        let (load_non_intra_quant_matrix, non_intra_quant_matrix) = parse_one(&mut r)?;
        // The hardware path has no chroma matrix table; the C HAL refuses the
        // stream as soon as one is loaded, so we do the same.
        let chroma_intra_loaded = r.flag()?;
        if chroma_intra_loaded {
            return Err(M2vError::Unsupported("chroma quantisation matrices"));
        }
        let chroma_non_intra_loaded = r.flag()?;
        if chroma_non_intra_loaded {
            return Err(M2vError::Unsupported("chroma quantisation matrices"));
        }
        let load_intra_chroma_quant_matrix = false;
        let intra_chroma_quant_matrix = [0; 64];
        let load_non_intra_chroma_quant_matrix = false;
        let non_intra_chroma_quant_matrix = [0; 64];
        Ok(QuantMatrixExtension {
            load_intra_quant_matrix,
            intra_quant_matrix,
            load_non_intra_quant_matrix,
            non_intra_quant_matrix,
            load_intra_chroma_quant_matrix,
            intra_chroma_quant_matrix,
            load_non_intra_chroma_quant_matrix,
            non_intra_chroma_quant_matrix,
        })
    }
}

/// One decoded header or slice of the elementary stream.
#[derive(Debug, Clone)]
pub enum Unit<'a> {
    SequenceHeader(SequenceHeader),
    SequenceExtension(SequenceExtension),
    SequenceDisplayExtension(SequenceDisplayExtension),
    QuantMatrixExtension(QuantMatrixExtension),
    Picture(PictureHeader),
    PictureCodingExtension(PictureCodingExtension),
    Slice {
        number: u32,
        data: &'a [u8],
    },
    UserData(&'a [u8]),
    /// GOP headers and anything the hardware path does not read.
    Other {
        code: u8,
        data: &'a [u8],
    },
}

/// Split the stream on `00 00 01` start codes and decode every header.
///
/// The leading bytes before the first start code are skipped, so the caller may
/// hand this a packet of any alignment.
pub fn parse_units(data: &[u8]) -> Result<Vec<Unit<'_>>, M2vError> {
    split_start_codes(data)
        .into_iter()
        .map(|(code, payload)| parse_unit(code, payload))
        .collect()
}

/// Decode one start-code unit. Exposed so a caller can report which header of a
/// damaged stream it choked on.
pub fn parse_unit(code: u8, payload: &[u8]) -> Result<Unit<'_>, M2vError> {
    let unit = match code {
        CODE_SEQUENCE => Unit::SequenceHeader(SequenceHeader::parse(payload)?),
        CODE_PICTURE => Unit::Picture(PictureHeader::parse(payload)?),
        CODE_USER_DATA => Unit::UserData(payload),
        CODE_EXTENSION | CODE_EXTENSION_ALT => {
            // `extension_start_code_identifier` is four bits, so the body
            // is not byte aligned and must continue in the same reader.
            let mut r = Reader::new(payload);
            let id = r.bits(4)?;
            match id {
                EXT_SEQUENCE => Unit::SequenceExtension(SequenceExtension::read(&mut r)?),
                EXT_SEQUENCE_DISPLAY => {
                    Unit::SequenceDisplayExtension(SequenceDisplayExtension::read(&mut r)?)
                }
                EXT_QUANT_MATRIX => Unit::QuantMatrixExtension(QuantMatrixExtension::read(&mut r)?),
                EXT_PICTURE_CODING => {
                    Unit::PictureCodingExtension(PictureCodingExtension::read(&mut r)?)
                }
                // Picture display, copyright and the scalable extensions do
                // not feed the register file.
                _ => Unit::Other {
                    code,
                    data: payload,
                },
            }
        }
        c if (CODE_SLICE_MIN..=CODE_SLICE_MAX).contains(&c) => Unit::Slice {
            number: u32::from(c),
            data: payload,
        },
        code => Unit::Other {
            code,
            data: payload,
        },
    };
    Ok(unit)
}

/// Byte ranges of the picture groups the hardware path consumes.
///
/// The official parser hands the hardware one group at a time and cuts the
/// stream at every sequence header (`00 00 01 B3`) or picture header
/// (`00 00 01 00`) start code, so a group is either a bare header run or one
/// picture with the headers that belong to it. GOP, sequence display and
/// quant-matrix extensions stay with the picture they precede, and a group ends
/// in front of the next `00 00 01` -- `mpp_m2vd_parser_split` rewinds its
/// `dst_len` by exactly those three bytes.
pub fn picture_groups(data: &[u8]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i] == 0
            && data[i + 1] == 0
            && data[i + 2] == 1
            && matches!(data[i + 3], CODE_SEQUENCE | CODE_PICTURE)
        {
            if starts.is_empty() && i > 0 {
                starts.push(0);
            }
            starts.push(i);
            i += 4;
        } else {
            i += 1;
        }
    }
    if starts.is_empty() {
        return Vec::new();
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(data.len());
            (*start, end)
        })
        .collect()
}

/// Positions of every `00 00 01` start code, each unit running to the next.
pub fn split_start_codes(data: &[u8]) -> Vec<(u8, &[u8])> {
    let mut found = Vec::new();
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            found.push((i, data[i + 3]));
            i += 4;
        } else {
            i += 1;
        }
    }
    let mut units = Vec::with_capacity(found.len());
    for (index, (start, code)) in found.iter().enumerate() {
        let payload_start = start + 4;
        let payload_end = found
            .get(index + 1)
            .map(|(next, _)| *next)
            .unwrap_or(data.len());
        if payload_start <= payload_end && payload_end <= data.len() {
            units.push((*code, &data[payload_start..payload_end]));
        }
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROGRESSIVE: &[u8] = include_bytes!("../../../tests/data/m2v_320x240_12.m2v");
    const TINY: &[u8] = include_bytes!("../../../tests/data/m2v_64x64_3.m2v");
    const INTERLACED: &[u8] = include_bytes!("../../../tests/data/m2v_320x240_ilaced.m2v");

    fn picture_types(units: &[Unit<'_>]) -> String {
        units
            .iter()
            .filter_map(|u| match u {
                Unit::Picture(p) => Some(p.coding_letter()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn sequence_header_matches_the_encoder() {
        let units = parse_units(PROGRESSIVE).unwrap();
        let seq = units
            .iter()
            .find_map(|u| match u {
                Unit::SequenceHeader(s) => Some(s),
                _ => None,
            })
            .expect("sequence header");
        assert_eq!(seq.horizontal_size_value, 320);
        assert_eq!(seq.vertical_size_value, 240);
        // frame_rate_code 3 is 25 fps in the 13818-2 table.
        assert_eq!(seq.frame_rate_code, 3);
        assert_eq!(seq.aspect_ratio_information, 2, "320x240 is 4:3");
        assert!(!seq.load_intra_quant_matrix);
    }

    #[test]
    fn sequence_extension_names_the_profile_and_chroma() {
        let units = parse_units(PROGRESSIVE).unwrap();
        let ext = units
            .iter()
            .find_map(|u| match u {
                Unit::SequenceExtension(e) => Some(e),
                _ => None,
            })
            .expect("sequence extension");
        assert_eq!(ext.chroma_format, 1, "4:2:0");
        assert!(ext.progressive_sequence);
        // Main profile (0b100), Main level (0b1000), escape clear.
        assert_eq!(ext.profile_and_level_indication, 0x48);
        assert_eq!(ext.horizontal_size_extension, 0);
        assert_eq!(ext.vertical_size_extension, 0);
        assert_eq!(ext.frame_rate_extension_d, 0);
    }

    /// The picture types come straight from `ffprobe -show_entries frame=pict_type`
    /// on the same fixtures, so a bit-field drift shows up here first.
    #[test]
    fn picture_coding_types_follow_the_gop() {
        let units = parse_units(PROGRESSIVE).unwrap();
        assert_eq!(picture_types(&units), "IPBPBIPBPB");
        let units = parse_units(TINY).unwrap();
        assert_eq!(picture_types(&units), "III");
        let units = parse_units(INTERLACED).unwrap();
        assert_eq!(picture_types(&units), "IPBIBP");
    }

    /// f_code is the field the bit alignment shows up in first: an I picture
    /// carries the 0b1111 "not used" code, P and B carry 1..4 per direction.
    #[test]
    fn f_code_matches_the_picture_type() {
        let units = parse_units(INTERLACED).unwrap();
        let mut letters = Vec::new();
        let mut pending: Option<char> = None;
        for unit in &units {
            match unit {
                Unit::Picture(p) => pending = Some(p.coding_letter()),
                Unit::PictureCodingExtension(e) => {
                    letters.push((pending.take().unwrap(), e.f_code));
                }
                _ => {}
            }
        }
        assert_eq!(letters.len(), 6, "one extension per picture");
        for (letter, f_code) in &letters {
            match letter {
                'I' => assert_eq!(*f_code, [[15, 15], [15, 15]], "I pictures use 0b1111"),
                // ISO/IEC 13818-2 keeps f_code in 1..=4, but the FFmpeg encoder
                // does emit 5; the four-bit register field cannot hold more than
                // that, so the hardware path clamps rather than trusts.
                _ => assert!(
                    f_code.iter().flatten().all(|v| (1..=15).contains(v)),
                    "{letter} f_code {f_code:?} outside the four-bit field"
                ),
            }
        }
        assert_eq!(letters[1].1, [[1, 1], [15, 15]], "P forward f_code");
    }

    #[test]
    fn slices_are_counted_per_picture() {
        let units = parse_units(PROGRESSIVE).unwrap();
        let mut per_picture = Vec::new();
        let mut current: Option<usize> = None;
        for unit in &units {
            match unit {
                Unit::Picture(_) => {
                    if let Some(n) = current.take() {
                        per_picture.push(n);
                    }
                    current = Some(0);
                }
                Unit::Slice { .. } => {
                    *current.as_mut().unwrap() += 1;
                }
                _ => {}
            }
        }
        if let Some(n) = current {
            per_picture.push(n);
        }
        // 240 luma rows / 16 rows per slice = 15 slice rows, numbered from one.
        assert_eq!(per_picture, vec![15; 10]);
        let first = match units
            .iter()
            .find(|u| matches!(u, Unit::Slice { .. }))
            .unwrap()
        {
            Unit::Slice { number, .. } => *number,
            _ => unreachable!(),
        };
        assert_eq!(first, 1, "slice numbering starts at one");
    }

    #[test]
    fn interlaced_stream_carries_picture_coding_extension() {
        let units = parse_units(INTERLACED).unwrap();
        let ext = units
            .iter()
            .find_map(|u| match u {
                Unit::PictureCodingExtension(e) => Some(*e),
                _ => None,
            })
            .expect("picture coding extension");
        assert_ne!(ext.picture_structure, 0);
        assert!(ext.f_code[0][0] >= 1 && ext.f_code[1][1] >= 1, "{ext:?}");
        assert!(
            units.iter().any(|u| matches!(u, Unit::Picture(_))),
            "no picture start code"
        );
    }

    #[test]
    fn truncated_stream_reports_error_not_panic() {
        for cut in [1usize, 5, 9, 13, 20, 33] {
            let result = parse_units(&TINY[..cut]);
            assert!(
                result.is_ok() || result.unwrap_err() == M2vError::Truncated,
                "cut {cut}"
            );
        }
    }
}
