//! H.264 I-slice reconstruction.
//!
//! This drives the validated macroblock parser, residual assembler and
//! reconstruction layers in raster order, carrying QP and CAVLC non-zero
//! context across macroblocks.

use std::fmt;

use crate::codec::h264::parser::{BitReader, Pps, SliceHeader, Sps};

use super::h264::CavlcScan4x4;
use super::h264::CavlcScan8x8;
use super::h264_deblock::DeblockMap;
use super::h264_mb::{
    parse_i_macroblock, parse_i_macroblock_residual, MacroblockContext, MacroblockError,
    MacroblockNeighbors,
};
use super::h264_recon::{reconstruct_i_macroblock, PlaneMut, ReconstructionError};
use super::h264_residual::{
    assemble_i_macroblock_residual, H264ResidualError, ResidualPlaneOptions,
};

#[derive(Debug)]
pub enum SliceDecodeError {
    Macroblock(MacroblockError),
    Residual(H264ResidualError),
    Reconstruction(ReconstructionError),
    InvalidGeometry,
    InvalidQp,
    MacroblockAt {
        index: usize,
        bit: usize,
        error: MacroblockError,
    },
    ReconstructionAt {
        index: usize,
        x: usize,
        y: usize,
        error: ReconstructionError,
    },
    Unsupported(&'static str),
}

impl fmt::Display for SliceDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SliceDecodeError::Macroblock(error) => error.fmt(f),
            SliceDecodeError::Residual(error) => error.fmt(f),
            SliceDecodeError::Reconstruction(error) => error.fmt(f),
            SliceDecodeError::InvalidGeometry => write!(f, "invalid H.264 slice geometry"),
            SliceDecodeError::InvalidQp => write!(f, "invalid H.264 macroblock QP"),
            SliceDecodeError::MacroblockAt { index, bit, error } => {
                write!(f, "H.264 macroblock {index} at bit {bit}: {error}")
            }
            SliceDecodeError::ReconstructionAt { index, x, y, error } => write!(
                f,
                "H.264 macroblock {index} reconstruction at ({x},{y}): {error}"
            ),
            SliceDecodeError::Unsupported(what) => write!(f, "unsupported H.264 slice: {what}"),
        }
    }
}

impl std::error::Error for SliceDecodeError {}

impl From<MacroblockError> for SliceDecodeError {
    fn from(error: MacroblockError) -> Self {
        SliceDecodeError::Macroblock(error)
    }
}

impl From<H264ResidualError> for SliceDecodeError {
    fn from(error: H264ResidualError) -> Self {
        SliceDecodeError::Residual(error)
    }
}

impl From<ReconstructionError> for SliceDecodeError {
    fn from(error: ReconstructionError) -> Self {
        SliceDecodeError::Reconstruction(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSlice {
    pub macroblocks: usize,
    pub end_bit: usize,
    pub final_qp: u8,
    pub deblock: DeblockMap,
}

/// Reconstruct one CAVLC I slice into destination picture planes.
pub fn decode_i_slice(
    rbsp: &[u8],
    slice: &SliceHeader,
    sps: &Sps,
    pps: &Pps,
    luma: &mut [u16],
    chroma_cb: Option<&mut [u16]>,
    chroma_cr: Option<&mut [u16]>,
) -> Result<DecodedSlice, SliceDecodeError> {
    if !slice.is_i() {
        return Err(SliceDecodeError::Unsupported("non-I slice"));
    }
    if pps.entropy_coding {
        return Err(SliceDecodeError::Unsupported("CABAC slice"));
    }
    let mb_width =
        usize::try_from(sps.coded_width / 16).map_err(|_| SliceDecodeError::InvalidGeometry)?;
    let mb_height =
        usize::try_from(sps.coded_height / 16).map_err(|_| SliceDecodeError::InvalidGeometry)?;
    if mb_width == 0 || mb_height == 0 || sps.coded_width % 16 != 0 || sps.coded_height % 16 != 0 {
        return Err(SliceDecodeError::InvalidGeometry);
    }
    let context =
        MacroblockContext::from_sps_pps(sps, pps).map_err(SliceDecodeError::Macroblock)?;
    if context.chroma_format_idc == 3 {
        return Err(SliceDecodeError::Unsupported(
            "4:4:4 I-slice reconstruction",
        ));
    }
    let chroma_dims = match context.chroma_format_idc {
        1 => Some((2usize, 2usize)),
        2 => Some((2usize, 4usize)),
        _ => None,
    };
    let (chroma_width, chroma_height) = chroma_dims.unwrap_or((0, 0));
    let max_qp = 51 + 6 * i32::from(context.bit_depth_luma - 8);
    let mut qp = pps.pic_init_qp + slice.qp_delta;
    if !(0..=max_qp).contains(&qp) {
        return Err(SliceDecodeError::InvalidQp);
    }
    let total_mbs = mb_width * mb_height;
    let mut decoded = vec![false; total_mbs];
    let mut luma_counts = vec![[0u8; 16]; total_mbs];
    let mut luma_dc_counts = vec![0u8; total_mbs];
    let mut qp_map = vec![0u8; total_mbs];
    let chroma_block_count = chroma_width * chroma_height;
    let mut chroma_counts: [Vec<Vec<u8>>; 2] = [
        vec![vec![0; chroma_block_count]; total_mbs],
        vec![vec![0; chroma_block_count]; total_mbs],
    ];
    let mut bit_offset = slice.data_offset_bits;
    let mut macroblocks = 0;
    let first_mb =
        usize::try_from(slice.first_mb).map_err(|_| SliceDecodeError::InvalidGeometry)?;
    if first_mb >= total_mbs {
        return Err(SliceDecodeError::InvalidGeometry);
    }
    let mut luma_plane = PlaneMut {
        data: luma,
        width: sps.coded_width as usize,
        height: sps.coded_height as usize,
        stride: sps.coded_width as usize,
    };
    let mut cb_plane = chroma_cb.map(|data| PlaneMut {
        data,
        width: chroma_width * 4 * mb_width,
        height: chroma_height * 4 * mb_height,
        stride: chroma_width * 4 * mb_width,
    });
    let mut cr_plane = chroma_cr.map(|data| PlaneMut {
        data,
        width: chroma_width * 4 * mb_width,
        height: chroma_height * 4 * mb_height,
        stride: chroma_width * 4 * mb_width,
    });

    for mb_index in first_mb..total_mbs {
        if !more_rbsp_data(rbsp, bit_offset)? {
            break;
        }
        let macroblock = parse_i_macroblock(rbsp, bit_offset, context).map_err(|error| {
            SliceDecodeError::MacroblockAt {
                index: mb_index,
                bit: bit_offset,
                error,
            }
        })?;
        let neighbors = macroblock_neighbors(
            mb_index,
            mb_width,
            &decoded,
            &luma_counts,
            &luma_dc_counts,
            &chroma_counts,
            context,
        );
        let residual = parse_i_macroblock_residual(rbsp, &macroblock, context, &neighbors)
            .map_err(|error| SliceDecodeError::MacroblockAt {
                index: mb_index,
                bit: macroblock.residual_bit_offset,
                error,
            })?;
        qp = add_qp(qp, macroblock.qp_delta, max_qp)?;
        let transform_bypass = sps.transform_bypass && qp == 0;
        let field = slice.field_pic;
        let options = ResidualPlaneOptions {
            qp: qp as u8,
            chroma_qp_offset: [pps.chroma_qp_offset, pps.second_chroma_qp_offset],
            bit_depth_luma: context.bit_depth_luma,
            bit_depth_chroma: context.bit_depth_chroma,
            transform_bypass,
            scan4: if field {
                if transform_bypass {
                    CavlcScan4x4::FieldQ0
                } else {
                    CavlcScan4x4::Field
                }
            } else if transform_bypass {
                CavlcScan4x4::FrameQ0
            } else {
                CavlcScan4x4::Frame
            },
            scan8: if field {
                if transform_bypass {
                    CavlcScan8x8::FieldQ0
                } else {
                    CavlcScan8x8::Field
                }
            } else if transform_bypass {
                CavlcScan8x8::FrameQ0
            } else {
                CavlcScan8x8::Frame
            },
            luma_4x4_scaling: None,
            luma_8x8_scaling: None,
            chroma_4x4_scaling: [None, None],
        };
        let residual_planes =
            assemble_i_macroblock_residual(&macroblock, &residual, context, &options)?;

        let mb_x = (mb_index % mb_width) * 16;
        let mb_y = (mb_index / mb_width) * 16;
        reconstruct_i_macroblock(
            &macroblock,
            &residual_planes,
            context.bit_depth_luma.min(context.bit_depth_chroma),
            &mut luma_plane,
            cb_plane.as_mut(),
            cr_plane.as_mut(),
            mb_x,
            mb_y,
        )
        .map_err(|error| SliceDecodeError::ReconstructionAt {
            index: mb_index,
            x: mb_x,
            y: mb_y,
            error,
        })?;
        decoded[mb_index] = true;
        qp_map[mb_index] = qp as u8;
        luma_counts[mb_index] = residual.luma_non_zero;
        luma_dc_counts[mb_index] = residual.luma_dc_non_zero;
        chroma_counts[0][mb_index].copy_from_slice(&residual.chroma_non_zero[0]);
        chroma_counts[1][mb_index].copy_from_slice(&residual.chroma_non_zero[1]);
        bit_offset = residual.end_bit;
        macroblocks += 1;
    }

    Ok(DecodedSlice {
        macroblocks,
        end_bit: bit_offset,
        final_qp: qp as u8,
        deblock: DeblockMap {
            mb_width,
            mb_height,
            decoded,
            qp: qp_map,
            luma_non_zero: luma_counts,
            chroma_non_zero: chroma_counts,
        },
    })
}

fn macroblock_neighbors(
    index: usize,
    width: usize,
    decoded: &[bool],
    luma_counts: &[[u8; 16]],
    luma_dc_counts: &[u8],
    chroma_counts: &[Vec<Vec<u8>>; 2],
    context: MacroblockContext,
) -> MacroblockNeighbors {
    let left = index
        .checked_sub(1)
        .filter(|&left| index % width != 0 && decoded[left]);
    let top = index.checked_sub(width).filter(|&top| decoded[top]);
    let mut neighbors = MacroblockNeighbors::unavailable();
    if let Some(left) = left {
        neighbors.luma_dc_left = Some(luma_dc_counts[left]);
        for row in 0..4 {
            neighbors.luma_left[row] = Some(luma_counts[left][row * 4 + 3]);
        }
    }
    if let Some(top) = top {
        neighbors.luma_dc_top = Some(luma_dc_counts[top]);
        for column in 0..4 {
            neighbors.luma_top[column] = Some(luma_counts[top][3 * 4 + column]);
        }
    }
    if let Some((width, height)) = match context.chroma_format_idc {
        1 => Some((2usize, 2usize)),
        2 => Some((2usize, 4usize)),
        _ => None,
    } {
        for plane in 0..2 {
            if let Some(left) = left {
                for row in 0..height {
                    neighbors.chroma_left[plane][row] =
                        Some(chroma_counts[plane][left][row * width + width - 1]);
                }
            }
            if let Some(top) = top {
                for column in 0..width {
                    neighbors.chroma_top[plane][column] =
                        Some(chroma_counts[plane][top][(height - 1) * width + column]);
                }
            }
        }
    }
    neighbors
}

fn add_qp(qp: i32, delta: i32, max_qp: i32) -> Result<i32, SliceDecodeError> {
    let mut value = qp + delta;
    if !(0..=max_qp).contains(&value) {
        value += if value < 0 { max_qp + 1 } else { -(max_qp + 1) };
    }
    if (0..=max_qp).contains(&value) {
        Ok(value)
    } else {
        Err(SliceDecodeError::InvalidQp)
    }
}

fn more_rbsp_data(data: &[u8], bit_offset: usize) -> Result<bool, SliceDecodeError> {
    let offset = u32::try_from(bit_offset).map_err(|_| SliceDecodeError::InvalidGeometry)?;
    let mut reader = BitReader::new(data);
    reader
        .skip(offset)
        .map_err(|_| SliceDecodeError::Macroblock(MacroblockError::Truncated))?;
    Ok(reader.more_rbsp_data())
}

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

        fn finish(mut self) -> Vec<u8> {
            self.bits.push(1);
            while !self.bits.len().is_multiple_of(8) {
                self.bits.push(0);
            }
            let mut bytes = vec![0u8; self.bits.len() / 8];
            for (position, bit) in self.bits.into_iter().enumerate() {
                bytes[position / 8] |= bit << (7 - position % 8);
            }
            bytes
        }
    }

    fn parameters() -> (Sps, Pps, SliceHeader) {
        let sps = Sps {
            chroma_format_idc: 1,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            coded_width: 16,
            coded_height: 16,
            width: 16,
            height: 16,
            ..Sps::default()
        };
        let pps = Pps {
            entropy_coding: false,
            pic_init_qp: 26,
            chroma_qp_offset: 0,
            second_chroma_qp_offset: 0,
            transform_8x8: false,
            ..Pps::default()
        };
        let slice = SliceHeader {
            first_mb: 0,
            slice_type: 2,
            qp_delta: 0,
            data_offset_bits: 0,
            ..SliceHeader::default()
        };
        (sps, pps, slice)
    }

    #[test]
    fn reconstructs_a_minimal_i_slice_and_stops_at_the_rbsp_tail() {
        let mut writer = BitWriter::new();
        writer.ue(1); // I16x16 DC, CBP zero
        writer.ue(0); // chroma vertical
        writer.se(0); // mb_qp_delta
        writer.put(1, 1); // empty luma DC coeff_token
        let rbsp = writer.finish();
        let (sps, pps, slice) = parameters();
        let mut luma = vec![0u16; 16 * 16];
        let mut cb = vec![0u16; 8 * 8];
        let mut cr = vec![0u16; 8 * 8];
        let decoded = decode_i_slice(
            &rbsp,
            &slice,
            &sps,
            &pps,
            &mut luma,
            Some(&mut cb),
            Some(&mut cr),
        )
        .unwrap();
        assert_eq!(decoded.macroblocks, 1);
        assert_eq!(decoded.final_qp, 26);
        assert_eq!(luma, vec![128; 256]);
        assert_eq!(cb, vec![128; 64]);
        assert_eq!(cr, vec![128; 64]);

        let mut luma_plane = PlaneMut {
            data: &mut luma,
            width: 16,
            height: 16,
            stride: 16,
        };
        let mut cb_plane = PlaneMut {
            data: &mut cb,
            width: 8,
            height: 8,
            stride: 8,
        };
        let mut cr_plane = PlaneMut {
            data: &mut cr,
            width: 8,
            height: 8,
            stride: 8,
        };
        super::super::h264_deblock::deblock_i_picture(
            &mut luma_plane,
            Some(&mut cb_plane),
            Some(&mut cr_plane),
            &decoded.deblock,
            8,
            0,
            0,
            [0, 0],
        )
        .unwrap();
        assert_eq!(luma, vec![128; 256]);
    }

    #[test]
    fn rejects_cabac_and_out_of_range_qp_before_parsing() {
        let (sps, mut pps, slice) = parameters();
        let mut luma = vec![0u16; 256];
        pps.entropy_coding = true;
        assert!(matches!(
            decode_i_slice(&[], &slice, &sps, &pps, &mut luma, None, None),
            Err(SliceDecodeError::Unsupported("CABAC slice"))
        ));

        pps.entropy_coding = false;
        let mut bad_slice = slice;
        bad_slice.qp_delta = 52;
        assert!(matches!(
            decode_i_slice(&[], &bad_slice, &sps, &pps, &mut luma, None, None),
            Err(SliceDecodeError::InvalidQp)
        ));
    }
}
