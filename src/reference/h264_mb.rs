//! H.264 I-slice macroblock syntax.
//!
//! This layer parses one intra macroblock through the start of its residual
//! syntax. Pixel prediction and residual block traversal intentionally remain
//! separate so both can be validated independently.

use std::fmt;

use crate::codec::h264::parser::{BitReader, Pps, Sps};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntraMacroblockKind {
    I4x4,
    I8x8,
    I16x16 { prediction_mode: u8 },
    IPcm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedIMacroblock {
    pub kind: IntraMacroblockKind,
    pub luma_prediction_modes: Vec<u8>,
    pub chroma_prediction_mode: Option<u8>,
    pub coded_block_pattern: u8,
    pub qp_delta: i32,
    pub pcm_planes: Vec<Vec<u16>>,
    /// First residual bit for coded macroblocks, or the first bit after the PCM
    /// payload for `I_PCM`.
    pub residual_bit_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacroblockContext {
    pub chroma_format_idc: u8,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    pub transform_8x8: bool,
}

impl MacroblockContext {
    pub fn from_sps_pps(sps: &Sps, pps: &Pps) -> Result<Self, MacroblockError> {
        if sps.separate_colour_plane {
            return Err(MacroblockError::Unsupported("separate colour plane"));
        }
        let context = MacroblockContext {
            chroma_format_idc: u8::try_from(sps.chroma_format_idc)
                .map_err(|_| MacroblockError::Unsupported("chroma_format_idc"))?,
            bit_depth_luma: u8::try_from(sps.bit_depth_luma)
                .map_err(|_| MacroblockError::Unsupported("luma bit depth"))?,
            bit_depth_chroma: u8::try_from(sps.bit_depth_chroma)
                .map_err(|_| MacroblockError::Unsupported("chroma bit depth"))?,
            transform_8x8: pps.transform_8x8,
        };
        context.validate()?;
        Ok(context)
    }

    fn validate(self) -> Result<(), MacroblockError> {
        if self.chroma_format_idc > 3 {
            return Err(MacroblockError::Unsupported("chroma_format_idc"));
        }
        if !(8..=10).contains(&self.bit_depth_luma) || !(8..=10).contains(&self.bit_depth_chroma) {
            return Err(MacroblockError::Unsupported("bit depth"));
        }
        Ok(())
    }

    fn decode_chroma(self) -> bool {
        matches!(self.chroma_format_idc, 1 | 2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroblockError {
    Truncated,
    InvalidMbType(u32),
    InvalidCodedBlockPattern(u32),
    InvalidIntraMode(u32),
    InvalidChromaPredictionMode(u32),
    NonZeroPcmAlignment,
    Unsupported(&'static str),
}

impl fmt::Display for MacroblockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MacroblockError::Truncated => write!(f, "H.264 macroblock bitstream truncated"),
            MacroblockError::InvalidMbType(value) => {
                write!(f, "invalid H.264 I macroblock type {value}")
            }
            MacroblockError::InvalidCodedBlockPattern(value) => {
                write!(f, "invalid H.264 coded_block_pattern {value}")
            }
            MacroblockError::InvalidIntraMode(value) => {
                write!(f, "invalid H.264 intra prediction mode {value}")
            }
            MacroblockError::InvalidChromaPredictionMode(value) => {
                write!(f, "invalid H.264 chroma prediction mode {value}")
            }
            MacroblockError::NonZeroPcmAlignment => {
                write!(f, "H.264 I_PCM alignment bits must be zero")
            }
            MacroblockError::Unsupported(what) => write!(f, "unsupported H.264 macroblock: {what}"),
        }
    }
}

impl std::error::Error for MacroblockError {}

/// H.264 intra mode prediction from the available left and top modes.
pub fn predict_intra_mode(left: Option<u8>, top: Option<u8>) -> u8 {
    match (left, top) {
        (Some(left), Some(top)) => left.min(top),
        (Some(mode), None) | (None, Some(mode)) => mode,
        (None, None) => 2,
    }
}

/// Parse one I-slice macroblock starting at `bit_offset`.
pub fn parse_i_macroblock(
    data: &[u8],
    bit_offset: usize,
    context: MacroblockContext,
) -> Result<ParsedIMacroblock, MacroblockError> {
    context.validate()?;
    let offset = u32::try_from(bit_offset)
        .map_err(|_| MacroblockError::Unsupported("macroblock bit offset"))?;
    let mut reader = BitReader::new(data);
    reader
        .skip(offset)
        .map_err(|_| MacroblockError::Truncated)?;
    let mb_type = read_ue(&mut reader)?;
    if mb_type > 25 {
        return Err(MacroblockError::InvalidMbType(mb_type));
    }

    if mb_type == 25 {
        align_to_byte(&mut reader)?;
        let pcm_planes = read_pcm_planes(&mut reader, context)?;
        return Ok(ParsedIMacroblock {
            kind: IntraMacroblockKind::IPcm,
            luma_prediction_modes: Vec::new(),
            chroma_prediction_mode: None,
            coded_block_pattern: 0,
            qp_delta: 0,
            pcm_planes,
            residual_bit_offset: reader.bit_pos(),
        });
    }

    let (kind, luma_prediction_modes, mut cbp) = if mb_type == 0 {
        let transform_8x8 = context.transform_8x8 && read_bit(&mut reader)? == 1;
        let modes = if transform_8x8 {
            parse_intra_modes(&mut reader, 2)?
        } else {
            parse_intra_modes(&mut reader, 4)?
        };
        (
            if transform_8x8 {
                IntraMacroblockKind::I8x8
            } else {
                IntraMacroblockKind::I4x4
            },
            modes,
            0,
        )
    } else {
        let type_index = (mb_type - 1) as usize;
        let prediction_mode = [2, 1, 0, 3][type_index % 4];
        let cbp = [0, 16, 32, 15, 31, 47][type_index / 4];
        (
            IntraMacroblockKind::I16x16 { prediction_mode },
            vec![prediction_mode; 16],
            cbp,
        )
    };

    let chroma_prediction_mode = if context.decode_chroma() {
        let mode = read_ue(&mut reader)?;
        if mode > 3 {
            return Err(MacroblockError::InvalidChromaPredictionMode(mode));
        }
        Some(mode as u8)
    } else {
        None
    };

    if !matches!(kind, IntraMacroblockKind::I16x16 { .. }) {
        let coded = read_ue(&mut reader)?;
        cbp = if context.decode_chroma() {
            if coded > 47 {
                return Err(MacroblockError::InvalidCodedBlockPattern(coded));
            }
            INTRA_CBP[coded as usize]
        } else {
            if coded > 15 {
                return Err(MacroblockError::InvalidCodedBlockPattern(coded));
            }
            INTRA_CBP_GRAY[coded as usize]
        };
    } else if !context.decode_chroma() && cbp > 15 {
        return Err(MacroblockError::InvalidCodedBlockPattern(u32::from(cbp)));
    }

    let qp_delta = if cbp != 0 || matches!(kind, IntraMacroblockKind::I16x16 { .. }) {
        read_se(&mut reader)?
    } else {
        0
    };

    Ok(ParsedIMacroblock {
        kind,
        luma_prediction_modes,
        chroma_prediction_mode,
        coded_block_pattern: cbp,
        qp_delta,
        pcm_planes: Vec::new(),
        residual_bit_offset: reader.bit_pos(),
    })
}

fn parse_intra_modes(reader: &mut BitReader<'_>, grid: usize) -> Result<Vec<u8>, MacroblockError> {
    let mut modes = vec![0u8; grid * grid];
    for row in 0..grid {
        for column in 0..grid {
            let left = column.checked_sub(1).map(|left| modes[row * grid + left]);
            let top = row.checked_sub(1).map(|top| modes[top * grid + column]);
            let predicted = predict_intra_mode(left, top);
            let mode = if read_bit(reader)? == 1 {
                predicted
            } else {
                let remainder = read_bits(reader, 3)?;
                (remainder + u32::from(remainder >= u32::from(predicted))) as u8
            };
            if mode > 8 {
                return Err(MacroblockError::InvalidIntraMode(u32::from(mode)));
            }
            modes[row * grid + column] = mode;
        }
    }
    Ok(modes)
}

fn read_pcm_planes(
    reader: &mut BitReader<'_>,
    context: MacroblockContext,
) -> Result<Vec<Vec<u16>>, MacroblockError> {
    let (luma, chroma) = match context.chroma_format_idc {
        0 => (256, Vec::new()),
        1 => (256, vec![64, 64]),
        2 => (256, vec![128, 128]),
        3 => (256, vec![256, 256]),
        _ => return Err(MacroblockError::Unsupported("chroma_format_idc")),
    };
    let mut planes = Vec::with_capacity(1 + chroma.len());
    planes.push(read_samples(reader, luma, context.bit_depth_luma)?);
    for count in chroma {
        planes.push(read_samples(reader, count, context.bit_depth_chroma)?);
    }
    Ok(planes)
}

fn read_samples(
    reader: &mut BitReader<'_>,
    count: usize,
    bit_depth: u8,
) -> Result<Vec<u16>, MacroblockError> {
    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
        samples.push(read_bits(reader, u32::from(bit_depth))? as u16);
    }
    Ok(samples)
}

fn align_to_byte(reader: &mut BitReader<'_>) -> Result<(), MacroblockError> {
    while !reader.is_aligned_byte() {
        if read_bit(reader)? != 0 {
            return Err(MacroblockError::NonZeroPcmAlignment);
        }
    }
    Ok(())
}

fn read_bit(reader: &mut BitReader<'_>) -> Result<u32, MacroblockError> {
    reader.read_bit().map_err(|_| MacroblockError::Truncated)
}

fn read_bits(reader: &mut BitReader<'_>, count: u32) -> Result<u32, MacroblockError> {
    reader
        .read_bits(count)
        .map_err(|_| MacroblockError::Truncated)
}

fn read_ue(reader: &mut BitReader<'_>) -> Result<u32, MacroblockError> {
    reader.read_ue().map_err(|_| MacroblockError::Truncated)
}

fn read_se(reader: &mut BitReader<'_>) -> Result<i32, MacroblockError> {
    reader.read_se().map_err(|_| MacroblockError::Truncated)
}

const INTRA_CBP: [u8; 48] = [
    47, 31, 15, 0, 23, 27, 29, 30, 7, 11, 13, 14, 39, 43, 45, 46, 16, 3, 5, 10, 12, 19, 21, 26, 28,
    35, 37, 42, 44, 1, 2, 4, 8, 17, 18, 20, 24, 6, 9, 22, 25, 32, 33, 34, 36, 40, 38, 41,
];

const INTRA_CBP_GRAY: [u8; 16] = [15, 0, 7, 11, 13, 14, 3, 5, 10, 12, 1, 2, 4, 8, 6, 9];

#[cfg(test)]
mod tests {
    use super::*;

    struct BitWriter {
        bits: Vec<u8>,
    }

    impl BitWriter {
        fn new() -> Self {
            Self { bits: Vec::new() }
        }

        fn put(&mut self, value: u32, count: u32) -> &mut Self {
            for shift in (0..count).rev() {
                self.bits.push(((value >> shift) & 1) as u8);
            }
            self
        }

        fn ue(&mut self, value: u32) -> &mut Self {
            let mut zeros = 0;
            while (1u32 << (zeros + 1)) <= value + 1 {
                zeros += 1;
            }
            for _ in 0..zeros {
                self.bits.push(0);
            }
            self.put(value + 1, zeros + 1)
        }

        fn se(&mut self, value: i32) -> &mut Self {
            let code = if value <= 0 {
                value.unsigned_abs() * 2
            } else {
                value as u32 * 2 - 1
            };
            self.ue(code)
        }

        fn finish(self) -> Vec<u8> {
            let mut bytes = vec![0u8; self.bits.len().div_ceil(8)];
            for (position, bit) in self.bits.into_iter().enumerate() {
                bytes[position / 8] |= bit << (7 - position % 8);
            }
            bytes
        }
    }

    fn context(chroma_format_idc: u8) -> MacroblockContext {
        MacroblockContext {
            chroma_format_idc,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            transform_8x8: true,
        }
    }

    #[test]
    fn intra_mode_prediction_uses_available_neighbours() {
        assert_eq!(predict_intra_mode(None, None), 2);
        assert_eq!(predict_intra_mode(Some(5), None), 5);
        assert_eq!(predict_intra_mode(None, Some(7)), 7);
        assert_eq!(predict_intra_mode(Some(5), Some(3)), 3);
    }

    #[test]
    fn parses_i16x16_dc_chroma_mode_and_qp_delta() {
        let mut writer = BitWriter::new();
        writer.ue(1); // I16x16, DC mode, luma/chroma CBP zero
        writer.ue(2); // chroma DC
        writer.se(0); // mb_qp_delta
        let mb = parse_i_macroblock(&writer.finish(), 0, context(1)).unwrap();
        assert_eq!(mb.kind, IntraMacroblockKind::I16x16 { prediction_mode: 2 });
        assert_eq!(mb.chroma_prediction_mode, Some(2));
        assert_eq!(mb.coded_block_pattern, 0);
        assert_eq!(mb.qp_delta, 0);
        assert_eq!(mb.residual_bit_offset, 7);
    }

    #[test]
    fn parses_i4x4_modes_with_intra_macroblock_prediction() {
        let mut writer = BitWriter::new();
        writer.ue(0); // I4x4
        writer.put(0, 1); // transform_size_8x8_flag
        writer.put(1, 1); // block 0: predicted DC
        writer.put(0, 1); // block 1: explicit mode
        writer.put(0b010, 3); // remainder 2 -> mode 3
        for _ in 2..16 {
            writer.put(1, 1);
        }
        writer.ue(0); // chroma prediction mode
        writer.ue(0); // coded_block_pattern code for CBP 47
        writer.se(-1);
        let mb = parse_i_macroblock(&writer.finish(), 0, context(1)).unwrap();
        assert_eq!(mb.kind, IntraMacroblockKind::I4x4);
        assert_eq!(mb.luma_prediction_modes[0], 2);
        assert_eq!(mb.luma_prediction_modes[1], 3);
        assert_eq!(mb.chroma_prediction_mode, Some(0));
        assert_eq!(mb.coded_block_pattern, 47);
        assert_eq!(mb.qp_delta, -1);
    }

    #[test]
    fn parses_i8x8_modes_and_uses_the_same_intra_cbp_table() {
        let mut writer = BitWriter::new();
        writer.ue(0); // I4x4/I8x8 family
        writer.put(1, 1); // transform_size_8x8_flag
        writer.put(1, 1); // mode 0 predicted DC
        writer.put(1, 1); // mode 1 predicted DC
        writer.put(1, 1); // mode 2 predicted DC
        writer.put(1, 1); // mode 3 predicted DC
        writer.ue(1); // chroma horizontal
        writer.ue(2); // intra CBP code for 15
        writer.se(2);
        let mb = parse_i_macroblock(&writer.finish(), 0, context(1)).unwrap();
        assert_eq!(mb.kind, IntraMacroblockKind::I8x8);
        assert_eq!(mb.luma_prediction_modes, [2, 2, 2, 2]);
        assert_eq!(mb.coded_block_pattern, 15);
        assert_eq!(mb.qp_delta, 2);
    }

    #[test]
    fn parses_monochrome_i_pcm_after_zero_alignment() {
        let mut writer = BitWriter::new();
        writer.ue(25);
        while writer.bits.len() % 8 != 0 {
            writer.bits.push(0);
        }
        for sample in 0..256u32 {
            writer.put(sample & 0xff, 8);
        }
        let mb = parse_i_macroblock(&writer.finish(), 0, context(0)).unwrap();
        assert_eq!(mb.kind, IntraMacroblockKind::IPcm);
        assert_eq!(mb.pcm_planes.len(), 1);
        assert_eq!(mb.pcm_planes[0].len(), 256);
        assert_eq!(mb.pcm_planes[0][0], 0);
        assert_eq!(mb.pcm_planes[0][255], 255);
        assert_eq!(mb.residual_bit_offset, (9 + 7 + 256 * 8));
    }

    #[test]
    fn parses_i_pcm_chroma_at_ten_bits() {
        let mut writer = BitWriter::new();
        writer.ue(25);
        while writer.bits.len() % 8 != 0 {
            writer.bits.push(0);
        }
        for plane_size in [256, 64, 64] {
            for sample in 0..plane_size {
                writer.put((sample & 0x3ff) as u32, 10);
            }
        }
        let mut ctx = context(1);
        ctx.bit_depth_luma = 10;
        ctx.bit_depth_chroma = 10;
        let mb = parse_i_macroblock(&writer.finish(), 0, ctx).unwrap();
        assert_eq!(
            mb.pcm_planes.iter().map(Vec::len).collect::<Vec<_>>(),
            [256, 64, 64]
        );
        assert_eq!(mb.pcm_planes[0][255], 255);
        assert_eq!(mb.pcm_planes[1][63], 63);
    }

    #[test]
    fn rejects_nonzero_pcm_alignment_and_invalid_cbp() {
        let mut writer = BitWriter::new();
        writer.ue(25);
        while writer.bits.len() % 7 != 0 {
            writer.bits.push(1);
        }
        assert_eq!(
            parse_i_macroblock(&writer.finish(), 0, context(0)),
            Err(MacroblockError::NonZeroPcmAlignment)
        );

        let mut writer = BitWriter::new();
        writer.ue(0);
        writer.put(0, 1);
        for _ in 0..16 {
            writer.put(1, 1);
        }
        writer.ue(0);
        writer.ue(48);
        assert_eq!(
            parse_i_macroblock(&writer.finish(), 0, context(1)),
            Err(MacroblockError::InvalidCodedBlockPattern(48))
        );
    }
}
