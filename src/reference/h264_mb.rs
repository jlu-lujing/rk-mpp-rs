//! H.264 I-slice macroblock syntax.
//!
//! This layer parses one intra macroblock header and traverses its CAVLC
//! residual blocks through one bit cursor. Pixel prediction remains separate
//! so syntax and reconstruction can be validated independently.

use std::fmt;

use super::h264_cavlc::{Cavlc8x8Residual, CavlcBlockKind, CavlcError, CavlcReader, CavlcResidual};
use crate::codec::h264::parser::{BitReader, Pps, Sps};

pub(super) const I4X4_BLOCK_ORDER: [usize; 16] =
    [0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15];

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

/// Non-zero counts immediately above and left of this macroblock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroblockNeighbors {
    pub luma_dc_left: Option<u8>,
    pub luma_dc_top: Option<u8>,
    pub luma_left: [Option<u8>; 4],
    pub luma_top: [Option<u8>; 4],
    pub chroma_left: [[Option<u8>; 4]; 2],
    pub chroma_top: [[Option<u8>; 4]; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntraModeNeighbors {
    pub left: [Option<u8>; 4],
    pub top: [Option<u8>; 4],
}

impl IntraModeNeighbors {
    pub const fn unavailable() -> Self {
        Self {
            left: [None; 4],
            top: [None; 4],
        }
    }
}

impl MacroblockNeighbors {
    pub const fn unavailable() -> Self {
        Self {
            luma_dc_left: None,
            luma_dc_top: None,
            luma_left: [None; 4],
            luma_top: [None; 4],
            chroma_left: [[None; 4]; 2],
            chroma_top: [[None; 4]; 2],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMacroblockResidual {
    pub luma_dc: Option<CavlcResidual>,
    pub luma_4x4: [Option<CavlcResidual>; 16],
    pub luma_8x8: [Option<Cavlc8x8Residual>; 4],
    pub chroma_dc: [Option<CavlcResidual>; 2],
    pub chroma_ac: [[Option<CavlcResidual>; 8]; 2],
    pub luma_dc_non_zero: u8,
    pub luma_non_zero: [u8; 16],
    pub chroma_non_zero: [Vec<u8>; 2],
    pub end_bit: usize,
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
    Cavlc(CavlcError),
    CavlcAt {
        block: usize,
        bit: usize,
        n_c: i16,
        error: CavlcError,
    },
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
            MacroblockError::Cavlc(error) => error.fmt(f),
            MacroblockError::CavlcAt {
                block,
                bit,
                n_c,
                error,
            } => {
                write!(f, "CAVLC block {block} at bit {bit} (nC={n_c}): {error}")
            }
            MacroblockError::Unsupported(what) => write!(f, "unsupported H.264 macroblock: {what}"),
        }
    }
}

impl std::error::Error for MacroblockError {}

impl From<CavlcError> for MacroblockError {
    fn from(error: CavlcError) -> Self {
        MacroblockError::Cavlc(error)
    }
}

/// H.264 intra mode prediction from the available left and top modes.
pub fn predict_intra_mode(left: Option<u8>, top: Option<u8>) -> u8 {
    match (left, top) {
        (Some(left), Some(top)) => left.min(top),
        _ => 2,
    }
}

/// Parse one I-slice macroblock starting at `bit_offset`.
pub fn parse_i_macroblock(
    data: &[u8],
    bit_offset: usize,
    context: MacroblockContext,
) -> Result<ParsedIMacroblock, MacroblockError> {
    parse_i_macroblock_with_modes(data, bit_offset, context, IntraModeNeighbors::unavailable())
}

pub fn parse_i_macroblock_with_modes(
    data: &[u8],
    bit_offset: usize,
    context: MacroblockContext,
    mode_neighbors: IntraModeNeighbors,
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
            parse_i8x8_modes(&mut reader, mode_neighbors)?
        } else {
            parse_i4x4_modes(&mut reader, mode_neighbors)?
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
        let prediction_mode = (type_index % 4) as u8;
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

/// Parse the residual syntax following an already parsed I macroblock.
pub fn parse_i_macroblock_residual(
    data: &[u8],
    macroblock: &ParsedIMacroblock,
    context: MacroblockContext,
    neighbors: &MacroblockNeighbors,
) -> Result<ParsedMacroblockResidual, MacroblockError> {
    context.validate()?;
    if context.chroma_format_idc == 3 {
        return Err(MacroblockError::Unsupported(
            "4:4:4 macroblock residual traversal",
        ));
    }
    let mut reader = CavlcReader::at_bit_offset(data, macroblock.residual_bit_offset)?;
    let mut result = ParsedMacroblockResidual {
        luma_dc: None,
        luma_4x4: std::array::from_fn(|_| None),
        luma_8x8: std::array::from_fn(|_| None),
        chroma_dc: std::array::from_fn(|_| None),
        chroma_ac: std::array::from_fn(|_| std::array::from_fn(|_| None)),
        luma_dc_non_zero: 0,
        luma_non_zero: [0; 16],
        chroma_non_zero: [Vec::new(), Vec::new()],
        end_bit: macroblock.residual_bit_offset,
    };
    if let Some((width, height)) = chroma_grid(context.chroma_format_idc) {
        result.chroma_non_zero = [vec![0; width * height], vec![0; width * height]];
    }

    if macroblock.kind == IntraMacroblockKind::IPcm {
        result.luma_dc_non_zero = 16;
        result.luma_non_zero.fill(16);
        let (width, height) = chroma_grid(context.chroma_format_idc).unwrap_or((0, 0));
        for plane in 0..2 {
            result.chroma_non_zero[plane] = vec![16; width * height];
        }
        result.end_bit = macroblock.residual_bit_offset;
        return Ok(result);
    }

    let luma_cbp = macroblock.coded_block_pattern & 0x0f;
    if matches!(macroblock.kind, IntraMacroblockKind::I16x16 { .. }) {
        let n_c = super::h264_cavlc::predict_non_zero_count(
            neighbors.luma_dc_left,
            neighbors.luma_dc_top,
        )?;
        let bit = reader.bit_pos();
        let dc = reader
            .decode_residual_4x4(CavlcBlockKind::Regular { n_c, max_coeff: 16 })
            .map_err(|error| MacroblockError::CavlcAt {
                block: 100,
                bit,
                n_c,
                error,
            })?;
        result.luma_dc_non_zero = dc.total_coeff as u8;
        result.luma_dc = Some(dc);
    }

    match macroblock.kind {
        IntraMacroblockKind::I4x4 | IntraMacroblockKind::I16x16 { .. } => {
            decode_luma_4x4_groups(
                &mut reader,
                luma_cbp,
                if matches!(macroblock.kind, IntraMacroblockKind::I16x16 { .. }) {
                    15
                } else {
                    16
                },
                neighbors,
                &mut result.luma_4x4,
                &mut result.luma_non_zero,
            )?;
        }
        IntraMacroblockKind::I8x8 => {
            decode_luma_8x8_groups(
                &mut reader,
                luma_cbp,
                neighbors,
                &mut result.luma_8x8,
                &mut result.luma_non_zero,
            )?;
        }
        IntraMacroblockKind::IPcm => unreachable!(),
    }

    if context.decode_chroma() && macroblock.coded_block_pattern & 0x30 != 0 {
        let (width, height) = chroma_grid(context.chroma_format_idc).unwrap();
        for plane in 0..2 {
            let kind = if context.chroma_format_idc == 1 {
                CavlcBlockKind::ChromaDc420
            } else {
                CavlcBlockKind::ChromaDc422
            };
            let bit = reader.bit_pos();
            result.chroma_dc[plane] = Some(reader.decode_residual_4x4(kind).map_err(|error| {
                MacroblockError::CavlcAt {
                    block: 200 + plane,
                    bit,
                    n_c: -1,
                    error,
                }
            })?);
        }

        if macroblock.coded_block_pattern & 0x20 != 0 {
            for plane in 0..2 {
                for row in 0..height {
                    for column in 0..width {
                        let n_c = predict_block_nc(
                            &result.chroma_non_zero[plane],
                            width,
                            row,
                            column,
                            neighbors.chroma_left[plane][row],
                            neighbors.chroma_top[plane][column],
                        )?;
                        let bit = reader.bit_pos();
                        let block = reader
                            .decode_residual_4x4(CavlcBlockKind::Regular { n_c, max_coeff: 15 })
                            .map_err(|error| MacroblockError::CavlcAt {
                                block: 300 + plane * 8 + row * width + column,
                                bit,
                                n_c,
                                error,
                            })?;
                        let position = row * width + column;
                        result.chroma_non_zero[plane][position] = block.total_coeff as u8;
                        result.chroma_ac[plane][position] = Some(block);
                    }
                }
            }
        }
    }

    result.end_bit = reader.bit_pos();
    Ok(result)
}

fn decode_luma_4x4_groups(
    reader: &mut CavlcReader<'_>,
    cbp: u8,
    max_coeff: u8,
    neighbors: &MacroblockNeighbors,
    blocks: &mut [Option<CavlcResidual>; 16],
    non_zero: &mut [u8; 16],
) -> Result<(), MacroblockError> {
    for group in 0..4 {
        let base_row = (group / 2) * 2;
        let base_column = (group % 2) * 2;
        if cbp & (1 << group) == 0 {
            continue;
        }
        for sub in 0..4 {
            let row = base_row + sub / 2;
            let column = base_column + sub % 2;
            blocks[row * 4 + column] = Some(decode_one_4x4(
                reader, neighbors, non_zero, row, column, max_coeff,
            )?);
        }
    }
    Ok(())
}

fn decode_luma_8x8_groups(
    reader: &mut CavlcReader<'_>,
    cbp: u8,
    neighbors: &MacroblockNeighbors,
    blocks: &mut [Option<Cavlc8x8Residual>; 4],
    non_zero: &mut [u8; 16],
) -> Result<(), MacroblockError> {
    for group in 0..4 {
        let base_row = (group / 2) * 2;
        let base_column = (group % 2) * 2;
        if cbp & (1 << group) == 0 {
            continue;
        }
        let start = reader.bit_pos();
        let mut scan = [0i32; 64];
        let mut total_coeff = [0usize; 4];
        let mut trailing_ones = [0usize; 4];
        let mut block_bits = [0usize; 4];
        for sub in 0..4 {
            let row = base_row + sub / 2;
            let column = base_column + sub % 2;
            let n_c = predict_block_nc(
                non_zero,
                4,
                row,
                column,
                neighbors.luma_left[row],
                neighbors.luma_top[column],
            )?;
            let bit = reader.bit_pos();
            let block = reader
                .decode_residual_4x4(CavlcBlockKind::Regular { n_c, max_coeff: 16 })
                .map_err(|error| MacroblockError::CavlcAt {
                    block: 400 + group * 4 + sub,
                    bit,
                    n_c,
                    error,
                })?;
            scan[sub * 16..sub * 16 + 16].copy_from_slice(&block.scan);
            total_coeff[sub] = block.total_coeff;
            trailing_ones[sub] = block.trailing_ones;
            block_bits[sub] = block.bits_consumed;
            non_zero[row * 4 + column] = block.total_coeff as u8;
        }
        blocks[group] = Some(Cavlc8x8Residual {
            scan,
            total_coeff,
            trailing_ones,
            block_bits,
            bits_consumed: reader.bit_pos() - start,
        });
    }
    Ok(())
}

fn decode_one_4x4(
    reader: &mut CavlcReader<'_>,
    neighbors: &MacroblockNeighbors,
    non_zero: &mut [u8; 16],
    row: usize,
    column: usize,
    max_coeff: u8,
) -> Result<CavlcResidual, MacroblockError> {
    let n_c = predict_block_nc(
        non_zero,
        4,
        row,
        column,
        neighbors.luma_left[row],
        neighbors.luma_top[column],
    )?;
    let bit = reader.bit_pos();
    let block = reader
        .decode_residual_4x4(CavlcBlockKind::Regular { n_c, max_coeff })
        .map_err(|error| MacroblockError::CavlcAt {
            block: row * 4 + column,
            bit,
            n_c,
            error,
        })?;
    non_zero[row * 4 + column] = block.total_coeff as u8;
    Ok(block)
}

fn predict_block_nc(
    non_zero: &[u8],
    width: usize,
    row: usize,
    column: usize,
    external_left: Option<u8>,
    external_top: Option<u8>,
) -> Result<i16, MacroblockError> {
    let left = if column == 0 {
        external_left
    } else {
        Some(non_zero[row * width + column - 1])
    };
    let top = if row == 0 {
        external_top
    } else {
        Some(non_zero[(row - 1) * width + column])
    };
    super::h264_cavlc::predict_non_zero_count(left, top).map_err(Into::into)
}

fn chroma_grid(chroma_format_idc: u8) -> Option<(usize, usize)> {
    match chroma_format_idc {
        1 => Some((2, 2)),
        2 => Some((2, 4)),
        _ => None,
    }
}

fn parse_i8x8_modes(
    reader: &mut BitReader<'_>,
    mode_neighbors: IntraModeNeighbors,
) -> Result<Vec<u8>, MacroblockError> {
    let mut modes = [0u8; 4];
    for row in 0..2 {
        for column in 0..2 {
            let left = if column == 0 {
                mode_neighbors.left[row * 2]
            } else {
                Some(modes[row * 2 + column - 1])
            };
            let top = if row == 0 {
                mode_neighbors.top[column * 2]
            } else {
                Some(modes[(row - 1) * 2 + column])
            };
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
            modes[row * 2 + column] = mode;
        }
    }
    Ok(modes.to_vec())
}

fn parse_i4x4_modes(
    reader: &mut BitReader<'_>,
    mode_neighbors: IntraModeNeighbors,
) -> Result<Vec<u8>, MacroblockError> {
    let mut modes = [0u8; 16];
    for block in I4X4_BLOCK_ORDER {
        let row = block / 4;
        let column = block % 4;
        let left = if column == 0 {
            mode_neighbors.left[row]
        } else {
            Some(modes[row * 4 + column - 1])
        };
        let top = if row == 0 {
            mode_neighbors.top[column]
        } else {
            Some(modes[(row - 1) * 4 + column])
        };
        let predicted = predict_intra_mode(left, top);
        let previous = read_bit(reader)? == 1;
        let mode = if previous {
            predicted
        } else {
            let remainder = read_bits(reader, 3)?;
            (remainder + u32::from(remainder >= u32::from(predicted))) as u8
        };
        if mode > 8 {
            return Err(MacroblockError::InvalidIntraMode(u32::from(mode)));
        }
        modes[block] = mode;
    }
    Ok(modes.to_vec())
}

pub(crate) fn intra_mode_grid(macroblock: &ParsedIMacroblock) -> [u8; 16] {
    match macroblock.kind {
        IntraMacroblockKind::I4x4 => macroblock
            .luma_prediction_modes
            .as_slice()
            .try_into()
            .unwrap(),
        IntraMacroblockKind::I8x8 => std::array::from_fn(|position| {
            let row = position / 4;
            let column = position % 4;
            macroblock.luma_prediction_modes[(row / 2) * 2 + column / 2]
        }),
        IntraMacroblockKind::I16x16 { prediction_mode } => [prediction_mode; 16],
        IntraMacroblockKind::IPcm => [0; 16],
    }
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
    fn intra_mode_prediction_requires_both_neighbours() {
        assert_eq!(predict_intra_mode(None, None), 2);
        assert_eq!(predict_intra_mode(Some(5), None), 2);
        assert_eq!(predict_intra_mode(None, Some(7)), 2);
        assert_eq!(predict_intra_mode(Some(5), Some(3)), 3);
    }

    #[test]
    fn parses_i16x16_vertical_chroma_mode_and_qp_delta() {
        let mut writer = BitWriter::new();
        writer.ue(1); // I16x16, vertical mode, luma/chroma CBP zero
        writer.ue(2); // chroma DC
        writer.se(0); // mb_qp_delta
        let mb = parse_i_macroblock(&writer.finish(), 0, context(1)).unwrap();
        assert_eq!(mb.kind, IntraMacroblockKind::I16x16 { prediction_mode: 0 });
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
    fn i4x4_modes_follow_the_eight_by_eight_transmission_order() {
        let mut writer = BitWriter::new();
        writer.ue(0); // I4x4
        writer.put(0, 1); // transform_size_8x8_flag
        writer.put(1, 1); // block 0: predicted DC
        writer.put(1, 1); // block 1: predicted DC
        writer.put(0, 1); // transmitted block 2
        writer.put(0b011, 3); // remainder 3 -> mode 4
        for _ in 3..16 {
            writer.put(1, 1);
        }
        writer.ue(0); // chroma prediction mode
        writer.ue(0); // coded_block_pattern code for CBP 47
        writer.se(0);
        let mb = parse_i_macroblock(&writer.finish(), 0, context(1)).unwrap();
        assert_eq!(mb.luma_prediction_modes[4], 4);
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
    fn parses_complete_i16x16_residual_in_c_order() {
        let mut writer = BitWriter::new();
        writer.ue(21); // I16x16 DC, CBP 47
        writer.ue(0); // chroma vertical
        writer.se(0);
        writer.put(0b01, 2); // luma DC: one trailing one
        writer.put(0, 1); // +1
        writer.put(1, 1); // no total zeros
        for _ in 0..16 {
            writer.put(1, 1); // empty luma AC blocks
        }
        writer.put(0b01, 2); // empty Cb DC
        writer.put(0b01, 2); // empty Cr DC
        for _ in 0..8 {
            writer.put(1, 1); // empty chroma AC blocks
        }
        let data = writer.finish();
        let header = parse_i_macroblock(&data, 0, context(1)).unwrap();
        let residual = parse_i_macroblock_residual(
            &data,
            &header,
            context(1),
            &MacroblockNeighbors::unavailable(),
        )
        .unwrap();
        assert_eq!(
            header.kind,
            IntraMacroblockKind::I16x16 { prediction_mode: 0 }
        );
        assert_eq!(residual.luma_dc.as_ref().unwrap().total_coeff, 1);
        assert!(residual
            .luma_4x4
            .iter()
            .all(|block| { block.as_ref().is_some_and(|block| block.total_coeff == 0) }));
        assert!(residual
            .chroma_dc
            .iter()
            .all(|block| { block.as_ref().is_some_and(|block| block.total_coeff == 0) }));
        assert!(residual.chroma_ac[0][..4].iter().all(Option::is_some));
        assert!(residual.chroma_ac[1][..4].iter().all(Option::is_some));
        assert_eq!(residual.end_bit, 43);
    }

    #[test]
    fn traverses_i4x4_groups_and_updates_the_non_zero_grid() {
        let mut writer = BitWriter::new();
        writer.put(0b11, 2); // block 0 is empty in context 1 (nC=3)
        for _ in 1..16 {
            writer.put(1, 1);
        }
        let data = writer.finish();
        let mb = ParsedIMacroblock {
            kind: IntraMacroblockKind::I4x4,
            luma_prediction_modes: vec![2; 16],
            chroma_prediction_mode: Some(0),
            coded_block_pattern: 15,
            qp_delta: 0,
            pcm_planes: Vec::new(),
            residual_bit_offset: 0,
        };
        let mut neighbors = MacroblockNeighbors::unavailable();
        neighbors.luma_left[0] = Some(4);
        neighbors.luma_top[0] = Some(2);
        let residual = parse_i_macroblock_residual(&data, &mb, context(1), &neighbors).unwrap();
        assert!(residual.luma_4x4.iter().all(Option::is_some));
        assert_eq!(residual.luma_non_zero, [0; 16]);
        assert_eq!(residual.end_bit, 17);
    }

    #[test]
    fn i16_luma_dc_uses_its_own_neighbour_context() {
        let mb = ParsedIMacroblock {
            kind: IntraMacroblockKind::I16x16 { prediction_mode: 2 },
            luma_prediction_modes: vec![2; 16],
            chroma_prediction_mode: Some(0),
            coded_block_pattern: 0,
            qp_delta: 0,
            pcm_planes: Vec::new(),
            residual_bit_offset: 0,
        };
        let mut neighbors = MacroblockNeighbors::unavailable();
        neighbors.luma_dc_left = Some(4);
        neighbors.luma_dc_top = Some(2);
        // nC=3 selects context 1, whose empty coeff_token is 11.
        let residual =
            parse_i_macroblock_residual(&[0b1100_0000], &mb, context(1), &neighbors).unwrap();
        assert_eq!(residual.luma_dc.as_ref().unwrap().total_coeff, 0);
        assert_eq!(residual.luma_dc_non_zero, 0);
        assert_eq!(residual.end_bit, 2);
    }

    #[test]
    fn traverses_four_i8x8_groups_through_one_bit_cursor() {
        let mut writer = BitWriter::new();
        for _ in 0..16 {
            writer.put(1, 1);
        }
        let data = writer.finish();
        let mb = ParsedIMacroblock {
            kind: IntraMacroblockKind::I8x8,
            luma_prediction_modes: vec![2; 4],
            chroma_prediction_mode: Some(0),
            coded_block_pattern: 15,
            qp_delta: 0,
            pcm_planes: Vec::new(),
            residual_bit_offset: 0,
        };
        let residual = parse_i_macroblock_residual(
            &data,
            &mb,
            context(1),
            &MacroblockNeighbors::unavailable(),
        )
        .unwrap();
        assert!(residual.luma_8x8.iter().all(Option::is_some));
        assert_eq!(residual.end_bit, 16);
    }

    #[test]
    fn chroma_dc_only_cbp_does_not_consume_chroma_ac_blocks() {
        let mut writer = BitWriter::new();
        writer.put(0b01, 2); // Cb DC total_coeff=0
        writer.put(0b01, 2); // Cr DC total_coeff=0
        let data = writer.finish();
        let mb = ParsedIMacroblock {
            kind: IntraMacroblockKind::I4x4,
            luma_prediction_modes: vec![2; 16],
            chroma_prediction_mode: Some(0),
            coded_block_pattern: 0x10,
            qp_delta: 0,
            pcm_planes: Vec::new(),
            residual_bit_offset: 0,
        };
        let residual = parse_i_macroblock_residual(
            &data,
            &mb,
            context(1),
            &MacroblockNeighbors::unavailable(),
        )
        .unwrap();
        assert!(residual.luma_4x4.iter().all(Option::is_none));
        assert!(residual.chroma_dc.iter().all(Option::is_some));
        assert!(residual.chroma_ac.iter().flatten().all(Option::is_none));
        assert_eq!(residual.end_bit, 4);
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
