//! HEVC/H.265 NAL unit and parameter-set parsing.
//!
//! References: ITU-T H.265 §7.3.1 (NAL header), §7.3.2.1 (VPS),
//! §7.3.2.2 (SPS), §7.3.2.3 (PPS), §7.3.3 (slice segment header).

use std::borrow::Cow;
use std::fmt;

use crate::codec::h264::parser::BitReader;

// ---- NAL unit types (Table 7-1) ----
pub const NAL_TRAIL_N: u8 = 0;
pub const NAL_TRAIL_R: u8 = 1;
pub const NAL_TSA_N: u8 = 2;
pub const NAL_TSA_R: u8 = 3;
pub const NAL_IDR_W_RADL: u8 = 19;
pub const NAL_IDR_N_LP: u8 = 20;
pub const NAL_CRA: u8 = 21;
pub const NAL_VPS: u8 = 32;
pub const NAL_SPS: u8 = 33;
pub const NAL_PPS: u8 = 34;
pub const NAL_AUD: u8 = 35;
pub const NAL_EOS: u8 = 36;
pub const NAL_EOB: u8 = 37;
pub const NAL_PREFIX_SEI: u8 = 39;
pub const NAL_SUFFIX_SEI: u8 = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HevcError {
    Truncated,
    Unsupported(&'static str),
    BadValue,
}

impl fmt::Display for HevcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HevcError::Truncated => write!(f, "bitstream truncated"),
            HevcError::Unsupported(s) => write!(f, "unsupported: {s}"),
            HevcError::BadValue => write!(f, "invalid value"),
        }
    }
}

impl std::error::Error for HevcError {}

impl From<crate::codec::h264::parser::SpsError> for HevcError {
    fn from(_: crate::codec::h264::parser::SpsError) -> Self {
        HevcError::Truncated
    }
}

/// Remove emulation-prevention bytes (`00 00 03` -> `00 00`).
pub fn ebsp_to_rbsp(data: &[u8]) -> Cow<'_, [u8]> {
    if !data.windows(3).any(|w| w == [0, 0, 3]) {
        return Cow::Borrowed(data);
    }
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0usize;
    for &b in data {
        if zeros == 2 && b == 3 {
            zeros = 0;
            continue;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    Cow::Owned(out)
}

#[derive(Debug, Clone)]
pub struct NalUnit<'a> {
    /// 6-bit nal_unit_type.
    pub unit_type: u8,
    /// nuh_layer_id (0 for single-layer streams).
    pub layer_id: u8,
    /// nuh_temporal_id_plus1.
    pub temporal_id_plus1: u8,
    pub rbsp: Cow<'a, [u8]>,
    /// Byte offset of the start code in the original stream.
    pub offset: usize,
    pub len: usize,
}

impl NalUnit<'_> {
    pub fn is_vcl(&self) -> bool {
        self.unit_type <= 31
    }
    pub fn is_irap(&self) -> bool {
        (16..=23).contains(&self.unit_type)
    }
}

/// Split an Annex-B byte stream into HEVC NAL units (two-byte header).
pub fn split_annex_b(data: &[u8]) -> Vec<NalUnit<'_>> {
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            if data[i + 2] == 1 {
                starts.push((i, i + 3));
                i += 3;
                continue;
            }
            if i + 4 <= data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                starts.push((i, i + 4));
                i += 4;
                continue;
            }
        }
        i += 1;
    }
    let mut out = Vec::with_capacity(starts.len());
    for (n, &(sc_start, payload_start)) in starts.iter().enumerate() {
        let end = starts.get(n + 1).map(|&(s, _)| s).unwrap_or(data.len());
        if payload_start + 2 > end {
            continue;
        }
        let h0 = data[payload_start];
        let h1 = data[payload_start + 1];
        let unit_type = (h0 >> 1) & 0x3f;
        let layer_id = ((h0 & 1) << 5) | (h1 >> 3);
        let temporal_id_plus1 = h1 & 0x07;
        let body = &data[payload_start + 2..end];
        out.push(NalUnit {
            unit_type,
            layer_id,
            temporal_id_plus1,
            rbsp: ebsp_to_rbsp(body),
            offset: sc_start,
            len: end - sc_start,
        });
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct ProfileTierLevel {
    pub general_profile_space: u8,
    pub general_tier_flag: u8,
    pub general_profile_idc: u8,
    pub general_level_idc: u8,
}

fn skip_profile_tier_level(
    r: &mut BitReader<'_>,
    max_sub_layers_minus1: u32,
) -> Result<ProfileTierLevel, HevcError> {
    let mut p = ProfileTierLevel {
        general_profile_space: r.read_bits(2)? as u8,
        general_tier_flag: r.read_bit()? as u8,
        general_profile_idc: r.read_bits(5)? as u8,
        general_level_idc: 0,
    };
    r.skip(32)?; // general_profile_compatibility_flags
    r.skip(48)?; // general_constraint_indicator_flags
    p.general_level_idc = r.read_bits(8)? as u8;

    if max_sub_layers_minus1 > 0 {
        let mut profile_present = vec![false; max_sub_layers_minus1 as usize];
        let mut level_present = vec![false; max_sub_layers_minus1 as usize];
        for i in 0..max_sub_layers_minus1 as usize {
            profile_present[i] = r.read_bit()? == 1;
            level_present[i] = r.read_bit()? == 1;
        }
        // reserved_zero_2bits for sub-layers 0..8
        if max_sub_layers_minus1 < 8 {
            r.skip(2 * (8 - max_sub_layers_minus1))?;
        }
        for i in 0..max_sub_layers_minus1 as usize {
            if profile_present[i] {
                r.skip(88)?;
            }
            if level_present[i] {
                r.skip(8)?;
            }
        }
    }
    Ok(p)
}

#[derive(Debug, Clone, Default)]
pub struct Vps {
    pub vps_id: u32,
    pub max_sub_layers: u32,
    pub temporal_id_nesting: bool,
    pub profile: ProfileTierLevel,
}

pub fn parse_vps(rbsp: &[u8]) -> Result<Vps, HevcError> {
    let mut r = BitReader::new(rbsp);
    let vps_id = r.read_bits(4)?;
    r.skip(2)?; // vps_base_layer_internal_flag, vps_base_layer_available_flag
    let max_layers_minus1 = r.read_bits(6)?;
    let mut max_sub_layers = r.read_bits(3)? + 1;
    if max_sub_layers > 7 {
        max_sub_layers = 7; // 7 means "7 or more"
    }
    let temporal_id_nesting = r.read_bit()? == 1;
    r.skip(16)?; // reserved_0xffff_16bits
    let profile = skip_profile_tier_level(&mut r, max_sub_layers - 1)?;
    let _ = max_layers_minus1;
    Ok(Vps {
        vps_id,
        max_sub_layers,
        temporal_id_nesting,
        profile,
    })
}

#[derive(Debug, Clone, Default)]
pub struct Sps {
    pub sps_id: u32,
    pub vps_id: u32,
    pub max_sub_layers: u32,
    pub chroma_format_idc: u32,
    pub separate_colour_plane: bool,
    pub width: u32,
    pub height: u32,
    pub bit_depth_luma: u32,
    pub bit_depth_chroma: u32,
    pub log2_max_pic_order_cnt_lsb: u32,
    pub num_short_term_ref_pic_sets: u32,
    pub short_term_ref_pic_sets: Vec<ShortTermRps>,
    pub long_term_ref_pics_present: bool,
    pub temporal_mvp_enabled: bool,
    pub sample_adaptive_offset: bool,
    pub amp_enabled: bool,
    pub strong_intra_smoothing: bool,
    pub sao_enabled: bool,
    pub pcm_enabled: bool,
    pub profile: ProfileTierLevel,
    // Coding-tree geometry the hardware tables need.
    pub log2_min_cb: u32,
    pub log2_diff_max_min_cb: u32,
    pub log2_min_tb: u32,
    pub log2_diff_max_min_tb: u32,
    pub max_transform_hierarchy_depth_inter: u32,
    pub max_transform_hierarchy_depth_intra: u32,
    pub scaling_list_enabled: bool,
    pub scaling_list: Option<ScalingList>,
    pub pcm_bit_depth_luma: u32,
    pub pcm_bit_depth_chroma: u32,
    pub pcm_log2_min_cb: u32,
    pub pcm_log2_diff_max_min_cb: u32,
    pub pcm_loop_filter_disabled: bool,
    pub max_dec_pic_buffering: u32,
    pub max_num_reorder_pics: u32,
    pub num_long_term_ref_pics: u32,
    pub long_term_refs_sps: Vec<LongTermSpsRef>,
}

/// HEVC scaling matrices in 8x8 storage order. Size classes 0 and 1 use the
/// first 16 or 64 entries respectively; size class 3 uses matrices 0 and 3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalingList {
    pub matrices: [[[u8; 64]; 6]; 4],
    pub dc: [[u8; 6]; 2],
}

impl ScalingList {
    pub fn default_hevc() -> Self {
        const INTRA: [u8; 64] = [
            16, 16, 16, 16, 17, 18, 21, 24, 16, 16, 16, 16, 17, 19, 22, 25, 16, 16, 17, 18, 20, 22,
            25, 29, 16, 16, 18, 21, 24, 27, 31, 36, 17, 17, 20, 24, 30, 35, 41, 47, 18, 19, 22, 27,
            35, 44, 54, 65, 21, 22, 25, 31, 41, 54, 70, 88, 24, 25, 29, 36, 47, 65, 88, 115,
        ];
        const INTER: [u8; 64] = [
            16, 16, 16, 16, 17, 18, 20, 24, 16, 16, 16, 17, 18, 20, 24, 25, 16, 16, 17, 18, 20, 24,
            25, 28, 16, 17, 18, 20, 24, 25, 28, 33, 17, 18, 20, 24, 25, 28, 33, 41, 18, 20, 24, 25,
            28, 33, 41, 54, 20, 24, 25, 28, 33, 41, 54, 71, 24, 25, 28, 33, 41, 54, 71, 91,
        ];
        let mut matrices = [[[16u8; 64]; 6]; 4];
        for size in 1..4 {
            for matrix in 0..6 {
                let source = if matrix < 3 { INTRA } else { INTER };
                matrices[size][matrix] = source;
            }
        }
        Self {
            matrices,
            dc: [[16; 6]; 2],
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LongTermSpsRef {
    pub poc_lsb: u16,
    pub used: bool,
}

pub fn parse_sps(rbsp: &[u8]) -> Result<Sps, HevcError> {
    let mut r = BitReader::new(rbsp);
    let mut sps = Sps::default();
    sps.vps_id = r.read_bits(4)?;
    let max_sub_layers = r.read_bits(3)? + 1;
    if max_sub_layers > 7 {
        return Err(HevcError::Unsupported("sps_max_sub_layers > 7"));
    }
    sps.max_sub_layers = max_sub_layers;
    r.skip(1)?; // sps_temporal_id_nesting_flag
    sps.profile = skip_profile_tier_level(&mut r, max_sub_layers - 1)?;
    sps.sps_id = r.read_ue()?;
    sps.chroma_format_idc = r.read_ue()?;
    if sps.chroma_format_idc > 3 {
        return Err(HevcError::BadValue);
    }
    if sps.chroma_format_idc == 3 {
        sps.separate_colour_plane = r.read_bit()? == 1;
    }
    let width = r.read_ue()?;
    let height = r.read_ue()?;
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(HevcError::BadValue);
    }
    sps.width = width;
    sps.height = height;
    if r.read_bit()? == 1 {
        // conformance_window_flag
        let left = r.read_ue()?;
        let right = r.read_ue()?;
        let top = r.read_ue()?;
        let bottom = r.read_ue()?;
        let (sub_w, sub_h) = match sps.chroma_format_idc {
            1 => (2, 2),
            2 => (2, 1),
            _ => (1, 1),
        };
        sps.width = sps.width.saturating_sub((left + right) * sub_w);
        sps.height = sps.height.saturating_sub((top + bottom) * sub_h);
    }
    sps.bit_depth_luma = r.read_ue()? + 8;
    sps.bit_depth_chroma = r.read_ue()? + 8;
    if sps.bit_depth_luma != sps.bit_depth_chroma {
        return Err(HevcError::Unsupported("luma/chroma bit-depth mismatch"));
    }
    if sps.bit_depth_luma != 8 && sps.bit_depth_luma != 10 {
        return Err(HevcError::Unsupported("bit depth other than 8 or 10"));
    }
    if sps.separate_colour_plane {
        return Err(HevcError::Unsupported("separate colour planes"));
    }
    sps.log2_max_pic_order_cnt_lsb = r.read_ue()? + 4;
    let sub_layer_ordering_present = r.read_bit()? == 1;
    for i in 0..(if sub_layer_ordering_present {
        max_sub_layers
    } else {
        1
    }) {
        let dpb = r.read_ue()? + 1;
        let reorder = r.read_ue()?;
        let is_highest_sublayer = i + 1
            == (if sub_layer_ordering_present {
                max_sub_layers
            } else {
                1
            });
        if is_highest_sublayer {
            sps.max_dec_pic_buffering = dpb;
            sps.max_num_reorder_pics = reorder;
        }
        let _ = r.read_ue()?; // sps_max_latency_increase_plus1
    }
    sps.log2_min_cb = r.read_ue()? + 3;
    sps.log2_diff_max_min_cb = r.read_ue()?;
    sps.log2_min_tb = r.read_ue()? + 2;
    sps.log2_diff_max_min_tb = r.read_ue()?;
    sps.max_transform_hierarchy_depth_inter = r.read_ue()?;
    sps.max_transform_hierarchy_depth_intra = r.read_ue()?;
    // scaling_list_enabled_flag and its data block sit between the transform
    // hierarchy depths and amp_enabled_flag (H.265 7.3.2.2).
    let scaling_list_enabled = r.read_bit()? == 1;
    sps.scaling_list_enabled = scaling_list_enabled;
    if scaling_list_enabled {
        sps.scaling_list = Some(ScalingList::default_hevc());
        let present = r.read_bit()? == 1;
        if present {
            parse_scaling_list_data(&mut r, sps.scaling_list.as_mut().unwrap())?;
        }
    }
    sps.amp_enabled = r.read_bit()? == 1;
    sps.sao_enabled = r.read_bit()? == 1;
    sps.pcm_enabled = r.read_bit()? == 1;
    if sps.pcm_enabled {
        sps.pcm_bit_depth_luma = r.read_bits(4)? + 1;
        sps.pcm_bit_depth_chroma = r.read_bits(4)? + 1;
        sps.pcm_log2_min_cb = r.read_ue()? + 3;
        sps.pcm_log2_diff_max_min_cb = r.read_ue()?;
        sps.pcm_loop_filter_disabled = r.read_bit()? == 1;
    }
    sps.num_short_term_ref_pic_sets = r.read_ue()?;
    if sps.num_short_term_ref_pic_sets > 64 {
        return Err(HevcError::Unsupported("num_short_term_ref_pic_sets > 64"));
    }
    for i in 0..sps.num_short_term_ref_pic_sets {
        let set = parse_short_term_rps(&mut r, i, &sps.short_term_ref_pic_sets, false)?;
        sps.short_term_ref_pic_sets.push(set);
    }
    sps.long_term_ref_pics_present = r.read_bit()? == 1;
    if sps.long_term_ref_pics_present {
        let n = r.read_ue()?;
        if n > 32 {
            return Err(HevcError::Unsupported("num_long_term_ref_pics_sps"));
        }
        sps.num_long_term_ref_pics = n;
        for _ in 0..n {
            sps.long_term_refs_sps.push(LongTermSpsRef {
                poc_lsb: r.read_bits(sps.log2_max_pic_order_cnt_lsb)? as u16,
                used: r.read_bit()? == 1,
            });
        }
    }
    sps.temporal_mvp_enabled = r.read_bit()? == 1;
    sps.strong_intra_smoothing = r.read_bit()? == 1;
    Ok(sps)
}

#[derive(Debug, Clone, Default)]
pub struct Pps {
    pub pps_id: u32,
    pub sps_id: u32,
    pub dependent_slice_segments: bool,
    pub output_flag_present: bool,
    pub num_extra_slice_header_bits: u32,
    pub sign_data_hiding: bool,
    pub cabac_init_present: bool,
    pub num_ref_idx_l0_default_active: u32,
    pub num_ref_idx_l1_default_active: u32,
    pub init_qp: i32,
    pub constrained_intra_pred: bool,
    pub transform_skip_enabled: bool,
    pub cu_qp_delta_enabled: bool,
    pub diff_cu_qp_delta_depth: u32,
    pub cb_qp_offset: i32,
    pub cr_qp_offset: i32,
    pub slice_chroma_qp_offsets_present: bool,
    pub weighted_pred: bool,
    pub weighted_bipred: bool,
    pub transquant_bypass: bool,
    pub tiles_enabled: bool,
    pub tile_columns: u32,
    pub tile_rows: u32,
    pub uniform_spacing: bool,
    /// Explicit widths for all but the final tile column, in CTBs.
    pub tile_column_widths: Vec<u32>,
    /// Explicit heights for all but the final tile row, in CTBs.
    pub tile_row_heights: Vec<u32>,
    pub entropy_coding_sync: bool,
    pub loop_filter_across_slices: bool,
    pub loop_filter_across_tiles: bool,
    pub deblocking_filter_control_present: bool,
    pub deblocking_filter_override_enabled: bool,
    pub pps_deblocking_filter_disabled: bool,
    pub beta_offset_div2: i32,
    pub tc_offset_div2: i32,
    pub lists_modification_present: bool,
    pub log2_parallel_merge_level: u32,
    pub slice_segment_header_extension_present: bool,
    pub scaling_list: Option<ScalingList>,
}

pub fn parse_pps(rbsp: &[u8]) -> Result<Pps, HevcError> {
    let mut r = BitReader::new(rbsp);
    // MPP initializes H265dPps with this flag enabled. The bitstream only
    // carries the flag when tiles are enabled, so disabled-tile streams keep
    // the MPP default.
    let mut pps = Pps {
        loop_filter_across_tiles: true,
        ..Default::default()
    };
    pps.pps_id = r.read_ue()?;
    pps.sps_id = r.read_ue()?;
    pps.dependent_slice_segments = r.read_bit()? == 1;
    pps.output_flag_present = r.read_bit()? == 1;
    pps.num_extra_slice_header_bits = r.read_bits(3)?;
    pps.sign_data_hiding = r.read_bit()? == 1;
    pps.cabac_init_present = r.read_bit()? == 1;
    pps.num_ref_idx_l0_default_active = r.read_ue()? + 1;
    pps.num_ref_idx_l1_default_active = r.read_ue()? + 1;
    pps.init_qp = 26 + r.read_se()?;
    pps.constrained_intra_pred = r.read_bit()? == 1;
    pps.transform_skip_enabled = r.read_bit()? == 1;
    pps.cu_qp_delta_enabled = r.read_bit()? == 1;
    if pps.cu_qp_delta_enabled {
        pps.diff_cu_qp_delta_depth = r.read_ue()?;
    }
    pps.cb_qp_offset = r.read_se()?;
    pps.cr_qp_offset = r.read_se()?;
    pps.slice_chroma_qp_offsets_present = r.read_bit()? == 1;
    pps.weighted_pred = r.read_bit()? == 1;
    pps.weighted_bipred = r.read_bit()? == 1;
    pps.transquant_bypass = r.read_bit()? == 1;
    pps.tiles_enabled = r.read_bit()? == 1;
    pps.entropy_coding_sync = r.read_bit()? == 1;
    if pps.tiles_enabled {
        let cols = r.read_ue()? + 1;
        let rows = r.read_ue()? + 1;
        if cols > 20 || rows > 22 {
            return Err(HevcError::Unsupported("tile grid too large"));
        }
        pps.tile_columns = cols;
        pps.tile_rows = rows;
        let uniform = r.read_bit()? == 1;
        pps.uniform_spacing = uniform;
        if !uniform {
            for _ in 0..(cols - 1) {
                pps.tile_column_widths.push(r.read_ue()? + 1);
            }
            for _ in 0..(rows - 1) {
                pps.tile_row_heights.push(r.read_ue()? + 1);
            }
        }
        pps.loop_filter_across_tiles = r.read_bit()? == 1;
    }
    // MPP's h265d_nal_pps does not consume
    // pps_loop_filter_across_substreams_enabled_flag even though the HEVC
    // syntax places it here when WPP is enabled. The hardware packet is
    // populated from MPP's post-parse state, so consuming the bit shifts every
    // following PPS field by one and makes the first P picture fail.
    // The tail is optional in practice: x265 emits very short PPS NALs where
    // the remaining flags coincide with the rbsp stop bit, so every read from
    // here on is guarded by the bits actually left.
    let step = |r: &mut BitReader<'_>, need: usize| r.bits_left() >= need;
    if step(&mut r, 2) {
        pps.loop_filter_across_slices = r.read_bit()? == 1;
        pps.deblocking_filter_control_present = r.read_bit()? == 1;
        if pps.deblocking_filter_control_present && r.bits_left() >= 2 {
            pps.deblocking_filter_override_enabled = r.read_bit()? == 1;
            pps.pps_deblocking_filter_disabled = r.read_bit()? == 1;
            if !pps.pps_deblocking_filter_disabled && r.bits_left() >= 2 {
                pps.beta_offset_div2 = r.read_se().unwrap_or(0);
                pps.tc_offset_div2 = r.read_se().unwrap_or(0);
            }
        }
    }
    if r.bits_left() >= 1 && r.read_bit()? == 1 {
        let mut scaling = ScalingList::default_hevc();
        parse_scaling_list_data(&mut r, &mut scaling)?;
        pps.scaling_list = Some(scaling);
    }
    if r.bits_left() >= 1 {
        pps.lists_modification_present = r.read_bit()? == 1;
    }
    if r.bits_left() >= 4 {
        pps.log2_parallel_merge_level = r.read_ue().unwrap_or(0) + 2;
    }
    if r.bits_left() >= 1 {
        pps.slice_segment_header_extension_present = r.read_bit()? == 1;
    }
    Ok(pps)
}

const DIAG_SCAN_4X4: [usize; 16] = [0, 4, 1, 8, 5, 2, 12, 9, 6, 3, 13, 10, 7, 14, 11, 15];
const DIAG_SCAN_8X8: [usize; 64] = [
    0, 8, 1, 16, 9, 2, 24, 17, 10, 3, 32, 25, 18, 11, 4, 40, 33, 26, 19, 12, 5, 48, 41, 34, 27, 20,
    13, 6, 56, 49, 42, 35, 28, 21, 14, 7, 57, 50, 43, 36, 29, 22, 15, 58, 51, 44, 37, 30, 23, 59,
    52, 45, 38, 31, 60, 53, 46, 39, 61, 54, 47, 62, 55, 63,
];

/// Parse `scaling_list_data` (H.265 7.3.4), retaining MPP's modulo-256
/// coefficient update and diagonal-scan positions.
fn parse_scaling_list_data(
    r: &mut BitReader<'_>,
    scaling: &mut ScalingList,
) -> Result<(), HevcError> {
    for size_id in 0..4usize {
        let matrix_step = if size_id == 3 { 3 } else { 1 };
        for matrix_id in (0..6).step_by(matrix_step) {
            if r.read_bit()? == 0 {
                let delta = r.read_ue()? as usize;
                if delta != 0 {
                    let delta = delta * matrix_step;
                    if matrix_id < delta {
                        return Err(HevcError::BadValue);
                    }
                    scaling.matrices[size_id][matrix_id] =
                        scaling.matrices[size_id][matrix_id - delta];
                    if size_id > 1 {
                        scaling.dc[size_id - 2][matrix_id] =
                            scaling.dc[size_id - 2][matrix_id - delta];
                    }
                }
                continue;
            }

            // MaxTbLog2SizeY still carries at most 64 scan positions. MPP's
            // h265d_scaling_list_data applies MPP_MIN(64, ...) for the 32x32
            // class; without that bound the 1 << 10 expression overruns the
            // fixed diagonal-scan table and panics on explicit 32x32 matrices.
            let coefficient_count = (1usize << (4 + (size_id << 1))).min(64);
            let mut coefficient = 8i32;
            if size_id > 1 {
                let dc_minus_8 = r.read_se()?;
                if !(-7..=247).contains(&dc_minus_8) {
                    return Err(HevcError::BadValue);
                }
                coefficient = dc_minus_8 + 8;
                scaling.dc[size_id - 2][matrix_id] = coefficient as u8;
            }
            for index in 0..coefficient_count {
                let position = if size_id == 0 {
                    DIAG_SCAN_4X4[index]
                } else {
                    DIAG_SCAN_8X8[index]
                };
                coefficient = (coefficient + r.read_se()? + 256) % 256;
                scaling.matrices[size_id][matrix_id][position] = coefficient as u8;
            }
        }
    }
    Ok(())
}

/// One short-term reference picture set (H.265 7.3.7).
#[derive(Debug, Clone, Default)]
pub struct ShortTermRps {
    /// Delta POCs of the pictures before the current one, most recent first.
    pub s0: Vec<i32>,
    pub used_s0: Vec<bool>,
    /// Delta POCs after the current one, nearest first.
    pub s1: Vec<i32>,
    pub used_s1: Vec<bool>,
}

impl ShortTermRps {
    pub fn num_delta_pocs(&self) -> u32 {
        (self.s0.len() + self.s1.len()) as u32
    }
}

/// Parse a short-term RPS. `sets` holds the already parsed SPS sets (used by
/// the inter-set prediction branch); `idx` is the set index being parsed.
pub fn parse_short_term_rps(
    r: &mut BitReader<'_>,
    idx: u32,
    sets: &[ShortTermRps],
    is_slice_header: bool,
) -> Result<ShortTermRps, HevcError> {
    let predicted = if idx != 0 { r.read_bit()? == 1 } else { false };
    if predicted {
        if sets.is_empty() {
            return Err(HevcError::BadValue);
        }
        // SPS prediction always uses the immediately preceding set. Inline
        // slice prediction carries delta_idx_minus1 and can select any SPS set.
        let delta_idx = if is_slice_header {
            r.read_ue()?.checked_add(1).ok_or(HevcError::BadValue)?
        } else {
            1
        };
        let ref_idx = idx.checked_sub(delta_idx).ok_or(HevcError::BadValue)? as usize;
        let ref_set = sets.get(ref_idx).ok_or(HevcError::BadValue)?;
        let delta_rps_sign = r.read_bit()?;
        let abs_delta_rps_minus1 = r.read_ue()? as i32;
        let delta_rps = (1 - 2 * delta_rps_sign as i32) * (abs_delta_rps_minus1 + 1);

        let n = ref_set.num_delta_pocs() as usize;
        let mut used = vec![false; n + 1];
        let mut use_delta = vec![false; n + 1];
        for j in 0..=n {
            used[j] = r.read_bit()? == 1;
            if !used[j] {
                use_delta[j] = r.read_bit()? == 1;
            }
        }

        // Derive the set (H.265 7.4.8).
        let mut entry: Vec<(i32, bool)> = Vec::new();
        let refs: Vec<i32> = ref_set
            .s0
            .iter()
            .copied()
            .chain(ref_set.s1.iter().copied())
            .collect();
        for (j, &d) in refs.iter().enumerate() {
            if used[j] || use_delta[j] {
                entry.push((d + delta_rps, used[j]));
            }
        }
        if used[n] || use_delta[n] {
            entry.push((delta_rps, used[n]));
        }

        // MPP sorts delta POC ascending, counts negative entries, then reverses
        // that prefix. The resulting split is s0 near-to-far and s1 near-to-far.
        let mut neg: Vec<(i32, bool)> = entry.iter().copied().filter(|(d, _)| *d < 0).collect();
        neg.sort_by(|a, b| b.0.cmp(&a.0));
        let mut pos: Vec<(i32, bool)> = entry.iter().copied().filter(|(d, _)| *d > 0).collect();
        pos.sort_by(|a, b| a.0.cmp(&b.0));
        return Ok(ShortTermRps {
            s0: neg.iter().map(|(d, _)| *d).collect(),
            used_s0: neg.iter().map(|(_, u)| *u).collect(),
            s1: pos.iter().map(|(d, _)| *d).collect(),
            used_s1: pos.iter().map(|(_, u)| *u).collect(),
        });
    }

    let num_negative = r.read_ue()?;
    let num_positive = r.read_ue()?;
    if num_negative > 16 || num_positive > 16 {
        return Err(HevcError::Unsupported("ref pic set too large"));
    }
    let mut cur = 0i32;
    let mut s0 = Vec::with_capacity(num_negative as usize);
    let mut used_s0 = Vec::with_capacity(num_negative as usize);
    for _ in 0..num_negative {
        cur -= r.read_ue()? as i32 + 1;
        s0.push(cur);
        used_s0.push(r.read_bit()? == 1);
    }
    let mut cur = 0i32;
    let mut s1 = Vec::with_capacity(num_positive as usize);
    let mut used_s1 = Vec::with_capacity(num_positive as usize);
    for _ in 0..num_positive {
        cur += r.read_ue()? as i32 + 1;
        s1.push(cur);
        used_s1.push(r.read_bit()? == 1);
    }
    Ok(ShortTermRps {
        s0,
        used_s0,
        s1,
        used_s1,
    })
}

#[derive(Debug, Clone, Default)]
pub struct SliceHeader {
    pub first_slice: bool,
    pub no_output_of_prior_pics: bool,
    pub pps_id: u32,
    pub dependent: bool,
    pub segment_address: u32,
    pub slice_type: u32,
    pub poc_lsb: u32,
    pub short_term_ref_pic_set_sps_flag: bool,
    pub short_term_ref_pic_set_idx: u32,
    pub rps: ShortTermRps,
    pub long_term_rps: LongTermRps,
    pub temporal_mvp: bool,
    pub sao_luma: bool,
    pub sao_chroma: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LongTermRps {
    pub refs: Vec<LongTermRefPic>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LongTermRefPic {
    pub poc_lsb: u16,
    pub used: bool,
    pub delta_msb_cycle: u32,
}

impl LongTermRps {
    /// Resolve the 16-bit POC LSBs to full picture order counts (H.265 8.3.2).
    pub fn resolve_pocs(&self, current_poc: i32, log2_max_poc_lsb: u32) -> Vec<(i32, bool)> {
        let max_lsb = 1i32 << log2_max_poc_lsb;
        let current_base = current_poc - current_poc.rem_euclid(max_lsb);
        self.refs
            .iter()
            .map(|entry| {
                (
                    current_base - entry.delta_msb_cycle as i32 * max_lsb + entry.poc_lsb as i32,
                    entry.used,
                )
            })
            .collect()
    }
}

pub fn parse_slice_header(
    rbsp: &[u8],
    sps: &Sps,
    pps: &Pps,
    nal_type: u8,
) -> Result<SliceHeader, HevcError> {
    let mut r = BitReader::new(rbsp);
    let mut sh = SliceHeader::default();
    sh.first_slice = r.read_bit()? == 1;
    let irap = (16..=23).contains(&nal_type);
    if irap {
        sh.no_output_of_prior_pics = r.read_bit()? == 1;
    }
    sh.pps_id = r.read_ue()?;
    if !sh.first_slice {
        if pps.dependent_slice_segments {
            sh.dependent = r.read_bit()? == 1;
        }
        let (ctb_width, ctb_height) = ctb_grid(sps)?;
        let ctb_count = ctb_width
            .checked_mul(ctb_height)
            .ok_or(HevcError::BadValue)?;
        sh.segment_address = r.read_bits(ceil_log2(ctb_count))?;
        if sh.segment_address >= ctb_count || (sh.dependent && sh.segment_address == 0) {
            return Err(HevcError::BadValue);
        }
        if sh.dependent {
            return Ok(sh);
        }
    }
    for _ in 0..pps.num_extra_slice_header_bits {
        r.skip(1)?;
    }
    sh.slice_type = r.read_ue()?;
    if pps.output_flag_present {
        r.skip(1)?;
    }
    if sps.separate_colour_plane {
        r.skip(2)?;
    }
    if nal_type != NAL_IDR_W_RADL && nal_type != NAL_IDR_N_LP {
        sh.poc_lsb = r.read_bits(sps.log2_max_pic_order_cnt_lsb)?;
        sh.short_term_ref_pic_set_sps_flag = r.read_bit()? == 1;
        if !sh.short_term_ref_pic_set_sps_flag {
            sh.rps = parse_short_term_rps(
                &mut r,
                sps.num_short_term_ref_pic_sets,
                &sps.short_term_ref_pic_sets,
                true,
            )?;
        } else {
            if sps.short_term_ref_pic_sets.is_empty() {
                return Err(HevcError::BadValue);
            }
            if sps.num_short_term_ref_pic_sets > 1 {
                let bits = 32 - sps.num_short_term_ref_pic_sets.leading_zeros();
                sh.short_term_ref_pic_set_idx = r.read_bits(bits)?;
            }
            sh.rps = sps
                .short_term_ref_pic_sets
                .get(sh.short_term_ref_pic_set_idx as usize)
                .cloned()
                .ok_or(HevcError::BadValue)?;
        }
        if sps.long_term_ref_pics_present {
            sh.long_term_rps = parse_long_term_rps(&mut r, sps)?;
        }
        if sps.temporal_mvp_enabled {
            sh.temporal_mvp = r.read_bit()? == 1;
        }
    }
    if sps.sao_enabled {
        sh.sao_luma = r.read_bit()? == 1;
        if sps.chroma_format_idc != 0 {
            sh.sao_chroma = r.read_bit()? == 1;
        }
    }
    Ok(sh)
}

fn ctb_grid(sps: &Sps) -> Result<(u32, u32), HevcError> {
    let log2_ctb = sps
        .log2_min_cb
        .checked_add(sps.log2_diff_max_min_cb)
        .filter(|&v| v < 32)
        .ok_or(HevcError::BadValue)?;
    let ctb_size = 1u32 << log2_ctb;
    let width = sps.width.div_ceil(ctb_size);
    let height = sps.height.div_ceil(ctb_size);
    if width == 0 || height == 0 {
        return Err(HevcError::BadValue);
    }
    Ok((width, height))
}

fn parse_long_term_rps(r: &mut BitReader<'_>, sps: &Sps) -> Result<LongTermRps, HevcError> {
    let num_sps = if sps.num_long_term_ref_pics > 0 {
        let n = r.read_ue()?;
        if n > sps.num_long_term_ref_pics {
            return Err(HevcError::BadValue);
        }
        n
    } else {
        0
    };
    let num_pics = r.read_ue()?;
    let total = num_sps + num_pics;
    if total > 32 || num_sps > sps.long_term_refs_sps.len() as u32 {
        return Err(HevcError::BadValue);
    }

    let mut refs = Vec::with_capacity(total as usize);
    let mut previous_cycle = 0u32;
    for i in 0..total {
        let (poc_lsb, used) = if i < num_sps {
            let index = if sps.num_long_term_ref_pics > 1 {
                r.read_bits(ceil_log2(sps.num_long_term_ref_pics))?
            } else {
                0
            };
            let entry = sps
                .long_term_refs_sps
                .get(index as usize)
                .ok_or(HevcError::BadValue)?;
            (entry.poc_lsb, entry.used)
        } else {
            (
                r.read_bits(sps.log2_max_pic_order_cnt_lsb)? as u16,
                r.read_bit()? == 1,
            )
        };

        let delta = if r.read_bit()? == 1 {
            let value = r.read_ue()?;
            if i > 0 && i != num_sps {
                previous_cycle + value
            } else {
                value
            }
        } else {
            0
        };
        previous_cycle = delta;
        refs.push(LongTermRefPic {
            poc_lsb,
            used,
            delta_msb_cycle: delta,
        });
    }
    Ok(LongTermRps { refs })
}

fn ceil_log2(value: u32) -> u32 {
    if value <= 1 {
        0
    } else {
        32 - (value - 1).leading_zeros()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BitWriter {
        bits: Vec<u8>,
    }

    impl BitWriter {
        fn new() -> Self {
            BitWriter { bits: Vec::new() }
        }
        fn put(&mut self, v: u32, n: u32) -> &mut Self {
            for i in (0..n).rev() {
                self.bits.push(((v >> i) & 1) as u8);
            }
            self
        }
        fn ue(&mut self, v: u32) -> &mut Self {
            let mut n = 0;
            while (1u32 << (n + 1)) <= v + 1 {
                n += 1;
            }
            for _ in 0..n {
                self.bits.push(0);
            }
            self.put(v + 1, n + 1)
        }
        fn se(&mut self, v: i32) -> &mut Self {
            let k = if v <= 0 {
                (-v as u32) * 2
            } else {
                (v as u32) * 2 - 1
            };
            self.ue(k)
        }
        fn finish(&mut self) -> Vec<u8> {
            self.bits.push(1);
            while self.bits.len() % 8 != 0 {
                self.bits.push(0);
            }
            let mut out = vec![0u8; self.bits.len() / 8];
            for (i, b) in self.bits.iter().enumerate() {
                if *b != 0 {
                    out[i / 8] |= 1 << (7 - i % 8);
                }
            }
            out
        }
    }

    fn profile_tier_level(w: &mut BitWriter) {
        w.put(0, 2) // profile_space
            .put(0, 1) // tier
            .put(1, 5) // profile_idc = Main
            .put(0, 32) // compatibility flags
            .put(0, 32) // constraint flags high
            .put(0, 16) // constraint flags low
            .put(120, 8); // level 4.0
    }

    fn sps_with_chroma_and_depth(
        chroma_format_idc: u32,
        bit_depth_minus8: u32,
        chroma_bit_depth_minus8: u32,
    ) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put(0, 4) // vps_id
            .put(0, 3) // max_sub_layers_minus1
            .put(1, 1); // temporal_id_nesting
        profile_tier_level(&mut w);
        w.ue(0) // sps_id
            .ue(chroma_format_idc);
        if chroma_format_idc == 3 {
            w.put(0, 1); // separate_colour_plane_flag
        }
        w.ue(64)
            .ue(64)
            .put(0, 1) // conformance_window_flag
            .ue(bit_depth_minus8)
            .ue(chroma_bit_depth_minus8)
            .ue(4) // log2_max_pic_order_cnt_lsb_minus4
            .put(0, 1) // sub_layer_ordering_info_present
            .ue(3)
            .ue(0)
            .ue(0)
            .ue(0) // log2_min_luma_coding_block_size_minus3
            .ue(3) // log2_diff_max_min
            .ue(0) // log2_min_transform_block_size_minus2
            .ue(3) // log2_diff_max_min_transform
            .ue(0)
            .ue(0)
            .put(0, 1) // scaling_list_enabled_flag
            .put(0, 1) // amp_enabled
            .put(0, 1) // sao_enabled
            .put(0, 1) // pcm_enabled
            .ue(0) // num_short_term_ref_pic_sets
            .put(0, 1) // long_term_ref_pics_present
            .put(0, 1) // temporal_mvp_enabled
            .put(1, 1); // strong_intra_smoothing
        w.finish()
    }

    #[test]
    fn splits_annex_b_with_two_byte_header() {
        // VPS: type 32 -> (32<<1) = 0x40, layer 0, tid 1 -> 0x01
        let stream = [
            0, 0, 0, 1, 0x40, 0x01, 0x0c, //
            0, 0, 1, 0x42, 0x01, 0x01, // SPS
            0, 0, 1, 0x44, 0x01, 0xc0, // PPS
            0, 0, 1, 0x26, 0x01, 0xaf, // IDR_W_RADL
        ];
        let nals = split_annex_b(&stream);
        assert_eq!(nals.len(), 4);
        assert_eq!(nals[0].unit_type, NAL_VPS);
        assert_eq!(nals[1].unit_type, NAL_SPS);
        assert_eq!(nals[2].unit_type, NAL_PPS);
        assert_eq!(nals[3].unit_type, NAL_IDR_W_RADL);
        assert!(nals[3].is_irap());
        assert!(!nals[2].is_vcl());
        assert_eq!(nals[0].temporal_id_plus1, 1);
    }

    #[test]
    fn parses_sps_1080p_main() {
        let mut w = BitWriter::new();
        w.put(0, 4) // vps_id
            .put(0, 3) // max_sub_layers_minus1
            .put(1, 1) // temporal_id_nesting
            ;
        profile_tier_level(&mut w);
        w.ue(0) // sps_id
            .ue(1) // chroma_format_idc 4:2:0
            .ue(1920)
            .ue(1080)
            .put(0, 1) // conformance_window_flag
            .ue(0) // bit_depth_luma_minus8
            .ue(0) // bit_depth_chroma_minus8
            .ue(4) // log2_max_pic_order_cnt_lsb_minus4 -> 8
            .put(0, 1) // sub_layer_ordering_info_present
            .ue(3) // max_dec_pic_buffering_minus1
            .ue(2) // max_num_reorder_pics
            .ue(0) // max_latency_increase_plus1
            .ue(0) // log2_min_luma_coding_block_size_minus3
            .ue(3) // log2_diff_max_min
            .ue(0) // log2_min_transform_block_size_minus2
            .ue(3) // log2_diff_max_min_transform
            .ue(0) // max_transform_hierarchy_depth_inter
            .ue(0) // max_transform_hierarchy_depth_intra
            .put(1, 1) // scaling_list_enabled_flag
            .put(0, 1) // sps_scaling_list_data_present_flag
            .put(0, 1) // amp_enabled
            .put(1, 1) // sao_enabled
            .put(0, 1) // pcm_enabled
            .ue(0) // num_short_term_ref_pic_sets
            .put(0, 1) // long_term_ref_pics_present
            .put(1, 1) // temporal_mvp_enabled
            .put(1, 1); // strong_intra_smoothing
        let sps = parse_sps(&w.finish()).unwrap();
        assert_eq!((sps.width, sps.height), (1920, 1080));
        assert_eq!(sps.chroma_format_idc, 1);
        assert_eq!(sps.bit_depth_luma, 8);
        assert_eq!(sps.log2_max_pic_order_cnt_lsb, 8);
        assert_eq!(sps.max_num_reorder_pics, 2);
        assert!(sps.sao_enabled);
        assert!(sps.temporal_mvp_enabled);
        assert!(sps.scaling_list_enabled);
        assert_eq!(sps.scaling_list.as_ref().unwrap().matrices[3][5][63], 91);
        assert_eq!(sps.profile.general_profile_idc, 1);
        assert_eq!(sps.profile.general_level_idc, 120);
    }

    #[test]
    fn accepts_eight_and_ten_bit_rext_chroma_formats() {
        for chroma in 1..=3 {
            for depth in [0, 2] {
                let sps = parse_sps(&sps_with_chroma_and_depth(chroma, depth, depth)).unwrap();
                assert_eq!(sps.chroma_format_idc, chroma);
                assert_eq!(sps.bit_depth_luma, depth + 8);
            }
        }
    }

    #[test]
    fn rejects_twelve_bit_and_mismatched_bit_depths() {
        let twelve = parse_sps(&sps_with_chroma_and_depth(1, 4, 4)).unwrap_err();
        assert!(twelve.to_string().contains("8 or 10"));
        let mismatch = parse_sps(&sps_with_chroma_and_depth(2, 0, 2)).unwrap_err();
        assert!(mismatch.to_string().contains("bit-depth mismatch"));
    }

    #[test]
    fn rejects_separate_colour_plane_until_slice_headers_support_it() {
        let mut w = BitWriter::new();
        w.put(0, 4).put(0, 3).put(1, 1);
        profile_tier_level(&mut w);
        w.ue(0).ue(3).put(1, 1).ue(64).ue(64).put(0, 1).ue(0).ue(0);
        let error = parse_sps(&w.finish()).unwrap_err();
        assert!(error.to_string().contains("separate colour planes"));
    }

    #[test]
    fn parses_explicit_scaling_list_coefficients() {
        let mut w = BitWriter::new();
        w.put(1, 1); // first 4x4 matrix is explicit
        for _ in 0..16 {
            w.se(0);
        }
        for size_id in 0..4usize {
            let step = if size_id == 3 { 3 } else { 1 };
            for matrix_id in (0..6).step_by(step) {
                if size_id == 0 && matrix_id == 0 {
                    continue;
                }
                w.put(0, 1).ue(0); // use the preceding/default matrix
            }
        }
        let mut scaling = ScalingList::default_hevc();
        let data = w.finish();
        let mut reader = BitReader::new(&data);
        parse_scaling_list_data(&mut reader, &mut scaling).unwrap();

        assert!(scaling.matrices[0][0][..16].iter().all(|value| *value == 8));
        assert_eq!(
            scaling.matrices[1][0],
            ScalingList::default_hevc().matrices[1][0]
        );
    }

    #[test]
    fn explicit_32x32_scaling_list_stops_after_64_coefficients() {
        let mut w = BitWriter::new();
        for size_id in 0..4usize {
            let step = if size_id == 3 { 3 } else { 1 };
            for matrix_id in (0..6).step_by(step) {
                if size_id == 3 && matrix_id == 0 {
                    w.put(1, 1);
                    w.se(0); // dc_coef_minus_8
                    for _ in 0..64 {
                        w.se(0);
                    }
                } else {
                    w.put(0, 1).ue(0);
                }
            }
        }

        let mut scaling = ScalingList::default_hevc();
        let data = w.finish();
        let mut reader = BitReader::new(&data);
        parse_scaling_list_data(&mut reader, &mut scaling).unwrap();

        assert!(scaling.matrices[3][0].iter().all(|value| *value == 8));
        assert_eq!(scaling.dc[1][0], 8);
    }

    #[test]
    fn parses_sps_with_cropping() {
        let mut w = BitWriter::new();
        w.put(0, 4).put(0, 3).put(1, 1);
        profile_tier_level(&mut w);
        w.ue(0)
            .ue(1)
            .ue(1920)
            .ue(1088)
            .put(1, 1) // conformance window
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(4) // crop 4 rows bottom (4*2 = 8)
            .ue(0)
            .ue(0)
            .ue(0)
            .put(0, 1)
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(0)
            .put(0, 1) // scaling_list_enabled
            .put(0, 1)
            .put(0, 1)
            .put(0, 1)
            .ue(0)
            .put(0, 1)
            .put(0, 1)
            .put(0, 1);
        let sps = parse_sps(&w.finish()).unwrap();
        assert_eq!((sps.width, sps.height), (1920, 1080));
    }

    #[test]
    fn parses_long_term_ref_pics_in_sps() {
        let mut w = BitWriter::new();
        w.put(0, 4).put(0, 3).put(1, 1);
        profile_tier_level(&mut w);
        w.ue(0)
            .ue(1)
            .ue(1280)
            .ue(720)
            .put(0, 1)
            .ue(0)
            .ue(0)
            .ue(4)
            .put(0, 1)
            .ue(3)
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(3)
            .ue(0)
            .ue(3)
            .ue(0)
            .ue(0)
            .put(0, 1)
            .put(1, 1)
            .put(1, 1)
            .put(0, 1)
            .ue(0)
            .put(1, 1) // long_term_ref_pics_present
            .ue(2) // num_long_term_ref_pics_sps
            .put(3, 8)
            .put(1, 1)
            .put(7, 8)
            .put(0, 1)
            .put(0, 1) // temporal_mvp_enabled
            .put(1, 1); // strong_intra_smoothing

        let sps = parse_sps(&w.finish()).unwrap();
        assert!(sps.long_term_ref_pics_present);
        assert_eq!(sps.num_long_term_ref_pics, 2);
        assert_eq!(
            sps.long_term_refs_sps,
            vec![
                LongTermSpsRef {
                    poc_lsb: 3,
                    used: true,
                },
                LongTermSpsRef {
                    poc_lsb: 7,
                    used: false,
                },
            ]
        );
    }

    #[test]
    fn parses_short_term_ref_pic_sets_in_sps() {
        let mut w = BitWriter::new();
        w.put(0, 4).put(0, 3).put(1, 1);
        profile_tier_level(&mut w);
        w.ue(0)
            .ue(1)
            .ue(1280)
            .ue(720)
            .put(0, 1)
            .ue(0)
            .ue(0)
            .ue(4)
            .put(0, 1)
            .ue(3)
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(3)
            .ue(0)
            .ue(3)
            .ue(0)
            .ue(0)
            .put(0, 1)
            .put(1, 1)
            .put(1, 1)
            .put(0, 1)
            .ue(2) // num_short_term_ref_pic_sets
            // Set 0: -1 used, +1 unused.
            .ue(1)
            .ue(1)
            .ue(0)
            .put(1, 1)
            .ue(0)
            .put(0, 1)
            // Set 1 is predicted from set 0 and shifts it by +2.
            .put(1, 1)
            .put(0, 1)
            .ue(1)
            .put(1, 1)
            .put(1, 1)
            .put(1, 1)
            .put(0, 1) // long_term_ref_pics_present
            .put(0, 1) // temporal_mvp_enabled
            .put(1, 1); // strong_intra_smoothing

        let sps = parse_sps(&w.finish()).unwrap();
        assert_eq!(sps.short_term_ref_pic_sets.len(), 2);
        assert_eq!(sps.short_term_ref_pic_sets[0].s0, [-1]);
        assert_eq!(sps.short_term_ref_pic_sets[0].used_s0, [true]);
        assert_eq!(sps.short_term_ref_pic_sets[0].s1, [1]);
        assert_eq!(sps.short_term_ref_pic_sets[0].used_s1, [false]);
        assert_eq!(sps.short_term_ref_pic_sets[1].s1, [1, 2, 3]);
        assert_eq!(sps.short_term_ref_pic_sets[1].used_s1, [true, true, true]);
    }

    #[test]
    fn parses_pps() {
        let mut w = BitWriter::new();
        w.ue(0)
            .ue(0) // pps_id, sps_id
            .put(0, 1) // dependent_slice_segments
            .put(0, 1) // output_flag_present
            .put(0, 3) // num_extra_slice_header_bits
            .put(1, 1) // sign_data_hiding
            .put(0, 1) // cabac_init_present
            .ue(0)
            .ue(0) // num_ref_idx defaults
            .se(0) // init_qp_delta
            .put(0, 1) // constrained_intra
            .put(0, 1) // transform_skip
            .put(0, 1) // cu_qp_delta_enabled
            .se(0)
            .se(0) // cb/cr qp offset
            .put(0, 1) // slice_chroma_qp_offsets_present
            .put(1, 1) // weighted_pred
            .put(0, 1) // weighted_bipred
            .put(0, 1) // transquant_bypass
            .put(0, 1) // tiles
            .put(0, 1) // entropy_coding_sync
            .put(1, 1) // loop_filter_across_slices
            .put(1, 1) // deblocking_filter_control_present
            .put(0, 1) // override_enabled
            .put(0, 1) // pps_deblocking_filter_disabled
            .se(0)
            .se(0) // beta/tc offsets
            .put(0, 1) // scaling list present
            .put(1, 1) // lists_modification_present
            .ue(0) // log2_parallel_merge_level_minus2
            .put(0, 1); // slice_segment_header_extension_present
        let pps = parse_pps(&w.finish()).unwrap();
        assert_eq!(pps.pps_id, 0);
        assert_eq!(pps.init_qp, 26);
        assert!(pps.weighted_pred);
        assert!(pps.lists_modification_present);
        assert_eq!(pps.log2_parallel_merge_level, 2);
        assert!(!pps.tiles_enabled);
        assert!(
            pps.loop_filter_across_tiles,
            "MPP defaults this flag to 1 when tiles are disabled"
        );
    }

    #[test]
    fn parses_nonuniform_tile_geometry() {
        let mut w = BitWriter::new();
        w.ue(0)
            .ue(0)
            .put(0, 1)
            .put(0, 1)
            .put(0, 3)
            .put(0, 1)
            .put(0, 1)
            .ue(0)
            .ue(0)
            .se(0)
            .put(0, 1)
            .put(0, 1)
            .put(0, 1)
            .se(0)
            .se(0)
            .put(0, 1)
            .put(0, 1)
            .put(0, 1)
            .put(0, 1)
            .put(1, 1) // tiles_enabled
            .put(0, 1) // entropy_coding_sync
            .ue(2) // 3 tile columns
            .ue(1) // 2 tile rows
            .put(0, 1) // non-uniform spacing
            .ue(6) // first column width 7
            .ue(8) // second column width 9
            .ue(4) // first row height 5
            .put(1, 1) // loop_filter_across_tiles
            .put(1, 1) // loop_filter_across_slices
            .put(0, 1) // deblocking control absent
            .put(0, 1) // scaling list absent
            .put(1, 1) // lists modification present
            .ue(0) // log2_parallel_merge_level_minus2
            .put(0, 1); // slice header extension absent

        let pps = parse_pps(&w.finish()).unwrap();
        assert!(pps.tiles_enabled);
        assert!(!pps.uniform_spacing);
        assert_eq!(pps.tile_columns, 3);
        assert_eq!(pps.tile_rows, 2);
        assert_eq!(pps.tile_column_widths, [7, 9]);
        assert_eq!(pps.tile_row_heights, [5]);
    }

    fn sps_struct() -> Sps {
        Sps {
            sps_id: 0,
            chroma_format_idc: 1,
            width: 1280,
            height: 720,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            log2_max_pic_order_cnt_lsb: 8,
            num_short_term_ref_pic_sets: 0,
            sao_enabled: true,
            temporal_mvp_enabled: true,
            ..Default::default()
        }
    }

    fn pps_struct() -> Pps {
        Pps {
            pps_id: 0,
            sps_id: 0,
            num_extra_slice_header_bits: 0,
            ..Default::default()
        }
    }

    #[test]
    fn parses_idr_slice_header() {
        let mut w = BitWriter::new();
        w.put(1, 1) // first_slice_segment_in_pic_flag
            .put(0, 1) // no_output_of_prior_pics (IRAP)
            .ue(0) // pps id
            .ue(2) // slice_type = I
            .put(1, 1) // slice_sao_luma
            .put(1, 1); // slice_sao_chroma
        let sh =
            parse_slice_header(&w.finish(), &sps_struct(), &pps_struct(), NAL_IDR_W_RADL).unwrap();
        assert!(sh.first_slice);
        assert!(!sh.no_output_of_prior_pics);
        assert_eq!(sh.slice_type, 2);
        assert!(sh.sao_luma && sh.sao_chroma);
        assert_eq!(sh.poc_lsb, 0);
    }

    #[test]
    fn parses_dependent_non_first_slice_segment() {
        let sps = Sps {
            width: 1280,
            height: 720,
            log2_min_cb: 3,
            log2_diff_max_min_cb: 3,
            ..Default::default()
        };
        let pps = Pps {
            dependent_slice_segments: true,
            ..pps_struct()
        };
        let mut w = BitWriter::new();
        w.put(0, 1) // first_slice_segment_in_pic_flag
            .ue(0) // pps id
            .put(1, 1) // dependent_slice_segment_flag
            .put(37, 8); // slice_segment_address; 240 CTBs needs 8 bits

        let sh = parse_slice_header(&w.finish(), &sps, &pps, NAL_TRAIL_R).unwrap();
        assert!(!sh.first_slice);
        assert!(sh.dependent);
        assert_eq!(sh.segment_address, 37);
    }

    #[test]
    fn parses_independent_non_first_slice_segment_header() {
        let sps = Sps {
            width: 1280,
            height: 720,
            log2_min_cb: 3,
            log2_diff_max_min_cb: 3,
            sao_enabled: true,
            ..Default::default()
        };
        let pps = Pps {
            dependent_slice_segments: true,
            ..pps_struct()
        };
        let mut w = BitWriter::new();
        w.put(0, 1) // first_slice_segment_in_pic_flag
            .put(0, 1) // no_output_of_prior_pics (IRAP)
            .ue(0) // pps id
            .put(0, 1) // dependent_slice_segment_flag
            .put(37, 8) // slice_segment_address
            .ue(2) // slice_type = I
            .put(1, 1) // slice_sao_luma_flag
            .put(0, 1); // slice_sao_chroma_flag

        let sh = parse_slice_header(&w.finish(), &sps, &pps, NAL_IDR_W_RADL).unwrap();
        assert!(!sh.first_slice);
        assert!(!sh.dependent);
        assert_eq!(sh.segment_address, 37);
        assert_eq!(sh.slice_type, 2);
        assert!(sh.sao_luma);
        assert!(!sh.sao_chroma);
    }

    #[test]
    fn parses_p_slice_with_inline_rps() {
        let mut w = BitWriter::new();
        w.put(1, 1)
            .ue(0) // first_slice, pps id (non-IRAP: no no_output flag)
            .ue(1) // slice_type = P
            .put(42, 8) // poc_lsb
            .put(0, 1) // short_term_ref_pic_set_sps_flag = 0 -> inline
            .ue(1)
            .ue(0) // num_negative, num_positive
            .ue(0)
            .put(1, 1) // delta_poc_s0_minus1=0, used
            .put(1, 1) // slice_temporal_mvp_enabled_flag
            .put(0, 1)
            .put(0, 1); // sao luma/chroma
        let sh =
            parse_slice_header(&w.finish(), &sps_struct(), &pps_struct(), NAL_TRAIL_R).unwrap();
        assert_eq!(sh.slice_type, 1);
        assert_eq!(sh.poc_lsb, 42);
        assert_eq!(sh.rps.s0, vec![-1]);
        assert_eq!(sh.rps.used_s0, vec![true]);
        assert!(sh.rps.s1.is_empty());
        assert!(sh.temporal_mvp);
        assert!(!sh.sao_luma);
    }

    #[test]
    fn selects_the_sps_short_term_rps_named_by_the_slice() {
        let selected = ShortTermRps {
            s0: vec![-3, -1],
            used_s0: vec![true, false],
            s1: vec![2],
            used_s1: vec![true],
        };
        let sps = Sps {
            log2_max_pic_order_cnt_lsb: 8,
            num_short_term_ref_pic_sets: 3,
            short_term_ref_pic_sets: vec![
                ShortTermRps::default(),
                ShortTermRps {
                    s0: vec![-1],
                    used_s0: vec![true],
                    ..Default::default()
                },
                selected.clone(),
            ],
            ..Default::default()
        };
        let mut w = BitWriter::new();
        w.put(1, 1)
            .ue(0)
            .ue(1) // P slice
            .put(17, 8)
            .put(1, 1) // use an SPS set
            .put(2, 2); // short_term_ref_pic_set_idx = 2

        let sh = parse_slice_header(&w.finish(), &sps, &pps_struct(), NAL_TRAIL_R).unwrap();
        assert_eq!(sh.short_term_ref_pic_set_idx, 2);
        assert_eq!(sh.rps.s0, selected.s0);
        assert_eq!(sh.rps.used_s0, selected.used_s0);
        assert_eq!(sh.rps.s1, selected.s1);
        assert_eq!(sh.rps.used_s1, selected.used_s1);
    }

    #[test]
    fn parses_sps_and_explicit_long_term_slice_refs() {
        let sps = Sps {
            log2_max_pic_order_cnt_lsb: 8,
            long_term_ref_pics_present: true,
            num_long_term_ref_pics: 2,
            long_term_refs_sps: vec![
                LongTermSpsRef {
                    poc_lsb: 3,
                    used: true,
                },
                LongTermSpsRef {
                    poc_lsb: 7,
                    used: false,
                },
            ],
            ..Default::default()
        };
        let mut w = BitWriter::new();
        w.put(1, 1)
            .ue(0)
            .ue(1) // P slice
            .put(42, 8)
            .put(0, 1) // inline short-term RPS
            .ue(0)
            .ue(0)
            .ue(1) // num_long_term_sps
            .ue(1) // num_long_term_pics
            .put(1, 1) // lt_idx_sps = 1
            .put(0, 1) // no delta_poc_msb for the SPS entry
            .put(5, 8)
            .put(1, 1) // explicit POC 5 is used
            .put(1, 1) // delta_poc_msb_present
            .ue(1); // delta_poc_msb_cycle_lt

        let sh = parse_slice_header(&w.finish(), &sps, &pps_struct(), NAL_TRAIL_R).unwrap();
        assert_eq!(
            sh.long_term_rps.refs,
            vec![
                LongTermRefPic {
                    poc_lsb: 7,
                    used: false,
                    delta_msb_cycle: 0,
                },
                LongTermRefPic {
                    poc_lsb: 5,
                    used: true,
                    delta_msb_cycle: 1,
                },
            ]
        );
    }

    #[test]
    fn resolves_long_term_poc_msb_cycles() {
        let rps = LongTermRps {
            refs: vec![
                LongTermRefPic {
                    poc_lsb: 200,
                    used: false,
                    delta_msb_cycle: 0,
                },
                LongTermRefPic {
                    poc_lsb: 10,
                    used: true,
                    delta_msb_cycle: 1,
                },
            ],
        };

        assert_eq!(rps.resolve_pocs(300, 8), vec![(456, false), (10, true)]);
    }

    #[test]
    fn builds_predicted_rps() {
        // Base set: one negative picture at -1, then a predicted set that
        // shifts everything by -1 and adds the current reference.
        let base = ShortTermRps {
            s0: vec![-1],
            used_s0: vec![true],
            s1: vec![],
            used_s1: vec![],
        };
        let mut w = BitWriter::new();
        w.put(1, 1) // inter_ref_pic_set_prediction_flag
            .put(0, 1) // delta_rps_sign
            .ue(1) // abs_delta_rps_minus1 -> delta_rps = 2
            .put(1, 1) // used_by_curr_pic_flag[0]
            .put(1, 1); // used_by_curr_pic_flag[1]
        let data = w.finish();
        let mut r = BitReader::new(&data);
        let rps = parse_short_term_rps(&mut r, 1, &[base], false).unwrap();
        // Base -1 shifted by +2 becomes +1; the new picture at +2 joins it.
        assert_eq!(rps.s1, vec![1, 2]);
        assert!(rps.s0.is_empty());
    }

    #[test]
    fn predicted_slice_rps_reads_delta_idx_and_uses_only_the_signalled_flag() {
        let sets = [
            ShortTermRps {
                s0: vec![-2],
                used_s0: vec![false],
                s1: vec![],
                used_s1: vec![],
            },
            ShortTermRps {
                s0: vec![-9],
                used_s0: vec![true],
                s1: vec![],
                used_s1: vec![],
            },
        ];
        let mut w = BitWriter::new();
        w.put(1, 1) // inter_ref_pic_set_prediction_flag
            .ue(1) // delta_idx_minus1 selects set 0 rather than set 1
            .put(0, 1) // delta_rps_sign
            .ue(0) // abs_delta_rps_minus1 -> delta_rps = 1
            .put(1, 1) // used_by_curr_pic_flag[0]
            .put(1, 1); // used_by_curr_pic_flag[1]
        let data = w.finish();
        let mut r = BitReader::new(&data);

        let rps = parse_short_term_rps(&mut r, 2, &sets, true).unwrap();
        assert_eq!(rps.s0, [-1]);
        assert_eq!(rps.used_s0, [true]);
        assert_eq!(rps.s1, [1]);
        assert_eq!(rps.used_s1, [true]);
    }

    #[test]
    fn rejects_empty_sps() {
        assert!(matches!(parse_sps(&[]), Err(HevcError::Truncated)));
    }
}
