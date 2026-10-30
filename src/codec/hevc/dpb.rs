//! HEVC picture order count and decoded picture buffer.
//!
//! HEVC has no frame_num: POC is derived from `slice_pic_order_cnt_lsb`
//! (§8.3.1) and reference lists are built from the short-term RPS delta-POC
//! sets (§8.3.2) instead of a sliding window.

use super::parser::{ShortTermRps, SliceHeader, Sps};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameStore {
    pub poc: i32,
    pub slot: u8,
    pub buf: usize,
    /// Still referenced by a future picture.
    pub referenced: bool,
}

#[derive(Debug)]
pub struct Dpb {
    frames: Vec<FrameStore>,
    prev_poc_lsb: i32,
    prev_poc_msb: i32,
    started: bool,
    pub max_frame_num: u32,
}

impl Dpb {
    pub fn new() -> Dpb {
        Dpb {
            frames: Vec::new(),
            prev_poc_lsb: 0,
            prev_poc_msb: 0,
            started: false,
            max_frame_num: 0,
        }
    }

    pub fn frames(&self) -> &[FrameStore] {
        &self.frames
    }

    pub fn clear(&mut self) {
        self.frames.clear();
        self.prev_poc_lsb = 0;
        self.prev_poc_msb = 0;
        self.started = false;
    }

    /// §8.3.1 picture order count.
    ///
    /// `is_irap_no_rasl` covers IDR/BLA pictures whose POC is defined as zero
    /// and which reset the POC state.
    pub fn compute_poc(&mut self, sh: &SliceHeader, sps: &Sps, is_irap_no_rasl: bool) -> i32 {
        if is_irap_no_rasl {
            self.prev_poc_lsb = 0;
            self.prev_poc_msb = 0;
            self.started = true;
            return 0;
        }
        let max_lsb = 1i32 << sps.log2_max_pic_order_cnt_lsb;
        let lsb = sh.poc_lsb as i32;
        if !self.started {
            // First picture of the sequence: no previous POC to wrap against.
            self.prev_poc_lsb = lsb;
            self.prev_poc_msb = 0;
            self.started = true;
            return lsb;
        }
        let mut msb = self.prev_poc_msb;
        if lsb < self.prev_poc_lsb && (self.prev_poc_lsb - lsb) >= max_lsb / 2 {
            msb = self.prev_poc_msb + max_lsb;
        } else if lsb > self.prev_poc_lsb && (lsb - self.prev_poc_lsb) > max_lsb / 2 {
            msb = self.prev_poc_msb - max_lsb;
        }
        let poc = msb + lsb;
        self.prev_poc_msb = msb;
        self.prev_poc_lsb = lsb;
        poc
    }

    /// Reference list (L0) in RPS order: `s0` deltas then `s1` deltas, keeping
    /// only entries flagged as used by the current picture.
    pub fn build_ref_list(&self, rps: &ShortTermRps, poc: i32) -> Vec<u8> {
        let mut out = Vec::new();
        for (d, used) in rps.s0.iter().zip(rps.used_s0.iter()) {
            if !*used {
                continue;
            }
            if let Some(f) = self.frames.iter().find(|f| f.poc == poc + d) {
                out.push(f.slot);
            }
        }
        for (d, used) in rps.s1.iter().zip(rps.used_s1.iter()) {
            if !*used {
                continue;
            }
            if let Some(f) = self.frames.iter().find(|f| f.poc == poc + d) {
                out.push(f.slot);
            }
        }
        out
    }

    pub fn push(&mut self, frame: FrameStore) {
        self.frames.retain(|f| f.slot != frame.slot);
        self.frames.push(frame);
    }

    /// Keep at most `max_keep` pictures, dropping the oldest by POC. HEVC has
    /// no sliding window; this only bounds memory, the RPS decides what is
    /// actually referenced.
    pub fn trim(&mut self, max_keep: usize) {
        if self.frames.len() <= max_keep.max(1) {
            return;
        }
        self.frames.sort_by(|a, b| b.poc.cmp(&a.poc));
        self.frames.truncate(max_keep.max(1));
    }

    /// Drop pictures that no reference set points at any more.
    pub fn retain_referenced(&mut self, keep: &[u8]) {
        self.frames.retain(|f| keep.contains(&f.slot));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sps() -> Sps {
        Sps {
            log2_max_pic_order_cnt_lsb: 8,
            ..Default::default()
        }
    }

    fn slice(poc_lsb: u32) -> SliceHeader {
        SliceHeader {
            first_slice: true,
            poc_lsb,
            ..Default::default()
        }
    }

    fn frame(slot: u8, poc: i32) -> FrameStore {
        FrameStore {
            poc,
            slot,
            buf: slot as usize,
            referenced: true,
        }
    }

    #[test]
    fn idr_resets_poc() {
        let mut d = Dpb::new();
        let s = sps();
        d.compute_poc(&slice(5), &s, false);
        assert_eq!(d.compute_poc(&slice(0), &s, true), 0);
        assert_eq!(d.compute_poc(&slice(2), &s, false), 2);
    }

    #[test]
    fn poc_wraps_at_half_range() {
        let mut d = Dpb::new();
        let s = sps(); // max_lsb = 256
        assert_eq!(d.compute_poc(&slice(250), &s, false), 250);
        assert_eq!(d.compute_poc(&slice(4), &s, false), 260);
        assert_eq!(d.compute_poc(&slice(8), &s, false), 264);
    }

    #[test]
    fn rps_maps_deltas_to_slots() {
        let mut d = Dpb::new();
        d.push(frame(0, 0));
        d.push(frame(1, 4));
        d.push(frame(2, 8));
        let rps = ShortTermRps {
            s0: vec![-4, -8],
            used_s0: vec![true, true],
            s1: vec![],
            used_s1: vec![],
        };
        let l0 = d.build_ref_list(&rps, 8);
        assert_eq!(l0, vec![1, 0], "nearest past reference first");
    }

    #[test]
    fn rps_skips_unused_entries() {
        let mut d = Dpb::new();
        d.push(frame(0, 0));
        d.push(frame(1, 4));
        let rps = ShortTermRps {
            s0: vec![-4],
            used_s0: vec![false],
            s1: vec![],
            used_s1: vec![],
        };
        assert!(d.build_ref_list(&rps, 4).is_empty());
    }

    #[test]
    fn rps_keeps_future_references() {
        let mut d = Dpb::new();
        d.push(frame(0, 0));
        d.push(frame(3, 12));
        let rps = ShortTermRps {
            s0: vec![-4],
            used_s0: vec![true],
            s1: vec![8],
            used_s1: vec![true],
        };
        let l0 = d.build_ref_list(&rps, 4);
        assert_eq!(l0, vec![0, 3]);
    }
}
