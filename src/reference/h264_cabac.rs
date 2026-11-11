//! H.264 CABAC intra and P-slice reconstruction.
//!
//! The current gates cover I16x16, I4x4, I8x8 and I_PCM syntax, multi-macroblock
//! intra propagation, `P_Skip`, and zero-motion `P_L0_16x16` with CABAC
//! residuals.

use std::fmt;

use super::cabac::{CabacDecoder, Context};
use super::h264_cabac_pb::PB_INIT;
use super::h264_deblock::DeblockMap;
use super::h264_mb::{
    intra_mode_grid, predict_intra_mode, IntraMacroblockKind, IntraModeNeighbors,
    MacroblockContext, ParsedIMacroblock, ParsedMacroblockResidual, I4X4_BLOCK_ORDER,
};
use super::h264_recon::{
    reconstruct_i_macroblock, reconstruct_i_macroblock_with_availability,
    reconstruct_inter_macroblock, PlaneMut,
};
use super::h264_residual::{assemble_i_macroblock_residual, assemble_inter_macroblock_residual};
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

/// Reconstruct a CABAC P slice whose macroblocks all use `P_Skip`.
///
/// P_Skip derives a zero motion vector when every prior macroblock in the
/// slice is also skipped, so this first P-slice gate can copy the corresponding
/// macroblock from reference picture 0 without introducing a partial
/// motion-vector implementation. Any non-skip macroblock is rejected so a
/// later P_L0 path cannot accidentally be treated as complete.
#[allow(clippy::too_many_arguments)]
pub fn decode_cabac_p_skip_slice(
    rbsp: &[u8],
    slice: &SliceHeader,
    sps: &Sps,
    pps: &Pps,
    reference_luma: &[u16],
    reference_chroma_cb: &[u16],
    reference_chroma_cr: &[u16],
    luma: &mut [u16],
    chroma_cb: &mut [u16],
    chroma_cr: &mut [u16],
) -> Result<DecodedCabacSlice, H264CabacError> {
    if !slice.is_p() || !pps.entropy_coding {
        return Err(H264CabacError::Unsupported("non-CABAC-P slice"));
    }
    if slice.data_offset_bits % 8 != 0 {
        return Err(H264CabacError::Malformed("CABAC byte alignment"));
    }
    if !sps.frame_mbs_only || slice.field_pic {
        return Err(H264CabacError::Unsupported("interlaced P_Skip"));
    }
    if pps.num_slice_groups != 1 {
        return Err(H264CabacError::Unsupported("P_Skip slice groups"));
    }
    if pps.weighted_pred {
        return Err(H264CabacError::Unsupported("weighted P_Skip"));
    }
    if !pps.deblocking_filter_control || slice.deblocking_disable_idc != 1 {
        return Err(H264CabacError::Unsupported("P_Skip deblocking"));
    }
    if slice
        .mod_l0
        .iter()
        .any(|entry| *entry != crate::codec::h264::parser::RefMod::End)
    {
        return Err(H264CabacError::Unsupported("P_Skip reference reordering"));
    }

    let context =
        MacroblockContext::from_sps_pps(sps, pps).map_err(|_| H264CabacError::Malformed("SPS"))?;
    if context.chroma_format_idc != 1 {
        return Err(H264CabacError::Unsupported("P_Skip requires 4:2:0 chroma"));
    }
    let mb_width = usize::try_from(sps.coded_width / 16)
        .map_err(|_| H264CabacError::Malformed("macroblock width"))?;
    let mb_height = usize::try_from(sps.coded_height / 16)
        .map_err(|_| H264CabacError::Malformed("macroblock height"))?;
    if mb_width == 0 || mb_height == 0 || sps.coded_width % 16 != 0 || sps.coded_height % 16 != 0 {
        return Err(H264CabacError::Malformed("macroblock geometry"));
    }
    let width = sps.coded_width as usize;
    let height = sps.coded_height as usize;
    let chroma_width = width / 2;
    let chroma_height = height / 2;
    let minimum_luma = width
        .checked_mul(height)
        .ok_or(H264CabacError::Malformed("luma plane size"))?;
    let minimum_chroma = chroma_width
        .checked_mul(chroma_height)
        .ok_or(H264CabacError::Malformed("chroma plane size"))?;
    if reference_luma.len() < minimum_luma
        || luma.len() < minimum_luma
        || reference_chroma_cb.len() < minimum_chroma
        || reference_chroma_cr.len() < minimum_chroma
        || chroma_cb.len() < minimum_chroma
        || chroma_cr.len() < minimum_chroma
    {
        return Err(H264CabacError::Malformed("P_Skip plane geometry"));
    }

    let total_mbs = mb_width * mb_height;
    let first_mb =
        usize::try_from(slice.first_mb).map_err(|_| H264CabacError::Malformed("first_mb"))?;
    if first_mb != 0 {
        return Err(H264CabacError::Unsupported("P_Skip after first_mb 0"));
    }

    let data = rbsp
        .get(slice.data_offset_bits / 8..)
        .ok_or(H264CabacError::Malformed("CABAC data offset"))?;
    let mut decoder = CabacDecoder::new_h264(data);
    let slice_qp = pps.pic_init_qp + slice.qp_delta;
    let max_qp = 51 + 6 * i32::from(context.bit_depth_luma - 8);
    if !(0..=max_qp).contains(&slice_qp) {
        return Err(H264CabacError::Malformed("slice QP"));
    }
    let cabac_init_idc = slice
        .cabac_init_idc
        .ok_or(H264CabacError::Malformed("P-slice cabac_init_idc"))?;
    let mut contexts = init_p_contexts(cabac_init_idc, slice_qp, context.bit_depth_luma)?;

    let mut decoded = vec![false; total_mbs];
    let mut skipped = vec![false; total_mbs];
    let mut macroblocks = 0usize;
    for mb_index in first_mb..total_mbs {
        let left_non_skip = mb_index
            .checked_sub(1)
            .filter(|_| mb_index % mb_width != 0)
            .is_some_and(|left| decoded[left] && !skipped[left]);
        let top_non_skip = mb_index
            .checked_sub(mb_width)
            .is_some_and(|top| decoded[top] && !skipped[top]);
        let skip_context = 11 + usize::from(left_non_skip) + usize::from(top_non_skip);
        if decoder.decode_bin(&mut contexts[skip_context]) == 0 {
            return Err(H264CabacError::Unsupported(
                "P slice contains a non-P_Skip macroblock",
            ));
        }

        let mb_x = (mb_index % mb_width) * 16;
        let mb_y = (mb_index / mb_width) * 16;
        let chroma_x = mb_x / 2;
        let chroma_y = mb_y / 2;
        for row in 0..16 {
            let start = (mb_y + row) * width + mb_x;
            luma[start..start + 16].copy_from_slice(&reference_luma[start..start + 16]);
        }
        for row in 0..8 {
            let start = (chroma_y + row) * chroma_width + chroma_x;
            chroma_cb[start..start + 8].copy_from_slice(&reference_chroma_cb[start..start + 8]);
            chroma_cr[start..start + 8].copy_from_slice(&reference_chroma_cr[start..start + 8]);
        }

        decoded[mb_index] = true;
        skipped[mb_index] = true;
        macroblocks += 1;
        let end_of_slice = decoder.decode_terminate();
        if end_of_slice {
            break;
        }
        if mb_index + 1 == total_mbs {
            return Err(H264CabacError::Malformed("missing end_of_slice_flag"));
        }
    }

    Ok(DecodedCabacSlice {
        macroblocks,
        end_bit: slice.data_offset_bits + decoder.bit_pos(),
        final_qp: slice_qp as u8,
        deblock: DeblockMap {
            mb_width,
            mb_height,
            decoded,
            qp: vec![slice_qp as u8; total_mbs],
            luma_non_zero: vec![[0; 16]; total_mbs],
            chroma_non_zero: [vec![vec![0; 4]; total_mbs], vec![vec![0; 4]; total_mbs]],
        },
    })
}

/// Reconstruct a CABAC P slice made entirely of zero-motion
/// `P_L0_16x16` macroblocks.
///
/// This gate consumes the full P macroblock syntax, including MVD, coded-block
/// pattern, QP delta and inter residuals, while rejecting every other P
/// partition and non-zero motion. The zero motion predictor lets the picture
/// be built by copying reference picture 0 before adding residual samples.
#[allow(clippy::too_many_arguments)]
pub fn decode_cabac_p16_zero_mv_slice(
    rbsp: &[u8],
    slice: &SliceHeader,
    sps: &Sps,
    pps: &Pps,
    reference_luma: &[u16],
    reference_chroma_cb: &[u16],
    reference_chroma_cr: &[u16],
    luma: &mut [u16],
    chroma_cb: &mut [u16],
    chroma_cr: &mut [u16],
) -> Result<DecodedCabacSlice, H264CabacError> {
    if !slice.is_p() || !pps.entropy_coding {
        return Err(H264CabacError::Unsupported("non-CABAC-P slice"));
    }
    if slice.data_offset_bits % 8 != 0 {
        return Err(H264CabacError::Malformed("CABAC byte alignment"));
    }
    if !sps.frame_mbs_only || slice.field_pic {
        return Err(H264CabacError::Unsupported("interlaced P16"));
    }
    if pps.num_slice_groups != 1 {
        return Err(H264CabacError::Unsupported("P16 slice groups"));
    }
    if pps.weighted_pred {
        return Err(H264CabacError::Unsupported("weighted P16"));
    }
    if pps.transform_8x8 {
        return Err(H264CabacError::Unsupported("P16 transform8x8"));
    }
    if !pps.deblocking_filter_control || slice.deblocking_disable_idc != 1 {
        return Err(H264CabacError::Unsupported("P16 deblocking"));
    }
    if slice
        .mod_l0
        .iter()
        .any(|entry| *entry != crate::codec::h264::parser::RefMod::End)
    {
        return Err(H264CabacError::Unsupported("P16 reference reordering"));
    }
    if slice.ref_idx_l0_active != 1 {
        return Err(H264CabacError::Unsupported("P16 multiple references"));
    }

    let context =
        MacroblockContext::from_sps_pps(sps, pps).map_err(|_| H264CabacError::Malformed("SPS"))?;
    if context.chroma_format_idc != 1 {
        return Err(H264CabacError::Unsupported("P16 requires 4:2:0 chroma"));
    }
    let mb_width = usize::try_from(sps.coded_width / 16)
        .map_err(|_| H264CabacError::Malformed("macroblock width"))?;
    let mb_height = usize::try_from(sps.coded_height / 16)
        .map_err(|_| H264CabacError::Malformed("macroblock height"))?;
    if mb_width == 0 || mb_height == 0 || sps.coded_width % 16 != 0 || sps.coded_height % 16 != 0 {
        return Err(H264CabacError::Malformed("macroblock geometry"));
    }
    let width = sps.coded_width as usize;
    let height = sps.coded_height as usize;
    let chroma_width = width / 2;
    let chroma_height = height / 2;
    let minimum_luma = width
        .checked_mul(height)
        .ok_or(H264CabacError::Malformed("luma plane size"))?;
    let minimum_chroma = chroma_width
        .checked_mul(chroma_height)
        .ok_or(H264CabacError::Malformed("chroma plane size"))?;
    if reference_luma.len() < minimum_luma
        || luma.len() < minimum_luma
        || reference_chroma_cb.len() < minimum_chroma
        || reference_chroma_cr.len() < minimum_chroma
        || chroma_cb.len() < minimum_chroma
        || chroma_cr.len() < minimum_chroma
    {
        return Err(H264CabacError::Malformed("P16 plane geometry"));
    }
    if slice.first_mb != 0 {
        return Err(H264CabacError::Unsupported("P16 after first_mb 0"));
    }

    let data = rbsp
        .get(slice.data_offset_bits / 8..)
        .ok_or(H264CabacError::Malformed("CABAC data offset"))?;
    let mut decoder = CabacDecoder::new_h264(data);
    let slice_qp = pps.pic_init_qp + slice.qp_delta;
    let max_qp = 51 + 6 * i32::from(context.bit_depth_luma - 8);
    if !(0..=max_qp).contains(&slice_qp) {
        return Err(H264CabacError::Malformed("slice QP"));
    }
    let cabac_init_idc = slice
        .cabac_init_idc
        .ok_or(H264CabacError::Malformed("P-slice cabac_init_idc"))?;
    let mut contexts = init_p_contexts(cabac_init_idc, slice_qp, context.bit_depth_luma)?;

    let total_mbs = mb_width * mb_height;
    let mut state = CabacPictureState::new(mb_width, mb_height);
    let mut qp = slice_qp;
    let mut last_qscale_diff = 0;
    let mut macroblocks = 0usize;
    let mut luma_plane = PlaneMut {
        data: luma,
        width,
        height,
        stride: width,
    };
    let mut cb_plane = PlaneMut {
        data: chroma_cb,
        width: chroma_width,
        height: chroma_height,
        stride: chroma_width,
    };
    let mut cr_plane = PlaneMut {
        data: chroma_cr,
        width: chroma_width,
        height: chroma_height,
        stride: chroma_width,
    };

    for mb_index in 0..total_mbs {
        let mut neighbors = collect_cabac_neighbors(
            mb_index,
            mb_width,
            0,
            &state.slice_id,
            &state.decoded,
            &state.luma_counts,
            &state.chroma_counts,
            &state.chroma_modes,
            &state.luma_modes,
            &state.transform8,
            &state.i16_or_pcm,
            &state.coded_flags,
        );
        if !neighbors.left_decoded {
            neighbors.left_flags = 0x00f;
        }
        if !neighbors.top_decoded {
            neighbors.top_flags = 0x00f;
        }
        let left_non_skip = mb_index
            .checked_sub(1)
            .filter(|_| mb_index % mb_width != 0)
            .is_some_and(|left| state.decoded[left]);
        let top_non_skip = mb_index
            .checked_sub(mb_width)
            .is_some_and(|top| state.decoded[top]);
        let skip_context = 11 + usize::from(left_non_skip) + usize::from(top_non_skip);
        if decoder.decode_bin(&mut contexts[skip_context]) != 0 {
            return Err(H264CabacError::Unsupported(
                "P slice contains a P_Skip macroblock",
            ));
        }

        let decoded_mb = decode_cabac_p16_macroblock(
            &mut decoder,
            &mut contexts,
            context,
            neighbors,
            &mut last_qscale_diff,
        )?;
        if decoded_mb.motion_vector_delta != [0, 0] {
            return Err(H264CabacError::Unsupported(
                "P16 with a non-zero motion vector",
            ));
        }
        qp = add_cabac_qp(qp, decoded_mb.qp_delta, max_qp)?;
        let options = super::h264_residual::ResidualPlaneOptions {
            qp: qp as u8,
            chroma_qp_offset: [pps.chroma_qp_offset, pps.second_chroma_qp_offset],
            bit_depth_luma: context.bit_depth_luma,
            bit_depth_chroma: context.bit_depth_chroma,
            transform_bypass: false,
            scan4: super::h264::CavlcScan4x4::Frame,
            scan8: super::h264::CavlcScan8x8::CabacFrame,
            luma_4x4_scaling: None,
            luma_8x8_scaling: None,
            chroma_4x4_scaling: [None, None],
        };
        let residual_planes =
            assemble_inter_macroblock_residual(&decoded_mb.residual, context, &options)
                .map_err(|_| H264CabacError::Malformed("P16 residual"))?;
        let mb_x = (mb_index % mb_width) * 16;
        let mb_y = (mb_index / mb_width) * 16;
        copy_zero_motion_macroblock(
            reference_luma,
            reference_chroma_cb,
            reference_chroma_cr,
            &mut luma_plane,
            &mut cb_plane,
            &mut cr_plane,
            mb_x,
            mb_y,
        )?;
        reconstruct_inter_macroblock(
            &residual_planes,
            context.bit_depth_luma.min(context.bit_depth_chroma),
            &mut luma_plane,
            Some(&mut cb_plane),
            Some(&mut cr_plane),
            mb_x,
            mb_y,
        )
        .map_err(|_| H264CabacError::Malformed("P16 reconstruction"))?;

        state.decoded[mb_index] = true;
        state.luma_counts[mb_index] = decoded_mb.residual.luma_non_zero;
        state.chroma_counts[0][mb_index].copy_from_slice(&decoded_mb.residual.chroma_non_zero[0]);
        state.chroma_counts[1][mb_index].copy_from_slice(&decoded_mb.residual.chroma_non_zero[1]);
        state.coded_flags[mb_index] = decoded_mb.coded_flags;
        state.qp[mb_index] = qp as u8;
        macroblocks += 1;

        let end_of_slice = decoder.decode_terminate();
        if end_of_slice {
            break;
        }
        if mb_index + 1 == total_mbs {
            return Err(H264CabacError::Malformed("missing end_of_slice_flag"));
        }
    }

    Ok(DecodedCabacSlice {
        macroblocks,
        end_bit: slice.data_offset_bits + decoder.bit_pos(),
        final_qp: qp as u8,
        deblock: state.deblock_map(),
    })
}

#[allow(clippy::too_many_arguments)]
fn copy_zero_motion_macroblock(
    reference_luma: &[u16],
    reference_chroma_cb: &[u16],
    reference_chroma_cr: &[u16],
    luma: &mut PlaneMut<'_>,
    chroma_cb: &mut PlaneMut<'_>,
    chroma_cr: &mut PlaneMut<'_>,
    mb_x: usize,
    mb_y: usize,
) -> Result<(), H264CabacError> {
    if mb_x + 16 > luma.width
        || mb_y + 16 > luma.height
        || mb_x / 2 + 8 > chroma_cb.width
        || mb_y / 2 + 8 > chroma_cb.height
        || mb_x / 2 + 8 > chroma_cr.width
        || mb_y / 2 + 8 > chroma_cr.height
    {
        return Err(H264CabacError::Malformed("P16 prediction geometry"));
    }
    for row in 0..16 {
        let source = (mb_y + row) * luma.width + mb_x;
        let destination = (mb_y + row) * luma.stride + mb_x;
        luma.data[destination..destination + 16]
            .copy_from_slice(&reference_luma[source..source + 16]);
    }
    for row in 0..8 {
        let source = (mb_y / 2 + row) * chroma_cb.width + mb_x / 2;
        let destination = (mb_y / 2 + row) * chroma_cb.stride + mb_x / 2;
        chroma_cb.data[destination..destination + 8]
            .copy_from_slice(&reference_chroma_cb[source..source + 8]);
        chroma_cr.data[destination..destination + 8]
            .copy_from_slice(&reference_chroma_cr[source..source + 8]);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct CabacMbNeighbors {
    left_decoded: bool,
    top_decoded: bool,
    left_recon_available: bool,
    top_recon_available: bool,
    left_modes_available: bool,
    top_modes_available: bool,
    left_i16_or_pcm: bool,
    top_i16_or_pcm: bool,
    left_flags: u16,
    top_flags: u16,
    left_chroma_mode: u8,
    top_chroma_mode: u8,
    left_luma: [u8; 4],
    top_luma: [u8; 4],
    left_modes: [u8; 4],
    top_modes: [u8; 4],
    left_transform8: bool,
    top_transform8: bool,
    left_chroma: [[u8; 4]; 2],
    top_chroma: [[u8; 4]; 2],
}

struct DecodedCabacMacroblock {
    macroblock: ParsedIMacroblock,
    residual: ParsedMacroblockResidual,
    chroma_prediction_mode: u8,
    coded_flags: u16,
}

struct DecodedCabacPMacroblock {
    residual: ParsedMacroblockResidual,
    qp_delta: i32,
    motion_vector_delta: [i32; 2],
    coded_flags: u16,
}

fn decode_i_pcm_macroblock(
    decoder: &mut CabacDecoder<'_>,
    context: MacroblockContext,
) -> Result<(ParsedIMacroblock, ParsedMacroblockResidual), H264CabacError> {
    let chroma_samples = match context.chroma_format_idc {
        0 => 0,
        1 => 64 * 2,
        2 => 128 * 2,
        3 => 256 * 2,
        _ => return Err(H264CabacError::Unsupported("I_PCM chroma format")),
    };
    let chroma_blocks = match context.chroma_format_idc {
        0 => 0,
        1 => 4,
        2 => 8,
        3 => 16,
        _ => return Err(H264CabacError::Unsupported("I_PCM chroma format")),
    };
    let total_bits = 256 * usize::from(context.bit_depth_luma)
        + chroma_samples * usize::from(context.bit_depth_chroma);
    let raw = decoder
        .read_i_pcm_and_restart(total_bits.div_ceil(8))
        .ok_or(H264CabacError::Malformed("I_PCM payload"))?;
    let mut bit = 0;
    let mut read_plane = |count: usize, depth: u8| -> Result<Vec<u16>, H264CabacError> {
        let mut samples = Vec::with_capacity(count);
        for _ in 0..count {
            let mut value = 0u32;
            for _ in 0..depth {
                let byte = raw
                    .get(bit / 8)
                    .copied()
                    .ok_or(H264CabacError::Malformed("I_PCM samples"))?;
                value = (value << 1) | u32::from((byte >> (7 - bit % 8)) & 1);
                bit += 1;
            }
            samples.push(value as u16);
        }
        Ok(samples)
    };
    let mut planes = vec![read_plane(256, context.bit_depth_luma)?];
    if chroma_samples != 0 {
        planes.push(read_plane(chroma_samples / 2, context.bit_depth_chroma)?);
        planes.push(read_plane(chroma_samples / 2, context.bit_depth_chroma)?);
    }

    let residual = ParsedMacroblockResidual {
        luma_dc: None,
        luma_4x4: std::array::from_fn(|_| None),
        luma_8x8: std::array::from_fn(|_| None),
        chroma_dc: std::array::from_fn(|_| None),
        chroma_ac: std::array::from_fn(|_| std::array::from_fn(|_| None)),
        luma_dc_non_zero: 16,
        luma_non_zero: [16; 16],
        chroma_non_zero: [vec![16; chroma_blocks], vec![16; chroma_blocks]],
        end_bit: decoder.bit_pos(),
    };
    Ok((
        ParsedIMacroblock {
            kind: IntraMacroblockKind::IPcm,
            luma_prediction_modes: Vec::new(),
            chroma_prediction_mode: None,
            coded_block_pattern: if chroma_blocks == 0 { 15 } else { 47 },
            qp_delta: 0,
            pcm_planes: planes,
            residual_bit_offset: 0,
        },
        residual,
    ))
}

struct CabacPictureState {
    mb_width: usize,
    mb_height: usize,
    decoded: Vec<bool>,
    luma_counts: Vec<[u8; 16]>,
    chroma_counts: [Vec<Vec<u8>>; 2],
    chroma_modes: Vec<u8>,
    luma_modes: Vec<[u8; 16]>,
    transform8: Vec<bool>,
    i16_or_pcm: Vec<bool>,
    coded_flags: Vec<u16>,
    slice_id: Vec<usize>,
    qp: Vec<u8>,
}

impl CabacPictureState {
    fn new(mb_width: usize, mb_height: usize) -> Self {
        let total_mbs = mb_width * mb_height;
        Self {
            mb_width,
            mb_height,
            decoded: vec![false; total_mbs],
            luma_counts: vec![[0; 16]; total_mbs],
            chroma_counts: [vec![vec![0; 4]; total_mbs], vec![vec![0; 4]; total_mbs]],
            chroma_modes: vec![0; total_mbs],
            luma_modes: vec![[0; 16]; total_mbs],
            transform8: vec![false; total_mbs],
            i16_or_pcm: vec![false; total_mbs],
            coded_flags: vec![0; total_mbs],
            slice_id: vec![0; total_mbs],
            qp: vec![0; total_mbs],
        }
    }

    fn deblock_map(&self) -> DeblockMap {
        DeblockMap {
            mb_width: self.mb_width,
            mb_height: self.mb_height,
            decoded: self.decoded.clone(),
            qp: self.qp.clone(),
            luma_non_zero: self.luma_counts.clone(),
            chroma_non_zero: self.chroma_counts.clone(),
        }
    }
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
    if !slice.is_i() || !pps.entropy_coding {
        return Err(H264CabacError::Unsupported("non-CABAC-I slice"));
    }
    if slice.data_offset_bits % 8 != 0 {
        return Err(H264CabacError::Malformed("CABAC byte alignment"));
    }
    if mb_width * mb_height != 1 {
        return decode_cabac_i_slice_multi(
            rbsp, slice, sps, pps, mb_width, mb_height, luma, chroma_cb, chroma_cr,
        );
    }
    let offset = slice.data_offset_bits / 8;
    let data = rbsp
        .get(offset..)
        .ok_or(H264CabacError::Malformed("CABAC data offset"))?;
    let mut decoder = CabacDecoder::new_h264(data);
    let slice_qp = pps.pic_init_qp + slice.qp_delta;
    let mut contexts = init_i_contexts(slice_qp);
    let mut last_qscale_diff = 0;

    let (kind, luma_prediction_modes, mut cbp) = if decoder.decode_bin(&mut contexts[3]) == 0 {
        let transform_8x8 = pps.transform_8x8 && decoder.decode_bin(&mut contexts[399]) != 0;
        let modes = decode_cabac_luma_modes(
            &mut decoder,
            &mut contexts,
            transform_8x8,
            super::h264_mb::IntraModeNeighbors::unavailable(),
        );
        let kind = if transform_8x8 {
            IntraMacroblockKind::I8x8
        } else {
            IntraMacroblockKind::I4x4
        };
        (kind, modes, 0)
    } else {
        if decoder.decode_terminate() {
            let context = MacroblockContext::from_sps_pps(sps, pps)
                .map_err(|_| H264CabacError::Malformed("SPS"))?;
            let (macroblock, residual) = decode_i_pcm_macroblock(&mut decoder, context)?;
            let options = super::h264_residual::ResidualPlaneOptions {
                qp: 0,
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
            let residual_planes =
                assemble_i_macroblock_residual(&macroblock, &residual, context, &options)
                    .map_err(|_| H264CabacError::Malformed("I_PCM residual"))?;
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
            .map_err(|_| H264CabacError::Malformed("I_PCM reconstruction"))?;
            if !decoder.decode_terminate() {
                return Err(H264CabacError::Malformed("missing end_of_slice_flag"));
            }
            return Ok(DecodedCabacSlice {
                macroblocks: 1,
                end_bit: slice.data_offset_bits + decoder.bit_pos(),
                final_qp: 0,
                deblock: DeblockMap {
                    mb_width: 1,
                    mb_height: 1,
                    decoded: vec![true],
                    qp: vec![0],
                    luma_non_zero: vec![residual.luma_non_zero],
                    chroma_non_zero: [
                        vec![residual.chroma_non_zero[0].clone()],
                        vec![residual.chroma_non_zero[1].clone()],
                    ],
                },
            });
        }
        let mut type_index = 1usize;
        type_index += 12 * decoder.decode_bin(&mut contexts[6]) as usize;
        if decoder.decode_bin(&mut contexts[7]) != 0 {
            type_index += 4 + 4 * decoder.decode_bin(&mut contexts[8]) as usize;
        }
        type_index += 2 * decoder.decode_bin(&mut contexts[9]) as usize;
        type_index += decoder.decode_bin(&mut contexts[10]) as usize;
        let decoded_prediction_mode = i16_prediction_mode(type_index);
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

    if matches!(kind, IntraMacroblockKind::I4x4 | IntraMacroblockKind::I8x8) {
        let mut decoded_cbp = 0u8;
        decoded_cbp |= decoder.decode_bin(&mut contexts[73]) as u8;
        let context = usize::from(decoded_cbp & 1 == 0);
        decoded_cbp |= (decoder.decode_bin(&mut contexts[73 + context]) as u8) << 1;
        let context = 2 * usize::from(decoded_cbp & 1 == 0);
        decoded_cbp |= (decoder.decode_bin(&mut contexts[73 + context]) as u8) << 2;
        let context = usize::from(decoded_cbp & 4 == 0) + 2 * usize::from(decoded_cbp & 2 == 0);
        decoded_cbp |= (decoder.decode_bin(&mut contexts[73 + context]) as u8) << 3;
        let chroma_cbp = if decoder.decode_bin(&mut contexts[77]) == 0 {
            0
        } else if decoder.decode_bin(&mut contexts[81]) == 0 {
            1
        } else {
            2
        };
        cbp = decoded_cbp | (chroma_cbp << 4);
    }

    let context =
        MacroblockContext::from_sps_pps(sps, pps).map_err(|_| H264CabacError::Malformed("SPS"))?;
    let qp_delta = if matches!(kind, IntraMacroblockKind::I16x16 { .. }) || cbp != 0 {
        decode_cabac_qp_delta(&mut decoder, &mut contexts, &mut last_qscale_diff)?
    } else {
        0
    };
    let max_qp = 51 + 6 * i32::from(context.bit_depth_luma - 8);
    let qp = add_cabac_qp(slice_qp, qp_delta, max_qp)?;
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
                    ResidualCategory::LumaAc16,
                    15,
                    cbf_context,
                )?;
                if let Some(block_residual) = decoded {
                    residual.luma_non_zero[block] = block_residual.total_coeff as u8;
                    residual.luma_4x4[block] = Some(block_residual);
                }
            }
        }
    } else if kind == IntraMacroblockKind::I4x4 {
        for group in 0..4 {
            if cbp & (1 << group) == 0 {
                continue;
            }
            for sub in 0..4 {
                let block = I4X4_BLOCK_ORDER[group * 4 + sub];
                let cbf_context = luma_4x4_cbf_context(&residual.luma_non_zero, block);
                if let Some(block_residual) = decode_cabac_block(
                    &mut decoder,
                    &mut contexts,
                    ResidualCategory::Luma4x4,
                    16,
                    cbf_context,
                )? {
                    residual.luma_non_zero[block] = block_residual.total_coeff as u8;
                    residual.luma_4x4[block] = Some(block_residual);
                }
            }
        }
    } else if kind == IntraMacroblockKind::I8x8 {
        for group in 0..4 {
            if cbp & (1 << group) == 0 {
                continue;
            }
            let block_residual = decode_cabac_residual_8x8(&mut decoder, &mut contexts)?;
            set_luma8_non_zero(
                &mut residual.luma_non_zero,
                group,
                block_residual.total_coeff[0] as u8,
            );
            residual.luma_8x8[group] = Some(block_residual);
        }
    }

    if matches!(
        kind,
        IntraMacroblockKind::I16x16 { .. } | IntraMacroblockKind::I4x4 | IntraMacroblockKind::I8x8
    ) && cbp & 0x30 != 0
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
        scan8: if slice.field_pic {
            super::h264::CavlcScan8x8::CabacField
        } else {
            super::h264::CavlcScan8x8::CabacFrame
        },
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

fn decode_cabac_i_slice_multi(
    rbsp: &[u8],
    slice: &SliceHeader,
    sps: &Sps,
    pps: &Pps,
    mb_width: usize,
    mb_height: usize,
    luma: &mut [u16],
    chroma_cb: Option<&mut [u16]>,
    chroma_cr: Option<&mut [u16]>,
) -> Result<DecodedCabacSlice, H264CabacError> {
    if sps.coded_width % 16 != 0 || sps.coded_height % 16 != 0 || mb_width == 0 || mb_height == 0 {
        return Err(H264CabacError::Malformed("macroblock geometry"));
    }
    let context =
        MacroblockContext::from_sps_pps(sps, pps).map_err(|_| H264CabacError::Malformed("SPS"))?;
    if context.chroma_format_idc != 1 {
        return Err(H264CabacError::Unsupported(
            "CABAC multi-macroblock non-4:2:0 chroma",
        ));
    }
    let mut state = CabacPictureState::new(mb_width, mb_height);
    decode_cabac_i_slice_into_picture(
        rbsp, slice, sps, pps, context, luma, chroma_cb, chroma_cr, &mut state, 0,
    )
}

/// Decode consecutive CABAC I-slice packets into one picture and return the
/// combined deblocking map. Slice contexts restart per packet, while macroblock
/// mode/CBF/transform metadata remains available across slice boundaries.
pub fn decode_cabac_i_picture_slices(
    slices: &[(&[u8], &SliceHeader)],
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
    if mb_width == 0 || mb_height == 0 || sps.coded_width % 16 != 0 || sps.coded_height % 16 != 0 {
        return Err(H264CabacError::Malformed("macroblock geometry"));
    }
    let context =
        MacroblockContext::from_sps_pps(sps, pps).map_err(|_| H264CabacError::Malformed("SPS"))?;
    if context.chroma_format_idc != 1 {
        return Err(H264CabacError::Unsupported(
            "CABAC multi-slice non-4:2:0 chroma",
        ));
    }
    let mut state = CabacPictureState::new(mb_width, mb_height);
    let mut chroma_cb = chroma_cb;
    let mut chroma_cr = chroma_cr;
    let mut macroblocks = 0;
    let mut end_bit = 0;
    let mut final_qp = pps.pic_init_qp as u8;
    for (index, (rbsp, slice)) in slices.iter().enumerate() {
        let decoded = decode_cabac_i_slice_into_picture(
            rbsp,
            slice,
            sps,
            pps,
            context,
            luma,
            chroma_cb.as_deref_mut(),
            chroma_cr.as_deref_mut(),
            &mut state,
            index,
        )?;
        macroblocks += decoded.macroblocks;
        end_bit = decoded.end_bit;
        final_qp = decoded.final_qp;
        if index + 1 == slices.len() {
            return Ok(DecodedCabacSlice {
                macroblocks,
                end_bit,
                final_qp,
                deblock: state.deblock_map(),
            });
        }
    }
    Ok(DecodedCabacSlice {
        macroblocks,
        end_bit,
        final_qp,
        deblock: state.deblock_map(),
    })
}

#[allow(clippy::too_many_arguments)]
fn decode_cabac_i_slice_into_picture(
    rbsp: &[u8],
    slice: &SliceHeader,
    sps: &Sps,
    pps: &Pps,
    context: MacroblockContext,
    luma: &mut [u16],
    chroma_cb: Option<&mut [u16]>,
    chroma_cr: Option<&mut [u16]>,
    state: &mut CabacPictureState,
    slice_id: usize,
) -> Result<DecodedCabacSlice, H264CabacError> {
    let offset = slice.data_offset_bits / 8;
    let data = rbsp
        .get(offset..)
        .ok_or(H264CabacError::Malformed("CABAC data offset"))?;
    let mut decoder = CabacDecoder::new_h264(data);
    let slice_qp = pps.pic_init_qp + slice.qp_delta;
    let mut contexts = init_i_contexts(slice_qp);
    let mut qp = slice_qp;
    let mut last_qscale_diff = 0;

    let total_mbs = state.mb_width * state.mb_height;
    let first_mb =
        usize::try_from(slice.first_mb).map_err(|_| H264CabacError::Malformed("first_mb"))?;
    if first_mb >= total_mbs {
        return Err(H264CabacError::Malformed("first_mb_in_slice"));
    }

    let width = sps.coded_width as usize;
    let height = sps.coded_height as usize;
    let chroma_width = width / 2;
    let chroma_height = height / 2;
    let mut luma_plane = PlaneMut {
        data: luma,
        width,
        height,
        stride: width,
    };
    let mut cb_plane = chroma_cb.map(|data| PlaneMut {
        data,
        width: chroma_width,
        height: chroma_height,
        stride: chroma_width,
    });
    let mut cr_plane = chroma_cr.map(|data| PlaneMut {
        data,
        width: chroma_width,
        height: chroma_height,
        stride: chroma_width,
    });

    let mut macroblocks = 0;
    for mb_index in first_mb..total_mbs {
        let neighbors = collect_cabac_neighbors(
            mb_index,
            state.mb_width,
            slice_id,
            &state.slice_id,
            &state.decoded,
            &state.luma_counts,
            &state.chroma_counts,
            &state.chroma_modes,
            &state.luma_modes,
            &state.transform8,
            &state.i16_or_pcm,
            &state.coded_flags,
        );
        let decoded_mb = decode_cabac_i_macroblock(
            &mut decoder,
            &mut contexts,
            context,
            neighbors,
            &mut last_qscale_diff,
        )?;
        let max_qp = 51 + 6 * i32::from(context.bit_depth_luma - 8);
        qp = add_cabac_qp(qp, decoded_mb.macroblock.qp_delta, max_qp)?;
        let options = super::h264_residual::ResidualPlaneOptions {
            qp: qp as u8,
            chroma_qp_offset: [pps.chroma_qp_offset, pps.second_chroma_qp_offset],
            bit_depth_luma: context.bit_depth_luma,
            bit_depth_chroma: context.bit_depth_chroma,
            transform_bypass: false,
            scan4: super::h264::CavlcScan4x4::Frame,
            scan8: if slice.field_pic {
                super::h264::CavlcScan8x8::CabacField
            } else {
                super::h264::CavlcScan8x8::CabacFrame
            },
            luma_4x4_scaling: None,
            luma_8x8_scaling: None,
            chroma_4x4_scaling: [None, None],
        };
        let residual_planes = assemble_i_macroblock_residual(
            &decoded_mb.macroblock,
            &decoded_mb.residual,
            context,
            &options,
        )
        .map_err(|_| H264CabacError::Malformed("empty residual"))?;
        let mb_x = (mb_index % state.mb_width) * 16;
        let mb_y = (mb_index / state.mb_width) * 16;
        reconstruct_i_macroblock_with_availability(
            &decoded_mb.macroblock,
            &residual_planes,
            context.bit_depth_luma.min(context.bit_depth_chroma),
            &mut luma_plane,
            cb_plane.as_mut(),
            cr_plane.as_mut(),
            mb_x,
            mb_y,
            neighbors.top_recon_available,
            neighbors.left_recon_available,
        )
        .map_err(|_| H264CabacError::Malformed("macroblock reconstruction"))?;

        state.decoded[mb_index] = true;
        state.luma_counts[mb_index] = decoded_mb.residual.luma_non_zero;
        state.chroma_counts[0][mb_index].copy_from_slice(&decoded_mb.residual.chroma_non_zero[0]);
        state.chroma_counts[1][mb_index].copy_from_slice(&decoded_mb.residual.chroma_non_zero[1]);
        state.chroma_modes[mb_index] = decoded_mb.chroma_prediction_mode;
        state.luma_modes[mb_index] = if decoded_mb.macroblock.kind == IntraMacroblockKind::IPcm {
            [2; 16]
        } else {
            intra_mode_grid(&decoded_mb.macroblock)
        };
        state.transform8[mb_index] = decoded_mb.macroblock.kind == IntraMacroblockKind::I8x8;
        state.i16_or_pcm[mb_index] = matches!(
            decoded_mb.macroblock.kind,
            IntraMacroblockKind::I16x16 { .. } | IntraMacroblockKind::IPcm
        );
        state.coded_flags[mb_index] = decoded_mb.coded_flags;
        state.slice_id[mb_index] = slice_id;
        state.qp[mb_index] = if decoded_mb.macroblock.kind == IntraMacroblockKind::IPcm {
            0
        } else {
            qp as u8
        };
        macroblocks += 1;

        let end_of_slice = decoder.decode_terminate();
        if end_of_slice {
            break;
        }
        if mb_index + 1 == total_mbs {
            return Err(H264CabacError::Malformed("missing end_of_slice_flag"));
        }
    }

    Ok(DecodedCabacSlice {
        macroblocks,
        end_bit: slice.data_offset_bits + decoder.bit_pos(),
        final_qp: qp as u8,
        deblock: state.deblock_map(),
    })
}

fn collect_cabac_neighbors(
    index: usize,
    width: usize,
    slice_id: usize,
    slice_ids: &[usize],
    decoded: &[bool],
    luma_counts: &[[u8; 16]],
    chroma_counts: &[Vec<Vec<u8>>; 2],
    chroma_modes: &[u8],
    luma_modes: &[[u8; 16]],
    transform8: &[bool],
    i16_or_pcm: &[bool],
    coded_flags: &[u16],
) -> CabacMbNeighbors {
    let picture_left = index
        .checked_sub(1)
        .filter(|_| index % width != 0)
        .filter(|&left| decoded[left]);
    let picture_top = index.checked_sub(width).filter(|&top| decoded[top]);
    let left = picture_left.filter(|&left| slice_ids[left] == slice_id);
    let top = picture_top.filter(|&top| slice_ids[top] == slice_id);
    let mut neighbors = CabacMbNeighbors {
        left_decoded: left.is_some(),
        top_decoded: top.is_some(),
        left_recon_available: left.is_some(),
        top_recon_available: top.is_some(),
        left_modes_available: left.is_some(),
        top_modes_available: top.is_some(),
        left_i16_or_pcm: left.is_some_and(|left| i16_or_pcm[left]),
        top_i16_or_pcm: top.is_some_and(|top| i16_or_pcm[top]),
        left_flags: left.map_or(0x7cf, |left| coded_flags[left]),
        top_flags: top.map_or(0x7cf, |top| coded_flags[top]),
        left_chroma_mode: left.map_or(0, |left| chroma_modes[left]),
        top_chroma_mode: top.map_or(0, |top| chroma_modes[top]),
        left_luma: [0; 4],
        top_luma: [0; 4],
        left_modes: [0; 4],
        top_modes: [0; 4],
        left_transform8: left.is_some_and(|left| transform8[left]),
        top_transform8: top.is_some_and(|top| transform8[top]),
        left_chroma: [[0; 4]; 2],
        top_chroma: [[0; 4]; 2],
    };
    if let Some(left) = left {
        neighbors.left_luma = std::array::from_fn(|row| luma_counts[left][row * 4 + 3]);
        neighbors.left_modes = std::array::from_fn(|row| luma_modes[left][row * 4 + 3]);
        for plane in 0..2 {
            for row in 0..2 {
                neighbors.left_chroma[plane][row] = chroma_counts[plane][left][row * 2 + 1];
            }
        }
    }
    if let Some(top) = top {
        neighbors.top_luma = std::array::from_fn(|column| luma_counts[top][12 + column]);
        neighbors.top_modes = std::array::from_fn(|column| luma_modes[top][12 + column]);
        for plane in 0..2 {
            for column in 0..2 {
                neighbors.top_chroma[plane][column] = chroma_counts[plane][top][2 + column];
            }
        }
    }
    neighbors
}

fn decode_cabac_p16_macroblock(
    decoder: &mut CabacDecoder<'_>,
    contexts: &mut [Context],
    context: MacroblockContext,
    neighbors: CabacMbNeighbors,
    last_qscale_diff: &mut i32,
) -> Result<DecodedCabacPMacroblock, H264CabacError> {
    if decoder.decode_bin(&mut contexts[14]) != 0 {
        return Err(H264CabacError::Unsupported(
            "P slice contains an intra macroblock",
        ));
    }
    if decoder.decode_bin(&mut contexts[15]) != 0 {
        return Err(H264CabacError::Unsupported(
            "P slice contains P_16x8/P_8x16",
        ));
    }
    if decoder.decode_bin(&mut contexts[16]) != 0 {
        return Err(H264CabacError::Unsupported("P slice contains P_8x8"));
    }

    let mvd_x = decode_cabac_mvd(decoder, contexts, 40, 0)?;
    let mvd_y = decode_cabac_mvd(decoder, contexts, 47, 0)?;
    let cbp = decode_cabac_cbp(decoder, contexts, neighbors);
    let qp_delta = if cbp != 0 {
        decode_cabac_qp_delta(decoder, contexts, last_qscale_diff)?
    } else {
        *last_qscale_diff = 0;
        0
    };
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

    for group in 0..4 {
        if cbp & (1 << group) == 0 {
            continue;
        }
        for sub in 0..4 {
            let block = I4X4_BLOCK_ORDER[group * 4 + sub];
            let cbf_context = luma_4x4_cbf_context_inter(&residual.luma_non_zero, block, neighbors);
            if let Some(block_residual) = decode_cabac_block(
                decoder,
                contexts,
                ResidualCategory::Luma4x4,
                16,
                cbf_context,
            )? {
                residual.luma_non_zero[block] = block_residual.total_coeff as u8;
                residual.luma_4x4[block] = Some(block_residual);
            }
        }
    }

    if cbp & 0x30 != 0 && context.chroma_format_idc == 1 {
        for plane in 0..2 {
            let cbf_context = coded_block_flag_context_inter(97, 0x40u16 << plane, neighbors);
            residual.chroma_dc[plane] = decode_cabac_block(
                decoder,
                contexts,
                ResidualCategory::ChromaDc,
                4,
                cbf_context,
            )?;
        }
        if cbp & 0x20 != 0 {
            for plane in 0..2 {
                for block in 0..4 {
                    let cbf_context = chroma_ac_cbf_context_inter(
                        &residual.chroma_non_zero[plane],
                        block,
                        plane,
                        neighbors,
                    );
                    if let Some(block_residual) = decode_cabac_block(
                        decoder,
                        contexts,
                        ResidualCategory::ChromaAc,
                        15,
                        cbf_context,
                    )? {
                        residual.chroma_non_zero[plane][block] = block_residual.total_coeff as u8;
                        residual.chroma_ac[plane][block] = Some(block_residual);
                    }
                }
            }
        }
    }
    residual.end_bit = decoder.bit_pos();

    let mut coded_flags = u16::from(cbp);
    for plane in 0..2 {
        if residual.chroma_dc[plane]
            .as_ref()
            .is_some_and(|residual| residual.total_coeff != 0)
        {
            coded_flags |= 0x40u16 << plane;
        }
    }
    Ok(DecodedCabacPMacroblock {
        residual,
        qp_delta,
        motion_vector_delta: [mvd_x, mvd_y],
        coded_flags,
    })
}

fn decode_cabac_mvd(
    decoder: &mut CabacDecoder<'_>,
    contexts: &mut [Context],
    context_base: usize,
    neighbor_abs_sum: i32,
) -> Result<i32, H264CabacError> {
    let context =
        context_base + usize::from(neighbor_abs_sum > 2) + usize::from(neighbor_abs_sum > 32);
    if decoder.decode_bin(&mut contexts[context]) == 0 {
        return Ok(0);
    }
    let mut magnitude = 1i32;
    let mut context = context_base + 3;
    while magnitude < 9 && decoder.decode_bin(&mut contexts[context]) != 0 {
        if magnitude < 4 {
            context += 1;
        }
        magnitude += 1;
    }
    if magnitude >= 9 {
        let mut shift = 3u32;
        while decoder.decode_bypass() != 0 {
            magnitude = magnitude
                .checked_add(1 << shift)
                .ok_or(H264CabacError::Malformed("motion-vector delta"))?;
            shift += 1;
            if shift > 24 {
                return Err(H264CabacError::Malformed("motion-vector delta"));
            }
        }
        while shift > 0 {
            shift -= 1;
            magnitude += (decoder.decode_bypass() as i32) << shift;
        }
    }
    Ok(if decoder.decode_bypass() == 0 {
        -magnitude
    } else {
        magnitude
    })
}

fn decode_cabac_i_macroblock(
    decoder: &mut CabacDecoder<'_>,
    contexts: &mut [Context],
    context: MacroblockContext,
    neighbors: CabacMbNeighbors,
    last_qscale_diff: &mut i32,
) -> Result<DecodedCabacMacroblock, H264CabacError> {
    let mb_type_context =
        3 + usize::from(neighbors.left_i16_or_pcm) + usize::from(neighbors.top_i16_or_pcm);
    let (kind, luma_prediction_modes, mut cbp) =
        if decoder.decode_bin(&mut contexts[mb_type_context]) == 0 {
            let transform_context = 399
                + usize::from(neighbors.left_transform8)
                + usize::from(neighbors.top_transform8);
            let transform_8x8 =
                context.transform_8x8 && decoder.decode_bin(&mut contexts[transform_context]) != 0;
            let modes = decode_cabac_luma_modes(
                decoder,
                contexts,
                transform_8x8,
                IntraModeNeighbors {
                    left: std::array::from_fn(|row| {
                        neighbors
                            .left_modes_available
                            .then_some(neighbors.left_modes[row])
                    }),
                    top: std::array::from_fn(|column| {
                        neighbors
                            .top_modes_available
                            .then_some(neighbors.top_modes[column])
                    }),
                },
            );
            let kind = if transform_8x8 {
                IntraMacroblockKind::I8x8
            } else {
                IntraMacroblockKind::I4x4
            };
            (kind, modes, 0)
        } else {
            if decoder.decode_terminate() {
                let (macroblock, residual) = decode_i_pcm_macroblock(decoder, context)?;
                return Ok(DecodedCabacMacroblock {
                    macroblock,
                    residual,
                    chroma_prediction_mode: 0,
                    coded_flags: 0xf7ef,
                });
            }
            let mut type_index = 1usize;
            type_index += 12 * decoder.decode_bin(&mut contexts[6]) as usize;
            if decoder.decode_bin(&mut contexts[7]) != 0 {
                type_index += 4 + 4 * decoder.decode_bin(&mut contexts[8]) as usize;
            }
            type_index += 2 * decoder.decode_bin(&mut contexts[9]) as usize;
            type_index += decoder.decode_bin(&mut contexts[10]) as usize;
            let decoded_prediction_mode = i16_prediction_mode(type_index);
            let prediction_mode = check_i16_prediction_mode(
                decoded_prediction_mode,
                neighbors.top_modes_available,
                neighbors.left_modes_available,
            );
            let cbp = match type_index {
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
                cbp,
            )
        };

    let chroma_context = 64
        + usize::from(neighbors.left_decoded && neighbors.left_chroma_mode != 0)
        + 2 * usize::from(neighbors.top_decoded && neighbors.top_chroma_mode != 0);
    let decoded_chroma_prediction_mode = if decoder.decode_bin(&mut contexts[chroma_context]) == 0 {
        0
    } else if decoder.decode_bin(&mut contexts[67]) == 0 {
        1
    } else if decoder.decode_bin(&mut contexts[67]) == 0 {
        2
    } else {
        3
    };
    let chroma_prediction_mode = check_chroma_prediction_mode(
        decoded_chroma_prediction_mode,
        neighbors.top_modes_available,
        neighbors.left_modes_available,
    );

    if !matches!(kind, IntraMacroblockKind::I16x16 { .. }) {
        cbp = decode_cabac_cbp(decoder, contexts, neighbors);
    }
    let qp_delta = decode_cabac_qp_delta(decoder, contexts, last_qscale_diff)?;
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

    match kind {
        IntraMacroblockKind::I16x16 { .. } => {
            let luma_dc_context = coded_block_flag_context(85, 0x100, neighbors);
            let luma_dc = if decoder.decode_bin(&mut contexts[luma_dc_context]) == 0 {
                super::h264_cavlc::CavlcResidual {
                    scan: [0; 16],
                    total_coeff: 0,
                    trailing_ones: 0,
                    bits_consumed: 0,
                }
            } else {
                decode_cabac_residual(decoder, contexts, ResidualCategory::LumaDc, 16)?
            };
            residual.luma_dc_non_zero = luma_dc.total_coeff as u8;
            residual.luma_dc = Some(luma_dc);

            if cbp & 0x0f != 0 {
                for block in 0..16 {
                    let cbf_context = luma_ac_cbf_context_with_neighbors(
                        &residual.luma_non_zero,
                        block,
                        neighbors,
                    );
                    if let Some(block_residual) = decode_cabac_block(
                        decoder,
                        contexts,
                        ResidualCategory::LumaAc16,
                        15,
                        cbf_context,
                    )? {
                        residual.luma_non_zero[block] = block_residual.total_coeff as u8;
                        residual.luma_4x4[block] = Some(block_residual);
                    }
                }
            }
        }
        IntraMacroblockKind::I4x4 => {
            for group in 0..4 {
                if cbp & (1 << group) == 0 {
                    continue;
                }
                for sub in 0..4 {
                    let block = I4X4_BLOCK_ORDER[group * 4 + sub];
                    let cbf_context = luma_4x4_cbf_context_with_neighbors(
                        &residual.luma_non_zero,
                        block,
                        neighbors,
                    );
                    if let Some(block_residual) = decode_cabac_block(
                        decoder,
                        contexts,
                        ResidualCategory::Luma4x4,
                        16,
                        cbf_context,
                    )? {
                        residual.luma_non_zero[block] = block_residual.total_coeff as u8;
                        residual.luma_4x4[block] = Some(block_residual);
                    }
                }
            }
        }
        IntraMacroblockKind::I8x8 => {
            for group in 0..4 {
                if cbp & (1 << group) == 0 {
                    continue;
                }
                let block_residual = decode_cabac_residual_8x8(decoder, contexts)?;
                set_luma8_non_zero(
                    &mut residual.luma_non_zero,
                    group,
                    block_residual.total_coeff[0] as u8,
                );
                residual.luma_8x8[group] = Some(block_residual);
            }
        }
        IntraMacroblockKind::IPcm => unreachable!(),
    }

    if cbp & 0x30 != 0 && context.chroma_format_idc == 1 {
        for plane in 0..2 {
            let cbf_context = coded_block_flag_context(97, 0x40u16 << plane, neighbors);
            residual.chroma_dc[plane] = decode_cabac_block(
                decoder,
                contexts,
                ResidualCategory::ChromaDc,
                4,
                cbf_context,
            )?;
        }
        if cbp & 0x20 != 0 {
            for plane in 0..2 {
                for block in 0..4 {
                    let cbf_context = chroma_ac_cbf_context_with_neighbors(
                        &residual.chroma_non_zero[plane],
                        block,
                        plane,
                        neighbors,
                    );
                    if let Some(block_residual) = decode_cabac_block(
                        decoder,
                        contexts,
                        ResidualCategory::ChromaAc,
                        15,
                        cbf_context,
                    )? {
                        residual.chroma_non_zero[plane][block] = block_residual.total_coeff as u8;
                        residual.chroma_ac[plane][block] = Some(block_residual);
                    }
                }
            }
        }
    }
    residual.end_bit = decoder.bit_pos();
    let mut coded_flags = u16::from(cbp);
    if residual.luma_dc_non_zero != 0 {
        coded_flags |= 0x100;
    }
    for plane in 0..2 {
        if residual.chroma_dc[plane]
            .as_ref()
            .is_some_and(|residual| residual.total_coeff != 0)
        {
            coded_flags |= 0x40u16 << plane;
        }
    }

    Ok(DecodedCabacMacroblock {
        macroblock: ParsedIMacroblock {
            kind,
            luma_prediction_modes,
            chroma_prediction_mode: Some(chroma_prediction_mode),
            coded_block_pattern: cbp,
            qp_delta,
            pcm_planes: Vec::new(),
            residual_bit_offset: 0,
        },
        residual,
        chroma_prediction_mode,
        coded_flags,
    })
}

fn coded_block_flag_context(base: usize, mask: u16, neighbors: CabacMbNeighbors) -> usize {
    let left = !neighbors.left_decoded || neighbors.left_flags & mask != 0;
    let top = !neighbors.top_decoded || neighbors.top_flags & mask != 0;
    base + usize::from(left) + 2 * usize::from(top)
}

fn coded_block_flag_context_inter(base: usize, mask: u16, neighbors: CabacMbNeighbors) -> usize {
    let left = neighbors.left_decoded && neighbors.left_flags & mask != 0;
    let top = neighbors.top_decoded && neighbors.top_flags & mask != 0;
    base + usize::from(left) + 2 * usize::from(top)
}

fn decode_cabac_cbp(
    decoder: &mut CabacDecoder<'_>,
    contexts: &mut [Context],
    neighbors: CabacMbNeighbors,
) -> u8 {
    let left = neighbors.left_flags;
    let top = neighbors.top_flags;
    let mut cbp = 0u8;

    let context = usize::from(left & 0x02 == 0) + 2 * usize::from(top & 0x04 == 0);
    cbp |= decoder.decode_bin(&mut contexts[73 + context]) as u8;
    let context = usize::from(cbp & 0x01 == 0) + 2 * usize::from(top & 0x08 == 0);
    cbp |= (decoder.decode_bin(&mut contexts[73 + context]) as u8) << 1;
    let context = usize::from(left & 0x08 == 0) + 2 * usize::from(cbp & 0x01 == 0);
    cbp |= (decoder.decode_bin(&mut contexts[73 + context]) as u8) << 2;
    let context = usize::from(cbp & 0x04 == 0) + 2 * usize::from(cbp & 0x02 == 0);
    cbp |= (decoder.decode_bin(&mut contexts[73 + context]) as u8) << 3;

    let left_chroma = (left >> 4) & 0x03;
    let top_chroma = (top >> 4) & 0x03;
    let context = usize::from(left_chroma > 0) + 2 * usize::from(top_chroma > 0);
    let chroma = if decoder.decode_bin(&mut contexts[77 + context]) == 0 {
        0
    } else {
        let context = 4 + usize::from(left_chroma == 2) + 2 * usize::from(top_chroma == 2);
        1 + decoder.decode_bin(&mut contexts[77 + context]) as u8
    };
    cbp | (chroma << 4)
}

fn decode_cabac_qp_delta(
    decoder: &mut CabacDecoder<'_>,
    contexts: &mut [Context],
    last_qscale_diff: &mut i32,
) -> Result<i32, H264CabacError> {
    let first_context = 60 + usize::from(*last_qscale_diff != 0);
    if decoder.decode_bin(&mut contexts[first_context]) == 0 {
        *last_qscale_diff = 0;
        return Ok(0);
    }
    let mut magnitude = 1;
    let mut context = 2;
    while decoder.decode_bin(&mut contexts[60 + context]) != 0 {
        context = 3;
        magnitude += 1;
        if magnitude > 102 {
            return Err(H264CabacError::Malformed("mb_qp_delta"));
        }
    }
    let delta = qp_delta_from_magnitude(magnitude);
    *last_qscale_diff = delta;
    Ok(delta)
}

fn qp_delta_from_magnitude(magnitude: i32) -> i32 {
    if magnitude & 1 != 0 {
        (magnitude + 1) >> 1
    } else {
        -((magnitude + 1) >> 1)
    }
}

fn add_cabac_qp(qp: i32, delta: i32, max_qp: i32) -> Result<i32, H264CabacError> {
    if !(0..=max_qp).contains(&qp) {
        return Err(H264CabacError::Malformed("slice QP"));
    }
    let mut value = qp + delta;
    if value > max_qp {
        value -= max_qp + 1;
    } else if value < 0 {
        value += max_qp + 1;
    }
    Ok(value)
}

fn init_p_contexts(
    cabac_init_idc: u32,
    slice_qp: i32,
    bit_depth_luma: u8,
) -> Result<[Context; 1024], H264CabacError> {
    let table = PB_INIT
        .get(cabac_init_idc as usize)
        .ok_or(H264CabacError::Malformed("cabac_init_idc"))?;
    let init_qp = (slice_qp - 6 * (i32::from(bit_depth_luma) - 8)).clamp(0, 51);
    Ok(std::array::from_fn(|index| {
        let [m, n] = table.get(index).copied().unwrap_or([0, 0]);
        init_context(m, n, init_qp)
    }))
}

fn init_context(m: i8, n: i8, slice_qp: i32) -> Context {
    let mut pre = 2 * (((i32::from(m) * slice_qp) >> 4) + i32::from(n)) - 127;
    pre ^= pre >> 31;
    if pre > 124 {
        pre = 124 + (pre & 1);
    }
    Context::new((pre >> 1) as u8, (pre & 1) as u8)
}

fn init_i_contexts(slice_qp: i32) -> [Context; 1024] {
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
    const I8X8_FLAG: [[i8; 2]; 3] = [[31, 21], [31, 31], [25, 50]];
    const I8X8_ABSOLUTE: [[i8; 2]; 10] = [
        [-3, 75],
        [-1, 23],
        [1, 34],
        [1, 43],
        [0, 54],
        [-2, 55],
        [0, 61],
        [1, 64],
        [0, 68],
        [-9, 92],
    ];
    const I8X8_SIGNIFICANT: [[i8; 2]; 15] = [
        [-17, 120],
        [-20, 112],
        [-18, 114],
        [-11, 85],
        [-15, 92],
        [-14, 89],
        [-26, 71],
        [-15, 81],
        [-14, 80],
        [0, 68],
        [-14, 70],
        [-24, 56],
        [-23, 68],
        [-24, 50],
        [-11, 74],
    ];
    const I8X8_LAST: [[i8; 2]; 9] = [
        [23, -13],
        [26, -13],
        [40, -15],
        [49, -14],
        [44, 3],
        [45, 6],
        [44, 34],
        [33, 54],
        [19, 82],
    ];
    let mut init = [[0i8; 2]; 1024];
    init[..HEAD.len()].copy_from_slice(&HEAD);
    init[60..86].copy_from_slice(&TAIL);
    init[86..105].copy_from_slice(&CODED_BLOCK_FLAG);
    init[105..276].copy_from_slice(&RESIDUAL);
    init[399..402].copy_from_slice(&I8X8_FLAG);
    init[426..436].copy_from_slice(&I8X8_ABSOLUTE);
    init[402..417].copy_from_slice(&I8X8_SIGNIFICANT);
    init[417..426].copy_from_slice(&I8X8_LAST);
    std::array::from_fn(|index| {
        let [m, n] = init[index];
        init_context(m, n, slice_qp)
    })
}

#[derive(Debug, Clone, Copy)]
enum ResidualCategory {
    LumaDc,
    LumaAc16,
    Luma4x4,
    ChromaDc,
    ChromaAc,
}

impl ResidualCategory {
    fn context_bases(self) -> (usize, usize, usize) {
        match self {
            ResidualCategory::LumaDc => (105, 166, 227),
            ResidualCategory::LumaAc16 => (120, 181, 237),
            ResidualCategory::Luma4x4 => (134, 195, 247),
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

fn decode_cabac_residual_8x8(
    decoder: &mut CabacDecoder<'_>,
    contexts: &mut [Context],
) -> Result<super::h264_cavlc::Cavlc8x8Residual, H264CabacError> {
    const SIGNIFICANT_OFFSET: [usize; 63] = [
        0, 1, 2, 3, 4, 5, 5, 4, 4, 3, 3, 4, 4, 4, 5, 5, 4, 4, 4, 4, 3, 3, 6, 7, 7, 7, 8, 9, 10, 9,
        8, 7, 7, 6, 11, 12, 13, 11, 6, 7, 8, 9, 14, 10, 9, 8, 6, 11, 12, 13, 11, 6, 9, 14, 10, 9,
        11, 12, 13, 11, 14, 10, 12,
    ];
    const LAST_OFFSET: [usize; 63] = [
        0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
        2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5, 5, 6, 6, 6, 6, 7, 7, 7, 7,
        8, 8, 8,
    ];
    const LEVEL_ONE_CONTEXT: [usize; 8] = [1, 2, 3, 4, 0, 0, 0, 0];
    const LEVEL_GT_ONE_CONTEXT: [usize; 8] = [5, 5, 5, 5, 6, 7, 8, 9];
    const LEVEL_ONE_TRANSITION: [usize; 8] = [1, 2, 3, 3, 4, 5, 6, 7];
    const LEVEL_GT_ONE_TRANSITION: [usize; 8] = [4, 4, 4, 4, 5, 6, 7, 7];

    let mut positions = [0usize; 64];
    let mut coefficient_count = 0;
    let mut last = 0;
    while last < 63 {
        let significant_offset = SIGNIFICANT_OFFSET[last];
        let significant_context = 402 + significant_offset;
        let significant = decoder.decode_bin(&mut contexts[significant_context]);
        if significant != 0 {
            positions[coefficient_count] = last;
            coefficient_count += 1;
            let last_offset = LAST_OFFSET[last];
            let last_bit = decoder.decode_bin(&mut contexts[417 + last_offset]);
            if last_bit != 0 {
                last = 64;
                break;
            }
        }
        last += 1;
    }
    if last == 63 {
        positions[coefficient_count] = last;
        coefficient_count += 1;
    }
    if coefficient_count == 0 {
        return Err(H264CabacError::Malformed(
            "8x8 residual block has no significant coefficient",
        ));
    }

    let mut scan = [0i32; 64];
    let mut node_context = 0;
    for position in positions[..coefficient_count].iter().rev() {
        let one_context = 426 + LEVEL_ONE_CONTEXT[node_context];
        let mut magnitude = if decoder.decode_bin(&mut contexts[one_context]) == 0 {
            node_context = LEVEL_ONE_TRANSITION[node_context];
            1
        } else {
            let gt_one_context = 426 + LEVEL_GT_ONE_CONTEXT[node_context];
            node_context = LEVEL_GT_ONE_TRANSITION[node_context];
            let mut value = 2;
            while value < 15 && decoder.decode_bin(&mut contexts[gt_one_context]) != 0 {
                value += 1;
            }
            if value >= 15 {
                let mut suffix_length = 0;
                while decoder.decode_bypass() != 0 && suffix_length < 23 {
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

    Ok(super::h264_cavlc::Cavlc8x8Residual {
        scan,
        total_coeff: [coefficient_count; 4],
        trailing_ones: [0; 4],
        block_bits: [0; 4],
        bits_consumed: 0,
    })
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

fn luma_4x4_cbf_context(non_zero: &[u8; 16], block: usize) -> usize {
    let left = block % 4 == 0 || non_zero[block - 1] != 0;
    let top = block / 4 == 0 || non_zero[block - 4] != 0;
    93 + usize::from(left) + 2 * usize::from(top)
}

fn luma_4x4_cbf_context_with_neighbors(
    non_zero: &[u8; 16],
    block: usize,
    neighbors: CabacMbNeighbors,
) -> usize {
    let row = block / 4;
    let column = block % 4;
    let left = if column == 0 {
        !neighbors.left_decoded || neighbors.left_luma[row] != 0
    } else {
        non_zero[block - 1] != 0
    };
    let top = if row == 0 {
        !neighbors.top_decoded || neighbors.top_luma[column] != 0
    } else {
        non_zero[block - 4] != 0
    };
    93 + usize::from(left) + 2 * usize::from(top)
}

fn luma_4x4_cbf_context_inter(
    non_zero: &[u8; 16],
    block: usize,
    neighbors: CabacMbNeighbors,
) -> usize {
    let row = block / 4;
    let column = block % 4;
    let left = if column == 0 {
        neighbors.left_decoded && neighbors.left_luma[row] != 0
    } else {
        non_zero[block - 1] != 0
    };
    let top = if row == 0 {
        neighbors.top_decoded && neighbors.top_luma[column] != 0
    } else {
        non_zero[block - 4] != 0
    };
    93 + usize::from(left) + 2 * usize::from(top)
}

fn set_luma8_non_zero(non_zero: &mut [u8; 16], group: usize, count: u8) {
    let base_row = (group / 2) * 2;
    let base_column = (group % 2) * 2;
    for row in 0..2 {
        for column in 0..2 {
            non_zero[(base_row + row) * 4 + base_column + column] = count;
        }
    }
}

fn chroma_ac_cbf_context(non_zero: &[u8], block: usize) -> usize {
    let left = block % 2 == 0 || non_zero[block - 1] != 0;
    let top = block / 2 == 0 || non_zero[block - 2] != 0;
    101 + usize::from(left) + 2 * usize::from(top)
}

fn luma_ac_cbf_context_with_neighbors(
    non_zero: &[u8; 16],
    block: usize,
    neighbors: CabacMbNeighbors,
) -> usize {
    let row = block / 4;
    let column = block % 4;
    let left = if column == 0 {
        !neighbors.left_decoded || neighbors.left_luma[row] != 0
    } else {
        non_zero[block - 1] != 0
    };
    let top = if row == 0 {
        !neighbors.top_decoded || neighbors.top_luma[column] != 0
    } else {
        non_zero[block - 4] != 0
    };
    89 + usize::from(left) + 2 * usize::from(top)
}

fn chroma_ac_cbf_context_with_neighbors(
    non_zero: &[u8],
    block: usize,
    plane: usize,
    neighbors: CabacMbNeighbors,
) -> usize {
    let row = block / 2;
    let column = block % 2;
    let left = if column == 0 {
        !neighbors.left_decoded || neighbors.left_chroma[plane][row] != 0
    } else {
        non_zero[block - 1] != 0
    };
    let top = if row == 0 {
        !neighbors.top_decoded || neighbors.top_chroma[plane][column] != 0
    } else {
        non_zero[block - 2] != 0
    };
    101 + usize::from(left) + 2 * usize::from(top)
}

fn chroma_ac_cbf_context_inter(
    non_zero: &[u8],
    block: usize,
    plane: usize,
    neighbors: CabacMbNeighbors,
) -> usize {
    let row = block / 2;
    let column = block % 2;
    let left = if column == 0 {
        neighbors.left_decoded && neighbors.left_chroma[plane][row] != 0
    } else {
        non_zero[block - 1] != 0
    };
    let top = if row == 0 {
        neighbors.top_decoded && neighbors.top_chroma[plane][column] != 0
    } else {
        non_zero[block - 2] != 0
    };
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

fn i16_prediction_mode(type_index: usize) -> u8 {
    const MODE_MAP: [u8; 4] = [2, 1, 0, 3];
    let internal_mode = MODE_MAP[(type_index - 1) & 3];
    MODE_MAP[usize::from(internal_mode)]
}

fn decode_cabac_luma_modes(
    decoder: &mut CabacDecoder<'_>,
    contexts: &mut [Context],
    transform_8x8: bool,
    neighbors: super::h264_mb::IntraModeNeighbors,
) -> Vec<u8> {
    if transform_8x8 {
        let mut modes = [0u8; 4];
        for row in 0..2 {
            for column in 0..2 {
                let left = if column == 0 {
                    neighbors.left[row * 2]
                } else {
                    Some(modes[row * 2 + column - 1])
                };
                let top = if row == 0 {
                    neighbors.top[column * 2]
                } else {
                    Some(modes[(row - 1) * 2 + column])
                };
                let predicted = predict_intra_mode(left, top);
                let mode = decode_cabac_prediction_mode(decoder, contexts, predicted);
                modes[row * 2 + column] = mode;
            }
        }
        modes.to_vec()
    } else {
        let mut modes = [0u8; 16];
        for block in I4X4_BLOCK_ORDER {
            let row = block / 4;
            let column = block % 4;
            let left = if column == 0 {
                neighbors.left[row]
            } else {
                Some(modes[row * 4 + column - 1])
            };
            let top = if row == 0 {
                neighbors.top[column]
            } else {
                Some(modes[(row - 1) * 4 + column])
            };
            let predicted = predict_intra_mode(left, top);
            let mode = decode_cabac_prediction_mode(decoder, contexts, predicted);
            modes[block] = mode;
        }
        modes.to_vec()
    }
}

fn decode_cabac_prediction_mode(
    decoder: &mut CabacDecoder<'_>,
    contexts: &mut [Context],
    predicted: u8,
) -> u8 {
    if decoder.decode_bin(&mut contexts[68]) != 0 {
        predicted
    } else {
        let bit0 = decoder.decode_bin(&mut contexts[69]) as u8;
        let bit1 = decoder.decode_bin(&mut contexts[69]) as u8;
        let bit2 = decoder.decode_bin(&mut contexts[69]) as u8;
        let remainder = u32::from(bit0 | (bit1 << 1) | (bit2 << 2));
        (remainder + u32::from(remainder >= u32::from(predicted))) as u8
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_qp_delta_sign_by_magnitude() {
        for (magnitude, expected) in [(1, 1), (2, -1), (3, 2), (4, -2), (7, 4), (8, -4)] {
            assert_eq!(qp_delta_from_magnitude(magnitude), expected);
        }
    }

    #[test]
    fn maps_internal_i16_prediction_modes_to_h264_modes() {
        for (type_index, expected) in [
            (1, 0),
            (2, 1),
            (3, 2),
            (4, 3),
            (13, 0),
            (14, 1),
            (15, 2),
            (16, 3),
        ] {
            assert_eq!(i16_prediction_mode(type_index), expected);
        }
    }

    #[test]
    fn wraps_qp_delta_within_the_bit_depth_range() {
        assert_eq!(add_cabac_qp(51, 2, 51), Ok(1));
        assert_eq!(add_cabac_qp(0, -1, 51), Ok(51));
        assert_eq!(
            add_cabac_qp(52, 0, 51),
            Err(H264CabacError::Malformed("slice QP"))
        );
    }

    #[test]
    fn initializes_p_skip_contexts_from_table_9_12() {
        assert_eq!(init_p_contexts(0, 26, 8).unwrap()[11], Context::new(6, 1));
        assert_eq!(init_p_contexts(1, 26, 8).unwrap()[11], Context::new(3, 0));
        assert_eq!(init_p_contexts(2, 26, 8).unwrap()[11], Context::new(0, 0));
        assert_eq!(
            init_p_contexts(3, 26, 8),
            Err(H264CabacError::Malformed("cabac_init_idc"))
        );
    }

    #[test]
    fn p_skip_initialization_normalizes_high_bit_depth_qp() {
        assert_eq!(
            init_p_contexts(0, 32, 10).unwrap()[11],
            init_p_contexts(0, 20, 8).unwrap()[11]
        );
    }
}
