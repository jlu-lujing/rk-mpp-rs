//! H.264 intra macroblock reconstruction and writeback.
//!
//! The caller provides destination planes that already contain reconstructed
//! neighbours. This layer loads the required boundary samples, predicts the
//! current macroblock, adds the assembled residual plane, clips and writes the
//! result. Slice-level block ordering remains outside this module.

use std::fmt;

use super::h264_mb::{IntraMacroblockKind, ParsedIMacroblock, I4X4_BLOCK_ORDER};
use super::h264_pred::{
    predict_16x16, predict_4x4, predict_8x8, predict_chroma, Intra4x4Neighbors, Intra8x8Neighbors,
    PredictionError,
};
use super::h264_residual::MacroblockResidualPlanes;

pub struct PlaneMut<'a> {
    pub data: &'a mut [u16],
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

#[derive(Debug)]
pub enum ReconstructionError {
    Prediction(PredictionError),
    PlaneTooSmall,
    MacroblockOutOfBounds,
    MissingChroma,
    InvalidPcm,
    Unsupported(&'static str),
}

impl fmt::Display for ReconstructionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReconstructionError::Prediction(error) => error.fmt(f),
            ReconstructionError::PlaneTooSmall => write!(f, "destination plane is too small"),
            ReconstructionError::MacroblockOutOfBounds => {
                write!(f, "macroblock exceeds the destination plane")
            }
            ReconstructionError::MissingChroma => write!(f, "destination chroma plane is missing"),
            ReconstructionError::InvalidPcm => write!(f, "invalid I_PCM plane dimensions"),
            ReconstructionError::Unsupported(what) => {
                write!(f, "unsupported H.264 reconstruction: {what}")
            }
        }
    }
}

impl std::error::Error for ReconstructionError {}

impl From<PredictionError> for ReconstructionError {
    fn from(error: PredictionError) -> Self {
        ReconstructionError::Prediction(error)
    }
}

/// Reconstruct one parsed I macroblock into luma/chroma destination planes.
pub fn reconstruct_i_macroblock(
    macroblock: &ParsedIMacroblock,
    residual: &MacroblockResidualPlanes,
    bit_depth: u8,
    luma: &mut PlaneMut<'_>,
    chroma_cb: Option<&mut PlaneMut<'_>>,
    chroma_cr: Option<&mut PlaneMut<'_>>,
    mb_x: usize,
    mb_y: usize,
) -> Result<(), ReconstructionError> {
    validate_plane(luma)?;
    if mb_x + 16 > luma.width || mb_y + 16 > luma.height {
        return Err(ReconstructionError::MacroblockOutOfBounds);
    }
    if macroblock.kind == IntraMacroblockKind::IPcm {
        write_pcm(macroblock, luma, chroma_cb, chroma_cr, mb_x, mb_y)?;
        return Ok(());
    }

    match macroblock.kind {
        IntraMacroblockKind::I4x4 => {
            let mut decoded_blocks = 0u16;
            for block in I4X4_BLOCK_ORDER {
                let bx = mb_x + (block % 4) * 4;
                let by = mb_y + (block / 4) * 4;
                let row = block / 4;
                let column = block % 4;
                let top_right_available = if row == 0 {
                    mb_y > 0
                } else if column < 3 {
                    let top_right = (row - 1) * 4 + column + 1;
                    decoded_blocks & (1 << top_right) != 0
                } else {
                    false
                };
                let neighbors = gather_4x4(luma, bx, by, bit_depth, top_right_available);
                let prediction = predict_4x4(
                    macroblock.luma_prediction_modes[block],
                    bit_depth,
                    &neighbors,
                )?;
                write_block(
                    luma,
                    bx,
                    by,
                    4,
                    &prediction,
                    &residual.luma,
                    16,
                    block,
                    bit_depth,
                );
                decoded_blocks |= 1 << block;
            }
        }
        IntraMacroblockKind::I8x8 => {
            for block in 0..4 {
                let bx = mb_x + (block % 2) * 8;
                let by = mb_y + (block / 2) * 8;
                let neighbors = gather_8x8(luma, bx, by, bit_depth);
                let prediction = predict_8x8(
                    macroblock.luma_prediction_modes[block],
                    bit_depth,
                    &neighbors,
                )?;
                write_block(
                    luma,
                    bx,
                    by,
                    8,
                    &prediction,
                    &residual.luma,
                    16,
                    block,
                    bit_depth,
                );
            }
        }
        IntraMacroblockKind::I16x16 { prediction_mode } => {
            let top = (mb_y > 0).then(|| read_top(luma, mb_x, mb_y, 16, bit_depth));
            let left = (mb_x > 0).then(|| read_left(luma, mb_x, mb_y, 16, bit_depth));
            let top_left = read_top_left(luma, mb_x, mb_y, bit_depth);
            let prediction = predict_16x16(
                prediction_mode,
                bit_depth,
                top.as_ref(),
                left.as_ref(),
                top_left,
            )?;
            write_rectangle(
                luma,
                mb_x,
                mb_y,
                16,
                16,
                &prediction,
                &residual.luma,
                16,
                bit_depth,
            );
        }
        IntraMacroblockKind::IPcm => unreachable!(),
    }

    if let Some(mode) = macroblock.chroma_prediction_mode {
        let cb = chroma_cb.ok_or(ReconstructionError::MissingChroma)?;
        let cr = chroma_cr.ok_or(ReconstructionError::MissingChroma)?;
        reconstruct_chroma_plane(cb, &residual.chroma[0], mode, bit_depth, mb_x, mb_y)?;
        reconstruct_chroma_plane(cr, &residual.chroma[1], mode, bit_depth, mb_x, mb_y)?;
    }
    Ok(())
}

fn validate_plane(plane: &PlaneMut<'_>) -> Result<(), ReconstructionError> {
    if plane.stride < plane.width || plane.data.len() < plane.stride * plane.height {
        Err(ReconstructionError::PlaneTooSmall)
    } else {
        Ok(())
    }
}

fn gather_4x4(
    plane: &PlaneMut<'_>,
    x: usize,
    y: usize,
    bit_depth: u8,
    top_right_available: bool,
) -> Intra4x4Neighbors {
    let top = (y > 0).then(|| read_top(plane, x, y, 4, bit_depth));
    let left = (x > 0).then(|| read_left(plane, x, y, 4, bit_depth));
    let top_right = if top.is_some() && top_right_available && x + 8 <= plane.width {
        Some(std::array::from_fn(|index| {
            plane.data[(y - 1) * plane.stride + x + 4 + index]
        }))
    } else {
        None
    };
    Intra4x4Neighbors {
        top,
        left,
        top_right,
        top_left: read_top_left(plane, x, y, bit_depth),
    }
}

fn gather_8x8(plane: &PlaneMut<'_>, x: usize, y: usize, bit_depth: u8) -> Intra8x8Neighbors {
    let top = (y > 0).then(|| read_top(plane, x, y, 8, bit_depth));
    let left = (x > 0).then(|| read_left(plane, x, y, 8, bit_depth));
    let top_right_available = top.is_some() && (x % 16 == 0 || (y >= 16 && x + 16 <= plane.width));
    let top_right = if top_right_available {
        Some(std::array::from_fn(|index| {
            plane.data[(y - 1) * plane.stride + x + 8 + index]
        }))
    } else {
        None
    };
    Intra8x8Neighbors {
        top,
        left,
        top_right,
        top_left: read_top_left(plane, x, y, bit_depth),
    }
}

fn read_top<const N: usize>(
    plane: &PlaneMut<'_>,
    x: usize,
    y: usize,
    _count: usize,
    bit_depth: u8,
) -> [u16; N] {
    if y == 0 {
        return [if x > 0 {
            plane.data[x - 1]
        } else {
            mid(bit_depth)
        }; N];
    }
    std::array::from_fn(|index| plane.data[(y - 1) * plane.stride + x + index])
}

fn read_left<const N: usize>(
    plane: &PlaneMut<'_>,
    x: usize,
    y: usize,
    _count: usize,
    bit_depth: u8,
) -> [u16; N] {
    if x == 0 {
        return [if y > 0 {
            plane.data[(y - 1) * plane.stride]
        } else {
            mid(bit_depth)
        }; N];
    }
    std::array::from_fn(|index| plane.data[(y + index) * plane.stride + x - 1])
}

fn read_top_vec(plane: &PlaneMut<'_>, x: usize, y: usize, count: usize, bit_depth: u8) -> Vec<u16> {
    if y == 0 {
        vec![
            if x > 0 {
                plane.data[x - 1]
            } else {
                mid(bit_depth)
            };
            count
        ]
    } else {
        plane.data[(y - 1) * plane.stride + x..(y - 1) * plane.stride + x + count].to_vec()
    }
}

fn read_left_vec(
    plane: &PlaneMut<'_>,
    x: usize,
    y: usize,
    count: usize,
    bit_depth: u8,
) -> Vec<u16> {
    if x == 0 {
        vec![
            if y > 0 {
                plane.data[(y - 1) * plane.stride]
            } else {
                mid(bit_depth)
            };
            count
        ]
    } else {
        (0..count)
            .map(|row| plane.data[(y + row) * plane.stride + x - 1])
            .collect()
    }
}

fn read_top_left(plane: &PlaneMut<'_>, x: usize, y: usize, bit_depth: u8) -> Option<u16> {
    if x > 0 && y > 0 {
        Some(plane.data[(y - 1) * plane.stride + x - 1])
    } else if y > 0 {
        Some(plane.data[(y - 1) * plane.stride])
    } else if x > 0 {
        Some(plane.data[x - 1])
    } else {
        Some(mid(bit_depth))
    }
}

fn write_block(
    plane: &mut PlaneMut<'_>,
    x: usize,
    y: usize,
    size: usize,
    prediction: &[u16],
    residual: &[i32],
    residual_stride: usize,
    block: usize,
    bit_depth: u8,
) {
    let blocks_per_row = 16 / size;
    let rx = (block % blocks_per_row) * size;
    let ry = (block / blocks_per_row) * size;
    for row in 0..size {
        for column in 0..size {
            let value = i32::from(prediction[row * size + column])
                + residual[(ry + row) * residual_stride + rx + column];
            plane.data[(y + row) * plane.stride + x + column] = clip(value, bit_depth);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn write_rectangle(
    plane: &mut PlaneMut<'_>,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    prediction: &[u16],
    residual: &[i32],
    residual_stride: usize,
    bit_depth: u8,
) {
    for row in 0..height {
        for column in 0..width {
            let value = i32::from(prediction[row * width + column])
                + residual[row * residual_stride + column];
            plane.data[(y + row) * plane.stride + x + column] = clip(value, bit_depth);
        }
    }
}

fn write_pcm(
    macroblock: &ParsedIMacroblock,
    luma: &mut PlaneMut<'_>,
    chroma_cb: Option<&mut PlaneMut<'_>>,
    chroma_cr: Option<&mut PlaneMut<'_>>,
    mb_x: usize,
    mb_y: usize,
) -> Result<(), ReconstructionError> {
    if macroblock.pcm_planes.is_empty() || macroblock.pcm_planes[0].len() != 256 {
        return Err(ReconstructionError::InvalidPcm);
    }
    copy_plane_block(luma, mb_x, mb_y, 16, 16, &macroblock.pcm_planes[0])?;
    if macroblock.pcm_planes.len() == 1 {
        return Ok(());
    }
    let cb = chroma_cb.ok_or(ReconstructionError::MissingChroma)?;
    let cr = chroma_cr.ok_or(ReconstructionError::MissingChroma)?;
    write_pcm_plane(cb, &macroblock.pcm_planes[1], mb_x, mb_y)?;
    write_pcm_plane(cr, &macroblock.pcm_planes[2], mb_x, mb_y)?;
    Ok(())
}

fn reconstruct_chroma_plane(
    plane: &mut PlaneMut<'_>,
    residual: &[i32],
    mode: u8,
    bit_depth: u8,
    mb_x: usize,
    mb_y: usize,
) -> Result<(), ReconstructionError> {
    validate_plane(plane)?;
    let height = match residual.len() {
        64 => 8,
        128 => 16,
        _ => {
            return Err(ReconstructionError::Unsupported(
                "chroma residual plane size",
            ));
        }
    };
    let x = mb_x / 2;
    let y = mb_y / 2;
    if x + 8 > plane.width || y + height > plane.height {
        return Err(ReconstructionError::MacroblockOutOfBounds);
    }
    let top = (y > 0).then(|| read_top_vec(plane, x, y, 8, bit_depth));
    let left = (x > 0).then(|| read_left_vec(plane, x, y, height, bit_depth));
    let top_left = read_top_left(plane, x, y, bit_depth);
    let prediction_mode = match mode {
        0 => 2,
        2 => 0,
        other => other,
    };
    let prediction = predict_chroma(
        prediction_mode,
        bit_depth,
        top.as_deref(),
        left.as_deref(),
        height,
        top_left,
    )?;
    write_rectangle(plane, x, y, 8, height, &prediction, residual, 8, bit_depth);
    Ok(())
}

fn write_pcm_plane(
    plane: &mut PlaneMut<'_>,
    samples: &[u16],
    mb_x: usize,
    mb_y: usize,
) -> Result<(), ReconstructionError> {
    let height = match samples.len() {
        64 => 8,
        128 => 16,
        256 => 16,
        _ => return Err(ReconstructionError::InvalidPcm),
    };
    let width = if samples.len() == 256 { 16 } else { 8 };
    let x = if width == 16 { mb_x } else { mb_x / 2 };
    let y = if width == 16 { mb_y } else { mb_y / 2 };
    copy_plane_block(plane, x, y, width, height, samples)
}

fn copy_plane_block(
    plane: &mut PlaneMut<'_>,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    samples: &[u16],
) -> Result<(), ReconstructionError> {
    if samples.len() != width * height || x + width > plane.width || y + height > plane.height {
        return Err(ReconstructionError::InvalidPcm);
    }
    for row in 0..height {
        plane.data[(y + row) * plane.stride + x..(y + row) * plane.stride + x + width]
            .copy_from_slice(&samples[row * width..row * width + width]);
    }
    Ok(())
}

fn mid(bit_depth: u8) -> u16 {
    1 << (bit_depth - 1)
}

fn clip(value: i32, bit_depth: u8) -> u16 {
    value.clamp(0, (1 << bit_depth) - 1) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::h264_mb::{IntraMacroblockKind, ParsedIMacroblock};
    use crate::reference::h264_residual::MacroblockResidualPlanes;

    #[test]
    fn first_i16_macroblock_uses_midpoint_edges_and_adds_residual() {
        let mut luma_data = vec![0u16; 16 * 16];
        let mut residual = empty_residual();
        residual.luma.fill(1);
        let mb = empty_macroblock(IntraMacroblockKind::I16x16 { prediction_mode: 2 }, None);
        let mut luma = PlaneMut {
            data: &mut luma_data,
            width: 16,
            height: 16,
            stride: 16,
        };
        reconstruct_i_macroblock(&mb, &residual, 8, &mut luma, None, None, 0, 0).unwrap();
        assert_eq!(luma.data, vec![129u16; 256]);
    }

    #[test]
    fn i4x4_blocks_read_their_above_boundary() {
        let mut luma_data = vec![80u16; 32 * 32];
        let residual = empty_residual();
        let mb = empty_macroblock(IntraMacroblockKind::I4x4, None);
        let mut luma = PlaneMut {
            data: &mut luma_data,
            width: 32,
            height: 32,
            stride: 32,
        };
        reconstruct_i_macroblock(&mb, &residual, 8, &mut luma, None, None, 16, 16).unwrap();
        assert!(luma.data.iter().all(|&sample| sample == 80));
    }

    #[test]
    fn i8x8_blocks_read_their_above_boundary() {
        let mut luma_data = vec![90u16; 32 * 32];
        let residual = empty_residual();
        let mb = empty_macroblock(IntraMacroblockKind::I8x8, None);
        let mut luma = PlaneMut {
            data: &mut luma_data,
            width: 32,
            height: 32,
            stride: 32,
        };
        reconstruct_i_macroblock(&mb, &residual, 8, &mut luma, None, None, 16, 16).unwrap();
        assert!(luma.data.iter().all(|&sample| sample == 90));
    }

    #[test]
    fn chroma_vertical_prediction_writes_both_planes() {
        let mut luma_data = vec![80u16; 32 * 32];
        let mut cb_data = vec![30u16; 16 * 16];
        let mut cr_data = vec![40u16; 16 * 16];
        let residual = empty_residual();
        let mb = empty_macroblock(IntraMacroblockKind::I4x4, Some(2));
        let mut luma = PlaneMut {
            data: &mut luma_data,
            width: 32,
            height: 32,
            stride: 32,
        };
        let mut cb = PlaneMut {
            data: &mut cb_data,
            width: 16,
            height: 16,
            stride: 16,
        };
        let mut cr = PlaneMut {
            data: &mut cr_data,
            width: 16,
            height: 16,
            stride: 16,
        };
        reconstruct_i_macroblock(
            &mb,
            &residual,
            8,
            &mut luma,
            Some(&mut cb),
            Some(&mut cr),
            16,
            16,
        )
        .unwrap();
        assert_eq!(cb.data, vec![30u16; 256]);
        assert_eq!(cr.data, vec![40u16; 256]);
    }

    #[test]
    fn pcm_is_copied_without_prediction_or_residual() {
        let mut luma_data = vec![0u16; 16 * 16];
        let mut mb = empty_macroblock(IntraMacroblockKind::IPcm, None);
        mb.pcm_planes = vec![(0..256).map(|value| value as u16).collect()];
        let residual = empty_residual();
        let mut luma = PlaneMut {
            data: &mut luma_data,
            width: 16,
            height: 16,
            stride: 16,
        };
        reconstruct_i_macroblock(&mb, &residual, 8, &mut luma, None, None, 0, 0).unwrap();
        assert_eq!(luma.data, mb.pcm_planes[0]);
    }

    fn empty_macroblock(
        kind: IntraMacroblockKind,
        chroma_prediction_mode: Option<u8>,
    ) -> ParsedIMacroblock {
        let modes = match kind {
            IntraMacroblockKind::I4x4 => vec![0; 16],
            IntraMacroblockKind::I8x8 => vec![0; 4],
            _ => Vec::new(),
        };
        ParsedIMacroblock {
            kind,
            luma_prediction_modes: modes,
            chroma_prediction_mode,
            coded_block_pattern: 0,
            qp_delta: 0,
            pcm_planes: Vec::new(),
            residual_bit_offset: 0,
        }
    }

    fn empty_residual() -> MacroblockResidualPlanes {
        MacroblockResidualPlanes {
            luma: [0; 256],
            chroma: [vec![0; 64], vec![0; 64]],
        }
    }
}
