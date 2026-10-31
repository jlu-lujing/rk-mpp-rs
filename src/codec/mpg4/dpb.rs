//! MPEG-4 reference bookkeeping: the two-head rotation `mpp_mpg4_parser_update_dpb`
//! performs.
//!
//! `ref0` holds the most recent I/P, `ref1` the one before it. An I or P picture
//! hands `ref0` to the display queue (one picture of latency), rotates the heads
//! and lets go of `ref1`; a B picture displays itself at once and leaves the
//! heads alone. That is what turns decode order into output order with three
//! buffers, and it is also why an I picture reaches the display after the B
//! pictures that follow it: like MPEG-2's picture-keeping, the C parser only
//! releases a reference when the *next* reference arrives, and we mirror that so
//! the output stream lines up with the C decoder byte for byte.
//!
//! The register file wants the pairs `sw131`/`sw148` and `sw134`/`sw135`. For a
//! B picture the *older* head is the forward reference in time, so it goes to
//! `sw131`/`sw148` and the newer head becomes the backward reference.

use crate::codec::mpg4::parser::{VOP_B, VOP_I};

/// Reference state plus the `prev_pic_type` register input.
#[derive(Default)]
pub struct Dpb {
    /// Most recent I/P picture.
    pub ref0: Option<usize>,
    /// The I/P picture before that one.
    pub ref1: Option<usize>,
    ref0_coding: u32,
    /// Whether `ref0` has been handed to the display queue already.
    ref0_shown: bool,
}

impl Dpb {
    pub fn new() -> Self {
        Self::default()
    }

    /// `prev_coding_type` as the C parser reports it: `ref0`'s picture type, or
    /// I when there is no forward reference yet. `sw_prev_pic_type` is one bit
    /// wide, so a B-typed head truncates exactly as the C assignment does.
    pub fn prev_pic_type(&self) -> u32 {
        if self.ref0.is_some() {
            self.ref0_coding & 1
        } else {
            VOP_I
        }
    }

    /// A B picture needs both heads; the C parser rejects the DivX PBBI case
    /// that leaves one of them empty.
    pub fn accepts_picture(&self, coding: u32) -> bool {
        coding != VOP_B || (self.ref0.is_some() && self.ref1.is_some())
    }

    /// Buffers a live reference still owns. Everything else can be handed to the
    /// next picture, because display copies its pixels out immediately and B
    /// pictures are never referenced.
    pub fn is_live(&self, buffer: usize) -> bool {
        self.ref0 == Some(buffer) || self.ref1 == Some(buffer)
    }

    /// The pending picture's reference words `sw131`, `sw148`, `sw134`, `sw135`,
    /// resolved through the caller's `fd_of`. Missing references point at the
    /// current picture, which is what the C HAL does when its fd is negative.
    pub fn reference_words(
        &self,
        coding: u32,
        fd_of: impl Fn(usize) -> u32,
        current_fd: u32,
    ) -> [u32; 4] {
        let (forward, backward) = match coding {
            VOP_B => (self.ref1, self.ref0),
            VOP_I => (None, None),
            _ => (self.ref0, None),
        };
        let pick = |buffer: Option<usize>| match buffer {
            Some(buffer) => fd_of(buffer),
            None => current_fd,
        };
        let (forward, backward) = (pick(forward), pick(backward));
        [forward, forward, backward, backward]
    }

    /// Rotate the heads and report the buffers whose pictures are now displayable.
    pub fn update(&mut self, coding: u32, current: usize) -> Vec<usize> {
        if coding == VOP_B {
            return vec![current];
        }
        let mut shown = Vec::new();
        if let (false, Some(buffer)) = (self.ref0_shown, self.ref0) {
            self.ref0_shown = true;
            shown.push(buffer);
        }
        self.ref1 = self.ref0;
        self.ref0 = Some(current);
        self.ref0_coding = coding;
        self.ref0_shown = false;
        shown
    }

    /// Show the picture the last head is still holding.
    pub fn flush(&mut self) -> Vec<usize> {
        let mut shown = Vec::new();
        if let (false, Some(buffer)) = (self.ref0_shown, self.ref0) {
            shown.push(buffer);
        }
        self.ref0 = None;
        self.ref1 = None;
        self.ref0_shown = false;
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::Dpb;
    use crate::codec::mpg4::parser::{VOP_B, VOP_I, VOP_P};

    /// The heads as buffer indices, for readability in the assertions below.
    fn heads(dpb: &Dpb) -> (Option<usize>, Option<usize>) {
        (dpb.ref0, dpb.ref1)
    }

    /// Decode order I P B B, which the C decoder displays as I B B P.
    #[test]
    fn ipbb_reproduces_the_c_display_order() {
        let mut dpb = Dpb::new();
        let mut shown = Vec::new();
        for (coding, current) in [(VOP_I, 0), (VOP_P, 1), (VOP_B, 2), (VOP_B, 3)] {
            assert!(dpb.accepts_picture(coding));
            shown.extend(dpb.update(coding, current));
        }
        shown.extend(dpb.flush());
        assert_eq!(shown, vec![0, 2, 3, 1]);
    }

    /// An I picture is only released by the next reference picture, so it lands
    /// behind the B pictures that were decoded against it.
    #[test]
    fn an_i_picture_waits_for_the_next_reference() {
        let mut dpb = Dpb::new();
        let mut shown = Vec::new();
        for (coding, current) in [(VOP_I, 0), (VOP_I, 1), (VOP_B, 2)] {
            shown.extend(dpb.update(coding, current));
        }
        shown.extend(dpb.flush());
        assert_eq!(shown, vec![0, 2, 1]);
    }

    #[test]
    fn a_three_buffer_pool_never_runs_dry() {
        // Reserve the lowest index no live reference owns, as `reserve_buffer`
        // does, and check the pattern the C slot pool produces on the same
        // picture sequence. B pictures never become references, so every B after
        // the first reuses the same scratch buffer.
        let mut dpb = Dpb::new();
        let taken: Vec<usize> = [
            (VOP_I, 0usize),
            (VOP_P, 1),
            (VOP_B, 2),
            (VOP_B, 2),
            (VOP_I, 2),
            (VOP_P, 0),
        ]
        .iter()
        .map(|(coding, expected)| {
            let buffer = (0..3).find(|b| !dpb.is_live(*b)).expect("a free buffer");
            assert_eq!(buffer, *expected);
            dpb.update(*coding, buffer);
            buffer
        })
        .collect();
        assert_eq!(taken, vec![0, 1, 2, 2, 2, 0]);
        assert_eq!(heads(&dpb), (Some(0), Some(2)));
    }

    #[test]
    fn prev_pic_type_follows_the_forward_head() {
        let mut dpb = Dpb::new();
        assert_eq!(dpb.prev_pic_type(), VOP_I, "no reference yet");
        dpb.update(VOP_I, 0);
        assert_eq!(dpb.prev_pic_type(), VOP_I);
        dpb.update(VOP_P, 1);
        assert_eq!(dpb.prev_pic_type(), VOP_P, "the head holds the P picture");
        dpb.update(VOP_B, 2);
        assert_eq!(
            dpb.prev_pic_type(),
            VOP_P,
            "a B picture leaves the heads alone"
        );
    }

    #[test]
    fn a_b_picture_without_both_heads_is_refused() {
        let dpb = Dpb::new();
        assert!(dpb.accepts_picture(VOP_I));
        assert!(dpb.accepts_picture(VOP_P));
        assert!(!dpb.accepts_picture(VOP_B));
    }

    /// `sw131`/`sw148` take the forward reference and `sw134`/`sw135` the
    /// backward one; for a B picture the older head is forward.
    #[test]
    fn reference_words_pair_the_two_register_halves() {
        let dpb = Dpb::new();
        let fd_of = |buffer: usize| 10 + buffer as u32;
        assert_eq!(dpb.reference_words(VOP_I, fd_of, 9), [9, 9, 9, 9]);

        let mut dpb = Dpb::new();
        dpb.update(VOP_I, 0);
        assert_eq!(dpb.reference_words(VOP_P, fd_of, 9), [10, 10, 9, 9]);

        dpb.update(VOP_P, 1);
        assert_eq!(dpb.reference_words(VOP_B, fd_of, 9), [10, 10, 11, 11]);
    }
}
