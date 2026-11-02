//! H.264 CAVLC residual block reconstruction.
//!
//! This composes the syntax decoder, inverse scan, dequantisation and inverse
//! transform without introducing a second implementation of any stage. Luma
//! DC and chroma DC remain separate because their Hadamard transforms and QP
//! mappings are not ordinary 4x4 residual blocks.

use std::fmt;

use super::h264::{dequant_4x4, dequant_8x8_cavlc, inverse_4x4, inverse_8x8, CavlcScan8x8};
use super::h264_cavlc::{
    decode_residual_4x4, decode_residual_8x8, Cavlc8x8Residual, CavlcError, CavlcResidual,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H264ResidualError {
    Cavlc(CavlcError),
    InvalidMaxCoeff(u8),
    Dequant(&'static str),
}

impl fmt::Display for H264ResidualError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            H264ResidualError::Cavlc(error) => error.fmt(f),
            H264ResidualError::InvalidMaxCoeff(max_coeff) => {
                write!(
                    f,
                    "regular H.264 residual max_coeff must be 15 or 16, got {max_coeff}"
                )
            }
            H264ResidualError::Dequant(error) => write!(f, "H.264 dequantisation failed: {error}"),
        }
    }
}

impl std::error::Error for H264ResidualError {}

impl From<CavlcError> for H264ResidualError {
    fn from(error: CavlcError) -> Self {
        H264ResidualError::Cavlc(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconstructed4x4 {
    pub syntax: CavlcResidual,
    pub coefficients: [i32; 16],
    pub residual: [i32; 16],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconstructed8x8 {
    pub syntax: Cavlc8x8Residual,
    pub coefficients: [i32; 64],
    pub residual: [i32; 64],
}

/// Decode and reconstruct one ordinary CAVLC 4x4 residual block.
///
/// `max_coeff` is 16 for a DC+AC block and 15 for an Intra16x16 luma AC block.
/// The AC-only syntax leaves scan position zero (DC) implicitly zero.
pub fn reconstruct_regular_4x4(
    data: &[u8],
    n_c: i16,
    max_coeff: u8,
    qp: u8,
    bit_depth: u8,
    scaling: Option<&[u8; 16]>,
) -> Result<Reconstructed4x4, H264ResidualError> {
    if !matches!(max_coeff, 15 | 16) {
        return Err(H264ResidualError::InvalidMaxCoeff(max_coeff));
    }
    let syntax = decode_residual_4x4(
        data,
        super::h264_cavlc::CavlcBlockKind::Regular { n_c, max_coeff },
    )?;
    let mut scan = [0i32; 16];
    if max_coeff == 15 {
        scan[1..].copy_from_slice(&syntax.scan[..15]);
    } else {
        scan = syntax.scan;
    }
    let coefficients =
        dequant_4x4(scan, qp, bit_depth, scaling).map_err(H264ResidualError::Dequant)?;
    let residual = inverse_4x4(coefficients);
    Ok(Reconstructed4x4 {
        syntax,
        coefficients,
        residual,
    })
}

/// Decode and reconstruct one ordinary CAVLC-coded 8x8 residual block.
pub fn reconstruct_8x8_cavlc(
    data: &[u8],
    n_c: [i16; 4],
    qp: u8,
    bit_depth: u8,
    scaling: Option<&[u8; 64]>,
    layout: CavlcScan8x8,
) -> Result<Reconstructed8x8, H264ResidualError> {
    let syntax = decode_residual_8x8(data, n_c)?;
    let coefficients = dequant_8x8_cavlc(syntax.scan, qp, bit_depth, scaling, layout)
        .map_err(H264ResidualError::Dequant)?;
    let residual = inverse_8x8(coefficients);
    Ok(Reconstructed8x8 {
        syntax,
        coefficients,
        residual,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_4x4_dc_runs_through_dequant_and_transform() {
        // total_coeff=1, trailing_ones=0, level=+2, total_zeros=0
        let data = [0b0001_0110, 0b1000_0000];
        let block = reconstruct_regular_4x4(&data, 0, 16, 24, 8, None).unwrap();
        assert_eq!(block.syntax.total_coeff, 1);
        assert_eq!(block.syntax.bits_consumed, 9);
        assert_eq!(block.coefficients[0], 320);
        assert_eq!(block.residual, [5; 16]);
    }

    #[test]
    fn ac_only_4x4_inserts_the_implicit_zero_dc() {
        // Same residual syntax, but max_coeff=15 places the level at AC scan 1.
        let data = [0b0001_0110, 0b1000_0000];
        let block = reconstruct_regular_4x4(&data, 0, 15, 24, 8, None).unwrap();
        assert_eq!(block.syntax.scan[0], 2);
        assert_eq!(block.coefficients[1], 416);
        assert_eq!(
            block.residual,
            [7, 3, -3, -6, 7, 3, -3, -6, 7, 3, -3, -6, 7, 3, -3, -6]
        );
    }

    #[test]
    fn explicit_zero_or_invalid_max_coeff_is_rejected_before_parsing() {
        assert_eq!(
            reconstruct_regular_4x4(&[], 0, 0, 24, 8, None),
            Err(H264ResidualError::InvalidMaxCoeff(0))
        );
    }

    #[test]
    fn empty_8x8_partitions_reconstruct_to_a_zero_block() {
        let block =
            reconstruct_8x8_cavlc(&[0b1111_0000], [-1; 4], 24, 8, None, CavlcScan8x8::Frame)
                .unwrap();
        assert_eq!(block.syntax.bits_consumed, 4);
        assert_eq!(block.coefficients, [0; 64]);
        assert_eq!(block.residual, [0; 64]);
    }

    #[test]
    fn four_nonzero_8x8_partitions_reach_the_transform() {
        // Each partition codes one +1 trailing one and no total zeros.
        let data = [0b0101_0101, 0b0101_0101];
        let block = reconstruct_8x8_cavlc(&data, [0; 4], 24, 8, None, CavlcScan8x8::Frame).unwrap();
        assert_eq!(block.syntax.total_coeff, [1; 4]);
        assert_eq!(block.syntax.block_bits, [4; 4]);
        assert_eq!(block.syntax.bits_consumed, 16);
        assert!(block.residual.iter().any(|&sample| sample != 0));
    }
}
