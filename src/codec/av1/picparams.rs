//! `DXVA_PicParams_AV1` construction for the AV1D_VPU front-end.
//!
//! Field order follows `mpp/common/av1d_syntax.h` and the assignments in
//! `av1d_parser.c::av1d_parser2_syntax`. This is a raw hardware ABI: pointer
//! fields, bitfield storage and padding are all significant.

use std::mem::{offset_of, size_of};
use std::ops::Range;

use super::cdf::Av1Cdf;
use super::global_motion;
use super::parser::{FrameHeader, FrameType, ReferenceFrame, SequenceHeader, MAX_TILES};

pub const PIC_PARAMS_SIZE: usize = 3_240;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PicEntry {
    pub b_pic_entry: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Tiles {
    pub cols: u16,
    pub rows: u16,
    pub context_update_id: u16,
    pub widths: [u16; 64],
    pub heights: [u16; 64],
    pub tile_offset_start: [u32; MAX_TILES],
    pub tile_offset_end: [u32; MAX_TILES],
    pub tile_sz_mag: u8,
}

impl Default for Tiles {
    fn default() -> Self {
        Self {
            cols: 0,
            rows: 0,
            context_update_id: 0,
            widths: [0; 64],
            heights: [0; 64],
            tile_offset_start: [0; MAX_TILES],
            tile_offset_end: [0; MAX_TILES],
            tile_sz_mag: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PictureFormat {
    pub frame_type: u8,
    pub show_frame: u8,
    pub showable_frame: u8,
    pub subsampling_x: u8,
    pub subsampling_y: u8,
    pub mono_chrome: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct LoopFilter {
    pub filter_level: [u8; 2],
    pub filter_level_u: u8,
    pub filter_level_v: u8,
    pub sharpness_level: u8,
    pub mode_ref_delta_enabled: u8,
    pub mode_ref_delta_update: u8,
    pub delta_lf_multi: u8,
    pub delta_lf_present: u8,
    pub delta_lf_res: u8,
    pub ref_deltas: [i8; 8],
    pub mode_deltas: [i8; 2],
    pub frame_restoration_type: [u8; 3],
    pub log2_restoration_unit_size: [u8; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Quantization {
    pub delta_q_present: u8,
    pub delta_q_res: u8,
    pub base_qindex: u8,
    pub y_dc_delta_q: i8,
    pub u_dc_delta_q: i8,
    pub v_dc_delta_q: i8,
    pub u_ac_delta_q: i8,
    pub v_ac_delta_q: i8,
    pub using_qmatrix: i8,
    pub qm_y: u8,
    pub qm_u: u8,
    pub qm_v: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CdefStrength {
    pub primary: u8,
    pub secondary: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Cdef {
    pub damping: u8,
    pub bits: u8,
    pub y_strengths: [CdefStrength; 8],
    pub uv_strengths: [CdefStrength; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Segmentation {
    pub enabled: u8,
    pub update_map: u8,
    pub update_data: u8,
    pub temporal_update: u8,
    pub feature_mask: [u8; 8],
    pub feature_data: [[i32; 8]; 8],
    pub last_active: u8,
    pub preskip: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FilmGrain {
    pub apply_grain: u8,
    pub scaling_shift_minus8: u8,
    pub chroma_scaling_from_luma: u8,
    pub ar_coeff_lag: u8,
    pub ar_coeff_shift_minus6: u8,
    pub grain_scale_shift: u8,
    pub overlap_flag: u8,
    pub clip_to_restricted_range: u8,
    pub matrix_coefficients: u8,
    pub matrix_coeff_is_identity: u8,
    pub num_y_points: u8,
    pub num_cb_points: u8,
    pub num_cr_points: u8,
    pub scaling_points_y: [[u8; 2]; 14],
    pub scaling_points_cb: [[u8; 2]; 10],
    pub scaling_points_cr: [[u8; 2]; 10],
    pub ar_coeffs_y: [u8; 24],
    pub ar_coeffs_cb: [u8; 25],
    pub ar_coeffs_cr: [u8; 25],
    pub cb_mult: u8,
    pub cb_luma_mult: u8,
    pub cr_mult: u8,
    pub cr_luma_mult: u8,
    pub grain_seed: u16,
    pub update_grain: u16,
    pub cb_offset: u16,
    pub cr_offset: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PicReference {
    pub width: u32,
    pub height: u32,
    pub order_hint: u32,
    pub lst_frame_offset: u32,
    pub lst2_frame_offset: u32,
    pub lst3_frame_offset: u32,
    pub gld_frame_offset: u32,
    pub bwd_frame_offset: u32,
    pub alt2_frame_offset: u32,
    pub alt_frame_offset: u32,
    pub is_intra_frame: u32,
    pub intra_only: u32,
    pub index: i8,
    pub wminvalid: u8,
    pub wmtype: u8,
    pub wmmat: [i32; 6],
    pub wmmat_val: [i32; 6],
    pub alpha: u16,
    pub beta: u16,
    pub gamma: u16,
    pub delta: u16,
}

impl Default for PicReference {
    fn default() -> Self {
        // C initializes the whole DXVA structure with memset(0). Deriving
        // Default would leave the ABI padding before `wmmat` uninitialized,
        // making even a "matching" field assignment nondeterministic.
        // SAFETY: this repr(C) C-ABI struct contains only integer fields, so
        // all-zero is a valid initialized value including its explicit padding.
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PicReferenceState {
    pub valid: i32,
    pub frame_id: i32,
    pub upscaled_width: i32,
    pub frame_width: i32,
    pub frame_height: i32,
    pub render_width: i32,
    pub render_height: i32,
    pub frame_type: i32,
    pub subsampling_x: i32,
    pub subsampling_y: i32,
    pub bit_depth: i32,
    pub order_hint: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Av1PicParams {
    pub curr_pic: PicEntry,
    pub width: u16,
    pub height: u16,
    pub max_width: u16,
    pub max_height: u16,
    pub curr_pic_texture_index: u16,
    pub use_superres: u16,
    pub superres_denom: u16,
    pub bitdepth: u16,
    pub seq_profile: u16,
    pub frame_header_size: u16,
    pub coding: u64,
    pub tiles: Tiles,
    pub format: PictureFormat,
    pub primary_ref_frame: u8,
    pub enable_order_hint: u8,
    pub order_hint: u8,
    pub order_hint_bits: u8,
    pub loop_filter: LoopFilter,
    pub quantization: Quantization,
    pub cdef: Cdef,
    pub segmentation: Segmentation,
    pub film_grain: FilmGrain,
    pub ref_frame_valued: u32,
    pub ref_frame_idx: [u32; 7],
    pub ref_order_hint: [u32; 8],
    pub frame_refs: [PicReference; 8],
    pub frame_ref_state: [PicReferenceState; 8],
    pub ref_frame_sign_bias: [u8; 8],
    pub coded_lossless: u8,
    pub all_lossless: i32,
    pub interp_filter: u8,
    pub ref_frame_map_texture_index: [u8; 8],
    pub upscaled_width: u32,
    pub frame_to_show_map_idx: u32,
    pub show_existing_frame: u32,
    pub frame_tag_size: u32,
    pub offset_to_dct_parts: u32,
    pub skip_ref0: u8,
    pub skip_ref1: u8,
    pub refresh_frame_flags: u8,
    pub cdfs: *const u8,
    pub cdfs_ndvc: *const u8,
    pub tile_cols_log2: u8,
    pub tile_rows_log2: u8,
}

impl Default for Av1PicParams {
    fn default() -> Self {
        // All-zero initialisation mirrors the C `memset(pp, 0, sizeof(*pp))`.
        // SAFETY: this repr(C) hardware ABI struct admits an all-zero value;
        // pointers are represented as integer-address fields and are not
        // dereferenced by Default.
        unsafe { std::mem::zeroed() }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PicParamsPointers {
    pub cdfs: u64,
    pub intrabc_cdfs: u64,
}

impl Av1PicParams {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        sequence: &SequenceHeader,
        header: &FrameHeader,
        references: &[ReferenceFrame; 8],
        _cdf: &Av1Cdf,
        tile_ranges: &[Option<Range<usize>>; MAX_TILES],
        frame_start: u32,
        frame_tag_size: u32,
        current_slot: u8,
        reference_texture_slots: &[u8; 8],
        pointers: PicParamsPointers,
    ) -> Self {
        let mut params = Self::default();
        params.curr_pic.b_pic_entry = current_slot & 0x7f;
        params.width = header.width as u16;
        params.height = header.height as u16;
        params.max_width = sequence.max_frame_width as u16;
        params.max_height = sequence.max_frame_height as u16;
        params.curr_pic_texture_index = u16::from(current_slot);
        params.use_superres = u16::from(header.use_superres);
        params.superres_denom = if header.use_superres {
            u16::from(header.coded_denom)
        } else {
            8
        };
        params.bitdepth = u16::from(sequence.bit_depth);
        params.seq_profile = u16::from(sequence.profile);
        params.frame_header_size = header.header_bits as u16;
        params.coding = coding_flags(sequence, header);

        params.tiles.cols = header.tile_cols as u16;
        params.tiles.rows = header.tile_rows as u16;
        params.tiles.context_update_id = header.context_update_tile_id;
        for (destination, source) in params
            .tiles
            .widths
            .iter_mut()
            .zip(&header.width_in_sbs_minus_1)
        {
            *destination = u16::from(*source) + 1;
        }
        for (destination, source) in params
            .tiles
            .heights
            .iter_mut()
            .zip(&header.height_in_sbs_minus_1)
        {
            *destination = u16::from(*source) + 1;
        }
        params.tiles.tile_sz_mag = header.tile_size_bytes_minus_1;

        params.format = PictureFormat {
            frame_type: header.frame_type as u8,
            show_frame: u8::from(header.show_frame),
            showable_frame: u8::from(header.showable_frame),
            subsampling_x: u8::from(sequence.color_config.subsampling_x),
            subsampling_y: u8::from(sequence.color_config.subsampling_y),
            mono_chrome: u8::from(sequence.color_config.mono_chrome),
        };
        params.primary_ref_frame = header.primary_ref_frame;
        params.enable_order_hint = u8::from(sequence.enable_order_hint);
        params.order_hint = header.order_hint;
        params.order_hint_bits = if sequence.enable_order_hint {
            sequence.order_hint_bits
        } else {
            0
        };
        params.coded_lossless = u8::from(header.coded_lossless);
        params.all_lossless = i32::from(header.all_lossless);
        params.interp_filter = header.interpolation_filter;
        params.ref_frame_valued = u32::from(!header.frame_type.is_intra_for_picparams());
        for (index, source) in header.ref_frame_idx.iter().enumerate() {
            params.ref_frame_idx[index] = u32::from(*source);
        }
        for (index, source) in header.ref_order_hint.iter().enumerate() {
            params.ref_order_hint[index] = u32::from(*source);
        }

        params.ref_frame_map_texture_index.fill(0xff);
        let warped_motion = global_motion::resolve(header, references);
        for index in 0..8 {
            let reference = references[index];
            let slot = reference_texture_slots[index];
            // `frame_refs` describes the seven AV1 inter references.  The
            // RK3588 path resolves those through `ref_frame_idx`; the DPB
            // state below is indexed directly by the original slot number.
            if index < 7 {
                let source = header.ref_frame_idx[index] as usize;
                let mapped_reference = references[source];
                let pic_reference = &mut params.frame_refs[index];
                pic_reference.width = mapped_reference.frame_width;
                pic_reference.height = mapped_reference.frame_height;
                pic_reference.order_hint = u32::from(mapped_reference.order_hint);
                pic_reference.lst_frame_offset = mapped_reference.lst_frame_offset;
                pic_reference.lst2_frame_offset = mapped_reference.lst2_frame_offset;
                pic_reference.lst3_frame_offset = mapped_reference.lst3_frame_offset;
                pic_reference.gld_frame_offset = mapped_reference.gld_frame_offset;
                pic_reference.bwd_frame_offset = mapped_reference.bwd_frame_offset;
                pic_reference.alt2_frame_offset = mapped_reference.alt2_frame_offset;
                pic_reference.alt_frame_offset = mapped_reference.alt_frame_offset;
                pic_reference.is_intra_frame = u32::from(mapped_reference.is_intra_frame);
                pic_reference.intra_only = u32::from(mapped_reference.intra_only);
                pic_reference.index = reference_texture_slots[source] as i8;
                write_global_motion(pic_reference, &warped_motion[index + 1]);
            }

            params.frame_ref_state[index] = PicReferenceState {
                valid: i32::from(reference.valid),
                frame_id: reference.frame_id as i32,
                upscaled_width: reference.upscaled_width as i32,
                frame_width: reference.frame_width as i32,
                frame_height: reference.frame_height as i32,
                render_width: reference.render_width as i32,
                render_height: reference.render_height as i32,
                frame_type: reference.frame_type as i32,
                subsampling_x: i32::from(reference.subsampling_x),
                subsampling_y: i32::from(reference.subsampling_y),
                bit_depth: i32::from(reference.bit_depth),
                order_hint: i32::from(reference.order_hint),
            };
            if slot < 0x7f {
                params.ref_frame_map_texture_index[index] = slot;
            }
        }

        params.loop_filter = LoopFilter {
            filter_level: [header.loop_filter_level[0], header.loop_filter_level[1]],
            filter_level_u: header.loop_filter_level[2],
            filter_level_v: header.loop_filter_level[3],
            sharpness_level: header.loop_filter_sharpness,
            mode_ref_delta_enabled: u8::from(header.loop_filter_delta_enabled),
            mode_ref_delta_update: u8::from(header.loop_filter_delta_update),
            delta_lf_multi: u8::from(header.delta_lf_multi),
            delta_lf_present: u8::from(header.delta_lf_present),
            delta_lf_res: header.delta_lf_res,
            ref_deltas: header.loop_filter_ref_deltas,
            mode_deltas: header.loop_filter_mode_deltas,
            frame_restoration_type: header.lr_type.map(remap_restoration_type),
            log2_restoration_unit_size: restoration_unit_sizes(header),
        };
        params.quantization = Quantization {
            delta_q_present: u8::from(header.delta_q_present),
            delta_q_res: header.delta_q_res,
            base_qindex: header.base_q_idx,
            y_dc_delta_q: header.delta_q_y_dc,
            u_dc_delta_q: header.delta_q_u_dc,
            v_dc_delta_q: header.delta_q_v_dc,
            u_ac_delta_q: header.delta_q_u_ac,
            v_ac_delta_q: header.delta_q_v_ac,
            using_qmatrix: i8::from(header.using_qmatrix),
            qm_y: if header.using_qmatrix {
                header.qm_y
            } else {
                0xff
            },
            qm_u: if header.using_qmatrix {
                header.qm_u
            } else {
                0xff
            },
            qm_v: if header.using_qmatrix {
                header.qm_v
            } else {
                0xff
            },
        };
        params.cdef.damping = header.cdef_damping_minus_3;
        params.cdef.bits = header.cdef_bits;
        for index in 0..8 {
            params.cdef.y_strengths[index] = CdefStrength {
                primary: header.cdef_y_pri_strength[index],
                secondary: header.cdef_y_sec_strength[index],
            };
            params.cdef.uv_strengths[index] = CdefStrength {
                primary: header.cdef_uv_pri_strength[index],
                secondary: header.cdef_uv_sec_strength[index],
            };
        }
        params.segmentation = Segmentation {
            enabled: u8::from(header.segmentation_enabled),
            update_map: u8::from(header.segmentation_update_map),
            update_data: u8::from(header.segmentation_update_data),
            temporal_update: u8::from(header.segmentation_temporal_update),
            feature_mask: std::array::from_fn(|segment| {
                header.feature_enabled[segment]
                    .iter()
                    .enumerate()
                    .fold(0u8, |mask, (feature, enabled)| {
                        mask | (u8::from(*enabled) << feature)
                    })
            }),
            feature_data: header.feature_value.map(|features| features.map(i32::from)),
            last_active: header.segmentation_id_last_active,
            preskip: u8::from(header.segmentation_id_preskip),
        };
        write_film_grain(&mut params.film_grain, sequence, header);

        params.upscaled_width = header.upscaled_width;
        params.frame_to_show_map_idx = u32::from(header.frame_to_show_map_idx);
        params.show_existing_frame = u32::from(header.show_existing_frame);
        params.frame_tag_size = frame_tag_size;
        params.skip_ref0 = header.skip_mode_frame_idx[0];
        params.skip_ref1 = header.skip_mode_frame_idx[1];
        params.refresh_frame_flags = header.refresh_frame_flags;
        params.cdfs = pointers.cdfs as usize as *const u8;
        params.cdfs_ndvc = pointers.intrabc_cdfs as usize as *const u8;
        params.tile_cols_log2 = header.tile_cols_log2;
        params.tile_rows_log2 = header.tile_rows_log2;
        params.write_tile_offsets(tile_ranges, frame_start.saturating_add(frame_tag_size));
        params
    }

    pub fn write_tile_offsets(
        &mut self,
        tile_ranges: &[Option<Range<usize>>; MAX_TILES],
        frame_tag_size: u32,
    ) {
        for (index, range) in tile_ranges.iter().enumerate() {
            let Some(range) = range else {
                continue;
            };
            let base = frame_tag_size as usize;
            self.tiles.tile_offset_start[index] = range.start.saturating_sub(base) as u32;
            self.tiles.tile_offset_end[index] = range.end.saturating_sub(base) as u32;
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: the type is repr(C), contains only integer/pointer values and
        // intentionally models the immutable C hardware object.
        unsafe { std::slice::from_raw_parts((self as *const Self).cast::<u8>(), size_of::<Self>()) }
    }

    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: see `as_bytes`; all bytes come from initialized C-layout
        // fields, including explicit padding produced by the default value.
        unsafe {
            std::slice::from_raw_parts_mut((self as *mut Self).cast::<u8>(), size_of::<Self>())
        }
    }
}

fn coding_flags(sequence: &SequenceHeader, header: &FrameHeader) -> u64 {
    let mut flags = 0u64;
    let operating_point = sequence
        .operating_points
        .first()
        .map_or(0, |operating_point| operating_point.idc);
    flags |= u64::from(operating_point & 0xfff);
    flags |= u64::from(sequence.use_128x128_superblock) << 12;
    flags |= u64::from(sequence.enable_intra_edge_filter) << 13;
    flags |= u64::from(sequence.enable_interintra_compound) << 14;
    flags |= u64::from(sequence.enable_masked_compound) << 15;
    flags |= u64::from(header.allow_warped_motion) << 16;
    flags |= u64::from(sequence.enable_dual_filter) << 17;
    flags |= u64::from(sequence.enable_jnt_comp) << 18;
    flags |= u64::from(header.allow_screen_content_tools) << 19;
    flags |= u64::from(u8::from(header.force_integer_mv)) << 20;
    flags |= u64::from(sequence.enable_cdef) << 22;
    flags |= u64::from(sequence.enable_restoration) << 23;
    flags |= u64::from(sequence.film_grain_params_present) << 24;
    flags |= u64::from(header.allow_intrabc) << 25;
    flags |= u64::from(header.allow_high_precision_mv) << 26;
    flags |= u64::from(header.is_motion_mode_switchable) << 27;
    flags |= u64::from(sequence.enable_filter_intra) << 28;
    flags |= u64::from(header.disable_frame_end_update_cdf) << 29;
    flags |= u64::from(header.disable_cdf_update) << 30;
    flags |= u64::from(header.reference_select) << 31;
    flags |= u64::from(header.skip_mode_present) << 32;
    flags |= u64::from(header.reduced_tx_set) << 33;
    flags |= u64::from(header.use_superres) << 34;
    flags |= u64::from(header.tx_mode as u8) << 35;
    flags |= u64::from(header.use_ref_frame_mvs) << 38;
    flags |= u64::from(sequence.enable_ref_frame_mvs) << 39;
    flags |= 1 << 40;
    flags |= u64::from(header.error_resilient_mode) << 41;
    flags
}

fn write_global_motion(reference: &mut PicReference, warped_motion: &global_motion::WarpedMotion) {
    reference.wminvalid = u8::from(warped_motion.wmtype == 0);
    reference.wmtype = warped_motion.wmtype;
    reference.wmmat = warped_motion.wmmat;
    reference.wmmat_val = warped_motion.wmmat_val;
    reference.alpha = warped_motion.alpha as u16;
    reference.beta = warped_motion.beta as u16;
    reference.gamma = warped_motion.gamma as u16;
    reference.delta = warped_motion.delta as u16;
}

fn write_film_grain(film_grain: &mut FilmGrain, sequence: &SequenceHeader, header: &FrameHeader) {
    film_grain.matrix_coefficients = sequence.color_config.matrix_coefficients as u8;
    let grain = &header.film_grain;
    if !grain.apply_grain {
        return;
    }
    film_grain.apply_grain = 1;
    film_grain.scaling_shift_minus8 = grain.grain_scaling_minus_8;
    film_grain.chroma_scaling_from_luma = u8::from(grain.chroma_scaling_from_luma);
    film_grain.ar_coeff_lag = grain.ar_coeff_lag;
    film_grain.ar_coeff_shift_minus6 = grain.ar_coeff_shift_minus_6;
    film_grain.grain_scale_shift = grain.grain_scale_shift;
    film_grain.overlap_flag = u8::from(grain.overlap_flag);
    film_grain.clip_to_restricted_range = u8::from(grain.clip_to_restricted_range);
    film_grain.matrix_coeff_is_identity = u8::from(sequence.color_config.matrix_coefficients == 0);
    film_grain.grain_seed = grain.grain_seed;
    film_grain.update_grain = u16::from(grain.update_grain);
    film_grain.num_y_points = grain.num_y_points;
    film_grain.num_cb_points = grain.num_cb_points;
    film_grain.num_cr_points = grain.num_cr_points;
    for index in 0..usize::from(grain.num_y_points) {
        film_grain.scaling_points_y[index] =
            [grain.point_y_value[index], grain.point_y_scaling[index]];
    }
    for index in 0..usize::from(grain.num_cb_points) {
        film_grain.scaling_points_cb[index] =
            [grain.point_cb_value[index], grain.point_cb_scaling[index]];
    }
    for index in 0..usize::from(grain.num_cr_points) {
        film_grain.scaling_points_cr[index] =
            [grain.point_cr_value[index], grain.point_cr_scaling[index]];
    }
    film_grain.ar_coeffs_y = grain.ar_coeffs_y_plus_128;
    film_grain.ar_coeffs_cb = grain.ar_coeffs_cb_plus_128;
    film_grain.ar_coeffs_cr = grain.ar_coeffs_cr_plus_128;
    film_grain.cb_mult = grain.cb_mult;
    film_grain.cb_luma_mult = grain.cb_luma_mult;
    film_grain.cr_mult = grain.cr_mult;
    film_grain.cr_luma_mult = grain.cr_luma_mult;
    film_grain.cb_offset = grain.cb_offset;
    film_grain.cr_offset = grain.cr_offset;
}

fn remap_restoration_type(value: u8) -> u8 {
    match value {
        0 => 0,
        1 => 3,
        2 => 1,
        3 => 2,
        _ => 0,
    }
}

fn restoration_unit_sizes(header: &FrameHeader) -> [u8; 3] {
    let uses_restoration = header.lr_type.iter().any(|kind| *kind != 0);
    if !uses_restoration {
        return [3; 3];
    }
    [
        1 + header.lr_unit_shift,
        1 + header.lr_unit_shift - u8::from(header.lr_uv_shift),
        1 + header.lr_unit_shift - u8::from(header.lr_uv_shift),
    ]
}

trait FrameTypeForPicParams {
    fn is_intra_for_picparams(self) -> bool;
}

impl FrameTypeForPicParams for FrameType {
    fn is_intra_for_picparams(self) -> bool {
        matches!(self, FrameType::Key | FrameType::IntraOnly)
    }
}

const _: () = {
    assert!(PIC_PARAMS_SIZE == size_of::<Av1PicParams>());
    assert!(2 == offset_of!(Av1PicParams, width));
    assert!(24 == offset_of!(Av1PicParams, coding));
    assert!(32 == offset_of!(Av1PicParams, tiles));
    assert!(1_324 == offset_of!(Av1PicParams, format));
    assert!(1_334 == offset_of!(Av1PicParams, loop_filter));
    assert!(1_408 == offset_of!(Av1PicParams, segmentation));
    assert!(1_680 == offset_of!(Av1PicParams, film_grain));
    assert!(1_852 == offset_of!(Av1PicParams, ref_frame_idx));
    assert!(1_912 == offset_of!(Av1PicParams, frame_refs));
    assert!(2_776 == offset_of!(Av1PicParams, frame_ref_state));
    assert!(3_160 == offset_of!(Av1PicParams, ref_frame_sign_bias));
    assert!(3_188 == offset_of!(Av1PicParams, upscaled_width));
    assert!(3_216 == offset_of!(Av1PicParams, cdfs));
    assert!(3_232 == offset_of!(Av1PicParams, tile_cols_log2));
};
