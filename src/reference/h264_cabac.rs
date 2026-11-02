//! H.264 CABAC I-slice reconstruction.
//!
//! The current gate covers one I16x16/CBP0 macroblock with no residual. Context
//! tables beyond the header/CBP subset are intentionally absent until residual
//! syntax has a differential fixture.

use std::fmt;

use super::cabac::{CabacDecoder, Context};
use super::h264_deblock::{deblock_i_picture, DeblockMap};
use super::h264_mb::{
    predict_intra_mode, IntraMacroblockKind, MacroblockContext, ParsedIMacroblock,
    ParsedMacroblockResidual, I4X4_BLOCK_ORDER,
};
use super::h264_recon::{reconstruct_i_macroblock, PlaneMut};
use super::h264_residual::assemble_i_macroblock_residual;
use crate::codec::h264::parser::{Pps, SliceHeader, Sps};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum H264CabacError {
    Unsupported(&'static str),
    Malformed(&'static str),
}

impl fmt::Display for H264CabacError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            H264CabacError::Unsupported(what) => {
                write!(f, "unsupported H.264 CABAC path: {what}")
            }
            H264CabacError::Malformed(what) => write!(f, "malformed H.264 CABAC data: {what}"),
        }
    }
}

impl std::error::Error for H264CabacError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedCabacSlice {
    pub macroblocks: usize,
    pub end_bit: usize,
    pub final_qp: u8,
    pub deblock: DeblockMap,
}

pub fn decode_cabac_i_slice(
    rbsp: &[u8],
    slice: &SliceHeader,
    sps: &Sps,
    pps: &Pps,
    luma: &mut [u16],
    chroma_cb: Option<&mut [u16]>,
    chroma_cr: Option<&mut [u16]>,
) -> Result<DecodedCabacSlice, H264CabacError> {
    let mb_width = usize::try_from(sps.coded_width / 16)
        .map_err(|_| H264CabacError::Malformed("macroblock width"))?;
    let mb_height = usize::try_from(sps.coded_height / 16)
        .map_err(|_| H264CabacError::Malformed("macroblock height"))?;
    if mb_width != 1 || mb_height != 1 {
        return Err(H264CabacError::Unsupported(
            "CABAC gate currently covers one macroblock",
        ));
    }
    if !slice.is_i() || !pps.entropy_coding {
        return Err(H264CabacError::Unsupported("non-CABAC-I slice"));
    }
    if slice.data_offset_bits % 8 != 0 {
        return Err(H264CabacError::Malformed("CABAC byte alignment"));
    }
    let offset = slice.data_offset_bits / 8;
    let data = rbsp
        .get(offset..)
        .ok_or(H264CabacError::Malformed("CABAC data offset"))?;
    let mut decoder = CabacDecoder::new_h264(data);
    let mut contexts = init_i_contexts(pps.pic_init_qp + slice.qp_delta);

    let (kind, luma_prediction_modes, cbp) = if decoder.decode_bin(&mut contexts[3]) == 0 {
        let mut modes = vec![0u8; 16];
        for block in I4X4_BLOCK_ORDER {
            let row = block / 4;
            let column = block % 4;
            let left = column.checked_sub(1).map(|left| modes[row * 4 + left]);
            let top = row.checked_sub(1).map(|top| modes[top * 4 + column]);
            let predicted = predict_intra_mode(left, top);
            let mode = if decoder.decode_bin(&mut contexts[68]) != 0 {
                predicted
            } else {
                let bit0 = decoder.decode_bin(&mut contexts[69]) as u8;
                let bit1 = decoder.decode_bin(&mut contexts[69]) as u8;
                let bit2 = decoder.decode_bin(&mut contexts[69]) as u8;
                let remainder = u32::from(bit0 | (bit1 << 1) | (bit2 << 2));
                (remainder + u32::from(remainder >= u32::from(predicted))) as u8
            };
            modes[block] = mode;
        }
        (IntraMacroblockKind::I4x4, modes, 0)
    } else {
        if decoder.decode_terminate() {
            return Err(H264CabacError::Unsupported("I_PCM macroblock"));
        }
        let mut type_index = 1usize;
        type_index += 12 * decoder.decode_bin(&mut contexts[6]) as usize;
        if decoder.decode_bin(&mut contexts[7]) != 0 {
            type_index += 4 + 4 * decoder.decode_bin(&mut contexts[8]) as usize;
        }
        type_index += 2 * decoder.decode_bin(&mut contexts[9]) as usize;
        type_index += decoder.decode_bin(&mut contexts[10]) as usize;
        if type_index > 4 {
            return Err(H264CabacError::Unsupported("CABAC residual CBP"));
        }
        let prediction_mode = (type_index - 1) as u8;
        (
            IntraMacroblockKind::I16x16 { prediction_mode },
            vec![prediction_mode; 16],
            0,
        )
    };

    let chroma_prediction_mode = if decoder.decode_bin(&mut contexts[64]) == 0 {
        0
    } else if decoder.decode_bin(&mut contexts[67]) == 0 {
        1
    } else if decoder.decode_bin(&mut contexts[67]) == 0 {
        2
    } else {
        3
    };

    if kind == IntraMacroblockKind::I4x4 {
        let mut decoded_cbp = 0u8;
        for (bit, context) in [3usize, 3, 3, 3].into_iter().enumerate() {
            decoded_cbp |= (decoder.decode_bin(&mut contexts[73 + context]) as u8) << bit;
        }
        let chroma_cbp = if decoder.decode_bin(&mut contexts[77]) == 0 {
            0
        } else if decoder.decode_bin(&mut contexts[81]) == 0 {
            1
        } else {
            2
        };
        if decoded_cbp != 0 || chroma_cbp != 0 {
            return Err(H264CabacError::Unsupported(
                "CABAC residual syntax is not implemented",
            ));
        }
    }

    let qp_delta = if matches!(kind, IntraMacroblockKind::I16x16 { .. }) {
        if decoder.decode_bin(&mut contexts[60]) != 0 {
            return Err(H264CabacError::Unsupported("CABAC mb_qp_delta"));
        }
        0
    } else {
        0
    };
    let qp = pps.pic_init_qp + qp_delta;
    let (luma_dc, luma_dc_non_zero) = if matches!(kind, IntraMacroblockKind::I16x16 { .. }) {
        if decoder.decode_bin(&mut contexts[85]) != 0 {
            return Err(H264CabacError::Unsupported("CABAC luma-DC residual"));
        }
        (
            Some(super::h264_cavlc::CavlcResidual {
                scan: [0; 16],
                total_coeff: 0,
                trailing_ones: 0,
                bits_consumed: 0,
            }),
            0,
        )
    } else {
        (None, 0)
    };

    let macroblock = ParsedIMacroblock {
        kind,
        luma_prediction_modes,
        chroma_prediction_mode: Some(chroma_prediction_mode),
        coded_block_pattern: cbp,
        qp_delta,
        pcm_planes: Vec::new(),
        residual_bit_offset: 0,
    };
    let residual = ParsedMacroblockResidual {
        luma_dc,
        luma_4x4: std::array::from_fn(|_| None),
        luma_8x8: std::array::from_fn(|_| None),
        chroma_dc: std::array::from_fn(|_| None),
        chroma_ac: std::array::from_fn(|_| std::array::from_fn(|_| None)),
        luma_dc_non_zero,
        luma_non_zero: [0; 16],
        chroma_non_zero: [vec![0; 4], vec![0; 4]],
        end_bit: 0,
    };
    let context =
        MacroblockContext::from_sps_pps(sps, pps).map_err(|_| H264CabacError::Malformed("SPS"))?;
    let options = super::h264_residual::ResidualPlaneOptions {
        qp: qp as u8,
        chroma_qp_offset: [pps.chroma_qp_offset, pps.second_chroma_qp_offset],
        bit_depth_luma: context.bit_depth_luma,
        bit_depth_chroma: context.bit_depth_chroma,
        transform_bypass: false,
        scan4: super::h264::CavlcScan4x4::Frame,
        scan8: super::h264::CavlcScan8x8::Frame,
        luma_4x4_scaling: None,
        luma_8x8_scaling: None,
        chroma_4x4_scaling: [None, None],
    };
    let residual_planes = assemble_i_macroblock_residual(&macroblock, &residual, context, &options)
        .map_err(|_| H264CabacError::Malformed("empty residual"))?;
    let mut luma_plane = PlaneMut {
        data: luma,
        width: 16,
        height: 16,
        stride: 16,
    };
    let mut cb_plane = chroma_cb.map(|data| PlaneMut {
        data,
        width: 8,
        height: 8,
        stride: 8,
    });
    let mut cr_plane = chroma_cr.map(|data| PlaneMut {
        data,
        width: 8,
        height: 8,
        stride: 8,
    });
    reconstruct_i_macroblock(
        &macroblock,
        &residual_planes,
        context.bit_depth_luma.min(context.bit_depth_chroma),
        &mut luma_plane,
        cb_plane.as_mut(),
        cr_plane.as_mut(),
        0,
        0,
    )
    .map_err(|_| H264CabacError::Malformed("macroblock reconstruction"))?;

    let deblock = DeblockMap {
        mb_width: 1,
        mb_height: 1,
        decoded: vec![true],
        qp: vec![qp as u8],
        luma_non_zero: vec![[0; 16]],
        chroma_non_zero: [vec![vec![0; 4]], vec![vec![0; 4]]],
    };
    deblock_i_picture(
        &mut luma_plane,
        cb_plane.as_mut(),
        cr_plane.as_mut(),
        &deblock,
        context.bit_depth_luma.min(context.bit_depth_chroma),
        slice.deblocking_alpha * 2,
        slice.deblocking_beta * 2,
        [pps.chroma_qp_offset, pps.second_chroma_qp_offset],
    )
    .map_err(|_| H264CabacError::Malformed("deblocking"))?;

    if !decoder.decode_terminate() {
        return Err(H264CabacError::Malformed("missing end_of_slice_flag"));
    }
    Ok(DecodedCabacSlice {
        macroblocks: 1,
        end_bit: slice.data_offset_bits + decoder.bit_pos(),
        final_qp: qp as u8,
        deblock,
    })
}

fn init_i_contexts(slice_qp: i32) -> [Context; 86] {
    const HEAD: [[i8; 2]; 11] = [
        [20, -15],
        [2, 54],
        [3, 74],
        [20, -15],
        [2, 54],
        [3, 74],
        [-28, 127],
        [-23, 104],
        [-6, 53],
        [-1, 54],
        [7, 51],
    ];
    const TAIL: [[i8; 2]; 26] = [
        [0, 41],
        [0, 63],
        [0, 63],
        [0, 63],
        [-9, 83],
        [4, 86],
        [0, 97],
        [-7, 72],
        [13, 41],
        [3, 62],
        [0, 11],
        [1, 55],
        [0, 69],
        [-17, 127],
        [-13, 102],
        [0, 82],
        [-7, 74],
        [-21, 107],
        [-27, 127],
        [-31, 127],
        [-24, 127],
        [-18, 95],
        [-27, 127],
        [-21, 114],
        [-30, 127],
        [-17, 123],
    ];
    let mut init = [[0i8; 2]; 86];
    init[..HEAD.len()].copy_from_slice(&HEAD);
    init[60..].copy_from_slice(&TAIL);
    std::array::from_fn(|index| {
        let [m, n] = init[index];
        let mut pre = 2 * (((i32::from(m) * slice_qp) >> 4) + i32::from(n)) - 127;
        pre ^= pre >> 31;
        if pre > 124 {
            pre = 124 + (pre & 1);
        }
        Context::new((pre >> 1) as u8, (pre & 1) as u8)
    })
}
