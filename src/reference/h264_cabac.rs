//! H.264 CABAC I-slice reconstruction.
//!
//! The current gate covers one I16x16 macroblock through luma DC/AC and 4:2:0
//! chroma DC/AC residual syntax. Larger pictures, I4x4 residual syntax and
//! 8x8 transforms remain open.

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
    let slice_qp = pps.pic_init_qp + slice.qp_delta;
    let mut contexts = init_i_contexts(slice_qp);

    let (kind, luma_prediction_modes, mut cbp) = if decoder.decode_bin(&mut contexts[3]) == 0 {
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
        let decoded_prediction_mode = [2, 1, 0, 3][(type_index - 1) & 3];
        let prediction_mode = check_i16_prediction_mode(decoded_prediction_mode, false, false);
        let decoded_cbp = match type_index {
            1..=4 => 0,
            5..=8 => 16,
            9..=12 => 32,
            13..=16 => 15,
            17..=20 => 31,
            21..=24 => 47,
            _ => return Err(H264CabacError::Malformed("intra macroblock type")),
        };
        (
            IntraMacroblockKind::I16x16 { prediction_mode },
            vec![prediction_mode; 16],
            decoded_cbp,
        )
    };

    let decoded_chroma_prediction_mode = if decoder.decode_bin(&mut contexts[64]) == 0 {
        0
    } else if decoder.decode_bin(&mut contexts[67]) == 0 {
        1
    } else if decoder.decode_bin(&mut contexts[67]) == 0 {
        2
    } else {
        3
    };
    let chroma_prediction_mode =
        check_chroma_prediction_mode(decoded_chroma_prediction_mode, false, false);

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
        cbp = decoded_cbp | (chroma_cbp << 4);
        if decoded_cbp != 0 || chroma_cbp != 0 {
            return Err(H264CabacError::Unsupported("CABAC I4x4 residual syntax"));
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
    let qp = slice_qp + qp_delta;
    let context =
        MacroblockContext::from_sps_pps(sps, pps).map_err(|_| H264CabacError::Malformed("SPS"))?;
    let mut residual = ParsedMacroblockResidual {
        luma_dc: None,
        luma_4x4: std::array::from_fn(|_| None),
        luma_8x8: std::array::from_fn(|_| None),
        chroma_dc: std::array::from_fn(|_| None),
        chroma_ac: std::array::from_fn(|_| std::array::from_fn(|_| None)),
        luma_dc_non_zero: 0,
        luma_non_zero: [0; 16],
        chroma_non_zero: [vec![0; 4], vec![0; 4]],
        end_bit: 0,
    };

    if matches!(kind, IntraMacroblockKind::I16x16 { .. }) {
        let start = decoder.bit_pos();
        let luma_dc_cbf = decoder.decode_bin(&mut contexts[88]);
        let luma_dc = if luma_dc_cbf == 0 {
            super::h264_cavlc::CavlcResidual {
                scan: [0; 16],
                total_coeff: 0,
                trailing_ones: 0,
                bits_consumed: 0,
            }
        } else {
            decode_cabac_residual(&mut decoder, &mut contexts, ResidualCategory::LumaDc, 16)?
        };
        residual.luma_dc_non_zero = luma_dc.total_coeff as u8;
        let mut luma_dc = luma_dc;
        luma_dc.bits_consumed = decoder.bit_pos() - start;
        residual.luma_dc = Some(luma_dc);

        if cbp & 0x0f != 0 {
            for block in 0..16 {
                let cbf_context = luma_ac_cbf_context(&residual.luma_non_zero, block);
                let decoded = decode_cabac_block(
                    &mut decoder,
                    &mut contexts,
                    ResidualCategory::Luma4x4,
                    15,
                    cbf_context,
                )?;
                if let Some(block_residual) = decoded {
                    residual.luma_non_zero[block] = block_residual.total_coeff as u8;
                    residual.luma_4x4[block] = Some(block_residual);
                }
            }
        }
    }

    if matches!(kind, IntraMacroblockKind::I16x16 { .. })
        && cbp & 0x30 != 0
        && context.chroma_format_idc == 1
    {
        for plane in 0..2 {
            residual.chroma_dc[plane] = decode_cabac_block(
                &mut decoder,
                &mut contexts,
                ResidualCategory::ChromaDc,
                4,
                100,
            )?;
        }
        if cbp & 0x20 != 0 {
            for plane in 0..2 {
                for block in 0..4 {
                    let cbf_context =
                        chroma_ac_cbf_context(&residual.chroma_non_zero[plane], block);
                    let decoded = decode_cabac_block(
                        &mut decoder,
                        &mut contexts,
                        ResidualCategory::ChromaAc,
                        15,
                        cbf_context,
                    )?;
                    if let Some(block_residual) = decoded {
                        residual.chroma_non_zero[plane][block] = block_residual.total_coeff as u8;
                        residual.chroma_ac[plane][block] = Some(block_residual);
                    }
                }
            }
        }
    }
    residual.end_bit = decoder.bit_pos();

    let macroblock = ParsedIMacroblock {
        kind,
        luma_prediction_modes,
        chroma_prediction_mode: Some(chroma_prediction_mode),
        coded_block_pattern: cbp,
        qp_delta,
        pcm_planes: Vec::new(),
        residual_bit_offset: 0,
    };
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
        luma_non_zero: vec![residual.luma_non_zero],
        chroma_non_zero: [
            vec![residual.chroma_non_zero[0].clone()],
            vec![residual.chroma_non_zero[1].clone()],
        ],
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

fn init_i_contexts(slice_qp: i32) -> [Context; 276] {
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
    const CODED_BLOCK_FLAG: [[i8; 2]; 19] = [
        [-12, 115],
        [-16, 122],
        [-11, 115],
        [-12, 63],
        [-2, 68],
        [-15, 84],
        [-13, 104],
        [-3, 70],
        [-8, 93],
        [-10, 90],
        [-30, 127],
        [-1, 74],
        [-6, 97],
        [-7, 91],
        [-20, 127],
        [-4, 56],
        [-5, 82],
        [-7, 76],
        [-22, 125],
    ];
    const RESIDUAL: [[i8; 2]; 171] = [
        [-7, 93],
        [-11, 87],
        [-3, 77],
        [-5, 71],
        [-4, 63],
        [-4, 68],
        [-12, 84],
        [-7, 62],
        [-7, 65],
        [8, 61],
        [5, 56],
        [-2, 66],
        [1, 64],
        [0, 61],
        [-2, 78],
        [1, 50],
        [7, 52],
        [10, 35],
        [0, 44],
        [11, 38],
        [1, 45],
        [0, 46],
        [5, 44],
        [31, 17],
        [1, 51],
        [7, 50],
        [28, 19],
        [16, 33],
        [14, 62],
        [-13, 108],
        [-15, 100],
        [-13, 101],
        [-13, 91],
        [-12, 94],
        [-10, 88],
        [-16, 84],
        [-10, 86],
        [-7, 83],
        [-13, 87],
        [-19, 94],
        [1, 70],
        [0, 72],
        [-5, 74],
        [18, 59],
        [-8, 102],
        [-15, 100],
        [0, 95],
        [-4, 75],
        [2, 72],
        [-11, 75],
        [-3, 71],
        [15, 46],
        [-13, 69],
        [0, 62],
        [0, 65],
        [21, 37],
        [-15, 72],
        [9, 57],
        [16, 54],
        [0, 62],
        [12, 72],
        [24, 0],
        [15, 9],
        [8, 25],
        [13, 18],
        [15, 9],
        [13, 19],
        [10, 37],
        [12, 18],
        [6, 29],
        [20, 33],
        [15, 30],
        [4, 45],
        [1, 58],
        [0, 62],
        [7, 61],
        [12, 38],
        [11, 45],
        [15, 39],
        [11, 42],
        [13, 44],
        [16, 45],
        [12, 41],
        [10, 49],
        [30, 34],
        [18, 42],
        [10, 55],
        [17, 51],
        [17, 46],
        [0, 89],
        [26, -19],
        [22, -17],
        [26, -17],
        [30, -25],
        [28, -20],
        [33, -23],
        [37, -27],
        [33, -23],
        [40, -28],
        [38, -17],
        [33, -11],
        [40, -15],
        [41, -6],
        [38, 1],
        [41, 17],
        [30, -6],
        [27, 3],
        [26, 22],
        [37, -16],
        [35, -4],
        [38, -8],
        [38, -3],
        [37, 3],
        [38, 5],
        [42, 0],
        [35, 16],
        [39, 22],
        [14, 48],
        [27, 37],
        [21, 60],
        [12, 68],
        [2, 97],
        [-3, 71],
        [-6, 42],
        [-5, 50],
        [-3, 54],
        [-2, 62],
        [0, 58],
        [1, 63],
        [-2, 72],
        [-1, 74],
        [-9, 91],
        [-5, 67],
        [-5, 27],
        [-3, 39],
        [-2, 44],
        [0, 46],
        [-16, 64],
        [-8, 68],
        [-10, 78],
        [-6, 77],
        [-10, 86],
        [-12, 92],
        [-15, 55],
        [-10, 60],
        [-6, 62],
        [-4, 65],
        [-12, 73],
        [-8, 76],
        [-7, 80],
        [-9, 88],
        [-17, 110],
        [-11, 97],
        [-20, 84],
        [-11, 79],
        [-6, 73],
        [-4, 74],
        [-13, 86],
        [-13, 96],
        [-11, 97],
        [-19, 117],
        [-8, 78],
        [-5, 33],
        [-4, 48],
        [-2, 53],
        [-3, 62],
        [-13, 71],
        [-10, 79],
        [-12, 86],
        [-13, 90],
        [-14, 97],
    ];
    let mut init = [[0i8; 2]; 276];
    init[..HEAD.len()].copy_from_slice(&HEAD);
    init[60..86].copy_from_slice(&TAIL);
    init[86..105].copy_from_slice(&CODED_BLOCK_FLAG);
    init[105..276].copy_from_slice(&RESIDUAL);
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

#[derive(Debug, Clone, Copy)]
enum ResidualCategory {
    LumaDc,
    Luma4x4,
    ChromaDc,
    ChromaAc,
}

impl ResidualCategory {
    fn context_bases(self) -> (usize, usize, usize) {
        match self {
            ResidualCategory::LumaDc => (105, 166, 227),
            ResidualCategory::Luma4x4 => (120, 181, 237),
            ResidualCategory::ChromaDc => (149, 210, 257),
            ResidualCategory::ChromaAc => (152, 213, 266),
        }
    }
}

fn decode_cabac_block(
    decoder: &mut CabacDecoder<'_>,
    contexts: &mut [Context],
    category: ResidualCategory,
    max_coefficient: usize,
    cbf_context: usize,
) -> Result<Option<super::h264_cavlc::CavlcResidual>, H264CabacError> {
    if decoder.decode_bin(&mut contexts[cbf_context]) == 0 {
        return Ok(None);
    }
    let start = decoder.bit_pos();
    let mut residual = decode_cabac_residual(decoder, contexts, category, max_coefficient)?;
    residual.bits_consumed = decoder.bit_pos() - start;
    Ok(Some(residual))
}

fn decode_cabac_residual(
    decoder: &mut CabacDecoder<'_>,
    contexts: &mut [Context],
    category: ResidualCategory,
    max_coefficient: usize,
) -> Result<super::h264_cavlc::CavlcResidual, H264CabacError> {
    const POSITION_CAPACITY: usize = 16;
    const LEVEL_ONE_CONTEXT: [usize; 8] = [1, 2, 3, 4, 0, 0, 0, 0];
    const LEVEL_GT_ONE_CONTEXT: [usize; 8] = [5, 5, 5, 5, 6, 7, 8, 9];
    const LEVEL_ONE_TRANSITION: [usize; 8] = [1, 2, 3, 3, 4, 5, 6, 7];
    const LEVEL_GT_ONE_TRANSITION: [usize; 8] = [4, 4, 4, 4, 5, 6, 7, 7];
    if !(1..=POSITION_CAPACITY).contains(&max_coefficient) {
        return Err(H264CabacError::Malformed(
            "residual block coefficient limit",
        ));
    }
    let (significant_base, last_base, absolute_base) = category.context_bases();

    let mut positions = [0usize; POSITION_CAPACITY];
    let mut coefficient_count = 0;
    let mut last = 0;
    while last < max_coefficient - 1 {
        let significant = decoder.decode_bin(&mut contexts[significant_base + last]);
        if significant != 0 {
            positions[coefficient_count] = last;
            coefficient_count += 1;
            let is_last = decoder.decode_bin(&mut contexts[last_base + last]);
            if is_last != 0 {
                last = max_coefficient;
                break;
            }
        }
        last += 1;
    }
    if last == max_coefficient - 1 {
        positions[coefficient_count] = last;
        coefficient_count += 1;
    }
    if coefficient_count == 0 {
        return Err(H264CabacError::Malformed(
            "residual block has no significant coefficient",
        ));
    }
    let mut scan = [0i32; 16];
    let mut node_context = 0;
    for position in positions[..coefficient_count].iter().rev() {
        let one_context = absolute_base + LEVEL_ONE_CONTEXT[node_context];
        let mut magnitude = if decoder.decode_bin(&mut contexts[one_context]) == 0 {
            node_context = LEVEL_ONE_TRANSITION[node_context];
            1
        } else {
            let gt_one_context = absolute_base + LEVEL_GT_ONE_CONTEXT[node_context];
            node_context = LEVEL_GT_ONE_TRANSITION[node_context];
            let mut value = 2;
            while value < 15 && decoder.decode_bin(&mut contexts[gt_one_context]) != 0 {
                value += 1;
            }
            if value >= 15 {
                let mut suffix_length = 0;
                loop {
                    let more = decoder.decode_bypass() != 0;
                    if !more || suffix_length == 23 {
                        break;
                    }
                    suffix_length += 1;
                }
                value = 1;
                for _ in 0..suffix_length {
                    value = value * 2 + decoder.decode_bypass() as i32;
                }
                value += 14;
            }
            value
        };
        if decoder.decode_bypass() != 0 {
            magnitude = -magnitude;
        }
        scan[*position] = magnitude;
    }

    Ok(super::h264_cavlc::CavlcResidual {
        scan,
        total_coeff: coefficient_count,
        trailing_ones: 0,
        bits_consumed: 0,
    })
}

fn luma_ac_cbf_context(non_zero: &[u8; 16], block: usize) -> usize {
    let left = block % 4 == 0 || non_zero[block - 1] != 0;
    let top = block / 4 == 0 || non_zero[block - 4] != 0;
    89 + usize::from(left) + 2 * usize::from(top)
}

fn chroma_ac_cbf_context(non_zero: &[u8], block: usize) -> usize {
    let left = block % 2 == 0 || non_zero[block - 1] != 0;
    let top = block / 2 == 0 || non_zero[block - 2] != 0;
    101 + usize::from(left) + 2 * usize::from(top)
}

fn check_i16_prediction_mode(mode: u8, top_available: bool, left_available: bool) -> u8 {
    if (mode == 0 && !top_available)
        || (mode == 1 && !left_available)
        || (mode == 3 && (!top_available || !left_available))
    {
        2
    } else {
        mode
    }
}

fn check_chroma_prediction_mode(mode: u8, top_available: bool, left_available: bool) -> u8 {
    if (mode == 1 && !left_available)
        || (mode == 2 && !top_available)
        || (mode == 3 && (!top_available || !left_available))
    {
        0
    } else {
        mode
    }
}
