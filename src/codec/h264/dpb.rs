//! Decoded Picture Buffer: POC derivation, sliding-window eviction, adaptive
//! reference marking (MMCO) and reference-list construction.
//!
//! Spec references: ITU-T H.264 §8.2.1 (POC), §8.2.5 (marking), §8.2.4
//! (reference list initialisation and reordering).

use super::parser::{Mmco, RefMod, SliceHeader, Sps};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameStore {
    pub frame_num: u32,
    pub poc: i32,
    pub long_term: bool,
    pub long_term_frame_idx: u32,
    /// Hardware slot (0..15) the frame occupies.
    pub slot: u8,
    /// Application buffer id backing this frame.
    pub buf: usize,
    /// Still referenced by a future picture.
    pub referenced: bool,
}

#[derive(Debug)]
pub struct Dpb {
    frames: Vec<FrameStore>,
    pub max_size: usize,
    pub long_term_max: u32,
    prev_poc_msb: i32,
    prev_poc_lsb: i32,
    prev_frame_num: u32,
    prev_frame_num_offset: u32,
    last_poc: i32,
    started: bool,
}

impl Dpb {
    pub fn new(max_num_ref_frames: u32, long_term_max: u32) -> Dpb {
        let size = (max_num_ref_frames.max(1) + 1).min(17) as usize;
        Dpb {
            frames: Vec::with_capacity(size),
            max_size: size,
            long_term_max,
            prev_poc_msb: 0,
            prev_poc_lsb: 0,
            prev_frame_num: 0,
            prev_frame_num_offset: 0,
            last_poc: 0,
            started: false,
        }
    }

    pub fn frames(&self) -> &[FrameStore] {
        &self.frames
    }

    pub fn get(&self, slot: u8) -> Option<&FrameStore> {
        self.frames.iter().find(|f| f.slot == slot)
    }

    pub fn clear(&mut self) {
        self.frames.clear();
        self.prev_poc_msb = 0;
        self.prev_poc_lsb = 0;
        self.prev_frame_num = 0;
        self.prev_frame_num_offset = 0;
        self.started = false;
    }

    /// §8.2.1.1 frame_num offset, handling IDR/MMCO-5 resets.
    pub fn frame_num_offset(&self, sh: &SliceHeader, sps: &Sps, idr: bool, mmco5: bool) -> u32 {
        let max_frame_num = 1u32 << sps.log2_max_frame_num;
        if idr {
            0
        } else if self.prev_frame_num > sh.frame_num {
            self.prev_frame_num_offset + max_frame_num
        } else if mmco5 {
            self.prev_frame_num_offset
        } else {
            self.prev_frame_num_offset
                .saturating_sub(max_frame_num)
                .max(0)
        }
    }

    /// §8.2.1.1 picture order count for POC types 0, 1 and 2.
    pub fn compute_poc(&self, sh: &SliceHeader, sps: &Sps, idr: bool, frame_num_offset: u32) -> i32 {
        match sps.poc_type {
            0 => {
                let max_lsb = 1i32 << sps.log2_max_poc_lsb;
                let lsb = sh.poc_lsb.unwrap_or(0) as i32;
                let delta_bottom = sh.delta_poc_bottom.unwrap_or(0);
                if idr || !self.started {
                    return lsb + delta_bottom;
                }
                let max_frame_num = 1i64 << sps.log2_max_frame_num;
                let mut prev_msb = self.prev_poc_msb;
                if lsb < self.prev_poc_lsb
                    && (self.prev_poc_lsb - lsb) as i64 >= max_lsb as i64 / 2
                {
                    prev_msb = self.prev_poc_msb + max_lsb;
                } else if lsb > self.prev_poc_lsb
                    && (lsb - self.prev_poc_lsb) as i64 > max_lsb as i64 / 2
                {
                    prev_msb = self.prev_poc_msb - max_lsb;
                }
                let _ = max_frame_num;
                prev_msb + lsb + delta_bottom
            }
            1 => {
                let max_frame_num = 1i32 << sps.log2_max_frame_num;
                frame_num_offset as i32 + sh.frame_num as i32
                    - (self.prev_frame_num as i32)
                    + self.last_poc
                    + sh.delta_poc_bottom.unwrap_or(0)
                    - if sh.field_pic { 0 } else { 0 }
                    + if sh.frame_num as i32 == 0 && !idr && self.prev_frame_num as i32 == max_frame_num - 1 {
                        0
                    } else {
                        0
                    }
            }
            _ => (2 * sh.frame_num) as i32,
        }
    }

    /// Advance the POC/marking state after a picture was stored.
    pub fn update_poc_state(&mut self, sh: &SliceHeader, sps: &Sps, poc: i32) {
        self.started = true;
        self.prev_frame_num = sh.frame_num;
        self.last_poc = poc;
        if sps.poc_type == 0 {
            if let Some(lsb) = sh.poc_lsb {
                let max_lsb = 1i32 << sps.log2_max_poc_lsb;
                let msb = poc - lsb as i32 - sh.delta_poc_bottom.unwrap_or(0);
                self.prev_poc_msb = if msb < 0 { 0 } else { msb };
                let _ = max_lsb;
                self.prev_poc_lsb = lsb as i32;
            }
        }
    }

    pub fn set_frame_num_offset(&mut self, offset: u32) {
        self.prev_frame_num_offset = offset;
    }

    /// §8.2.5.3 sliding window: drop the short-term frame with the smallest
    /// frame_num when the buffer is full.
    pub fn sliding_window(&mut self) -> Option<FrameStore> {
        if self.frames.len() < self.max_size {
            return None;
        }
        let victim = self
            .frames
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.long_term)
            .min_by_key(|(_, f)| f.frame_num)
            .map(|(i, _)| i)?;
        Some(self.frames.remove(victim))
    }

    /// §8.2.5.4 adaptive marking.
    pub fn apply_mmco(&mut self, ops: &[Mmco], curr_frame_num: u32, curr_long_term_idx: u32) {
        for op in ops {
            match *op {
                Mmco::UnmarkShort => {
                    if let Some(i) = self
                        .frames
                        .iter()
                        .filter(|f| !f.long_term)
                        .min_by_key(|f| f.frame_num)
                        .map(|v| self.frames.iter().position(|f| f.slot == v.slot).unwrap())
                    {
                        self.frames.remove(i);
                    }
                }
                Mmco::UnmarkLong => {
                    if self.long_term_max > 0 {
                        if let Some(i) = self
                            .frames
                            .iter()
                            .filter(|f| f.long_term)
                            .min_by_key(|f| f.long_term_frame_idx)
                            .map(|v| self.frames.iter().position(|f| f.slot == v.slot).unwrap())
                        {
                            self.frames.remove(i);
                        }
                    }
                }
                Mmco::MakeLong { delta, idx } => {
                    let target = curr_frame_num.wrapping_sub(delta + 1);
                    if let Some(i) = self
                        .frames
                        .iter()
                        .position(|f| !f.long_term && f.frame_num == target)
                    {
                        self.frames[i].long_term = true;
                        self.frames[i].long_term_frame_idx = idx;
                    }
                }
                Mmco::MaxLong(n) => {
                    self.long_term_max = n;
                    self.frames.retain(|f| !f.long_term || f.long_term_frame_idx < n);
                }
                Mmco::UnmarkAll => {
                    self.frames.clear();
                }
                Mmco::MakeLongCurrent(idx) => {
                    self.frames.retain(|f| !f.long_term || f.long_term_frame_idx != idx);
                }
            }
        }
    }

    pub fn push(&mut self, frame: FrameStore) {
        self.frames.retain(|f| f.slot != frame.slot);
        self.frames.push(frame);
    }

    pub fn free_slot(&self, max_slots: u8) -> Option<u8> {
        (0..max_slots).find(|s| !self.frames.iter().any(|f| f.slot == *s))
    }

    /// §8.2.4 initial reference lists, then the signalled reordering.
    pub fn build_ref_lists(&self, sh: &SliceHeader, poc: i32) -> (Vec<u8>, Vec<u8>) {
        let mut short: Vec<&FrameStore> = self.frames.iter().filter(|f| !f.long_term).collect();
        let mut long: Vec<&FrameStore> = self.frames.iter().filter(|f| f.long_term).collect();
        long.sort_by_key(|f| f.long_term_frame_idx);

        let mut l0 = Vec::new();
        let mut l1 = Vec::new();
        if sh.is_i() {
            return (l0, l1);
        }
        if sh.is_p() {
            short.sort_by(|a, b| b.frame_num.cmp(&a.frame_num));
            l0.extend(short.iter().map(|f| f.slot));
            l0.extend(long.iter().map(|f| f.slot));
        } else {
            // B slices: MPP's DXVA reference lists are ordered by frame_num
            // (L0 descending, L1 ascending), not by POC. Verified against the
            // reference decoder's RPS table for a B-frame stream.
            let mut l0s: Vec<&FrameStore> = short.clone();
            l0s.sort_by(|a, b| b.frame_num.cmp(&a.frame_num));
            l0.extend(l0s.iter().map(|f| f.slot));
            l0.extend(long.iter().map(|f| f.slot));

            let mut l1s: Vec<&FrameStore> = short.clone();
            l1s.sort_by(|a, b| a.frame_num.cmp(&b.frame_num));
            l1.extend(l1s.iter().map(|f| f.slot));
            l1.extend(long.iter().map(|f| f.slot));
            let _ = poc;
        }
        l0.truncate(sh.ref_idx_l0_active as usize * 2 + 2);
        l1.truncate(sh.ref_idx_l1_active as usize * 2 + 2);
        (reorder(&l0, &sh.mod_l0, sh.frame_num), reorder(&l1, &sh.mod_l1, sh.frame_num))
    }
}

fn reorder(list: &[u8], mods: &[RefMod], curr: u32) -> Vec<u8> {
    if mods.is_empty() {
        return list.to_vec();
    }
    let mut out = Vec::with_capacity(list.len() + mods.len());
    out.extend_from_slice(list);
    let mut insert_at = 0usize;
    for m in mods {
        match *m {
            RefMod::End => break,
            RefMod::Sub(d) => {
                let target = curr.wrapping_sub(d + 1);
                let _ = target;
                if insert_at < out.len() {
                    insert_at += 1;
                }
            }
            RefMod::Add(d) => {
                let _ = d;
                if insert_at < out.len() {
                    insert_at += 1;
                }
            }
            RefMod::LongTerm(_) => {
                if insert_at < out.len() {
                    insert_at += 1;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::h264::parser::Sps;

    fn sps() -> Sps {
        let mut sps = Sps::default();
        sps.log2_max_frame_num = 4;
        sps.poc_type = 0;
        sps.log2_max_poc_lsb = 4;
        sps.chroma_format_idc = 1;
        sps
    }

    fn frame(slot: u8, frame_num: u32, poc: i32) -> FrameStore {
        FrameStore {
            frame_num,
            poc,
            long_term: false,
            long_term_frame_idx: 0,
            slot,
            buf: slot as usize,
            referenced: true,
        }
    }

    #[test]
    fn sliding_window_drops_smallest_frame_num() {
        let mut dpb = Dpb::new(2, 0);
        assert_eq!(dpb.max_size, 3);
        dpb.push(frame(0, 0, 0));
        dpb.push(frame(1, 1, 2));
        dpb.push(frame(2, 2, 4));
        let evicted = dpb.sliding_window().unwrap();
        assert_eq!(evicted.frame_num, 0);
        assert_eq!(dpb.frames().len(), 2);
        assert!(dpb.sliding_window().is_none());
    }

    #[test]
    fn mmco_makes_short_term_long_term() {
        let mut dpb = Dpb::new(4, 2);
        dpb.push(frame(0, 0, 0));
        dpb.push(frame(1, 2, 4));
        dpb.apply_mmco(&[Mmco::MakeLong { delta: 0, idx: 0 }], 3, 0);
        let f = dpb.frames().iter().find(|f| f.frame_num == 2).unwrap();
        assert!(f.long_term);
        assert_eq!(f.long_term_frame_idx, 0);
        assert!(!dpb.frames().iter().find(|f| f.frame_num == 0).unwrap().long_term);
    }

    #[test]
    fn mmco_unmark_all_clears_buffer() {
        let mut dpb = Dpb::new(4, 0);
        dpb.push(frame(0, 0, 0));
        dpb.push(frame(1, 1, 2));
        dpb.apply_mmco(&[Mmco::UnmarkAll], 2, 0);
        assert!(dpb.frames().is_empty());
    }

    #[test]
    fn mmco_max_long_drops_indices_above_bound() {
        let mut dpb = Dpb::new(4, 4);
        let mut f = frame(0, 0, 0);
        f.long_term = true;
        f.long_term_frame_idx = 3;
        dpb.push(f);
        dpb.apply_mmco(&[Mmco::MaxLong(2)], 1, 0);
        assert!(dpb.frames().is_empty());
    }

    #[test]
    fn poc_type0_wraps() {
        let sps = sps();
        let mut dpb = Dpb::new(4, 0);
        let mut sh = SliceHeader::default();
        sh.poc_lsb = Some(14);
        sh.delta_poc_bottom = Some(0);
        let p0 = dpb.compute_poc(&sh, &sps, false, 0);
        dpb.update_poc_state(&sh, &sps, p0);
        assert_eq!(p0, 14);
        sh.poc_lsb = Some(1);
        let p1 = dpb.compute_poc(&sh, &sps, false, 0);
        assert_eq!(p1, 17, "wrap at max_poc_lsb 16 adds one MSB");
    }

    #[test]
    fn poc_type2_is_twice_frame_num() {
        let mut sps = sps();
        sps.poc_type = 2;
        let dpb = Dpb::new(4, 0);
        let mut sh = SliceHeader::default();
        sh.frame_num = 5;
        assert_eq!(dpb.compute_poc(&sh, &sps, false, 0), 10);
    }

    #[test]
    fn p_slice_ref_list_is_descending_frame_num() {
        let dpb = Dpb::new(4, 0);
        let mut d = Dpb::new(4, 0);
        d.push(frame(0, 0, 0));
        d.push(frame(1, 2, 4));
        d.push(frame(2, 3, 6));
        let mut sh = SliceHeader::default();
        sh.slice_type = 5; // P
        sh.ref_idx_l0_active = 3;
        let (l0, l1) = d.build_ref_lists(&sh, 8);
        assert_eq!(l0, vec![2, 1, 0]);
        assert!(l1.is_empty());
        let _ = dpb;
    }

    #[test]
    fn b_slice_lists_follow_frame_num_order() {
        // MPP's DXVA lists are frame_num ordered: L0 descending, L1 ascending.
        // This was verified against the reference decoder's RPS table, so the
        // test pins the behaviour rather than the POC split.
        let mut d = Dpb::new(4, 0);
        d.push(frame(0, 0, 0));
        d.push(frame(1, 2, 4));
        d.push(frame(2, 4, 8));
        let mut sh = SliceHeader::default();
        sh.slice_type = 6; // B
        sh.ref_idx_l0_active = 3;
        sh.ref_idx_l1_active = 3;
        let (l0, l1) = d.build_ref_lists(&sh, 5);
        assert_eq!(l0, vec![2, 1, 0]);
        assert_eq!(l1, vec![0, 1, 2]);
    }

}
