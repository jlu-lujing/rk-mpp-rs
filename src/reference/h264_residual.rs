//! H.264 CAVLC residual block reconstruction.
//!
//! This composes the syntax decoder, inverse scan, dequantisation and inverse
//! transform without introducing a second implementation of any stage. Luma
//! DC and chroma DC remain separate because their Hadamard transforms and QP
//! mappings are not ordinary 4x4 residual blocks.

use std::fmt;

use super::h264::{
    chroma_dc_transform_bypass, chroma_qp, dequant_4x4_layout, dequant_8x8_cavlc,
    dequant_chroma_dc_420, dequant_chroma_dc_422, dequant_luma_dc_4x4, inverse_4x4, inverse_8x8,
    inverse_scan_4x4_layout, inverse_scan_8x8_cavlc, luma_dc_transform_bypass, CavlcScan4x4,
    CavlcScan8x8,
};
use super::h264_cavlc::{
    decode_residual_4x4, decode_residual_8x8, Cavlc8x8Residual, CavlcError, CavlcResidual,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H264ResidualError {
    Cavlc(CavlcError),
    InvalidMaxCoeff(u8),
    InvalidLosslessScan,
    TransformPathRequiresQScaleScan,
    InvalidBitDepth(u8),
    InvalidPlane(&'static str),
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
            H264ResidualError::InvalidLosslessScan => {
                write!(f, "H.264 lossless reconstruction requires a q0 scan")
            }
            H264ResidualError::TransformPathRequiresQScaleScan => {
                write!(f, "H.264 transform reconstruction cannot use a q0 scan")
            }
            H264ResidualError::InvalidBitDepth(bit_depth) => {
                write!(f, "H.264 residual supports 8..=10 bit, got {bit_depth}")
            }
            H264ResidualError::InvalidPlane(what) => {
                write!(f, "invalid H.264 residual plane: {what}")
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

pub struct ResidualPlaneOptions<'a> {
    pub qp: u8,
    pub chroma_qp_offset: [i32; 2],
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    pub transform_bypass: bool,
    pub scan4: CavlcScan4x4,
    pub scan8: CavlcScan8x8,
    pub luma_4x4_scaling: Option<&'a [u8; 16]>,
    pub luma_8x8_scaling: Option<&'a [u8; 64]>,
    pub chroma_4x4_scaling: [Option<&'a [u8; 16]>; 2],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroblockResidualPlanes {
    pub luma: [i32; 256],
    pub chroma: [Vec<i32>; 2],
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
    reconstruct_regular_4x4_with_scan(
        data,
        n_c,
        max_coeff,
        qp,
        bit_depth,
        scaling,
        CavlcScan4x4::Frame,
    )
}

/// Transform-path 4x4 reconstruction with an explicit frame/field scan.
pub fn reconstruct_regular_4x4_with_scan(
    data: &[u8],
    n_c: i16,
    max_coeff: u8,
    qp: u8,
    bit_depth: u8,
    scaling: Option<&[u8; 16]>,
    scan_layout: CavlcScan4x4,
) -> Result<Reconstructed4x4, H264ResidualError> {
    if !matches!(max_coeff, 15 | 16) {
        return Err(H264ResidualError::InvalidMaxCoeff(max_coeff));
    }
    if matches!(scan_layout, CavlcScan4x4::FrameQ0 | CavlcScan4x4::FieldQ0) {
        return Err(H264ResidualError::TransformPathRequiresQScaleScan);
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
    let coefficients = dequant_4x4_layout(scan, qp, bit_depth, scaling, scan_layout)
        .map_err(H264ResidualError::Dequant)?;
    let residual = inverse_4x4(coefficients);
    Ok(Reconstructed4x4 {
        syntax,
        coefficients,
        residual,
    })
}

/// Dequantise one already decoded ordinary 4x4 residual block.
pub fn coefficients_4x4_from_syntax(
    syntax: &CavlcResidual,
    max_coeff: u8,
    qp: u8,
    bit_depth: u8,
    scaling: Option<&[u8; 16]>,
    scan_layout: CavlcScan4x4,
) -> Result<[i32; 16], H264ResidualError> {
    if !matches!(max_coeff, 15 | 16) {
        return Err(H264ResidualError::InvalidMaxCoeff(max_coeff));
    }
    if matches!(scan_layout, CavlcScan4x4::FrameQ0 | CavlcScan4x4::FieldQ0) {
        return Err(H264ResidualError::TransformPathRequiresQScaleScan);
    }
    let mut scan = [0i32; 16];
    if max_coeff == 15 {
        scan[1..].copy_from_slice(&syntax.scan[..15]);
    } else {
        scan = syntax.scan;
    }
    dequant_4x4_layout(scan, qp, bit_depth, scaling, scan_layout)
        .map_err(H264ResidualError::Dequant)
}

/// Reconstruct a transform-bypass 4x4 block at `qscale == 0`.
///
/// H.264 replaces the dequant factors with 64 in bypass mode, so the stored
/// coefficients are exactly the signed residual samples after the q0 scan.
pub fn reconstruct_regular_4x4_lossless(
    data: &[u8],
    n_c: i16,
    max_coeff: u8,
    bit_depth: u8,
    scan_layout: CavlcScan4x4,
) -> Result<Reconstructed4x4, H264ResidualError> {
    if !(8..=10).contains(&bit_depth) {
        return Err(H264ResidualError::InvalidBitDepth(bit_depth));
    }
    if !matches!(scan_layout, CavlcScan4x4::FrameQ0 | CavlcScan4x4::FieldQ0) {
        return Err(H264ResidualError::InvalidLosslessScan);
    }
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
    let coefficients = inverse_scan_4x4_layout(scan, scan_layout);
    Ok(Reconstructed4x4 {
        syntax,
        coefficients,
        residual: coefficients,
    })
}

/// Place already decoded transform-bypass 4x4 levels without dequant/transform.
pub fn coefficients_4x4_lossless_from_syntax(
    syntax: &CavlcResidual,
    max_coeff: u8,
    bit_depth: u8,
    scan_layout: CavlcScan4x4,
) -> Result<[i32; 16], H264ResidualError> {
    if !(8..=10).contains(&bit_depth) {
        return Err(H264ResidualError::InvalidBitDepth(bit_depth));
    }
    if !matches!(scan_layout, CavlcScan4x4::FrameQ0 | CavlcScan4x4::FieldQ0) {
        return Err(H264ResidualError::InvalidLosslessScan);
    }
    if !matches!(max_coeff, 15 | 16) {
        return Err(H264ResidualError::InvalidMaxCoeff(max_coeff));
    }
    let mut scan = [0i32; 16];
    if max_coeff == 15 {
        scan[1..].copy_from_slice(&syntax.scan[..15]);
    } else {
        scan = syntax.scan;
    }
    Ok(inverse_scan_4x4_layout(scan, scan_layout))
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
    if matches!(layout, CavlcScan8x8::FrameQ0 | CavlcScan8x8::FieldQ0) {
        return Err(H264ResidualError::TransformPathRequiresQScaleScan);
    }
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

pub fn coefficients_8x8_from_syntax(
    syntax: &Cavlc8x8Residual,
    qp: u8,
    bit_depth: u8,
    scaling: Option<&[u8; 64]>,
    layout: CavlcScan8x8,
) -> Result<[i32; 64], H264ResidualError> {
    if matches!(layout, CavlcScan8x8::FrameQ0 | CavlcScan8x8::FieldQ0) {
        return Err(H264ResidualError::TransformPathRequiresQScaleScan);
    }
    dequant_8x8_cavlc(syntax.scan, qp, bit_depth, scaling, layout)
        .map_err(H264ResidualError::Dequant)
}

/// Reconstruct a transform-bypass CAVLC 8x8 block at `qscale == 0`.
pub fn reconstruct_8x8_cavlc_lossless(
    data: &[u8],
    n_c: [i16; 4],
    bit_depth: u8,
    layout: CavlcScan8x8,
) -> Result<Reconstructed8x8, H264ResidualError> {
    if !(8..=10).contains(&bit_depth) {
        return Err(H264ResidualError::InvalidBitDepth(bit_depth));
    }
    if !matches!(layout, CavlcScan8x8::FrameQ0 | CavlcScan8x8::FieldQ0) {
        return Err(H264ResidualError::InvalidLosslessScan);
    }
    let syntax = decode_residual_8x8(data, n_c)?;
    let coefficients = inverse_scan_8x8_cavlc(syntax.scan, layout);
    Ok(Reconstructed8x8 {
        syntax,
        coefficients,
        residual: coefficients,
    })
}

pub fn coefficients_8x8_lossless_from_syntax(
    syntax: &Cavlc8x8Residual,
    bit_depth: u8,
    layout: CavlcScan8x8,
) -> Result<[i32; 64], H264ResidualError> {
    if !(8..=10).contains(&bit_depth) {
        return Err(H264ResidualError::InvalidBitDepth(bit_depth));
    }
    if !matches!(layout, CavlcScan8x8::FrameQ0 | CavlcScan8x8::FieldQ0) {
        return Err(H264ResidualError::InvalidLosslessScan);
    }
    Ok(inverse_scan_8x8_cavlc(syntax.scan, layout))
}

/// Assemble a parsed I macroblock's residual blocks into 16x16 luma and chroma planes.
pub fn assemble_i_macroblock_residual(
    macroblock: &super::h264_mb::ParsedIMacroblock,
    residual: &super::h264_mb::ParsedMacroblockResidual,
    context: super::h264_mb::MacroblockContext,
    options: &ResidualPlaneOptions<'_>,
) -> Result<MacroblockResidualPlanes, H264ResidualError> {
    if context.chroma_format_idc == 3 {
        return Err(H264ResidualError::InvalidPlane(
            "4:4:4 residual assembly is not implemented",
        ));
    }
    validate_plane_options(options)?;
    let chroma_geometry = match context.chroma_format_idc {
        0 => None,
        1 => Some((2usize, 2usize)),
        2 => Some((2usize, 4usize)),
        _ => return Err(H264ResidualError::InvalidPlane("chroma_format_idc")),
    };
    let mut planes = MacroblockResidualPlanes {
        luma: [0; 256],
        chroma: [
            vec![0; chroma_geometry.map_or(0, |(w, h)| w * 4 * h * 4)],
            vec![0; chroma_geometry.map_or(0, |(w, h)| w * 4 * h * 4)],
        ],
    };
    if macroblock.kind == super::h264_mb::IntraMacroblockKind::IPcm {
        return Ok(planes);
    }

    match macroblock.kind {
        super::h264_mb::IntraMacroblockKind::I4x4 => {
            for block in 0..16 {
                let Some(syntax) = &residual.luma_4x4[block] else {
                    continue;
                };
                let coefficients = if options.transform_bypass {
                    coefficients_4x4_lossless_from_syntax(
                        syntax,
                        16,
                        options.bit_depth_luma,
                        options.scan4,
                    )?
                } else {
                    coefficients_4x4_from_syntax(
                        syntax,
                        16,
                        options.qp,
                        options.bit_depth_luma,
                        options.luma_4x4_scaling,
                        options.scan4,
                    )?
                };
                let samples = if options.transform_bypass {
                    coefficients
                } else {
                    inverse_4x4(coefficients)
                };
                place_4x4(
                    &mut planes.luma,
                    16,
                    (block % 4) * 4,
                    (block / 4) * 4,
                    samples,
                );
            }
        }
        super::h264_mb::IntraMacroblockKind::I8x8 => {
            for group in 0..4 {
                let Some(syntax) = &residual.luma_8x8[group] else {
                    continue;
                };
                let coefficients = if options.transform_bypass {
                    coefficients_8x8_lossless_from_syntax(
                        syntax,
                        options.bit_depth_luma,
                        options.scan8,
                    )?
                } else {
                    coefficients_8x8_from_syntax(
                        syntax,
                        options.qp,
                        options.bit_depth_luma,
                        options.luma_8x8_scaling,
                        options.scan8,
                    )?
                };
                let samples = if options.transform_bypass {
                    coefficients
                } else {
                    inverse_8x8(coefficients)
                };
                place_8x8(
                    &mut planes.luma,
                    16,
                    (group % 2) * 8,
                    (group / 2) * 8,
                    samples,
                );
            }
        }
        super::h264_mb::IntraMacroblockKind::I16x16 { .. } => {
            let dc = residual
                .luma_dc
                .as_ref()
                .ok_or(H264ResidualError::InvalidPlane("missing I16 luma DC block"))?;
            let dc_values = if options.transform_bypass {
                luma_dc_transform_bypass(dc.scan, options.scan4)
                    .map_err(H264ResidualError::Dequant)?
            } else {
                let scale = options.luma_4x4_scaling.map_or(16, |matrix| matrix[0]);
                dequant_luma_dc_4x4(
                    dc.scan,
                    options.qp,
                    options.bit_depth_luma,
                    scale,
                    options.scan4,
                )
                .map_err(H264ResidualError::Dequant)?
            };
            for block in 0..16 {
                let mut coefficients = if let Some(syntax) = &residual.luma_4x4[block] {
                    if options.transform_bypass {
                        coefficients_4x4_lossless_from_syntax(
                            syntax,
                            15,
                            options.bit_depth_luma,
                            options.scan4,
                        )?
                    } else {
                        coefficients_4x4_from_syntax(
                            syntax,
                            15,
                            options.qp,
                            options.bit_depth_luma,
                            options.luma_4x4_scaling,
                            options.scan4,
                        )?
                    }
                } else {
                    [0; 16]
                };
                coefficients[0] = dc_values[block];
                let samples = if options.transform_bypass {
                    coefficients
                } else {
                    inverse_4x4(coefficients)
                };
                place_4x4(
                    &mut planes.luma,
                    16,
                    (block % 4) * 4,
                    (block / 4) * 4,
                    samples,
                );
            }
        }
        super::h264_mb::IntraMacroblockKind::IPcm => unreachable!(),
    }

    if let Some((grid_width, grid_height)) = chroma_geometry {
        for plane in 0..2 {
            if residual.chroma_ac[plane].iter().any(Option::is_some)
                && residual.chroma_dc[plane].is_none()
            {
                return Err(H264ResidualError::InvalidPlane("chroma AC without DC"));
            }
            let Some(dc) = &residual.chroma_dc[plane] else {
                continue;
            };
            let chroma_qp = chroma_qp(
                i32::from(options.qp),
                options.chroma_qp_offset[plane],
                options.bit_depth_chroma,
            )
            .map_err(H264ResidualError::Dequant)?;
            let block_count = grid_width * grid_height;
            let dc_values = if options.transform_bypass {
                chroma_dc_transform_bypass(&dc.scan[..block_count], options.scan8)
                    .map_err(H264ResidualError::Dequant)?
            } else {
                let scale = options.chroma_4x4_scaling[plane].map_or(16, |matrix| matrix[0]);
                if context.chroma_format_idc == 1 {
                    dequant_chroma_dc_420(
                        dc.scan[..4].try_into().unwrap(),
                        chroma_qp,
                        options.bit_depth_chroma,
                        scale,
                    )
                    .map_err(H264ResidualError::Dequant)?
                    .to_vec()
                } else {
                    dequant_chroma_dc_422(
                        dc.scan[..8].try_into().unwrap(),
                        chroma_qp + 3,
                        options.bit_depth_chroma,
                        scale,
                    )
                    .map_err(H264ResidualError::Dequant)?
                    .to_vec()
                }
            };
            for block in 0..block_count {
                let mut coefficients = if let Some(syntax) = &residual.chroma_ac[plane][block] {
                    if options.transform_bypass {
                        coefficients_4x4_lossless_from_syntax(
                            syntax,
                            15,
                            options.bit_depth_chroma,
                            options.scan4,
                        )?
                    } else {
                        coefficients_4x4_from_syntax(
                            syntax,
                            15,
                            chroma_qp,
                            options.bit_depth_chroma,
                            options.chroma_4x4_scaling[plane],
                            options.scan4,
                        )?
                    }
                } else {
                    [0; 16]
                };
                coefficients[0] = dc_values[block];
                let samples = if options.transform_bypass {
                    coefficients
                } else {
                    inverse_4x4(coefficients)
                };
                place_4x4(
                    &mut planes.chroma[plane],
                    8,
                    (block % grid_width) * 4,
                    (block / grid_width) * 4,
                    samples,
                );
            }
        }
    }
    Ok(planes)
}

fn validate_plane_options(options: &ResidualPlaneOptions<'_>) -> Result<(), H264ResidualError> {
    if !(8..=10).contains(&options.bit_depth_luma) || !(8..=10).contains(&options.bit_depth_chroma)
    {
        return Err(H264ResidualError::InvalidPlane("bit depth"));
    }
    let q0_4 = matches!(options.scan4, CavlcScan4x4::FrameQ0 | CavlcScan4x4::FieldQ0);
    let q0_8 = matches!(options.scan8, CavlcScan8x8::FrameQ0 | CavlcScan8x8::FieldQ0);
    if options.transform_bypass && options.qp != 0 {
        return Err(H264ResidualError::InvalidPlane(
            "transform bypass requires qscale zero",
        ));
    }
    if options.transform_bypass != (q0_4 && q0_8) {
        return Err(H264ResidualError::InvalidPlane(
            "scan layouts do not match transform mode",
        ));
    }
    Ok(())
}

fn place_4x4(destination: &mut [i32], stride: usize, x: usize, y: usize, samples: [i32; 16]) {
    for row in 0..4 {
        destination[(y + row) * stride + x..(y + row) * stride + x + 4]
            .copy_from_slice(&samples[row * 4..row * 4 + 4]);
    }
}

fn place_8x8(destination: &mut [i32], stride: usize, x: usize, y: usize, samples: [i32; 64]) {
    for row in 0..8 {
        destination[(y + row) * stride + x..(y + row) * stride + x + 8]
            .copy_from_slice(&samples[row * 8..row * 8 + 8]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_4x4_dc_runs_through_dequant_and_transform() {
        // total_coeff=1, trailing_ones=0, level=+2, total_zeros=0
        let data = [0b0001_0111];
        let block = reconstruct_regular_4x4(&data, 0, 16, 24, 8, None).unwrap();
        assert_eq!(block.syntax.total_coeff, 1);
        assert_eq!(block.syntax.bits_consumed, 8);
        assert_eq!(block.coefficients[0], 320);
        assert_eq!(block.residual, [5; 16]);
    }

    #[test]
    fn ac_only_4x4_inserts_the_implicit_zero_dc() {
        // Same residual syntax, but max_coeff=15 places the level at AC scan 1.
        let data = [0b0001_0111];
        let block = reconstruct_regular_4x4(&data, 0, 15, 24, 8, None).unwrap();
        assert_eq!(block.syntax.scan[0], 2);
        assert_eq!(block.coefficients[1], 416);
        assert_eq!(
            block.residual,
            [7, 3, -3, -6, 7, 3, -3, -6, 7, 3, -3, -6, 7, 3, -3, -6]
        );
    }

    #[test]
    fn lossless_4x4_uses_the_q0_scan_without_dequant_or_transform() {
        // One +1 coefficient at CAVLC scan position 1.
        let block =
            reconstruct_regular_4x4_lossless(&[0b0100_1100], 0, 16, 8, CavlcScan4x4::FrameQ0)
                .unwrap();
        assert_eq!(block.syntax.bits_consumed, 6);
        assert_eq!(block.coefficients[4], 1);
        assert_eq!(block.residual, block.coefficients);
        assert_eq!(
            block.residual.iter().filter(|&&sample| sample != 0).count(),
            1
        );
    }

    #[test]
    fn lossless_8x8_places_scan_four_at_the_transposed_raster_position() {
        // Partition 0 has one +1 at scan position 4; the other partitions are empty.
        let block = reconstruct_8x8_cavlc_lossless(
            &[0b0100_0101, 0b1100_0000],
            [0; 4],
            8,
            CavlcScan8x8::FrameQ0,
        )
        .unwrap();
        assert_eq!(block.syntax.bits_consumed, 10);
        assert_eq!(block.syntax.total_coeff, [1, 0, 0, 0]);
        assert_eq!(block.coefficients[33], 1);
        assert_eq!(block.residual, block.coefficients);
        assert_eq!(
            block.residual.iter().filter(|&&sample| sample != 0).count(),
            1
        );
    }

    #[test]
    fn lossless_entry_points_reject_transformed_scans_and_bit_depths() {
        assert_eq!(
            reconstruct_regular_4x4_lossless(&[], 0, 16, 8, CavlcScan4x4::Frame),
            Err(H264ResidualError::InvalidLosslessScan)
        );
        assert_eq!(
            reconstruct_8x8_cavlc_lossless(&[], [0; 4], 11, CavlcScan8x8::FrameQ0),
            Err(H264ResidualError::InvalidBitDepth(11))
        );
        assert_eq!(
            reconstruct_regular_4x4_with_scan(&[], 0, 16, 24, 8, None, CavlcScan4x4::FrameQ0),
            Err(H264ResidualError::TransformPathRequiresQScaleScan)
        );
        assert_eq!(
            reconstruct_8x8_cavlc(&[], [0; 4], 24, 8, None, CavlcScan8x8::FrameQ0),
            Err(H264ResidualError::TransformPathRequiresQScaleScan)
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

    #[test]
    fn assembles_i16_dc_across_all_sixteen_luma_blocks() {
        let mut residual = empty_residual();
        let mut dc = [0i32; 16];
        dc[0] = 1;
        residual.luma_dc = Some(CavlcResidual {
            scan: dc,
            total_coeff: 1,
            trailing_ones: 0,
            bits_consumed: 0,
        });
        let macroblock = i_macroblock(
            crate::reference::h264_mb::IntraMacroblockKind::I16x16 { prediction_mode: 2 },
            0,
        );
        let planes = assemble_i_macroblock_residual(
            &macroblock,
            &residual,
            plane_context(1),
            &plane_options(24, false),
        )
        .unwrap();
        assert_eq!(planes.luma, [1; 256]);
        assert_eq!(planes.chroma, [vec![0; 64], vec![0; 64]]);
    }

    #[test]
    fn assembles_i4x4_full_dc_block_at_its_macroblock_position() {
        let mut residual = empty_residual();
        let mut scan = [0i32; 16];
        scan[0] = 4096;
        residual.luma_4x4[5] = Some(CavlcResidual {
            scan,
            total_coeff: 1,
            trailing_ones: 0,
            bits_consumed: 0,
        });
        let macroblock = i_macroblock(crate::reference::h264_mb::IntraMacroblockKind::I4x4, 15);
        let planes = assemble_i_macroblock_residual(
            &macroblock,
            &residual,
            plane_context(1),
            &plane_options(0, false),
        )
        .unwrap();
        for row in 0..4 {
            for column in 0..4 {
                assert_eq!(planes.luma[(row + 4) * 16 + column + 4], 640);
            }
        }
        assert_eq!(
            planes.luma.iter().filter(|&&sample| sample != 0).count(),
            16
        );
    }

    #[test]
    fn assembles_i4x4_block_two_at_the_top_right_position() {
        let mut residual = empty_residual();
        let mut scan = [0i32; 16];
        scan[0] = 4096;
        residual.luma_4x4[2] = Some(CavlcResidual {
            scan,
            total_coeff: 1,
            trailing_ones: 0,
            bits_consumed: 0,
        });
        let macroblock = i_macroblock(crate::reference::h264_mb::IntraMacroblockKind::I4x4, 15);
        let planes = assemble_i_macroblock_residual(
            &macroblock,
            &residual,
            plane_context(1),
            &plane_options(0, false),
        )
        .unwrap();
        for row in 0..4 {
            for column in 0..4 {
                assert_eq!(planes.luma[row * 16 + column + 8], 640);
            }
        }
        assert_eq!(
            planes.luma.iter().filter(|&&sample| sample != 0).count(),
            16
        );
    }

    #[test]
    fn assembles_i8x8_dc_at_the_first_eight_by_eight_group() {
        let mut residual = empty_residual();
        let mut scan = [0i32; 64];
        scan[0] = 128;
        residual.luma_8x8[0] = Some(Cavlc8x8Residual {
            scan,
            total_coeff: [1, 0, 0, 0],
            trailing_ones: [0; 4],
            block_bits: [0; 4],
            bits_consumed: 0,
        });
        let macroblock = i_macroblock(crate::reference::h264_mb::IntraMacroblockKind::I8x8, 15);
        let planes = assemble_i_macroblock_residual(
            &macroblock,
            &residual,
            plane_context(1),
            &plane_options(0, false),
        )
        .unwrap();
        for row in 0..8 {
            for column in 0..8 {
                assert_eq!(planes.luma[row * 16 + column], 10);
            }
        }
        assert_eq!(
            planes.luma.iter().filter(|&&sample| sample != 0).count(),
            64
        );
    }

    #[test]
    fn assembles_chroma_dc_only_blocks_for_both_planes() {
        let mut residual = empty_residual();
        for block in &mut residual.chroma_dc {
            let mut scan = [0i32; 16];
            scan[0] = 1;
            *block = Some(CavlcResidual {
                scan,
                total_coeff: 1,
                trailing_ones: 0,
                bits_consumed: 0,
            });
        }
        let macroblock = i_macroblock(crate::reference::h264_mb::IntraMacroblockKind::I4x4, 0x10);
        let planes = assemble_i_macroblock_residual(
            &macroblock,
            &residual,
            plane_context(1),
            &plane_options(24, false),
        )
        .unwrap();
        assert_eq!(planes.luma, [0; 256]);
        assert_eq!(planes.chroma, [vec![1; 64], vec![1; 64]]);
    }

    #[test]
    fn assembles_chroma_ac_block_one_at_the_top_right_position() {
        let mut residual = empty_residual();
        for block in &mut residual.chroma_dc {
            *block = Some(CavlcResidual {
                scan: [0; 16],
                total_coeff: 0,
                trailing_ones: 0,
                bits_consumed: 0,
            });
        }
        let mut scan = [0i32; 16];
        scan[1] = 7;
        residual.chroma_ac[0][1] = Some(CavlcResidual {
            scan,
            total_coeff: 1,
            trailing_ones: 0,
            bits_consumed: 0,
        });
        let macroblock = i_macroblock(crate::reference::h264_mb::IntraMacroblockKind::I4x4, 0x20);
        let planes = assemble_i_macroblock_residual(
            &macroblock,
            &residual,
            plane_context(1),
            &plane_options(0, true),
        )
        .unwrap();
        assert_eq!(planes.chroma[0][5], 7);
    }

    #[test]
    fn transform_bypass_places_levels_without_inverse_transform() {
        let mut residual = empty_residual();
        let mut scan = [0i32; 16];
        scan[0] = 7;
        residual.luma_4x4[0] = Some(CavlcResidual {
            scan,
            total_coeff: 1,
            trailing_ones: 0,
            bits_consumed: 0,
        });
        let macroblock = i_macroblock(crate::reference::h264_mb::IntraMacroblockKind::I4x4, 15);
        let planes = assemble_i_macroblock_residual(
            &macroblock,
            &residual,
            plane_context(1),
            &plane_options(0, true),
        )
        .unwrap();
        assert_eq!(planes.luma[0], 7);
        assert_eq!(planes.luma.iter().filter(|&&sample| sample != 0).count(), 1);
    }

    fn empty_residual() -> crate::reference::h264_mb::ParsedMacroblockResidual {
        crate::reference::h264_mb::ParsedMacroblockResidual {
            luma_dc: None,
            luma_4x4: std::array::from_fn(|_| None),
            luma_8x8: std::array::from_fn(|_| None),
            chroma_dc: std::array::from_fn(|_| None),
            chroma_ac: std::array::from_fn(|_| std::array::from_fn(|_| None)),
            luma_dc_non_zero: 0,
            luma_non_zero: [0; 16],
            chroma_non_zero: [vec![0; 4], vec![0; 4]],
            end_bit: 0,
        }
    }

    fn i_macroblock(
        kind: crate::reference::h264_mb::IntraMacroblockKind,
        cbp: u8,
    ) -> crate::reference::h264_mb::ParsedIMacroblock {
        crate::reference::h264_mb::ParsedIMacroblock {
            kind,
            luma_prediction_modes: Vec::new(),
            chroma_prediction_mode: Some(0),
            coded_block_pattern: cbp,
            qp_delta: 0,
            pcm_planes: Vec::new(),
            residual_bit_offset: 0,
        }
    }

    fn plane_context(chroma_format_idc: u8) -> crate::reference::h264_mb::MacroblockContext {
        crate::reference::h264_mb::MacroblockContext {
            chroma_format_idc,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            transform_8x8: true,
        }
    }

    fn plane_options(qp: u8, transform_bypass: bool) -> ResidualPlaneOptions<'static> {
        ResidualPlaneOptions {
            qp,
            chroma_qp_offset: [0, 0],
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            transform_bypass,
            scan4: if transform_bypass {
                CavlcScan4x4::FrameQ0
            } else {
                CavlcScan4x4::Frame
            },
            scan8: if transform_bypass {
                CavlcScan8x8::FrameQ0
            } else {
                CavlcScan8x8::Frame
            },
            luma_4x4_scaling: None,
            luma_8x8_scaling: None,
            chroma_4x4_scaling: [None, None],
        }
    }
}
