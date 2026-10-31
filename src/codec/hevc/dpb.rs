//! HEVC picture order count and decoded picture buffer.
//!
//! HEVC has no frame_num: POC is derived from `slice_pic_order_cnt_lsb`
//! (§8.3.1) and reference lists are built from the short-term RPS delta-POC
//! sets (§8.3.2) instead of a sliding window.

use super::parser::{LongTermRps, ShortTermRps, SliceHeader, Sps};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameStore {
    pub poc: i32,
    pub slot: u8,
    pub buf: usize,
    /// Still referenced by a future picture.
    pub referenced: bool,
    /// A hardware or reference failure makes dependent pictures invalid too.
    pub error: bool,
    /// Retain this picture even when its POC is no longer among the newest.
    pub long_term: bool,
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
            // POC values restart at zero. Keeping the previous epoch's frames
            // makes RPS lookup ambiguous and can select a stale reference with
            // the same POC instead of the new IDR.
            self.clear();
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

    /// Hardware reference slots in DPB order, keeping only pictures flagged
    /// used by the current RPS. MPP's `fill_picture_parameters()` walks its DPB
    /// when filling `reg67_82_ref_poc[]`; the RPS and L0 order are separate.
    pub fn build_ref_list(
        &self,
        rps: &ShortTermRps,
        long_term: &LongTermRps,
        poc: i32,
        log2_max_poc_lsb: u32,
    ) -> Vec<u8> {
        let mut used_pocs: Vec<i32> = rps
            .s0
            .iter()
            .zip(rps.used_s0.iter())
            .chain(rps.s1.iter().zip(rps.used_s1.iter()))
            .filter_map(|(delta, used)| used.then_some(poc + *delta))
            .collect();
        used_pocs.extend(
            long_term
                .resolve_pocs(poc, log2_max_poc_lsb)
                .into_iter()
                .filter_map(|(poc, used)| used.then_some(poc)),
        );
        self.frames
            .iter()
            .filter(|frame| used_pocs.contains(&frame.poc))
            .take(16)
            .map(|frame| frame.slot)
            .collect()
    }

    pub fn reference_list_has_error(&self, slots: &[u8]) -> bool {
        slots.iter().any(|slot| {
            self.frames
                .iter()
                .find(|frame| frame.slot == *slot)
                .is_some_and(|frame| frame.error)
        })
    }

    /// A used RPS entry is invalid when the picture was never decoded or when
    /// the stored reference itself failed.
    pub fn reference_set_has_error(
        &self,
        rps: &ShortTermRps,
        long_term: &LongTermRps,
        poc: i32,
        log2_max_poc_lsb: u32,
    ) -> bool {
        let has_error = |delta: &i32, used: &bool| {
            *used
                && self
                    .frames
                    .iter()
                    .find(|frame| frame.poc == poc + *delta)
                    .map_or(true, |frame| frame.error)
        };
        rps.s0
            .iter()
            .zip(rps.used_s0.iter())
            .any(|(delta, used)| has_error(delta, used))
            || rps
                .s1
                .iter()
                .zip(rps.used_s1.iter())
                .any(|(delta, used)| has_error(delta, used))
            || long_term
                .resolve_pocs(poc, log2_max_poc_lsb)
                .into_iter()
                .any(|(ref_poc, used)| {
                    used && self
                        .frames
                        .iter()
                        .find(|frame| frame.poc == ref_poc)
                        .map_or(true, |frame| frame.error)
                })
    }

    /// MPP rebuilds reference status for each inter picture. Long-term entries
    /// remain retained even when `used_by_curr_pic_lt_flag` is zero.
    pub fn mark_long_term_refs(
        &mut self,
        long_term: &LongTermRps,
        poc: i32,
        log2_max_poc_lsb: u32,
    ) {
        for frame in &mut self.frames {
            frame.long_term = false;
        }
        for (ref_poc, _) in long_term.resolve_pocs(poc, log2_max_poc_lsb) {
            if let Some(frame) = self.frames.iter_mut().find(|frame| frame.poc == ref_poc) {
                frame.long_term = true;
            }
        }
    }

    /// Clear reference marks without releasing stored pictures.
    ///
    /// A CRA picture has an empty current RPS in an open GOP, but the
    /// following RASL pictures may still reference pictures decoded before the
    /// CRA. C MPP's `h265d_build_rps()` therefore leaves those pictures in its
    /// DPB for the first RASL picture to select.
    pub fn clear_reference_marks(&mut self) {
        for frame in &mut self.frames {
            frame.referenced = false;
            frame.long_term = false;
        }
    }

    pub fn push(&mut self, frame: FrameStore) {
        self.frames.retain(|f| f.slot != frame.slot);
        self.frames.push(frame);
    }

    /// Rebuild DPB retention from the RPS of the picture being decoded.
    ///
    /// MPP calls `h265d_build_rps()` before every non-I picture: it clears
    /// every frame's short/long reference flags, marks the current RPS
    /// entries, then releases frames with no remaining flags. Newer POC is
    /// not a substitute for that marking rule.
    pub fn refresh_references(
        &mut self,
        rps: &ShortTermRps,
        long_term: &LongTermRps,
        poc: i32,
        log2_max_poc_lsb: u32,
    ) {
        let short_term: Vec<i32> = rps
            .s0
            .iter()
            .zip(rps.used_s0.iter())
            .chain(rps.s1.iter().zip(rps.used_s1.iter()))
            .filter_map(|(delta, used)| used.then_some(poc + *delta))
            .collect();
        let long_term: Vec<i32> = long_term
            .resolve_pocs(poc, log2_max_poc_lsb)
            .into_iter()
            .map(|(poc, _)| poc)
            .collect();

        for frame in &mut self.frames {
            frame.referenced = short_term.contains(&frame.poc);
            frame.long_term = long_term.contains(&frame.poc);
        }
        self.frames
            .retain(|frame| frame.referenced || frame.long_term);
    }

    /// Keep at most `max_keep` pictures, dropping the oldest by POC. HEVC has
    /// no sliding window; this only bounds memory, the RPS decides what is
    /// actually referenced.
    pub fn trim(&mut self, max_keep: usize) {
        self.frames.sort_by(|a, b| b.poc.cmp(&a.poc));
        let mut short_term_kept = 0usize;
        self.frames.retain(|frame| {
            if frame.long_term {
                true
            } else {
                let keep = short_term_kept < max_keep.max(1);
                short_term_kept += 1;
                keep
            }
        });
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
            error: false,
            long_term: false,
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
    fn idr_clears_references_from_the_previous_epoch() {
        let mut d = Dpb::new();
        d.push(frame(7, 29));

        assert_eq!(d.compute_poc(&slice(0), &sps(), true), 0);
        assert!(
            d.frames().is_empty(),
            "references with the old POC epoch must not leak into the new IDR"
        );
    }

    #[test]
    fn trim_keeps_long_term_references() {
        let mut d = Dpb::new();
        let mut long_term = frame(0, 0);
        long_term.long_term = true;
        d.push(long_term);
        d.push(frame(1, 4));
        d.push(frame(2, 8));

        d.trim(1);

        assert_eq!(
            d.frames().iter().map(|frame| frame.poc).collect::<Vec<_>>(),
            vec![8, 0]
        );
    }

    #[test]
    fn current_long_term_rps_refreshes_reference_markers() {
        use super::super::parser::{LongTermRefPic, LongTermRps};

        let mut d = Dpb::new();
        let mut stale = frame(0, 0);
        stale.long_term = true;
        d.push(stale);
        d.push(frame(1, 8));
        let long = LongTermRps {
            refs: vec![LongTermRefPic {
                poc_lsb: 8,
                used: false,
                delta_msb_cycle: 0,
            }],
        };

        d.mark_long_term_refs(&long, 8, 8);

        assert!(!d.frames()[0].long_term);
        assert!(d.frames()[1].long_term);
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
    fn hardware_reference_slots_follow_dpb_order() {
        let mut d = Dpb::new();
        d.push(frame(0, 0));
        d.push(frame(1, 2));
        d.push(frame(2, 1));
        let rps = ShortTermRps {
            s0: vec![-3, -5],
            used_s0: vec![true, true],
            s1: vec![],
            used_s1: vec![],
        };
        let refs = d.build_ref_list(&rps, &LongTermRps::default(), 5, 0);
        assert_eq!(
            refs,
            vec![0, 1],
            "hardware ref_poc[] is in DPB order, not RPS/L0 order"
        );
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
        assert!(d
            .build_ref_list(&rps, &LongTermRps::default(), 4, 0)
            .is_empty());
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
        let l0 = d.build_ref_list(&rps, &LongTermRps::default(), 4, 0);
        assert_eq!(l0, vec![0, 3]);
    }

    #[test]
    fn rps_refresh_retains_an_old_picture_named_by_the_current_set() {
        let mut d = Dpb::new();
        d.push(frame(0, 0));
        d.push(frame(1, 4));
        d.push(frame(2, 6));
        d.push(frame(3, 8));
        let rps = ShortTermRps {
            s0: vec![-8],
            used_s0: vec![true],
            s1: vec![],
            used_s1: vec![],
        };

        d.refresh_references(&rps, &LongTermRps::default(), 8, 8);

        assert_eq!(
            d.frames().iter().map(|frame| frame.poc).collect::<Vec<_>>(),
            vec![0],
            "the current RPS marks POC 0 for retention even though newer POCs exist"
        );
    }

    #[test]
    fn cra_clears_marks_without_releasing_open_gop_pictures() {
        let mut d = Dpb::new();
        d.push(frame(0, 0));
        d.push(frame(1, 4));
        d.push(frame(2, 6));
        d.push(frame(3, 10));

        d.clear_reference_marks();

        assert_eq!(
            d.frames().iter().map(|frame| frame.poc).collect::<Vec<_>>(),
            vec![0, 4, 6, 10]
        );
        assert!(d.frames().iter().all(|frame| !frame.referenced));

        let rasl_rps = ShortTermRps {
            s0: vec![-2, -4, -6],
            used_s0: vec![true, true, true],
            s1: vec![2],
            used_s1: vec![true],
        };
        assert_eq!(
            d.build_ref_list(&rasl_rps, &LongTermRps::default(), 8, 8),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn hardware_reference_list_combines_short_and_long_term_refs() {
        use super::super::parser::{LongTermRefPic, LongTermRps};

        let mut d = Dpb::new();
        d.push(frame(0, 0));
        d.push(frame(1, 8));
        d.push(frame(2, 200));
        let short = ShortTermRps {
            s0: vec![-4],
            used_s0: vec![true],
            s1: vec![],
            used_s1: vec![],
        };
        let long = LongTermRps {
            refs: vec![LongTermRefPic {
                poc_lsb: 0,
                used: true,
                delta_msb_cycle: 0,
            }],
        };

        assert_eq!(d.build_ref_list(&short, &long, 204, 8), vec![0, 2]);
    }

    #[test]
    fn reference_errors_reach_pictures_that_depend_on_them() {
        let mut d = Dpb::new();
        let mut errored = frame(1, 4);
        errored.error = true;
        d.push(frame(0, 0));
        d.push(errored);
        let rps = ShortTermRps {
            s0: vec![-4],
            used_s0: vec![true],
            s1: vec![],
            used_s1: vec![],
        };
        let l0 = d.build_ref_list(&rps, &LongTermRps::default(), 8, 0);

        assert!(d.reference_list_has_error(&l0));
    }

    #[test]
    fn long_term_reference_errors_reach_dependent_pictures() {
        use super::super::parser::{LongTermRefPic, LongTermRps};

        let mut d = Dpb::new();
        let mut errored = frame(0, 0);
        errored.error = true;
        d.push(errored);
        let short = ShortTermRps::default();
        let long = LongTermRps {
            refs: vec![LongTermRefPic {
                poc_lsb: 0,
                used: true,
                delta_msb_cycle: 0,
            }],
        };

        assert!(d.reference_set_has_error(&short, &long, 204, 8));
    }

    #[test]
    fn missing_used_references_mark_the_picture_as_invalid() {
        let mut d = Dpb::new();
        d.push(frame(0, 0));
        let rps = ShortTermRps {
            s0: vec![-2],
            used_s0: vec![true],
            s1: vec![2],
            used_s1: vec![true],
        };

        assert!(d.reference_set_has_error(&rps, &LongTermRps::default(), 2, 0));
    }

    #[test]
    fn picture_recovers_when_bad_reference_leaves_the_rps() {
        let mut d = Dpb::new();
        let mut errored = frame(1, 4);
        errored.error = true;
        d.push(frame(0, 0));
        d.push(errored);

        let current_bad = ShortTermRps {
            s0: vec![-4],
            used_s0: vec![true],
            s1: vec![],
            used_s1: vec![],
        };
        assert!(d.reference_set_has_error(&current_bad, &LongTermRps::default(), 8, 0));

        let current_good = ShortTermRps {
            s0: vec![-8],
            used_s0: vec![true],
            s1: vec![],
            used_s1: vec![],
        };
        assert!(!d.reference_set_has_error(&current_good, &LongTermRps::default(), 8, 0));
    }
}
