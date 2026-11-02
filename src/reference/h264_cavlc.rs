//! H.264 CAVLC residual syntax decoder.
//!
//! This module decodes the bitstream portion of one residual block through
//! `run_before`. It deliberately stops before inverse scan, dequantisation and
//! transform, which live in [`super::h264`]. The data tables are the CAVLC
//! tables from ITU-T H.264 §9.2; the level-prefix rules follow the same section.

use std::fmt;

use crate::codec::h264::parser::BitReader;

/// The syntax class of one CAVLC residual block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CavlcBlockKind {
    /// Luma AC/DC and chroma AC blocks. `max_coeff` is 16 normally and 15 for
    /// the luma AC block following an Intra16x16 luma DC block.
    Regular { n_c: i16, max_coeff: u8 },
    /// 4:2:0 chroma DC, with four coefficients.
    ChromaDc420,
    /// 4:2:2 chroma DC, with eight coefficients.
    ChromaDc422,
}

impl CavlcBlockKind {
    fn max_coeff(self) -> Result<usize, CavlcError> {
        match self {
            CavlcBlockKind::Regular { max_coeff, .. } if (1..=16).contains(&max_coeff) => {
                Ok(usize::from(max_coeff))
            }
            CavlcBlockKind::Regular { .. } => Err(CavlcError::InvalidMaxCoeff),
            CavlcBlockKind::ChromaDc420 => Ok(4),
            CavlcBlockKind::ChromaDc422 => Ok(8),
        }
    }
}

/// One decoded residual in scan order, ready for inverse scan and dequant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CavlcResidual {
    pub scan: [i32; 16],
    pub total_coeff: usize,
    pub trailing_ones: usize,
    pub bits_consumed: usize,
}

/// Four consecutive CAVLC residual partitions that form one 8x8 transform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cavlc8x8Residual {
    pub scan: [i32; 64],
    pub total_coeff: [usize; 4],
    pub trailing_ones: [usize; 4],
    pub block_bits: [usize; 4],
    pub bits_consumed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CavlcError {
    Truncated,
    InvalidMaxCoeff,
    InvalidNonZeroCount,
    InvalidCoeffToken,
    InvalidLevelPrefix,
    InvalidTotalZeros,
    InvalidRunBefore,
    TotalCoeffExceedsBlock,
    ZeroCountExceedsBlock,
}

impl fmt::Display for CavlcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CavlcError::Truncated => write!(f, "CAVLC bitstream truncated"),
            CavlcError::InvalidMaxCoeff => write!(f, "CAVLC max_coeff is outside 1..=16"),
            CavlcError::InvalidNonZeroCount => {
                write!(f, "CAVLC predicted non-zero count is outside -1..=16")
            }
            CavlcError::InvalidCoeffToken => write!(f, "invalid CAVLC coeff_token"),
            CavlcError::InvalidLevelPrefix => write!(f, "invalid CAVLC level_prefix"),
            CavlcError::InvalidTotalZeros => write!(f, "invalid CAVLC total_zeros"),
            CavlcError::InvalidRunBefore => write!(f, "invalid CAVLC run_before"),
            CavlcError::TotalCoeffExceedsBlock => {
                write!(f, "CAVLC total_coeff exceeds the block capacity")
            }
            CavlcError::ZeroCountExceedsBlock => {
                write!(f, "CAVLC zero count exceeds the block capacity")
            }
        }
    }
}

impl std::error::Error for CavlcError {}

/// Decode one CAVLC residual block from `data`.
///
/// `data` starts at the block's `coeff_token` and may contain following
/// syntax; `bits_consumed` identifies the exact boundary.
pub fn decode_residual_4x4(data: &[u8], kind: CavlcBlockKind) -> Result<CavlcResidual, CavlcError> {
    let mut reader = BitReader::new(data);
    decode_residual(&mut reader, kind)
}

/// Decode the four CAVLC residual partitions belonging to one 8x8 transform.
///
/// H.264 still codes each partition with the 4x4 coefficient-token syntax.
/// They are concatenated at the bit level in `n_c` order; the returned scan is
/// ready for [`super::h264::inverse_scan_8x8_cavlc`].
pub fn decode_residual_8x8(data: &[u8], n_c: [i16; 4]) -> Result<Cavlc8x8Residual, CavlcError> {
    let mut reader = BitReader::new(data);
    let mut scan = [0i32; 64];
    let mut total_coeff = [0usize; 4];
    let mut trailing_ones = [0usize; 4];
    let mut block_bits = [0usize; 4];
    for (partition, predicted) in n_c.into_iter().enumerate() {
        let block = decode_residual(
            &mut reader,
            CavlcBlockKind::Regular {
                n_c: predicted,
                max_coeff: 16,
            },
        )?;
        scan[partition * 16..partition * 16 + 16].copy_from_slice(&block.scan);
        total_coeff[partition] = block.total_coeff;
        trailing_ones[partition] = block.trailing_ones;
        block_bits[partition] = block.bits_consumed;
    }
    Ok(Cavlc8x8Residual {
        scan,
        total_coeff,
        trailing_ones,
        block_bits,
        bits_consumed: reader.bit_pos(),
    })
}

fn decode_residual(
    reader: &mut BitReader<'_>,
    kind: CavlcBlockKind,
) -> Result<CavlcResidual, CavlcError> {
    let start = reader.bit_pos();
    let max_coeff = kind.max_coeff()?;
    let (total_coeff, trailing_ones) = decode_coeff_token(reader, kind)?;
    if total_coeff > max_coeff {
        return Err(CavlcError::TotalCoeffExceedsBlock);
    }
    if total_coeff == 0 {
        return Ok(CavlcResidual {
            scan: [0; 16],
            total_coeff,
            trailing_ones,
            bits_consumed: reader.bit_pos() - start,
        });
    }

    let levels = decode_levels(reader, total_coeff, trailing_ones)?;
    let total_zeros = if total_coeff == max_coeff {
        0
    } else {
        decode_total_zeros(reader, kind, total_coeff)?
    };
    if total_zeros + total_coeff > max_coeff {
        return Err(CavlcError::ZeroCountExceedsBlock);
    }

    let mut scan = [0i32; 16];
    let mut zeros_left = total_zeros;
    let mut position = zeros_left + total_coeff - 1;
    scan[position] = levels[0];

    let mut coefficient = 1;
    while coefficient < total_coeff && zeros_left > 0 {
        let run_before = decode_run_before(reader, zeros_left)?;
        if run_before > zeros_left {
            return Err(CavlcError::InvalidRunBefore);
        }
        zeros_left -= run_before;
        let advance = run_before + 1;
        position = position
            .checked_sub(advance)
            .ok_or(CavlcError::InvalidRunBefore)?;
        scan[position] = levels[coefficient];
        coefficient += 1;
    }
    while coefficient < total_coeff {
        position = position
            .checked_sub(1)
            .ok_or(CavlcError::InvalidRunBefore)?;
        scan[position] = levels[coefficient];
        coefficient += 1;
    }

    Ok(CavlcResidual {
        scan,
        total_coeff,
        trailing_ones,
        bits_consumed: reader.bit_pos() - start,
    })
}

fn decode_coeff_token(
    reader: &mut BitReader<'_>,
    kind: CavlcBlockKind,
) -> Result<(usize, usize), CavlcError> {
    let (lengths, bits): (&[u8], &[u8]) = match kind {
        CavlcBlockKind::ChromaDc420 => (&CHROMA_DC_COEFF_TOKEN_LEN, &CHROMA_DC_COEFF_TOKEN_BITS),
        CavlcBlockKind::ChromaDc422 => (
            &CHROMA422_DC_COEFF_TOKEN_LEN,
            &CHROMA422_DC_COEFF_TOKEN_BITS,
        ),
        CavlcBlockKind::Regular { n_c, .. } => {
            if !(-1..=16).contains(&n_c) {
                return Err(CavlcError::InvalidNonZeroCount);
            }
            let context = match n_c {
                ..=1 => 0,
                2..=3 => 1,
                4..=7 => 2,
                _ => 3,
            };
            (&COEFF_TOKEN_LEN[context], &COEFF_TOKEN_BITS[context])
        }
    };
    let symbol = decode_vlc(reader, lengths, bits)?.ok_or(CavlcError::InvalidCoeffToken)?;
    Ok((symbol >> 2, symbol & 3))
}

fn decode_levels(
    reader: &mut BitReader<'_>,
    total_coeff: usize,
    trailing_ones: usize,
) -> Result<[i32; 16], CavlcError> {
    debug_assert!(total_coeff <= 16);
    debug_assert!(trailing_ones <= 3 && trailing_ones <= total_coeff);
    let mut levels = [0i32; 16];
    for level in levels.iter_mut().take(trailing_ones) {
        *level = if read_bit(reader)? == 0 { 1 } else { -1 };
    }
    if trailing_ones == total_coeff {
        return Ok(levels);
    }

    let mut suffix_length = if total_coeff > 10 && trailing_ones < 3 {
        0usize
    } else {
        1usize
    };

    for index in trailing_ones..total_coeff {
        let level_prefix = read_level_prefix(reader)?;
        let mut level_code = if index == trailing_ones {
            let code = if level_prefix < 14 {
                if suffix_length == 0 {
                    u64::from(level_prefix)
                } else {
                    (u64::from(level_prefix) << 1) | u64::from(read_bits(reader, 1)?)
                }
            } else if level_prefix == 14 {
                if suffix_length == 0 {
                    14 + u64::from(read_bits(reader, 4)?)
                } else {
                    (14u64 << 1) | u64::from(read_bits(reader, 1)?)
                }
            } else {
                first_level_code_with_prefix(reader, level_prefix)?
            };
            let code = if trailing_ones < 3 { code + 2 } else { code };
            suffix_length = 2;
            code
        } else if level_prefix < 15 {
            let suffix = read_bits(reader, suffix_length as u32)?;
            (u64::from(level_prefix) << suffix_length) | u64::from(suffix)
        } else {
            level_code_with_prefix(reader, level_prefix, suffix_length)?
        };
        levels[index] = signed_level(&mut level_code)?;

        if index != trailing_ones {
            let limit = SUFFIX_LIMIT[suffix_length];
            if u64::from(levels[index].unsigned_abs()) > limit {
                suffix_length += 1;
            }
        }
    }
    Ok(levels)
}

fn first_level_code_with_prefix(
    reader: &mut BitReader<'_>,
    level_prefix: u32,
) -> Result<u64, CavlcError> {
    if !(15..=28).contains(&level_prefix) {
        return Err(CavlcError::InvalidLevelPrefix);
    }
    let mut code = 30u64;
    if level_prefix >= 16 {
        code += (1u64 << (level_prefix - 3)) - 4096;
    }
    code += u64::from(read_bits(reader, level_prefix - 3)?);
    Ok(code)
}

fn level_code_with_prefix(
    reader: &mut BitReader<'_>,
    level_prefix: u32,
    suffix_length: usize,
) -> Result<u64, CavlcError> {
    if !(15..=28).contains(&level_prefix) {
        return Err(CavlcError::InvalidLevelPrefix);
    }
    let mut code = 15u64 << suffix_length;
    if level_prefix >= 16 {
        code += (1u64 << (level_prefix - 3)) - 4096;
    }
    code += u64::from(read_bits(reader, level_prefix - 3)?);
    Ok(code)
}

fn signed_level(level_code: &mut u64) -> Result<i32, CavlcError> {
    let negative = *level_code & 1 != 0;
    let magnitude = (*level_code + 2) >> 1;
    let signed = if negative {
        -i64::try_from(magnitude).map_err(|_| CavlcError::InvalidLevelPrefix)?
    } else {
        i64::try_from(magnitude).map_err(|_| CavlcError::InvalidLevelPrefix)?
    };
    i32::try_from(signed).map_err(|_| CavlcError::InvalidLevelPrefix)
}

fn read_level_prefix(reader: &mut BitReader<'_>) -> Result<u32, CavlcError> {
    let mut prefix = 0u32;
    while read_bit(reader)? == 0 {
        prefix += 1;
        if prefix > 28 {
            return Err(CavlcError::InvalidLevelPrefix);
        }
    }
    Ok(prefix)
}

fn decode_total_zeros(
    reader: &mut BitReader<'_>,
    kind: CavlcBlockKind,
    total_coeff: usize,
) -> Result<usize, CavlcError> {
    let (lengths, bits) = match kind {
        CavlcBlockKind::ChromaDc420 => {
            if !(1..=3).contains(&total_coeff) {
                return Err(CavlcError::InvalidTotalZeros);
            }
            (
                &CHROMA_DC_TOTAL_ZEROS_LEN[total_coeff - 1][..],
                &CHROMA_DC_TOTAL_ZEROS_BITS[total_coeff - 1][..],
            )
        }
        CavlcBlockKind::ChromaDc422 => {
            if !(1..=7).contains(&total_coeff) {
                return Err(CavlcError::InvalidTotalZeros);
            }
            (
                &CHROMA422_DC_TOTAL_ZEROS_LEN[total_coeff - 1][..],
                &CHROMA422_DC_TOTAL_ZEROS_BITS[total_coeff - 1][..],
            )
        }
        CavlcBlockKind::Regular { .. } => {
            if !(1..=15).contains(&total_coeff) {
                return Err(CavlcError::InvalidTotalZeros);
            }
            (
                &TOTAL_ZEROS_LEN[total_coeff - 1][..],
                &TOTAL_ZEROS_BITS[total_coeff - 1][..],
            )
        }
    };
    decode_vlc(reader, lengths, bits)?.ok_or(CavlcError::InvalidTotalZeros)
}

fn decode_run_before(reader: &mut BitReader<'_>, zeros_left: usize) -> Result<usize, CavlcError> {
    if zeros_left == 0 {
        return Err(CavlcError::InvalidRunBefore);
    }
    let row = (zeros_left - 1).min(6);
    decode_vlc(reader, &RUN_LEN[row], &RUN_BITS[row])?.ok_or(CavlcError::InvalidRunBefore)
}

fn decode_vlc(
    reader: &mut BitReader<'_>,
    lengths: &[u8],
    bits: &[u8],
) -> Result<Option<usize>, CavlcError> {
    debug_assert_eq!(lengths.len(), bits.len());
    let max_length = lengths.iter().copied().max().unwrap_or(0);
    let mut code = 0u32;
    for length in 1..=u32::from(max_length) {
        code = (code << 1) | read_bits(reader, 1)?;
        for (symbol, (&symbol_length, &symbol_bits)) in lengths.iter().zip(bits).enumerate() {
            if u32::from(symbol_length) == length && u32::from(symbol_bits) == code {
                return Ok(Some(symbol));
            }
        }
    }
    Ok(None)
}

fn read_bit(reader: &mut BitReader<'_>) -> Result<u32, CavlcError> {
    reader.read_bit().map_err(|_| CavlcError::Truncated)
}

fn read_bits(reader: &mut BitReader<'_>, count: u32) -> Result<u32, CavlcError> {
    reader.read_bits(count).map_err(|_| CavlcError::Truncated)
}

const SUFFIX_LIMIT: [u64; 7] = [0, 3, 6, 12, 24, 48, u64::MAX];

const CHROMA_DC_COEFF_TOKEN_LEN: [u8; 20] =
    [2, 0, 0, 0, 6, 1, 0, 0, 6, 6, 3, 0, 6, 7, 7, 6, 6, 8, 8, 7];

const CHROMA_DC_COEFF_TOKEN_BITS: [u8; 20] =
    [1, 0, 0, 0, 7, 1, 0, 0, 4, 6, 1, 0, 3, 3, 2, 5, 2, 3, 2, 0];

const CHROMA422_DC_COEFF_TOKEN_LEN: [u8; 36] = [
    1, 0, 0, 0, 7, 2, 0, 0, 7, 7, 3, 0, 9, 7, 7, 5, 9, 9, 7, 6, 10, 10, 9, 7, 11, 11, 10, 7, 12,
    12, 11, 10, 13, 12, 12, 11,
];

const CHROMA422_DC_COEFF_TOKEN_BITS: [u8; 36] = [
    1, 0, 0, 0, 15, 1, 0, 0, 14, 13, 1, 0, 7, 12, 11, 1, 6, 5, 10, 1, 7, 6, 4, 9, 7, 6, 5, 8, 7, 6,
    5, 4, 7, 5, 4, 4,
];

const COEFF_TOKEN_LEN: [[u8; 68]; 4] = [
    [
        1, 0, 0, 0, 6, 2, 0, 0, 8, 6, 3, 0, 9, 8, 7, 5, 10, 9, 8, 6, 11, 10, 9, 7, 13, 11, 10, 8,
        13, 13, 11, 9, 13, 13, 13, 10, 14, 14, 13, 11, 14, 14, 14, 13, 15, 15, 14, 14, 15, 15, 15,
        14, 16, 15, 15, 15, 16, 16, 16, 15, 16, 16, 16, 16, 16, 16, 16, 16,
    ],
    [
        2, 0, 0, 0, 6, 2, 0, 0, 6, 5, 3, 0, 7, 6, 6, 4, 8, 6, 6, 4, 8, 7, 7, 5, 9, 8, 8, 6, 11, 9,
        9, 6, 11, 11, 11, 7, 12, 11, 11, 9, 12, 12, 12, 11, 12, 12, 12, 11, 13, 13, 13, 12, 13, 13,
        13, 13, 13, 14, 13, 13, 14, 14, 14, 13, 14, 14, 14, 14,
    ],
    [
        4, 0, 0, 0, 6, 4, 0, 0, 6, 5, 4, 0, 6, 5, 5, 4, 7, 5, 5, 4, 7, 5, 5, 4, 7, 6, 6, 4, 7, 6,
        6, 4, 8, 7, 7, 5, 8, 8, 7, 6, 9, 8, 8, 7, 9, 9, 8, 8, 9, 9, 9, 8, 10, 9, 9, 9, 10, 10, 10,
        10, 10, 10, 10, 10, 10, 10, 10, 10,
    ],
    [
        6, 0, 0, 0, 6, 6, 0, 0, 6, 6, 6, 0, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
        6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
        6, 6, 6, 6, 6, 6, 6, 6,
    ],
];

const COEFF_TOKEN_BITS: [[u8; 68]; 4] = [
    [
        1, 0, 0, 0, 5, 1, 0, 0, 7, 4, 1, 0, 7, 6, 5, 3, 7, 6, 5, 3, 7, 6, 5, 4, 15, 6, 5, 4, 11,
        14, 5, 4, 8, 10, 13, 4, 15, 14, 9, 4, 11, 10, 13, 12, 15, 14, 9, 12, 11, 10, 13, 8, 15, 1,
        9, 12, 11, 14, 13, 8, 7, 10, 9, 12, 4, 6, 5, 8,
    ],
    [
        3, 0, 0, 0, 11, 2, 0, 0, 7, 7, 3, 0, 7, 10, 9, 5, 7, 6, 5, 4, 4, 6, 5, 6, 7, 6, 5, 8, 15,
        6, 5, 4, 11, 14, 13, 4, 15, 10, 9, 4, 11, 14, 13, 12, 8, 10, 9, 8, 15, 14, 13, 12, 11, 10,
        9, 12, 7, 11, 6, 8, 9, 8, 10, 1, 7, 6, 5, 4,
    ],
    [
        15, 0, 0, 0, 15, 14, 0, 0, 11, 15, 13, 0, 8, 12, 14, 12, 15, 10, 11, 11, 11, 8, 9, 10, 9,
        14, 13, 9, 8, 10, 9, 8, 15, 14, 13, 13, 11, 14, 10, 12, 15, 10, 13, 12, 11, 14, 9, 12, 8,
        10, 13, 8, 13, 7, 9, 12, 9, 12, 11, 10, 5, 8, 7, 6, 1, 4, 3, 2,
    ],
    [
        3, 0, 0, 0, 0, 1, 0, 0, 4, 5, 6, 0, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
        22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44,
        45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63,
    ],
];

const TOTAL_ZEROS_LEN: [&[u8]; 15] = [
    &[1, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 9],
    &[3, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 6, 6, 6, 6],
    &[4, 3, 3, 3, 4, 4, 3, 3, 4, 5, 5, 6, 5, 6],
    &[5, 3, 4, 4, 3, 3, 3, 4, 3, 4, 5, 5, 5],
    &[4, 4, 4, 3, 3, 3, 3, 3, 4, 5, 4, 5],
    &[6, 5, 3, 3, 3, 3, 3, 3, 4, 3, 6],
    &[6, 5, 3, 3, 3, 2, 3, 4, 3, 6],
    &[6, 4, 5, 3, 2, 2, 3, 3, 6],
    &[6, 6, 4, 2, 2, 3, 2, 5],
    &[5, 5, 3, 2, 2, 2, 4],
    &[4, 4, 3, 3, 1, 3],
    &[4, 4, 2, 1, 3],
    &[3, 3, 1, 2],
    &[2, 2, 1],
    &[1, 1],
];

const TOTAL_ZEROS_BITS: [&[u8]; 15] = [
    &[1, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 1],
    &[7, 6, 5, 4, 3, 5, 4, 3, 2, 3, 2, 3, 2, 1, 0],
    &[5, 7, 6, 5, 4, 3, 4, 3, 2, 3, 2, 1, 1, 0],
    &[3, 7, 5, 4, 6, 5, 4, 3, 3, 2, 2, 1, 0],
    &[5, 4, 3, 7, 6, 5, 4, 3, 2, 1, 1, 0],
    &[1, 1, 7, 6, 5, 4, 3, 2, 1, 1, 0],
    &[1, 1, 5, 4, 3, 3, 2, 1, 1, 0],
    &[1, 1, 1, 3, 3, 2, 2, 1, 0],
    &[1, 0, 1, 3, 2, 1, 1, 1],
    &[1, 0, 1, 3, 2, 1, 1],
    &[0, 1, 1, 2, 1, 3],
    &[0, 1, 1, 1, 1],
    &[0, 1, 1, 1],
    &[0, 1, 1],
    &[0, 1],
];

const CHROMA_DC_TOTAL_ZEROS_LEN: [[u8; 4]; 3] = [[1, 2, 3, 3], [1, 2, 2, 0], [1, 1, 0, 0]];

const CHROMA_DC_TOTAL_ZEROS_BITS: [[u8; 4]; 3] = [[1, 1, 1, 0], [1, 1, 0, 0], [1, 0, 0, 0]];

const CHROMA422_DC_TOTAL_ZEROS_LEN: [[u8; 8]; 7] = [
    [1, 3, 3, 4, 4, 4, 5, 5],
    [3, 2, 3, 3, 3, 3, 3, 0],
    [3, 3, 2, 2, 3, 3, 0, 0],
    [3, 2, 2, 2, 3, 0, 0, 0],
    [2, 2, 2, 2, 0, 0, 0, 0],
    [2, 2, 1, 0, 0, 0, 0, 0],
    [1, 1, 0, 0, 0, 0, 0, 0],
];

const CHROMA422_DC_TOTAL_ZEROS_BITS: [[u8; 8]; 7] = [
    [1, 2, 3, 2, 3, 1, 1, 0],
    [0, 1, 1, 4, 5, 6, 7, 0],
    [0, 1, 1, 2, 6, 7, 0, 0],
    [6, 0, 1, 2, 7, 0, 0, 0],
    [0, 1, 2, 3, 0, 0, 0, 0],
    [0, 1, 1, 0, 0, 0, 0, 0],
    [0, 1, 0, 0, 0, 0, 0, 0],
];

const RUN_LEN: [[u8; 16]; 7] = [
    [1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 2, 2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 2, 2, 3, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 2, 3, 3, 3, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 3, 3, 3, 3, 3, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [3, 3, 3, 3, 3, 3, 3, 4, 5, 6, 7, 8, 9, 10, 11, 0],
];

const RUN_BITS: [[u8; 16]; 7] = [
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [3, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [3, 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [3, 2, 3, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [3, 0, 1, 3, 2, 5, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [7, 6, 5, 4, 3, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0],
];

#[cfg(test)]
mod tests {
    use super::*;

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

        fn push(&mut self, value: u32, count: u32) {
            for shift in (0..count).rev() {
                if self.bits % 8 == 0 {
                    self.bytes.push(0);
                }
                let bit = ((value >> shift) & 1) as u8;
                let byte = self.bytes.last_mut().unwrap();
                *byte |= bit << (7 - self.bits % 8);
                self.bits += 1;
            }
        }

        fn finish(self) -> Vec<u8> {
            self.bytes
        }
    }

    #[test]
    fn zero_run_decodes_from_all_coeff_token_table_families() {
        let regular = decode_residual_4x4(
            &[0b1000_0000],
            CavlcBlockKind::Regular {
                n_c: -1,
                max_coeff: 16,
            },
        )
        .unwrap();
        assert_eq!(regular.total_coeff, 0);
        assert_eq!(regular.bits_consumed, 1);
        assert_eq!(regular.scan, [0; 16]);

        let chroma420 = decode_residual_4x4(&[0b0100_0000], CavlcBlockKind::ChromaDc420).unwrap();
        assert_eq!(chroma420.total_coeff, 0);
        assert_eq!(chroma420.bits_consumed, 2);

        let chroma422 = decode_residual_4x4(&[0b1000_0000], CavlcBlockKind::ChromaDc422).unwrap();
        assert_eq!(chroma422.total_coeff, 0);
        assert_eq!(chroma422.bits_consumed, 1);
    }

    #[test]
    fn coefficient_token_contexts_follow_the_predicted_non_zero_count() {
        // Context 3 total_coeff=1, trailing_ones=0 is the six-bit code 000000.
        let bits = [0b0000_0000];
        let mut reader = BitReader::new(&bits);
        let (total_coeff, trailing_ones) = decode_coeff_token(
            &mut reader,
            CavlcBlockKind::Regular {
                n_c: 8,
                max_coeff: 16,
            },
        )
        .unwrap();
        assert_eq!(total_coeff, 1);
        assert_eq!(trailing_ones, 0);
    }

    #[test]
    fn trailing_one_sign_and_total_zeros_are_applied() {
        let mut writer = BitWriter::new();
        writer.push(0b01, 2); // total_coeff=1, trailing_ones=1
        writer.push(1, 1); // -1
        writer.push(1, 1); // total_zeros=0
        let residual = decode_residual_4x4(
            &writer.finish(),
            CavlcBlockKind::Regular {
                n_c: 0,
                max_coeff: 16,
            },
        )
        .unwrap();
        assert_eq!(residual.total_coeff, 1);
        assert_eq!(residual.trailing_ones, 1);
        assert_eq!(residual.scan[0], -1);
        assert_eq!(residual.bits_consumed, 4);
    }

    #[test]
    fn run_before_places_coefficients_from_the_end_of_scan_order() {
        let mut writer = BitWriter::new();
        writer.push(0b001, 3); // total_coeff=2, trailing_ones=2
        writer.push(0, 1); // +1
        writer.push(1, 1); // -1
        writer.push(0b110, 3); // total_zeros=1
        writer.push(1, 1); // run_before=0
        let residual = decode_residual_4x4(
            &writer.finish(),
            CavlcBlockKind::Regular {
                n_c: 0,
                max_coeff: 16,
            },
        )
        .unwrap();
        assert_eq!(residual.scan[..3], [0, -1, 1]);
        assert_eq!(residual.bits_consumed, 9);
    }

    #[test]
    fn level_suffix_adapts_after_the_first_non_trailing_level() {
        let mut writer = BitWriter::new();
        writer.push(0b10, 2); // first level +2 at suffix_length=1
        writer.push(0b110, 3); // second level +2 at suffix_length=2
        writer.push(0b0101, 4); // third level -3 at suffix_length=2
        let bits = writer.finish();
        let mut reader = BitReader::new(&bits);
        let levels = decode_levels(&mut reader, 3, 0).unwrap();
        assert_eq!(levels[..3], [2, 2, -3]);
    }

    #[test]
    fn chroma_dc_uses_its_shorter_total_zeros_table() {
        let mut writer = BitWriter::new();
        writer.push(0b000111, 6); // 4:2:0 total_coeff=1, trailing_ones=0
        writer.push(0b10, 2); // first level +2 at suffix_length=1
        writer.push(0b01, 2); // total_zeros=1
        let residual = decode_residual_4x4(&writer.finish(), CavlcBlockKind::ChromaDc420).unwrap();
        assert_eq!(residual.total_coeff, 1);
        assert_eq!(residual.scan[..2], [0, 2]);
    }

    #[test]
    fn first_level_uses_the_full_prefix_branch_instead_of_the_remaining_level_base() {
        let mut writer = BitWriter::new();
        writer.push(0b000101, 6); // context 0: total_coeff=1, trailing_ones=0
        writer.push(0, 15);
        writer.push(1, 1); // level_prefix=15
        writer.push(0, 12); // suffix zero
        writer.push(1, 1); // total_zeros=0
        let residual = decode_residual_4x4(
            &writer.finish(),
            CavlcBlockKind::Regular {
                n_c: 0,
                max_coeff: 16,
            },
        )
        .unwrap();
        assert_eq!(residual.scan[0], 17);
        assert_eq!(residual.bits_consumed, 35);
    }

    #[test]
    fn every_vlc_table_entry_round_trips_to_its_symbol() {
        for (context, (lengths, bits)) in COEFF_TOKEN_LEN
            .iter()
            .zip(COEFF_TOKEN_BITS.iter())
            .enumerate()
        {
            for (symbol, (&length, &code)) in lengths.iter().zip(bits).enumerate() {
                if length == 0 {
                    continue;
                }
                let mut writer = BitWriter::new();
                writer.push(u32::from(code), u32::from(length));
                let bytes = writer.finish();
                let mut reader = BitReader::new(&bytes);
                assert_eq!(
                    decode_vlc(&mut reader, lengths, bits),
                    Ok(Some(symbol)),
                    "coeff_token context {context}, symbol {symbol}"
                );
                assert_eq!(reader.bit_pos(), usize::from(length));
            }
        }

        for (table, (lengths, bits)) in TOTAL_ZEROS_LEN
            .iter()
            .zip(TOTAL_ZEROS_BITS.iter())
            .enumerate()
        {
            check_vlc_table(table, lengths, bits);
        }
        for (table, (lengths, bits)) in RUN_LEN.iter().zip(RUN_BITS.iter()).enumerate() {
            check_vlc_table(table, lengths, bits);
        }
        check_vlc_table(100, &CHROMA_DC_COEFF_TOKEN_LEN, &CHROMA_DC_COEFF_TOKEN_BITS);
        check_vlc_table(
            101,
            &CHROMA422_DC_COEFF_TOKEN_LEN,
            &CHROMA422_DC_COEFF_TOKEN_BITS,
        );
        for (table, (lengths, bits)) in CHROMA_DC_TOTAL_ZEROS_LEN
            .iter()
            .zip(CHROMA_DC_TOTAL_ZEROS_BITS.iter())
            .enumerate()
        {
            check_vlc_table(110 + table, lengths, bits);
        }
        for (table, (lengths, bits)) in CHROMA422_DC_TOTAL_ZEROS_LEN
            .iter()
            .zip(CHROMA422_DC_TOTAL_ZEROS_BITS.iter())
            .enumerate()
        {
            check_vlc_table(120 + table, lengths, bits);
        }
    }

    #[test]
    fn four_empty_8x8_partitions_consume_four_bits() {
        let mut writer = BitWriter::new();
        for _ in 0..4 {
            writer.push(1, 1); // context 0 total_coeff=0
        }
        let residual = decode_residual_8x8(&writer.finish(), [-1; 4]).unwrap();
        assert_eq!(residual.scan, [0; 64]);
        assert_eq!(residual.total_coeff, [0; 4]);
        assert_eq!(residual.trailing_ones, [0; 4]);
        assert_eq!(residual.block_bits, [1; 4]);
        assert_eq!(residual.bits_consumed, 4);
    }

    #[test]
    fn eight_by_eight_partitions_continue_at_the_same_bit_cursor() {
        let mut writer = BitWriter::new();
        writer.push(0b01, 2); // block 0: one trailing one
        writer.push(1, 1); // -1
        writer.push(1, 1); // total_zeros=0
        writer.push(0b001, 3); // block 1: two trailing ones
        writer.push(0b01, 2); // +1, -1
        writer.push(0b110, 3); // total_zeros=1
        writer.push(1, 1); // run_before=0
        writer.push(1, 1); // block 2: empty
        writer.push(0b000101, 6); // block 3: total_coeff=1, trailing_ones=0
        writer.push(0b10, 2); // level +2 at suffix_length=1
        writer.push(1, 1); // total_zeros=0

        let residual = decode_residual_8x8(&writer.finish(), [0; 4]).unwrap();
        assert_eq!(residual.total_coeff, [1, 2, 0, 1]);
        assert_eq!(residual.trailing_ones, [1, 2, 0, 0]);
        assert_eq!(residual.block_bits, [4, 9, 1, 9]);
        assert_eq!(residual.bits_consumed, 23);
        assert_eq!(residual.scan[0], -1);
        assert_eq!(residual.scan[17], -1);
        assert_eq!(residual.scan[18], 1);
        assert_eq!(residual.scan[48], 2);
    }

    fn check_vlc_table(table: usize, lengths: &[u8], bits: &[u8]) {
        for (symbol, (&length, &code)) in lengths.iter().zip(bits).enumerate() {
            if length == 0 {
                continue;
            }
            let mut writer = BitWriter::new();
            writer.push(u32::from(code), u32::from(length));
            let bytes = writer.finish();
            let mut reader = BitReader::new(&bytes);
            assert_eq!(
                decode_vlc(&mut reader, lengths, bits),
                Ok(Some(symbol)),
                "table {table}, symbol {symbol}"
            );
            assert_eq!(reader.bit_pos(), usize::from(length));
        }
    }

    #[test]
    fn malformed_blocks_are_rejected_instead_of_guessed() {
        assert_eq!(
            decode_residual_4x4(
                &[],
                CavlcBlockKind::Regular {
                    n_c: 0,
                    max_coeff: 16,
                },
            ),
            Err(CavlcError::Truncated)
        );
        assert_eq!(
            decode_residual_4x4(
                &[0xff],
                CavlcBlockKind::Regular {
                    n_c: 17,
                    max_coeff: 16,
                },
            ),
            Err(CavlcError::InvalidNonZeroCount)
        );
        assert_eq!(
            decode_residual_4x4(
                &[0x80],
                CavlcBlockKind::Regular {
                    n_c: 0,
                    max_coeff: 0,
                },
            ),
            Err(CavlcError::InvalidMaxCoeff)
        );
    }
}
