//! VDPU383 AV1 software header layout for RK3576.
//!
//! `vdpu38x_av1d_uncomp_hdr()` serializes `DXVA_PicParams_AV1` into a
//! 5160-bit header, pads it to 128 bits and reserves 16 trailing bytes in the
//! per-job record. The header is a hardware ABI: field order, signed low bits
//! and the C HAL's deliberate quirks are significant.

use crate::codec::av1::picparams::Av1PicParams;
use crate::hal::vdpu383_regs::{self as reg, Field};

pub const UNCOMPRESSED_HEADER_BITS: usize = 5160;
pub const UNCOMPRESSED_HEADER_ALIGN: usize = 128;
pub const UNCOMPRESSED_HEADER_SIZE: usize = 672;
pub const REGISTER_WORD_COUNT: usize = 360;
pub const DEFAULT_CDF_SIZE: usize = 29_600;
const NON_COEF_CDF_SIZE: u32 = 434 * 16;
const COEF_CDF_SIZE: u32 = 354 * 16;
const ALL_CDF_SIZE: u32 = NON_COEF_CDF_SIZE + COEF_CDF_SIZE * 4;
const DEFAULT_CDF: &[u8; DEFAULT_CDF_SIZE] =
    include_bytes!("../codec/av1/data/cdf_vdpu383_default.bin");

pub fn default_cdf_image() -> &'static [u8; DEFAULT_CDF_SIZE] {
    DEFAULT_CDF
}

struct BitWriter {
    words: [u64; UNCOMPRESSED_HEADER_SIZE / 8],
    index: usize,
    bitpos: usize,
    bvalue: u64,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            words: [0; UNCOMPRESSED_HEADER_SIZE / 8],
            index: 0,
            bitpos: 0,
            bvalue: 0,
        }
    }

    fn put(&mut self, value: u32, width: usize) {
        assert!((1..=32).contains(&width));
        if self.index >= self.words.len() {
            return;
        }
        let mask = (1u64 << width) - 1;
        let value = u64::from(value) & mask;
        self.bvalue |= value << self.bitpos;
        if self.bitpos + width >= 64 {
            self.words[self.index] = self.bvalue;
            self.bvalue = value >> (64 - self.bitpos);
            self.index += 1;
        }
        if self.index < self.words.len() {
            self.words[self.index] = self.bvalue;
            self.bitpos = (self.bitpos + width) & 63;
        }
    }

    fn align(&mut self, width: usize) {
        let bits = self.index * 64 + self.bitpos;
        let pad = (width - bits % width) % width;
        for _ in 0..pad {
            self.put(0, 1);
        }
    }

    fn finish(self) -> [u8; UNCOMPRESSED_HEADER_SIZE] {
        let mut bytes = [0; UNCOMPRESSED_HEADER_SIZE];
        for (chunk, word) in bytes.chunks_exact_mut(8).zip(self.words) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        bytes
    }
}

/// Serialize the VDPU383 AV1 global header exactly as
/// `vdpu38x_av1d_uncomp_hdr()` does for RK3576.
pub fn build_uncompressed_header(params: &Av1PicParams) -> [u8; UNCOMPRESSED_HEADER_SIZE] {
    assert_ne!(params.width, 0);
    assert_ne!(params.height, 0);
    let mut w = BitWriter::new();

    // Sequence header.
    w.put(coding_bits(params.coding, 0, 12), 12);
    w.put(coding_bit(params.coding, 12), 1);
    w.put(coding_bit(params.coding, 28), 1);
    w.put(coding_bit(params.coding, 13), 1);
    w.put(coding_bit(params.coding, 14), 1);
    w.put(coding_bit(params.coding, 15), 1);
    w.put(coding_bit(params.coding, 17), 1);
    w.put(u32::from(params.enable_order_hint), 1);
    w.put(coding_bit(params.coding, 18), 1);
    w.put(coding_bit(params.coding, 39), 1);
    w.put(u32::from(params.order_hint_bits.saturating_sub(1)), 3);

    let cdef_enabled = params.coded_lossless == 0
        && (params.cdef.bits != 0
            || params.cdef.y_strengths[0].primary != 0
            || params.cdef.y_strengths[0].secondary != 0
            || params.cdef.uv_strengths[0].primary != 0
            || params.cdef.uv_strengths[0].secondary != 0);
    w.put(u32::from(cdef_enabled), 1);
    w.put(u32::from(params.bitdepth.saturating_sub(8)), 3);

    let yuv_format = if params.format.mono_chrome != 0 {
        0
    } else if params.format.subsampling_y == 1 && params.format.subsampling_y == 1 {
        1
    } else if params.format.subsampling_x == 1 {
        2
    } else {
        3
    };
    w.put(yuv_format, 2);
    w.put(u32::from(params.film_grain.matrix_coefficients), 8);
    w.put(coding_bit(params.coding, 24), 1);

    // Frame header.
    let frame_is_intra = matches!(params.format.frame_type, 0 | 2);
    w.put(u32::from(frame_is_intra), 1);
    w.put(coding_bit(params.coding, 30), 1);
    w.put(coding_bit(params.coding, 19), 1);
    w.put(coding_bit(params.coding, 20) | u32::from(frame_is_intra), 1);
    w.put(u32::from(params.order_hint), 8);
    w.put(u32::from(params.width), 16);
    w.put(u32::from(params.height), 16);
    w.put(u32::from(params.use_superres), 1);
    w.put(u32::from(params.superres_denom), 5);
    w.put(params.upscaled_width, 16);
    w.put(coding_bit(params.coding, 26), 1);
    w.put(coding_bit(params.coding, 25), 1);

    for index in 0..7 {
        w.put(
            if params.ref_frame_valued != 0 {
                params.ref_frame_idx[index]
            } else {
                u32::MAX
            },
            3,
        );
    }
    w.put(u32::from(params.interp_filter), 3);
    w.put(coding_bit(params.coding, 27), 1);
    w.put(coding_bit(params.coding, 38), 1);

    for reference in 0..8 {
        w.put(params.frame_refs[reference].order_hint, 8);
    }
    for index in 0..7 {
        let mapped = params.ref_frame_idx[index] as usize & 7;
        w.put(params.ref_order_hint[mapped], 8);
    }
    for index in 0..7 {
        let reference = params.ref_frame_idx[index] as usize & 7;
        let bias = if params.order_hint_bits == 0 || params.frame_refs[index].index < 0 {
            0
        } else {
            u32::from(
                relative_distance(
                    params.frame_refs[reference].order_hint as i32,
                    i32::from(params.order_hint),
                    params.order_hint_bits,
                ) > 0,
            )
        };
        w.put(bias, 1);
    }
    w.put(coding_bit(params.coding, 29), 1);

    // Quantization.
    w.put(u32::from(params.quantization.base_qindex), 8);
    w.put(params.quantization.y_dc_delta_q as u32, 7);
    w.put(params.quantization.u_dc_delta_q as u32, 7);
    w.put(params.quantization.u_ac_delta_q as u32, 7);
    w.put(params.quantization.v_dc_delta_q as u32, 7);
    w.put(params.quantization.v_ac_delta_q as u32, 7);
    w.put(params.quantization.using_qmatrix as u32, 1);

    // Segmentation.
    w.put(u32::from(params.segmentation.enabled), 1);
    w.put(u32::from(params.segmentation.update_map), 1);
    w.put(u32::from(params.segmentation.temporal_update), 1);

    let mi_rows = align(usize::from(params.width), 8) >> 2;
    let mi_cols = align(usize::from(params.height), 8) >> 2;
    let prev_mi_rows = align(params.frame_refs[0].width as usize, 8) >> 2;
    let prev_mi_cols = align(params.frame_refs[0].height as usize, 8) >> 2;
    w.put(
        u32::from(
            params.segmentation.enabled != 0
                && params.primary_ref_frame != 0
                && mi_rows == prev_mi_rows
                && mi_cols == prev_mi_cols,
        ),
        1,
    );

    for segment in 0..8 {
        w.put(u32::from(params.segmentation.feature_mask[segment]), 8);
    }
    for segment in 0..8 {
        for feature in 0..5 {
            let width = if feature == 0 { 9 } else { 7 };
            w.put(
                params.segmentation.feature_data[segment][feature] as u32,
                width,
            );
        }
        w.put(params.segmentation.feature_data[segment][5] as u32, 3);
    }
    w.put(u32::from(params.segmentation.last_active), 3);
    w.put(u32::from(params.segmentation.preskip), 1);
    w.put(u32::from(params.quantization.delta_q_present), 1);
    w.put(
        if params.quantization.delta_q_present != 0 {
            u32::from(params.quantization.delta_q_res)
        } else {
            1 << params.quantization.delta_q_res
        },
        2,
    );

    // Delta LF and per-segment lossless flags.
    w.put(u32::from(params.loop_filter.delta_lf_present), 1);
    w.put(1 << params.loop_filter.delta_lf_res, 2);
    w.put(u32::from(params.loop_filter.delta_lf_multi), 1);
    w.put(u32::from(params.coded_lossless), 1);
    for segment in 0..8 {
        let qindex = if params.segmentation.feature_mask[segment] & 1 != 0 {
            i32::from(params.quantization.base_qindex)
                + params.segmentation.feature_data[segment][0]
        } else {
            i32::from(params.quantization.base_qindex)
        }
        .clamp(0, 255);
        let lossless = qindex == 0
            && params.quantization.y_dc_delta_q == 0
            && params.quantization.u_dc_delta_q == 0
            && params.quantization.v_dc_delta_q == 0
            && params.quantization.u_ac_delta_q == 0
            && params.quantization.v_ac_delta_q == 0;
        w.put(u32::from(lossless), 1);
    }
    w.put(params.all_lossless as u32, 1);

    // Dequantization and loop filter.
    w.put(u32::from(params.quantization.qm_y), 4);
    w.put(u32::from(params.quantization.qm_u), 4);
    w.put(u32::from(params.quantization.qm_v), 4);
    w.put(u32::from(params.loop_filter.filter_level[0]), 6);
    w.put(u32::from(params.loop_filter.filter_level[1]), 6);
    w.put(u32::from(params.loop_filter.filter_level_u), 6);
    w.put(u32::from(params.loop_filter.filter_level_v), 6);
    w.put(u32::from(params.loop_filter.sharpness_level), 3);
    w.put(u32::from(params.loop_filter.mode_ref_delta_enabled), 1);
    for delta in params.loop_filter.ref_deltas {
        w.put(delta as u32, 7);
    }
    for delta in params.loop_filter.mode_deltas {
        w.put(delta as u32, 7);
    }

    // CDEF.
    w.put(u32::from(params.cdef.damping + 3), 3);
    w.put(u32::from(params.cdef.bits), 2);
    for strength in params.cdef.y_strengths {
        w.put(u32::from(strength.primary), 4);
    }
    for strength in params.cdef.uv_strengths {
        w.put(u32::from(strength.primary), 4);
    }
    for strength in params.cdef.y_strengths {
        w.put(u32::from(strength.secondary), 2);
    }
    for strength in params.cdef.uv_strengths {
        w.put(u32::from(strength.secondary), 2);
    }

    // Loop restoration.
    let planes = if params.format.mono_chrome != 0 { 1 } else { 3 };
    w.put(
        u32::from(
            params.loop_filter.frame_restoration_type[..planes]
                .iter()
                .any(|kind| *kind != 0),
        ),
        1,
    );
    for kind in params.loop_filter.frame_restoration_type {
        w.put(u32::from(kind), 2);
    }
    for size in params.loop_filter.log2_restoration_unit_size[..2].iter() {
        w.put(u32::from(*size), 2);
    }

    // Inter controls and global motion.
    w.put(coding_bits(params.coding, 35, 2), 2);
    w.put(coding_bit(params.coding, 31), 1);
    w.put(u32::from(params.skip_ref0), 3);
    w.put(u32::from(params.skip_ref1), 3);
    w.put(coding_bit(params.coding, 32), 1);
    w.put(coding_bit(params.coding, 16), 1);
    w.put(coding_bit(params.coding, 33), 1);
    for reference in 0..7 {
        w.put(u32::from(params.frame_refs[reference].wmtype), 2);
    }
    for reference in 0..7 {
        for matrix in 0..6 {
            w.put(params.frame_refs[reference].wmmat_val[matrix] as u32, 17);
        }
    }

    write_film_grain(&mut w, params);

    // Reference-frame state.
    for reference in 0..8 {
        w.put(params.frame_ref_state[reference].upscaled_width as u32, 16);
    }
    for reference in 0..8 {
        w.put(params.frame_ref_state[reference].frame_height as u32, 16);
    }
    for reference in 0..8 {
        w.put(params.frame_ref_state[reference].frame_width as u32, 16);
    }
    for reference in 0..8 {
        w.put(params.frame_ref_state[reference].frame_type as u32, 2);
    }
    for reference in 0..8 {
        for offset in [
            params.frame_refs[reference].lst_frame_offset,
            params.frame_refs[reference].lst2_frame_offset,
            params.frame_refs[reference].lst3_frame_offset,
            params.frame_refs[reference].gld_frame_offset,
            params.frame_refs[reference].bwd_frame_offset,
            params.frame_refs[reference].alt2_frame_offset,
            params.frame_refs[reference].alt_frame_offset,
        ] {
            w.put(offset, 8);
        }
    }

    let mut mapped_width = [0u32; 8];
    let mut mapped_height = [0u32; 8];
    for index in 0..7 {
        let mapped = params.ref_frame_idx[index] as usize & 7;
        mapped_width[mapped] = params.frame_ref_state[mapped].frame_width as u32;
        mapped_height[mapped] = params.frame_ref_state[mapped].frame_height as u32;
    }
    for index in 0..8 {
        let (horizontal, vertical) = if coding_bit(params.coding, 25) != 0 {
            (u32::from(params.width), u32::from(params.height))
        } else {
            (mapped_width[index], mapped_height[index])
        };
        let width = u32::from(params.width);
        let height = u32::from(params.height);
        w.put((horizontal << 14).wrapping_add(width / 2) / width, 16);
        w.put((vertical << 14).wrapping_add(height / 2) / height, 16);
    }

    // Tile geometry.
    w.put((u32::from(params.frame_header_size) + 7) >> 3, 10);
    w.put(u32::from(params.tiles.cols), 7);
    w.put(u32::from(params.tiles.rows), 7);
    w.put(u32::from(params.tiles.context_update_id), 12);
    w.put(u32::from(params.tiles.tile_sz_mag + 1), 3);
    w.put(
        u32::from(params.tiles.cols) * u32::from(params.tiles.rows),
        13,
    );
    w.put(u32::from(params.tile_cols_log2 + params.tile_rows_log2), 12);
    for width in params.tiles.widths {
        w.put(u32::from(width), 7);
    }
    for height in params.tiles.heights {
        w.put(u32::from(height), 10);
    }

    let bits = w.index * 64 + w.bitpos;
    assert!(
        bits <= UNCOMPRESSED_HEADER_BITS.div_ceil(UNCOMPRESSED_HEADER_ALIGN)
            * UNCOMPRESSED_HEADER_ALIGN
    );
    w.align(UNCOMPRESSED_HEADER_ALIGN);
    w.finish()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DmaBuffers {
    pub stream: u32,
    pub stream_len: u32,
    pub global_header: u32,
    pub rcb: u32,
    pub cdf_default: u32,
    pub cdf_current: u32,
    pub output: u32,
    pub colmv: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegOffset {
    pub reg_idx: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferPlan {
    pub global_header: usize,
    pub default_cdf: usize,
    pub working_cdf: usize,
    pub segmentation: usize,
    pub colmv: usize,
    pub rcb: usize,
}

pub fn buffer_plan(params: &Av1PicParams) -> BufferPlan {
    let width = usize::from(params.width);
    let height = usize::from(params.height);
    let segmentation = width.div_ceil(128) * height.div_ceil(128) * 32 * 16;
    BufferPlan {
        global_header: UNCOMPRESSED_HEADER_SIZE,
        default_cdf: DEFAULT_CDF_SIZE,
        working_cdf: ALL_CDF_SIZE as usize + segmentation,
        segmentation,
        colmv: width.div_ceil(64) * height.div_ceil(64) * 1024,
        rcb: rcb_layout(params).total as usize,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registers {
    words: [u32; REGISTER_WORD_COUNT],
    offsets: Vec<RegOffset>,
}

impl Registers {
    pub fn build_intra(params: &Av1PicParams, buffers: &DmaBuffers) -> Result<Self, &'static str> {
        if !matches!(params.format.frame_type, 0 | 2) {
            return Err("VDPU383 AV1 register foundation currently accepts intra frames");
        }
        if params.width == 0 || params.height == 0 {
            return Err("VDPU383 AV1 picture has zero geometry");
        }

        let mut regs = Self {
            words: [0; REGISTER_WORD_COUNT],
            offsets: Vec::new(),
        };
        regs.words[8] = 4; // AV1 decode mode.
        regs.words[10] = 0x3ff; // Enable all engine auto-gating.
        regs.words[13] = 0x00ff_ffff;
        regs.words[16] = 1; // error_proc_disable
        regs.words[20] = 0xffff_ffdf;
        regs.words[21] = 0x3fff_ffff;
        regs.set(reg::w28::AXI_PERF_WORK_E, 1);
        regs.set(reg::w28::AXI_CNT_TYPE, 1);
        regs.set(reg::w28::RD_LATENCY_ID, 11);
        regs.set(reg::w29::ADDR_ALIGN_TYPE, 1);
        regs.set(reg::w29::AW_CNT_ID_TYPE, 1);
        regs.set(reg::w29::AR_COUNT_ID, 17);

        let stride = tile_output_stride(usize::from(params.width));
        let height = align(usize::from(params.height), 8);
        regs.words[66] = align(buffers.stream_len as usize, 128).max(128) as u32;
        regs.words[67] = (UNCOMPRESSED_HEADER_SIZE / 16) as u32;
        regs.words[68] = (stride / 16) as u32;
        regs.words[69] = (stride / 16) as u32;
        regs.words[70] = (stride * height / 16) as u32;
        regs.words[80] = regs.words[68];
        regs.words[81] = regs.words[69];
        regs.words[82] = regs.words[70];

        let rcb = rcb_layout(params);
        for index in 0..rcb.lengths.len() {
            let base_register = 140 + index * 2;
            let length = rcb.lengths[index];
            regs.words[base_register] = buffers.rcb;
            regs.words[base_register + 1] = length;
            if rcb.offsets[index] != 0 {
                regs.offsets.push(RegOffset {
                    reg_idx: base_register as u32,
                    offset: rcb.offsets[index],
                });
            }
        }

        regs.words[128] = buffers.stream;
        regs.words[131] = buffers.global_header;
        regs.words[168] = buffers.output;
        regs.words[169] = buffers.stream;
        regs.words[178] = buffers.cdf_default;
        regs.words[179] = buffers.cdf_current;
        regs.words[182] = buffers.cdf_current;
        regs.words[184] = buffers.cdf_default;
        regs.words[185] = buffers.cdf_current;
        regs.words[192] = buffers.output;
        regs.words[216] = buffers.colmv;

        let coefficient_index = match params.quantization.base_qindex {
            0..=20 => 0,
            21..=60 => 1,
            61..=120 => 2,
            _ => 3,
        };
        regs.offsets.extend([
            RegOffset {
                reg_idx: 178,
                offset: NON_COEF_CDF_SIZE + COEF_CDF_SIZE * coefficient_index,
            },
            RegOffset {
                reg_idx: 179,
                offset: NON_COEF_CDF_SIZE,
            },
            RegOffset {
                reg_idx: 181,
                offset: ALL_CDF_SIZE,
            },
            RegOffset {
                reg_idx: 182,
                offset: ALL_CDF_SIZE,
            },
        ]);
        regs.offsets.sort_unstable_by_key(|offset| offset.reg_idx);
        Ok(regs)
    }

    pub fn words(&self) -> &[u32; REGISTER_WORD_COUNT] {
        &self.words
    }

    pub fn offsets(&self) -> &[RegOffset] {
        &self.offsets
    }

    /// All five C `Vdpu383RegSet` members concatenated in declaration order.
    pub fn reference_sections(&self) -> Vec<u32> {
        let mut output = Vec::with_capacity(212);
        output.push(self.words[0]);
        output.extend_from_slice(&self.words[8..31]);
        output.extend_from_slice(&self.words[64..107]);
        output.extend_from_slice(&self.words[128..233]);
        output.extend_from_slice(&self.words[320..360]);
        output
    }

    fn set(&mut self, field: Field, value: u32) {
        let mask = if field.width >= 32 {
            u32::MAX
        } else {
            (1u32 << field.width) - 1
        };
        let shift = u32::from(field.lsb);
        self.words[field.word] =
            (self.words[field.word] & !(mask << shift)) | ((value & mask) << shift);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RcbLayout {
    lengths: [u32; 11],
    offsets: [u32; 11],
    total: u32,
}

fn rcb_layout(params: &Av1PicParams) -> RcbLayout {
    // VDPU383 fixes the RCB calculation at 10-bit storage even for 8-bit AV1.
    let width = usize::from(params.width);
    let height = usize::from(params.height);
    let bit_depth = 10u32;
    let lr = u32::from(
        params
            .loop_filter
            .frame_restoration_type
            .iter()
            .any(|kind| *kind != 0),
    );
    let (intra_uv, filter_row_uv, filter_col_uv) = if params.format.mono_chrome != 0 {
        (1.0, 0.0, 0.0)
    } else if params.format.subsampling_x != 0 && params.format.subsampling_y != 0 {
        (2.0, 1.0, 1.0)
    } else if params.format.subsampling_x != 0 {
        (2.0, 1.0, 3.0)
    } else {
        (3.0, 3.0, 3.0)
    };
    let in_row = align(width, 64);
    let on_row = width;
    let on_col = height;
    let filter_append = if width > 4096 {
        256f64 * 16.0 * 8.0
    } else {
        864f64 * 16.0 * 8.0
    };
    let filter_factor = 1.6 * f64::from(bit_depth) + 0.5;
    let filter_row_total = 12.5 + 6.0 * filter_row_uv + 1.5 * f64::from(lr);
    let filter_col_total = 14.0
        + 7.0 * filter_col_uv
        + (14.0 + 12.5 * filter_col_uv) * f64::from(lr)
        + if params.use_superres != 0 {
            8.5 + 7.0 * filter_col_uv
        } else {
            5.0 + filter_col_uv
        };
    let inter_row = align(width, 64) as f64 / 64.0 * 2752.0;

    let bits = [
        width.div_ceil(8) as f64 * 100.0,
        0.0,
        inter_row,
        inter_row,
        align(
            (in_row as f64 * f64::from(bit_depth + 2) * intra_uv) as usize,
            512,
        ) as f64,
        align(
            (width as f64 * f64::from(bit_depth + 2) * intra_uv) as usize,
            512,
        ) as f64,
        in_row as f64 * filter_factor * filter_row_total / 2.0 + filter_append,
        in_row as f64 * filter_factor * filter_row_total / 2.0 + filter_append,
        on_row as f64 * filter_factor * filter_row_total,
        align(on_col, 64) as f64 * filter_factor * filter_col_total,
        on_col.div_ceil(64) as f64 * f64::from(bit_depth) * 22.0,
    ];
    let mut lengths = [0u32; 11];
    let mut offsets = [0u32; 11];
    let mut total = 0u32;
    for index in 0..lengths.len() {
        offsets[index] = total;
        lengths[index] = rcb_bytes(bits[index]);
        total += lengths[index];
    }
    RcbLayout {
        lengths,
        offsets,
        total,
    }
}

fn rcb_bytes(bits: f64) -> u32 {
    align(((bits.ceil() as u32).saturating_add(7) / 8) as usize, 64) as u32
}

fn tile_output_stride(width: usize) -> usize {
    let aligned = align(width, 64);
    if (aligned - 64) % 256 == 128 {
        aligned
    } else {
        (align(aligned, 128) | 128) + 64
    }
}

fn write_film_grain(w: &mut BitWriter, params: &Av1PicParams) {
    let grain = &params.film_grain;
    w.put(u32::from(grain.apply_grain), 1);
    w.put(u32::from(grain.grain_seed), 16);
    w.put(u32::from(grain.update_grain), 1);
    w.put(u32::from(grain.num_y_points), 4);
    for point in grain.scaling_points_y {
        w.put(u32::from(point[0]), 8);
    }
    for point in grain.scaling_points_y {
        w.put(u32::from(point[1]), 8);
    }
    w.put(u32::from(grain.chroma_scaling_from_luma), 1);
    w.put(u32::from(grain.num_cb_points), 4);
    for point in grain.scaling_points_cb {
        w.put(u32::from(point[0]), 8);
    }
    for point in grain.scaling_points_cb {
        w.put(u32::from(point[1]), 8);
    }
    w.put(u32::from(grain.num_cr_points), 4);
    for point in grain.scaling_points_cr {
        w.put(u32::from(point[0]), 8);
    }
    for point in grain.scaling_points_cr {
        w.put(u32::from(point[1]), 8);
    }
    w.put(u32::from(grain.scaling_shift_minus8), 2);
    w.put(u32::from(grain.ar_coeff_lag), 2);
    for coefficient in grain.ar_coeffs_y {
        w.put(u32::from(coefficient), 8);
    }
    for coefficient in grain.ar_coeffs_cb {
        w.put(u32::from(coefficient), 8);
    }
    for coefficient in grain.ar_coeffs_cr {
        w.put(u32::from(coefficient), 8);
    }
    w.put(u32::from(grain.ar_coeff_shift_minus6), 2);
    w.put(u32::from(grain.grain_scale_shift), 2);
    w.put(u32::from(grain.cb_mult), 8);
    w.put(u32::from(grain.cb_luma_mult), 8);
    w.put(u32::from(grain.cb_offset), 9);
    w.put(u32::from(grain.cr_mult), 8);
    w.put(u32::from(grain.cr_luma_mult), 8);
    w.put(u32::from(grain.cr_offset), 9);
    w.put(u32::from(grain.overlap_flag), 1);
    w.put(u32::from(grain.clip_to_restricted_range), 1);
}

fn coding_bit(coding: u64, bit: u8) -> u32 {
    ((coding >> bit) & 1) as u32
}

fn coding_bits(coding: u64, first: u8, width: u8) -> u32 {
    let mask = if width == 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    ((coding >> first) & mask) as u32
}

fn relative_distance(a: i32, b: i32, order_hint_bits: u8) -> i32 {
    if order_hint_bits == 0 {
        return 0;
    }
    let bits = i32::from(order_hint_bits) - 1;
    let modulo = 1 << bits;
    let difference = a - b;
    (difference & (modulo - 1)) - (difference & modulo)
}

const fn align(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::av1::assembler::{Av1PacketEvent, Av1PacketParser};
    use crate::codec::av1::parser::{parse_ivf, split_obus};
    use crate::codec::av1::picparams::Av1PicParams;
    use crate::codec::av1::picparams::PicParamsPointers;

    #[test]
    fn bit_writer_matches_mpp_little_endian_packing() {
        let mut writer = BitWriter::new();
        writer.put(0b101, 3);
        writer.put(0b11_0010, 6);
        writer.align(16);
        assert_eq!(writer.index * 64 + writer.bitpos, 16);
        assert_eq!(&writer.finish()[..2], &[0b1001_0101, 0b0000_0001]);
    }

    #[test]
    fn synthetic_header_has_the_vdpu383_record_size() {
        let mut params = Av1PicParams::default();
        params.width = 320;
        params.height = 240;
        params.max_width = 320;
        params.max_height = 240;
        params.upscaled_width = 320;
        params.superres_denom = 8;
        params.bitdepth = 8;
        params.coding = 0x0000_0001_0000_0000;
        params.tiles.cols = 1;
        params.tiles.rows = 1;
        params.tiles.widths[0] = 64;
        params.tiles.heights[0] = 64;
        params.frame_header_size = 100;
        let header = build_uncompressed_header(&params);
        assert_eq!(header.len(), UNCOMPRESSED_HEADER_SIZE);
        assert_eq!(&header[656..], &[0; 16]);
    }

    #[test]
    fn default_cdf_image_matches_the_official_vdpu383_table() {
        assert_eq!(DEFAULT_CDF_SIZE, 7400 * 4);
        let hash = DEFAULT_CDF
            .iter()
            .fold(0xcbf29ce484222325u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
            });
        assert_eq!(hash, 0x7ce90cbbd54eee76);
    }

    #[test]
    fn real_keyframe_buffer_plan_matches_the_c_layout() {
        let (params, _) = first_keyframe();
        assert_eq!(
            buffer_plan(&params),
            BufferPlan {
                global_header: 672,
                default_cdf: 29_600,
                working_cdf: 32_672,
                segmentation: 3_072,
                colmv: 20_480,
                rcb: 72_448,
            }
        );
    }

    fn first_keyframe() -> (Av1PicParams, u32) {
        let data = include_bytes!("../../tests/data/av1_tiles_320x240_10.ivf");
        let ivf = parse_ivf(data).unwrap();
        let packet = ivf.frames[0].data;
        let obus = split_obus(packet).unwrap();
        let mut parser = Av1PacketParser::new();
        parser.begin_packet();
        let mut cursor = 0;
        let mut coded = None;
        while let Some(event) = parser.parse_next_event(&obus, &mut cursor).unwrap() {
            if let Av1PacketEvent::Coded(frame) = event {
                coded = Some(frame);
                break;
            }
        }
        let frame = coded.unwrap();
        let sequence = parser.sequence().unwrap();
        let params = Av1PicParams::build(
            sequence,
            &frame.header,
            &frame.references_at_header,
            &frame.cdf,
            &frame.tile_ranges,
            frame.frame_start,
            frame.frame_tag_size,
            0,
            &[0xff; 8],
            PicParamsPointers::default(),
        );
        (params, frame.stream_len)
    }

    #[test]
    fn real_keyframe_header_matches_the_c_vdpu383_dump() {
        let golden = include_bytes!("../../tests/data/av1_vdpu383_global_key_320x240.bin");
        let (params, _) = first_keyframe();
        let header = build_uncompressed_header(&params);
        if let Some(path) = std::env::var_os("AV1_383_RUST_DUMP") {
            std::fs::write(path, header).unwrap();
        }
        assert_eq!(&header, golden);
        let hash = header.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
        assert_eq!(hash, 0x8da389d2e180d680);
    }

    #[test]
    fn real_keyframe_registers_match_the_c_vdpu383_dump() {
        let (params, stream_len) = first_keyframe();
        let buffers = DmaBuffers {
            stream: 13,
            stream_len,
            global_header: 11,
            rcb: 15,
            cdf_default: 12,
            cdf_current: 19,
            output: 14,
            colmv: 18,
        };
        let registers = Registers::build_intra(&params, &buffers).unwrap();
        let golden = include_bytes!("../../tests/data/av1_vdpu383_regs_key_320x240.bin");
        let expected = golden
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(registers.reference_sections(), expected);
    }
}
