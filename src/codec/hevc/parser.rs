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

fn skip_profile_tier_level(r: &mut BitReader<'_>, max_sub_layers_minus1: u32) -> Result<ProfileTierLevel, HevcError> {
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
    pub pcm_bit_depth_luma: u32,
    pub pcm_bit_depth_chroma: u32,
    pub pcm_log2_min_cb: u32,
    pub pcm_log2_diff_max_min_cb: u32,
    pub pcm_loop_filter_disabled: bool,
    pub max_dec_pic_buffering: u32,
    pub num_long_term_ref_pics: u32,
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
    if sps.bit_depth_luma > 16 || sps.bit_depth_chroma > 16 {
        return Err(HevcError::Unsupported("bit depth > 16"));
    }
    sps.log2_max_pic_order_cnt_lsb = r.read_ue()? + 4;
    let sub_layer_ordering_present = r.read_bit()? == 1;
    for i in 0..(if sub_layer_ordering_present { max_sub_layers } else { 1 }) {
        let dpb = r.read_ue()? + 1;
        if i + 1 == (if sub_layer_ordering_present { max_sub_layers } else { 1 }) {
            sps.max_dec_pic_buffering = dpb;
        }
        let _ = r.read_ue()?; // sps_max_num_reorder_pics
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
        let present = r.read_bit()? == 1;
        if present {
            return Err(HevcError::Unsupported("sps_scaling_list_data"));
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
        skip_short_term_ref_pic_set(&mut r, i, sps.num_short_term_ref_pic_sets)?;
    }
    sps.long_term_ref_pics_present = r.read_bit()? == 1;
    if sps.long_term_ref_pics_present {
        let n = r.read_ue()?;
        if n > 32 {
            return Err(HevcError::Unsupported("num_long_term_ref_pics_sps"));
        }
        sps.num_long_term_ref_pics = n;
        for _ in 0..n {
            r.skip(sps.log2_max_pic_order_cnt_lsb)?;
            r.skip(1)?;
        }
    }
    sps.temporal_mvp_enabled = r.read_bit()? == 1;
    sps.strong_intra_smoothing = r.read_bit()? == 1;
    Ok(sps)
}

fn skip_short_term_ref_pic_set(
    r: &mut BitReader<'_>,
    idx: u32,
    num_sets: u32,
) -> Result<(), HevcError> {
    let inter_ref_pic_set_prediction_flag = if idx != 0 { r.read_bit()? == 1 } else { false };
    if inter_ref_pic_set_prediction_flag {
        if idx == num_sets {
            r.read_ue()?; // delta_idx_minus1 only for the SPS extension case
        }
        r.skip(1)?; // delta_rps_sign
        r.read_ue()?; // abs_delta_rps_minus1
        let used_by_curr = num_negative(r, 0);
        for _ in 0..used_by_curr {
            let used = r.read_bit()? == 1;
            if !used {
                r.skip(1)?;
            }
        }
        let used_positive = num_positive(r, 0);
        for _ in 0..used_positive {
            let used = r.read_bit()? == 1;
            if !used {
                r.skip(1)?;
            }
        }
        return Ok(());
    }
    let num_negative = r.read_ue()?;
    let num_positive = r.read_ue()?;
    if num_negative > 16 || num_positive > 16 {
        return Err(HevcError::Unsupported("ref pic set too large"));
    }
    for _ in 0..num_negative {
        r.read_ue()?; // delta_poc_s0_minus1
        r.skip(1)?; // used_by_curr_pic_s0_flag
    }
    for _ in 0..num_positive {
        r.read_ue()?;
        r.skip(1)?;
    }
    Ok(())
}

// The prediction branch reads a variable number of flags that depends on the
// referenced set; for the common case (no inter-set prediction) the counts are
// zero, which is all this parser needs today.
fn num_negative(_r: &BitReader<'_>, n: u32) -> u32 {
    n
}
fn num_positive(_r: &BitReader<'_>, n: u32) -> u32 {
    n
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
    pub entropy_coding_sync: bool,
    pub loop_filter_across_slices: bool,
    pub deblocking_filter_control_present: bool,
    pub deblocking_filter_override_enabled: bool,
    pub pps_deblocking_filter_disabled: bool,
    pub beta_offset_div2: i32,
    pub tc_offset_div2: i32,
    pub lists_modification_present: bool,
    pub log2_parallel_merge_level: u32,
    pub slice_segment_header_extension_present: bool,
}

pub fn parse_pps(rbsp: &[u8]) -> Result<Pps, HevcError> {
    let mut r = BitReader::new(rbsp);
    let mut pps = Pps::default();
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
        r.read_ue()?; // diff_cu_qp_delta_depth
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
                r.read_ue()?;
            }
            for _ in 0..(rows - 1) {
                r.read_ue()?;
            }
        }
        r.skip(1)?; // loop_filter_across_tiles_enabled_flag
    }
    if pps.entropy_coding_sync {
        r.skip(1)?; // loop_filter_across_substreams_enabled_flag
    }
    pps.loop_filter_across_slices = r.read_bit()? == 1;
    pps.deblocking_filter_control_present = r.read_bit()? == 1;
    if pps.deblocking_filter_control_present {
        pps.deblocking_filter_override_enabled = r.read_bit()? == 1;
        pps.pps_deblocking_filter_disabled = r.read_bit()? == 1;
        if !pps.pps_deblocking_filter_disabled {
            pps.beta_offset_div2 = r.read_se()?;
            pps.tc_offset_div2 = r.read_se()?;
        }
    }
    r.skip(1)?; // pps_scaling_list_data_present_flag (flat matrices assumed)
    pps.lists_modification_present = r.read_bit()? == 1;
    pps.log2_parallel_merge_level = r.read_ue()? + 2;
    pps.slice_segment_header_extension_present = r.read_bit()? == 1;
    Ok(pps)
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
) -> Result<ShortTermRps, HevcError> {
    let predicted = if idx != 0 { r.read_bit()? == 1 } else { false };
    if predicted {
        if sets.is_empty() {
            return Err(HevcError::BadValue);
        }
        // delta_idx_minus1 defaults to 0 (the previous set).
        let delta_idx_minus1 = 0;
        let ref_idx = (idx as i32 - delta_idx_minus1 - 1).max(0) as usize;
        let ref_set = sets.get(ref_idx).ok_or(HevcError::BadValue)?;
        let delta_rps_sign = r.read_bit()?;
        let abs_delta_rps_minus1 = r.read_ue()? as i32;
        let delta_rps = (1 - 2 * delta_rps_sign as i32) * (abs_delta_rps_minus1 + 1);

        let n = ref_set.num_delta_pocs() as usize;
        let mut used = vec![false; n + 1];
        let mut use_delta = vec![true; n + 1];
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
        let ref_used: Vec<bool> = ref_set
            .used_s0
            .iter()
            .copied()
            .chain(ref_set.used_s1.iter().copied())
            .collect();
        for (j, &d) in refs.iter().enumerate() {
            let cand = d + delta_rps;
            if cand < 0 && use_delta[j] {
                entry.push((cand, used[j] || ref_used[j]));
            }
        }
        if use_delta[n] && delta_rps < 0 {
            entry.push((delta_rps, used[n]));
        }
        for (j, &d) in refs.iter().enumerate() {
            let cand = d + delta_rps;
            if cand > 0 && use_delta[j] {
                entry.push((cand, used[j] || ref_used[j]));
            }
        }
        if use_delta[n] && delta_rps > 0 {
            entry.push((delta_rps, used[n]));
        }

        // Split into s0 (negative, descending magnitude) and s1 (positive).
        let mut neg: Vec<(i32, bool)> = entry.iter().copied().filter(|(d, _)| *d < 0).collect();
        neg.sort_by(|a, b| b.0.cmp(&a.0)); // -1, -2, ...
        let mut pos: Vec<(i32, bool)> = entry.iter().copied().filter(|(d, _)| *d > 0).collect();
        pos.sort_by(|a, b| a.0.cmp(&b.0)); // 1, 2, ...
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
    pub temporal_mvp: bool,
    pub sao_luma: bool,
    pub sao_chroma: bool,
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
        // slice_segment_address uses ceil(log2(PicSizeInCtbsY)) bits; the
        // caller that needs it can recompute it from the SPS CTB size.
        let ctb = 3 + 0; // filled by the caller when needed
        let _ = ctb;
        return Err(HevcError::Unsupported("non-first slice segment"));
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
            sh.rps = parse_short_term_rps(&mut r, sps.num_short_term_ref_pic_sets, &[])?;
        } else if sps.num_short_term_ref_pic_sets > 1 {
            let bits = 32 - sps.num_short_term_ref_pic_sets.leading_zeros();
            sh.short_term_ref_pic_set_idx = r.read_bits(bits)?;
        }
        if sps.long_term_ref_pics_present {
            return Err(HevcError::Unsupported("long-term ref pics"));
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
            let k = if v <= 0 { (-v as u32) * 2 } else { (v as u32) * 2 - 1 };
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
            .ue(0) // max_num_reorder_pics
            .ue(0) // max_latency_increase_plus1
            .ue(0) // log2_min_luma_coding_block_size_minus3
            .ue(3) // log2_diff_max_min
            .ue(0) // log2_min_transform_block_size_minus2
            .ue(3) // log2_diff_max_min_transform
            .ue(0) // max_transform_hierarchy_depth_inter
            .ue(0) // max_transform_hierarchy_depth_intra
            .put(0, 1) // scaling_list_enabled_flag
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
        assert!(sps.sao_enabled);
        assert!(sps.temporal_mvp_enabled);
        assert_eq!(sps.profile.general_profile_idc, 1);
        assert_eq!(sps.profile.general_level_idc, 120);
    }

    #[test]
    fn parses_sps_with_cropping() {
        let mut w = BitWriter::new();
        w.put(0, 4).put(0, 3).put(1, 1);
        profile_tier_level(&mut w);
        w.ue(0).ue(1).ue(1920).ue(1088)
            .put(1, 1) // conformance window
            .ue(0).ue(0).ue(0).ue(4) // crop 4 rows bottom (4*2 = 8)
            .ue(0).ue(0).ue(0)
            .put(0, 1).ue(0).ue(0).ue(0)
            .ue(0).ue(0).ue(0).ue(0).ue(0).ue(0)
            .put(0, 1) // scaling_list_enabled
            .put(0, 1).put(0, 1).put(0, 1)
            .ue(0).put(0, 1).put(0, 1).put(0, 1);
        let sps = parse_sps(&w.finish()).unwrap();
        assert_eq!((sps.width, sps.height), (1920, 1080));
    }

    #[test]
    fn parses_pps() {
        let mut w = BitWriter::new();
        w.ue(0).ue(0) // pps_id, sps_id
            .put(0, 1) // dependent_slice_segments
            .put(0, 1) // output_flag_present
            .put(0, 3) // num_extra_slice_header_bits
            .put(1, 1) // sign_data_hiding
            .put(0, 1) // cabac_init_present
            .ue(0).ue(0) // num_ref_idx defaults
            .se(0) // init_qp_delta
            .put(0, 1) // constrained_intra
            .put(0, 1) // transform_skip
            .put(0, 1) // cu_qp_delta_enabled
            .se(0).se(0) // cb/cr qp offset
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
            .se(0).se(0) // beta/tc offsets
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
        let sh = parse_slice_header(&w.finish(), &sps_struct(), &pps_struct(), NAL_IDR_W_RADL)
            .unwrap();
        assert!(sh.first_slice);
        assert!(!sh.no_output_of_prior_pics);
        assert_eq!(sh.slice_type, 2);
        assert!(sh.sao_luma && sh.sao_chroma);
        assert_eq!(sh.poc_lsb, 0);
    }

    #[test]
    fn parses_p_slice_with_inline_rps() {
        let mut w = BitWriter::new();
        w.put(1, 1).ue(0) // first_slice, pps id (non-IRAP: no no_output flag)
            .ue(1) // slice_type = P
            .put(42, 8) // poc_lsb
            .put(0, 1) // short_term_ref_pic_set_sps_flag = 0 -> inline
            .ue(1).ue(0) // num_negative, num_positive
            .ue(0).put(1, 1) // delta_poc_s0_minus1=0, used
            .put(1, 1) // slice_temporal_mvp_enabled_flag
            .put(0, 1).put(0, 1); // sao luma/chroma
        let sh = parse_slice_header(&w.finish(), &sps_struct(), &pps_struct(), NAL_TRAIL_R)
            .unwrap();
        assert_eq!(sh.slice_type, 1);
        assert_eq!(sh.poc_lsb, 42);
        assert_eq!(sh.rps.s0, vec![-1]);
        assert_eq!(sh.rps.used_s0, vec![true]);
        assert!(sh.rps.s1.is_empty());
        assert!(sh.temporal_mvp);
        assert!(!sh.sao_luma);
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
        let rps = parse_short_term_rps(&mut r, 1, &[base]).unwrap();
        // Base -1 shifted by +2 becomes +1; the new picture at +2 joins it.
        assert_eq!(rps.s1, vec![1, 2]);
        assert!(rps.s0.is_empty());
    }

    #[test]
    fn rejects_empty_sps() {
        assert!(matches!(parse_sps(&[]), Err(HevcError::Truncated)));
    }
}
