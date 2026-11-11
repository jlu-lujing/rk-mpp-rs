//! Annex-B NAL splitting, RBSP de-escaping and the H.264 syntax structures
//! the hardware needs: SPS, PPS, slice header.
//!
//! References: ITU-T H.264 §7.3.1 (NAL), §7.3.2.1 (SPS), §7.3.2.2 (PPS),
//! §7.3.3 (slice header) and §7.4.1 for the emulation-prevention bytes.

use std::borrow::Cow;
use std::fmt;

pub const NAL_SLICE: u8 = 1;
pub const NAL_IDR: u8 = 5;
pub const NAL_SEI: u8 = 6;
pub const NAL_SPS: u8 = 7;
pub const NAL_PPS: u8 = 8;
pub const NAL_AUD: u8 = 9;
pub const NAL_END_SEQ: u8 = 10;
pub const NAL_END_STREAM: u8 = 11;
pub const NAL_FILLER: u8 = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpsError {
    Truncated,
    BadWidth,
    BadHeight,
    Unsupported(&'static str),
}

impl fmt::Display for SpsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpsError::Truncated => write!(f, "bitstream truncated"),
            SpsError::BadWidth => write!(f, "invalid width"),
            SpsError::BadHeight => write!(f, "invalid height"),
            SpsError::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

impl std::error::Error for SpsError {}

/// MSB-first bit reader over an RBSP.
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    pub fn bit_pos(&self) -> usize {
        self.pos
    }

    pub fn is_aligned_byte(&self) -> bool {
        self.pos % 8 == 0
    }

    pub fn bits_left(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    pub fn read_bit(&mut self) -> Result<u32, SpsError> {
        if self.pos >= self.data.len() * 8 {
            return Err(SpsError::Truncated);
        }
        let b = (self.data[self.pos / 8] >> (7 - (self.pos % 8))) & 1;
        self.pos += 1;
        Ok(b as u32)
    }

    pub fn read_bits(&mut self, n: u32) -> Result<u32, SpsError> {
        if n == 0 {
            return Ok(0);
        }
        if n > 32 || self.bits_left() < n as usize {
            return Err(SpsError::Truncated);
        }
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.read_bit()?;
        }
        Ok(v)
    }

    pub fn read_bits_u64(&mut self, n: u32) -> Result<u64, SpsError> {
        let mut v = 0u64;
        for _ in 0..n {
            v = (v << 1) | self.read_bit()? as u64;
        }
        Ok(v)
    }

    pub fn skip(&mut self, n: u32) -> Result<(), SpsError> {
        if self.bits_left() < n as usize {
            return Err(SpsError::Truncated);
        }
        self.pos += n as usize;
        Ok(())
    }

    /// Unsigned Exp-Golomb (ue(v)).
    pub fn read_ue(&mut self) -> Result<u32, SpsError> {
        let mut zeros = 0u32;
        while self.read_bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return Err(SpsError::Truncated);
            }
        }
        if zeros == 0 {
            return Ok(0);
        }
        let rest = self.read_bits(zeros)?;
        Ok((1u32 << zeros) - 1 + rest)
    }

    /// Signed Exp-Golomb (se(v)).
    pub fn read_se(&mut self) -> Result<i32, SpsError> {
        let k = self.read_ue()? as i64;
        let v = if k % 2 == 0 { -(k / 2) } else { (k + 1) / 2 };
        Ok(v as i32)
    }

    /// §7.2: true when at least one syntax element remains before the
    /// rbsp_stop_one_bit. The tail is the last set bit in the RBSP.
    pub fn more_rbsp_data(&self) -> bool {
        let total = self.data.len() * 8;
        if self.pos >= total {
            return false;
        }
        let mut last = total;
        for i in (self.pos..total).rev() {
            if (self.data[i / 8] >> (7 - i % 8)) & 1 == 1 {
                last = i;
                break;
            }
        }
        self.pos < last
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
    pub ref_idc: u8,
    pub unit_type: u8,
    pub rbsp: Cow<'a, [u8]>,
    /// Byte range in the original stream, for zero-copy submission.
    pub offset: usize,
    pub len: usize,
}

impl NalUnit<'_> {
    pub fn is_vcl(&self) -> bool {
        (1..=5).contains(&self.unit_type)
    }
}

/// Split an Annex-B byte stream into NAL units.
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
        let end = starts
            .get(n + 1)
            .map(|&(start, _)| start)
            .unwrap_or(data.len());
        if payload_start >= end {
            continue;
        }
        // Keep trailing_zero_8bits: they are part of the NAL for the
        // hardware parser (MPP's packets include them), and trimming them
        // truncates the last slice of an access unit.
        let payload_end = end;
        let header = data[payload_start];
        let body = &data[payload_start + 1..payload_end];
        out.push(NalUnit {
            ref_idc: (header >> 5) & 3,
            unit_type: header & 0x1f,
            rbsp: ebsp_to_rbsp(body),
            offset: sc_start,
            len: payload_end - sc_start,
        });
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct Sps {
    pub profile_idc: u8,
    pub level_idc: u8,
    pub sps_id: u32,
    pub chroma_format_idc: u32,
    pub separate_colour_plane: bool,
    pub transform_bypass: bool,
    pub bit_depth_luma: u32,
    pub bit_depth_chroma: u32,
    pub log2_max_frame_num: u32,
    pub poc_type: u32,
    pub log2_max_poc_lsb: u32,
    pub delta_pic_order_always_zero: bool,
    pub offset_for_non_ref_pic: i32,
    pub offset_for_top_to_bottom_field: i32,
    pub offset_for_ref_frame: Vec<i32>,
    pub max_num_ref_frames: u32,
    /// Cropped display dimensions.
    pub width: u32,
    pub height: u32,
    /// Dimensions before frame cropping. Interlaced 720p is coded as 736
    /// lines, but displayed as 720 lines.
    pub coded_width: u32,
    pub coded_height: u32,
    /// `pic_height_in_map_units_minus1 + 1`, the height field written to the
    /// hardware SPS/PPS table. For an MBAFF frame this is half the coded MB
    /// height.
    pub height_map_units: u32,
    pub frame_mbs_only: bool,
    pub mb_adaptive_frame_field: bool,
    pub gaps_in_frame_num_allowed: bool,
    pub direct_8x8_inference: bool,
    pub frame_crop: Option<(u32, u32, u32, u32)>,
    pub vui_timing: Option<(u32, u32)>,
    /// Display reorder depth from VUI bitstream restrictions. Missing VUI
    /// restrictions use the H.264 maximum of 16 pictures.
    pub max_num_reorder_frames: Option<u32>,
}

const HIGH_PROFILES: &[u8] = &[100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

pub fn parse_sps(rbsp: &[u8]) -> Result<Sps, SpsError> {
    let mut r = BitReader::new(rbsp);
    let mut sps = Sps::default();
    sps.profile_idc = r.read_bits(8)? as u8;
    r.skip(8)?; // constraint_set flags + reserved
    sps.level_idc = r.read_bits(8)? as u8;
    sps.sps_id = r.read_ue()?;
    if sps.sps_id > 31 {
        return Err(SpsError::Unsupported("sps_id > 31"));
    }

    sps.chroma_format_idc = 1;
    sps.bit_depth_luma = 8;
    sps.bit_depth_chroma = 8;
    if HIGH_PROFILES.contains(&sps.profile_idc) {
        sps.chroma_format_idc = r.read_ue()?;
        if sps.chroma_format_idc == 3 {
            sps.separate_colour_plane = r.read_bit()? == 1;
        }
        if sps.chroma_format_idc > 3 {
            return Err(SpsError::Unsupported("chroma_format_idc"));
        }
        sps.bit_depth_luma = r.read_ue()? + 8;
        sps.bit_depth_chroma = r.read_ue()? + 8;
        if sps.bit_depth_luma > 10 || sps.bit_depth_chroma > 10 {
            return Err(SpsError::Unsupported("bit depth > 10"));
        }
        sps.transform_bypass = r.read_bit()? == 1;
        if r.read_bit()? == 1 {
            // scaling matrices
            let count = if sps.chroma_format_idc != 3 { 8 } else { 12 };
            for i in 0..count {
                if r.read_bit()? == 1 {
                    let size = if i < 6 { 16 } else { 64 };
                    skip_scaling_list(&mut r, size)?;
                }
            }
        }
    }

    sps.log2_max_frame_num = r.read_ue()? + 4;
    if sps.log2_max_frame_num > 16 {
        return Err(SpsError::Unsupported("log2_max_frame_num"));
    }
    sps.poc_type = r.read_ue()?;
    if sps.poc_type == 0 {
        sps.log2_max_poc_lsb = r.read_ue()? + 4;
    } else if sps.poc_type == 1 {
        sps.delta_pic_order_always_zero = r.read_bit()? == 1;
        sps.offset_for_non_ref_pic = r.read_se()?;
        sps.offset_for_top_to_bottom_field = r.read_se()?;
        let n = r.read_ue()?;
        if n > 255 {
            return Err(SpsError::Unsupported("num_ref_frames_in_poc_cycle"));
        }
        for _ in 0..n {
            sps.offset_for_ref_frame.push(r.read_se()?);
        }
    } else if sps.poc_type > 2 {
        return Err(SpsError::Unsupported("pic_order_cnt_type"));
    }
    sps.max_num_ref_frames = r.read_ue()?;
    sps.gaps_in_frame_num_allowed = r.read_bit()? == 1;
    let width_mbs = r.read_ue()? + 1;
    let height_map_units = r.read_ue()? + 1;
    sps.frame_mbs_only = r.read_bit()? == 1;
    if !sps.frame_mbs_only {
        sps.mb_adaptive_frame_field = r.read_bit()? == 1;
    }
    sps.direct_8x8_inference = r.read_bit()? == 1;

    let mut w = width_mbs * 16;
    let mut h = height_map_units * 16 * if sps.frame_mbs_only { 1 } else { 2 };
    sps.coded_width = w;
    sps.coded_height = h;
    sps.height_map_units = height_map_units;

    if r.read_bit()? == 1 {
        let l = r.read_ue()?;
        let rt = r.read_ue()?;
        let t = r.read_ue()?;
        let b = r.read_ue()?;
        sps.frame_crop = Some((l, rt, t, b));
        let (sx, sy) = if sps.chroma_format_idc == 0 || sps.separate_colour_plane {
            (1, 1)
        } else if sps.chroma_format_idc == 3 {
            (1, 1)
        } else if sps.chroma_format_idc == 1 {
            (2, 2)
        } else {
            (2, 1)
        };
        let cux = sx;
        let cuy = sy * if sps.frame_mbs_only { 1 } else { 2 };
        w = w.saturating_sub((l + rt) * cux);
        h = h.saturating_sub((t + b) * cuy);
    }
    if w == 0 {
        return Err(SpsError::BadWidth);
    }
    if h == 0 {
        return Err(SpsError::BadHeight);
    }
    if w > 8192 || h > 8192 {
        return Err(SpsError::Unsupported("dimensions > 8192"));
    }
    sps.width = w;
    sps.height = h;

    if r.read_bit()? == 1 {
        let vui = parse_vui(&mut r)?;
        sps.vui_timing = vui.timing;
        sps.max_num_reorder_frames = vui.max_num_reorder_frames;
    }
    Ok(sps)
}

fn skip_scaling_list(r: &mut BitReader<'_>, size: usize) -> Result<(), SpsError> {
    let mut last = 8i32;
    let mut next = 8i32;
    for _ in 0..size {
        if next != 0 {
            let delta = r.read_se()?;
            next = (last + delta + 256) % 256;
        }
        if next != 0 {
            last = next;
        }
    }
    Ok(())
}

struct Vui {
    timing: Option<(u32, u32)>,
    max_num_reorder_frames: Option<u32>,
}

fn parse_vui(r: &mut BitReader<'_>) -> Result<Vui, SpsError> {
    if r.read_bit()? == 1 {
        let idc = r.read_bits(8)?;
        if idc == 255 {
            r.skip(32)?;
        }
    }
    if r.read_bit()? == 1 {
        r.skip(1)?;
    }
    if r.read_bit()? == 1 {
        r.skip(4)?;
        if r.read_bit()? == 1 {
            r.skip(24)?;
        }
    }
    if r.read_bit()? == 1 {
        let _ = r.read_ue()?;
        let _ = r.read_ue()?;
    }
    let mut timing = None;
    if r.read_bit()? == 1 {
        let num_units = r.read_bits(32)?;
        let time_scale = r.read_bits(32)?;
        r.skip(1)?;
        timing = Some((num_units, time_scale));
    }
    let nal_hrd = r.read_bit()? == 1;
    if nal_hrd {
        skip_hrd(r)?;
    }
    let vcl_hrd = r.read_bit()? == 1;
    if vcl_hrd {
        skip_hrd(r)?;
    }
    if nal_hrd || vcl_hrd {
        r.skip(1)?;
    }
    r.skip(1)?; // pic_struct_present_flag
    let mut max_num_reorder_frames = None;
    if r.read_bit()? == 1 {
        r.skip(1)?;
        let _ = r.read_ue()?;
        let _ = r.read_ue()?;
        let _ = r.read_ue()?;
        let _ = r.read_ue()?;
        max_num_reorder_frames = Some(r.read_ue()?);
        let _ = r.read_ue()?; // max_dec_frame_buffering
    }
    Ok(Vui {
        timing,
        max_num_reorder_frames,
    })
}

fn skip_hrd(r: &mut BitReader<'_>) -> Result<(), SpsError> {
    let cpb_cnt = r.read_ue()? + 1;
    if cpb_cnt > 32 {
        return Err(SpsError::Unsupported("cpb_cnt"));
    }
    r.skip(8)?;
    for _ in 0..cpb_cnt {
        let _ = r.read_ue()?;
        let _ = r.read_ue()?;
        r.skip(1)?;
    }
    r.skip(20)?;
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct Pps {
    pub pps_id: u32,
    pub sps_id: u32,
    pub entropy_coding: bool,
    pub bottom_field_pic_order: bool,
    pub num_slice_groups: u32,
    pub slice_group_map_type: u32,
    pub num_ref_idx_l0: u32,
    pub num_ref_idx_l1: u32,
    pub weighted_pred: bool,
    pub weighted_bipred_idc: u32,
    pub pic_init_qp: i32,
    pub pic_init_qs: i32,
    pub chroma_qp_offset: i32,
    pub deblocking_filter_control: bool,
    pub constrained_intra_pred: bool,
    pub redundant_pic_cnt: bool,
    pub transform_8x8: bool,
    pub second_chroma_qp_offset: i32,
}

pub fn parse_pps(rbsp: &[u8]) -> Result<Pps, SpsError> {
    let mut r = BitReader::new(rbsp);
    let mut pps = Pps::default();
    pps.pps_id = r.read_ue()?;
    pps.sps_id = r.read_ue()?;
    pps.entropy_coding = r.read_bit()? == 1;
    pps.bottom_field_pic_order = r.read_bit()? == 1;
    pps.num_slice_groups = r.read_ue()? + 1;
    if pps.num_slice_groups > 8 {
        return Err(SpsError::Unsupported("num_slice_groups > 8"));
    }
    if pps.num_slice_groups > 1 {
        let map_type = r.read_ue()?;
        pps.slice_group_map_type = map_type;
        match map_type {
            0 => {
                for _ in 0..pps.num_slice_groups {
                    let _ = r.read_ue()?;
                }
            }
            2 => {
                for _ in 0..(pps.num_slice_groups - 1) {
                    let _ = r.read_ue()?;
                    let _ = r.read_ue()?;
                }
            }
            3..=5 => {
                r.skip(1)?;
                let _ = r.read_ue()?;
            }
            6 => {
                let n = r.read_ue()? + 1;
                if n > 4096 {
                    return Err(SpsError::Unsupported("slice_group pic size"));
                }
                let bits = 32 - (pps.num_slice_groups - 1).leading_zeros();
                r.skip(n * bits)?;
            }
            _ => return Err(SpsError::Unsupported("slice_group_map_type")),
        }
    }
    pps.num_ref_idx_l0 = r.read_ue()? + 1;
    pps.num_ref_idx_l1 = r.read_ue()? + 1;
    pps.weighted_pred = r.read_bit()? == 1;
    pps.weighted_bipred_idc = r.read_bits(2)?;
    pps.pic_init_qp = 26 + r.read_se()?;
    pps.pic_init_qs = 26 + r.read_se()?;
    pps.chroma_qp_offset = r.read_se()?;
    pps.deblocking_filter_control = r.read_bit()? == 1;
    pps.constrained_intra_pred = r.read_bit()? == 1;
    pps.redundant_pic_cnt = r.read_bit()? == 1;
    pps.transform_8x8 = false;
    // When the optional tail is absent, second_chroma_qp_index_offset is
    // inferred to equal chroma_qp_index_offset (H.264 §7.3.2.2 / §7.4.2.2).
    pps.second_chroma_qp_offset = pps.chroma_qp_offset;
    if r.more_rbsp_data() {
        pps.transform_8x8 = r.read_bit()? == 1;
        if r.read_bit()? == 1 {
            return Err(SpsError::Unsupported("pic_scaling_matrix"));
        }
        pps.second_chroma_qp_offset = r.read_se()?;
    }
    Ok(pps)
}

/// Reference-list reordering operation (modification_of_pic_nums_idc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefMod {
    /// Subtract abs_diff_pic_num from the current frame number.
    Sub(u32),
    /// Add abs_diff_pic_num.
    Add(u32),
    /// Long-term reference with the given pic_num.
    LongTerm(u32),
    /// End of the list.
    End,
}

/// Decoded reference picture marking operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mmco {
    /// Mark the short-term picture `difference_of_pic_nums_minus1` before
    /// the current picture as unused.
    UnmarkShort(u32),
    /// Mark the long-term picture with the given `long_term_pic_num` as unused.
    UnmarkLong(u32),
    /// Mark a short-term picture as long-term with frame_num delta.
    MakeLong { delta: u32, idx: u32 },
    /// Limit the long-term index range.
    MaxLong(u32),
    /// Mark all pictures unused.
    UnmarkAll,
    /// Assign a long-term frame index.
    MakeLongCurrent(u32),
}

#[derive(Debug, Clone, Default)]
pub struct Weights {
    pub luma_log2_denom: u32,
    pub chroma_log2_denom: u32,
    pub l0: Vec<WeightEntry>,
    pub l1: Vec<WeightEntry>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WeightEntry {
    pub luma_flag: bool,
    pub luma_weight: i32,
    pub luma_offset: i32,
    pub chroma_flag: bool,
    pub chroma_weight: [i32; 2],
    pub chroma_offset: [i32; 2],
}

#[derive(Debug, Clone, Default)]
pub struct SliceHeader {
    pub first_mb: u32,
    pub slice_type: u32,
    pub pps_id: u32,
    pub nal_ref_idc: u8,
    pub frame_num: u32,
    pub field_pic: bool,
    pub bottom_field: bool,
    pub idr_pic_id: Option<u32>,
    pub poc_lsb: Option<u32>,
    pub delta_poc_bottom: Option<i32>,
    /// `delta_pic_order_cnt[0]` and `[1]` for POC type 1.
    pub delta_poc: [Option<i32>; 2],
    pub redundant_pic_cnt: Option<u32>,
    pub direct_spatial: bool,
    pub ref_idx_l0_active: u32,
    pub ref_idx_l1_active: u32,
    pub mod_l0: Vec<RefMod>,
    pub mod_l1: Vec<RefMod>,
    pub weights: Option<Weights>,
    pub mmco: Vec<Mmco>,
    /// `adaptive_ref_pic_buffering_mode`: the current picture's marking is
    /// specified by MMCO operations rather than the sliding window.
    pub adaptive_ref_pic_buffering: bool,
    pub no_output_of_prior_pics: bool,
    pub long_term_reference: bool,
    pub cabac_init_idc: Option<u32>,
    pub qp_delta: i32,
    pub qs_delta: i32,
    pub deblocking_disable_idc: u32,
    pub deblocking_alpha: i32,
    pub deblocking_beta: i32,
    pub slice_group_change_cycle: Option<u32>,
    /// Bit offset of the slice data inside the RBSP (CABAC alignment point).
    pub data_offset_bits: usize,
}

impl SliceHeader {
    pub fn is_i(&self) -> bool {
        self.slice_type % 5 == 2
    }
    pub fn is_p(&self) -> bool {
        self.slice_type % 5 == 0
    }
    pub fn is_b(&self) -> bool {
        self.slice_type % 5 == 1
    }
}

/// Read only `pps_id` from a slice header, so the caller can select the right
/// PPS before running the full parse.
pub fn slice_pps_id(rbsp: &[u8]) -> Result<u32, SpsError> {
    let mut r = BitReader::new(rbsp);
    let _first_mb = r.read_ue()?;
    let _slice_type = r.read_ue()?;
    r.read_ue()
}

pub fn parse_slice_header(
    rbsp: &[u8],
    sps: &Sps,
    pps: &Pps,
    nal_type: u8,
    ref_idc: u8,
) -> Result<SliceHeader, SpsError> {
    let mut r = BitReader::new(rbsp);
    let mut sh = SliceHeader::default();
    sh.nal_ref_idc = ref_idc;
    sh.first_mb = r.read_ue()?;
    sh.slice_type = r.read_ue()?;
    sh.pps_id = r.read_ue()?;
    if sps.separate_colour_plane {
        r.skip(2)?;
    }
    sh.frame_num = r.read_bits(sps.log2_max_frame_num)?;
    if !sps.frame_mbs_only {
        if r.read_bit()? == 1 {
            sh.field_pic = true;
            sh.bottom_field = r.read_bit()? == 1;
        }
    }
    if nal_type == NAL_IDR {
        sh.idr_pic_id = Some(r.read_ue()?);
    }
    if sps.poc_type == 0 {
        sh.poc_lsb = Some(r.read_bits(sps.log2_max_poc_lsb)?);
        if pps.bottom_field_pic_order && !sh.field_pic {
            sh.delta_poc_bottom = Some(r.read_se()?);
        }
    } else if sps.poc_type == 1 && !sps.delta_pic_order_always_zero {
        sh.delta_poc[0] = Some(r.read_se()?);
        if pps.bottom_field_pic_order && !sh.field_pic {
            sh.delta_poc[1] = Some(r.read_se()?);
        }
    }
    if pps.redundant_pic_cnt {
        sh.redundant_pic_cnt = Some(r.read_ue()?);
    }
    if sh.is_b() {
        sh.direct_spatial = r.read_bit()? == 1;
    }

    let (mut l0, mut l1) = (1u32, 1u32);
    if sh.is_p() || sh.is_b() {
        l0 = pps.num_ref_idx_l0;
        l1 = pps.num_ref_idx_l1;
        if r.read_bit()? == 1 {
            l0 = r.read_ue()? + 1;
            if sh.is_b() {
                l1 = r.read_ue()? + 1;
            }
        }
    }
    if l0 > 32 || l1 > 32 {
        return Err(SpsError::Unsupported("num_ref_idx_active"));
    }

    let read_mods = |r: &mut BitReader<'_>| -> Result<Vec<RefMod>, SpsError> {
        let mut out = Vec::new();
        if r.read_bit()? == 0 {
            return Ok(out);
        }
        loop {
            match r.read_ue()? {
                0 => out.push(RefMod::Sub(r.read_ue()?)),
                1 => out.push(RefMod::Add(r.read_ue()?)),
                2 => out.push(RefMod::LongTerm(r.read_ue()?)),
                3 => {
                    out.push(RefMod::End);
                    break;
                }
                4 | 5 => {
                    let _ = r.read_ue()?;
                }
                _ => return Err(SpsError::Unsupported("ref_pic_list_modification")),
            }
            if out.len() > 66 {
                return Err(SpsError::Unsupported("ref_pic_list_modification length"));
            }
        }
        Ok(out)
    };
    if sh.is_p() || sh.is_b() {
        sh.mod_l0 = read_mods(&mut r)?;
    }
    if sh.is_b() {
        sh.mod_l1 = read_mods(&mut r)?;
    }

    if (pps.weighted_pred && sh.is_p()) || (pps.weighted_bipred_idc == 1 && sh.is_b()) {
        let mut w = Weights {
            luma_log2_denom: r.read_ue()?,
            ..Default::default()
        };
        if sps.chroma_format_idc != 0 {
            w.chroma_log2_denom = r.read_ue()?;
        }
        let chroma = sps.chroma_format_idc != 0;
        w.l0 = read_weight_entries(&mut r, l0, chroma)?;
        if sh.is_b() {
            w.l1 = read_weight_entries(&mut r, l1, chroma)?;
        }
        sh.weights = Some(w);
    }

    sh.ref_idx_l0_active = l0;
    sh.ref_idx_l1_active = l1;

    if ref_idc != 0 {
        if nal_type == NAL_IDR {
            sh.no_output_of_prior_pics = r.read_bit()? == 1;
            sh.long_term_reference = r.read_bit()? == 1;
        } else if r.read_bit()? == 1 {
            sh.adaptive_ref_pic_buffering = true;
            loop {
                let op = r.read_ue()?;
                if op == 0 {
                    break;
                }
                let mmco = match op {
                    1 => Mmco::UnmarkShort(r.read_ue()?),
                    2 => Mmco::UnmarkLong(r.read_ue()?),
                    3 => Mmco::MakeLong {
                        delta: r.read_ue()?,
                        idx: r.read_ue()?,
                    },
                    4 => Mmco::MaxLong(r.read_ue()?),
                    5 => Mmco::UnmarkAll,
                    6 => Mmco::MakeLongCurrent(r.read_ue()?),
                    _ => return Err(SpsError::Unsupported("memory_management_control_operation")),
                };
                sh.mmco.push(mmco);
                if sh.mmco.len() > 66 {
                    return Err(SpsError::Unsupported("mmco length"));
                }
            }
        }
    }
    if pps.entropy_coding && !sh.is_i() {
        sh.cabac_init_idc = Some(r.read_ue()?);
    }
    sh.qp_delta = r.read_se()?;
    if pps.num_slice_groups > 1 && pps.slice_group_map_type == 6 {
        sh.slice_group_change_cycle = Some(r.read_bits(16)?);
    }
    if pps.deblocking_filter_control {
        sh.deblocking_disable_idc = r.read_ue()?;
        if sh.deblocking_disable_idc != 1 {
            sh.deblocking_alpha = r.read_se()?;
            sh.deblocking_beta = r.read_se()?;
        }
    }
    // CABAC alignment before the slice data.
    if pps.entropy_coding && !r.is_aligned_byte() {
        sh.data_offset_bits = (r.bit_pos() + 7) & !7;
    } else {
        sh.data_offset_bits = r.bit_pos();
    }
    Ok(sh)
}

fn read_weight_entries(
    r: &mut BitReader<'_>,
    refs: u32,
    chroma: bool,
) -> Result<Vec<WeightEntry>, SpsError> {
    let mut out = Vec::with_capacity(refs as usize);
    for _ in 0..refs {
        let mut e = WeightEntry::default();
        e.luma_flag = r.read_bit()? == 1;
        if e.luma_flag {
            e.luma_weight = r.read_se()?;
            e.luma_offset = r.read_se()?;
        }
        if chroma {
            e.chroma_flag = r.read_bit()? == 1;
            if e.chroma_flag {
                for c in 0..2 {
                    e.chroma_weight[c] = r.read_se()?;
                    e.chroma_offset[c] = r.read_se()?;
                }
            }
        }
        out.push(e);
    }
    Ok(out)
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

    fn baseline_sps(width_mbs: u32, height_map_units: u32) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put(66, 8) // profile baseline
            .put(0, 8)
            .put(30, 8) // level 3.0
            .ue(0) // sps_id
            .ue(0) // log2_max_frame_num - 4
            .ue(0) // poc_type
            .ue(0) // log2_max_poc_lsb - 4
            .ue(2) // max_num_ref_frames
            .put(0, 1) // gaps
            .ue(width_mbs - 1)
            .ue(height_map_units - 1)
            .put(1, 1) // frame_mbs_only
            .put(1, 1) // direct_8x8
            .put(0, 1) // no cropping
            .put(0, 1); // no vui
        w.finish()
    }

    #[test]
    fn parses_baseline_sps() {
        let rbsp = baseline_sps(40, 23);
        let sps = parse_sps(&rbsp).unwrap();
        assert_eq!(sps.profile_idc, 66);
        assert_eq!(sps.width, 640);
        assert_eq!(sps.height, 368);
        assert_eq!(sps.max_num_ref_frames, 2);
        assert_eq!(sps.log2_max_frame_num, 4);
        assert!(sps.frame_mbs_only);
        assert!(!sps.gaps_in_frame_num_allowed);
    }

    #[test]
    fn parses_cropped_sps() {
        let mut w = BitWriter::new();
        w.put(66, 8)
            .put(0, 8)
            .put(30, 8)
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(1)
            .put(0, 1)
            .ue(120 - 1)
            .ue(68 - 1) // 1920x1088 padded
            .put(1, 1)
            .put(1, 1)
            .put(1, 1) // cropping present
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(4) // crop 4 rows from the bottom
            .put(0, 1);
        let sps = parse_sps(&w.finish()).unwrap();
        assert_eq!((sps.width, sps.height), (1920, 1080));
        assert_eq!(sps.frame_crop, Some((0, 0, 0, 4)));
    }

    #[test]
    fn parses_mbaff_sps_geometry_and_flags() {
        let mut w = BitWriter::new();
        w.put(77, 8)
            .put(0, 8)
            .put(32, 8)
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(4)
            .put(0, 1)
            .ue(80 - 1)
            .ue(23 - 1)
            .put(0, 1) // frame_mbs_only
            .put(1, 1) // mb_adaptive_frame_field
            .put(1, 1) // direct_8x8
            .put(1, 1) // cropping present: 736 -> 720 lines
            .ue(0)
            .ue(0)
            .ue(0)
            .ue(4)
            .put(0, 1); // no VUI

        let sps = parse_sps(&w.finish()).unwrap();

        assert!(!sps.frame_mbs_only);
        assert!(sps.mb_adaptive_frame_field);
        assert_eq!(sps.height_map_units, 23);
        assert_eq!((sps.coded_width, sps.coded_height), (1280, 736));
        assert_eq!((sps.width, sps.height), (1280, 720));
    }

    #[test]
    fn parses_poc_type1_cycle_and_slice_deltas() {
        let mut sps_bits = BitWriter::new();
        sps_bits
            .put(66, 8)
            .put(0, 8)
            .put(30, 8)
            .ue(0)
            .ue(0)
            .ue(1)
            .put(0, 1)
            .se(-1)
            .se(2)
            .ue(2)
            .se(1)
            .se(3)
            .ue(2)
            .put(0, 1)
            .ue(39)
            .ue(22)
            .put(1, 1)
            .put(1, 1)
            .put(0, 1)
            .put(0, 1);
        let sps = parse_sps(&sps_bits.finish()).unwrap();
        assert_eq!(sps.poc_type, 1);
        assert!(!sps.delta_pic_order_always_zero);
        assert_eq!(sps.offset_for_non_ref_pic, -1);
        assert_eq!(sps.offset_for_top_to_bottom_field, 2);
        assert_eq!(sps.offset_for_ref_frame, [1, 3]);

        let pps = Pps {
            bottom_field_pic_order: true,
            ..Default::default()
        };
        let mut slice_bits = BitWriter::new();
        slice_bits
            .ue(0)
            .ue(2)
            .ue(0)
            .put(1, sps.log2_max_frame_num)
            .se(2)
            .se(-1)
            .put(0, 1)
            .se(0);
        let sh = parse_slice_header(&slice_bits.finish(), &sps, &pps, NAL_SLICE, 1).unwrap();

        assert_eq!(sh.nal_ref_idc, 1);
        assert_eq!(sh.delta_poc, [Some(2), Some(-1)]);
    }

    #[test]
    fn rejects_truncated_sps() {
        assert!(matches!(parse_sps(&[0x42, 0x00]), Err(SpsError::Truncated)));
    }

    #[test]
    fn exp_golomb_roundtrip() {
        let mut w = BitWriter::new();
        w.ue(0)
            .ue(1)
            .ue(7)
            .ue(255)
            .se(0)
            .se(1)
            .se(-1)
            .se(42)
            .se(-42);
        let data = w.finish();
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_ue().unwrap(), 0);
        assert_eq!(r.read_ue().unwrap(), 1);
        assert_eq!(r.read_ue().unwrap(), 7);
        assert_eq!(r.read_ue().unwrap(), 255);
        assert_eq!(r.read_se().unwrap(), 0);
        assert_eq!(r.read_se().unwrap(), 1);
        assert_eq!(r.read_se().unwrap(), -1);
        assert_eq!(r.read_se().unwrap(), 42);
        assert_eq!(r.read_se().unwrap(), -42);
    }

    #[test]
    fn deescapes_emulation_prevention() {
        let ebsp = [0x00, 0x00, 0x03, 0x01, 0x00, 0x00, 0x03, 0x00];
        assert_eq!(&*ebsp_to_rbsp(&ebsp), &[0x00, 0x00, 0x01, 0x00, 0x00, 0x00]);
        let clean = [0x11, 0x22];
        assert!(matches!(ebsp_to_rbsp(&clean), Cow::Borrowed(_)));
    }

    #[test]
    fn splits_annex_b() {
        let stream = [
            0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1e, //
            0, 0, 0, 1, 0x68, 0xce, //
            0, 0, 1, 0x65, 0x88, 0x84,
        ];
        let nals = split_annex_b(&stream);
        assert_eq!(nals.len(), 3);
        assert_eq!(nals[0].unit_type, NAL_SPS);
        assert_eq!(nals[1].unit_type, NAL_PPS);
        assert_eq!(nals[2].unit_type, NAL_IDR);
        assert!(nals[2].is_vcl());
        assert_eq!(&*nals[0].rbsp, &[0x42, 0x00, 0x1e]);
    }

    #[test]
    fn parses_pps() {
        let mut w = BitWriter::new();
        w.ue(0)
            .ue(0) // pps_id, sps_id
            .put(0, 1) // entropy (CAVLC)
            .put(0, 1) // bottom_field_pic_order
            .ue(0) // num_slice_groups - 1
            .ue(0)
            .ue(0) // num_ref_idx
            .put(0, 1) // weighted_pred
            .put(0, 2) // weighted_bipred_idc
            .se(0)
            .se(0)
            .se(0)
            .put(1, 1) // deblocking
            .put(0, 1)
            .put(0, 1);
        let pps = parse_pps(&w.finish()).unwrap();
        assert_eq!(pps.pps_id, 0);
        assert_eq!((pps.num_ref_idx_l0, pps.num_ref_idx_l1), (1, 1));
        assert_eq!(pps.pic_init_qp, 26);
        assert!(pps.deblocking_filter_control);
    }

    #[test]
    fn parses_idr_slice_header() {
        let sps = parse_sps(&baseline_sps(40, 23)).unwrap();
        let mut w = BitWriter::new();
        w.ue(0)
            .ue(0)
            .put(0, 1) // entropy
            .put(0, 1) // bottom field
            .ue(0) // slice groups - 1
            .ue(0)
            .ue(0)
            .put(0, 1)
            .put(0, 2)
            .se(0)
            .se(0)
            .se(0)
            .put(1, 1)
            .put(0, 1)
            .put(0, 1);
        let pps = parse_pps(&w.finish()).unwrap();

        let mut s = BitWriter::new();
        s.ue(0) // first_mb
            .ue(7) // slice_type: I (7 % 5 == 2)
            .ue(0) // pps_id
            .put(0, 4) // frame_num
            .ue(0) // idr_pic_id
            .put(0, 4) // pic_order_cnt_lsb
            .put(0, 1) // no_output_of_prior_pics
            .put(0, 1) // long_term_reference_flag
            .se(0) // slice_qp_delta
            .ue(0) // disable_deblocking_filter_idc
            .se(0) // alpha_c0_offset_div2
            .se(0); // beta_offset_div2
        let sh = parse_slice_header(&s.finish(), &sps, &pps, NAL_IDR, 3).unwrap();
        assert!(sh.is_i());
        assert_eq!(sh.idr_pic_id, Some(0));
        assert_eq!(sh.frame_num, 0);
        assert!(!sh.no_output_of_prior_pics);
        assert_eq!(sh.deblocking_disable_idc, 0);
    }
}

#[cfg(test)]
mod real_bitstream_tests {
    use super::*;

    /// A P slice from x264 that exercises the full header: CAVLC/CABAC header,
    /// ref-index override, four list-modification ops, weighted prediction and
    /// an MMCO-1 operation. Regression guard for field ordering.
    fn fixture() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/p_slice_mmco.264"
        ))
        .unwrap()
    }

    #[test]
    fn parses_weighted_p_slice_with_mmco() {
        let data = fixture();
        let nals = split_annex_b(&data);
        assert_eq!(nals.len(), 1);
        let n = &nals[0];
        assert_eq!(n.unit_type, NAL_SLICE);
        assert_eq!(n.ref_idc, 2);

        let mut sps = Sps::default();
        sps.log2_max_frame_num = 4;
        sps.poc_type = 0;
        sps.log2_max_poc_lsb = 6;
        sps.chroma_format_idc = 1;
        sps.frame_mbs_only = true;

        let mut pps = Pps::default();
        pps.entropy_coding = true;
        pps.num_ref_idx_l0 = 3;
        pps.num_ref_idx_l1 = 1;
        pps.weighted_pred = true;
        pps.weighted_bipred_idc = 2;
        pps.deblocking_filter_control = true;

        let sh = parse_slice_header(&n.rbsp, &sps, &pps, n.unit_type, n.ref_idc).unwrap();
        assert!(sh.is_p());
        assert_eq!(sh.frame_num, 3);
        assert_eq!(sh.poc_lsb, Some(58));
        assert_eq!(sh.ref_idx_l0_active, 4, "override flag bumps the count");
        assert_eq!(
            sh.mod_l0,
            vec![
                RefMod::Sub(1),
                RefMod::Sub(15),
                RefMod::Add(0),
                RefMod::Sub(2),
                RefMod::End
            ]
        );
        let w = sh.weights.as_ref().unwrap();
        assert_eq!(w.l0.len(), 4);
        assert!(w.l0[1].luma_flag);
        assert_eq!(w.l0[1].luma_weight, 1);
        assert_eq!(w.l0[1].luma_offset, -1);
        assert_eq!(sh.mmco, vec![Mmco::UnmarkShort(3)]);
        assert_eq!(sh.cabac_init_idc, Some(0));
        assert!(sh.data_offset_bits > 0);
    }
}
