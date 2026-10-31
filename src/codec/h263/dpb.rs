//! H.263 reference bookkeeping: the single-head rotation of
//! `mpp_h263_parser_update_dpb`.
//!
//! There is one forward reference and no reordering. A picture is enqueued for
//! display in the same breath in which it becomes the head, so display order is
//! decode order and the end-of-stream flush of the C parser has nothing left to
//! release. Two buffers are enough, and they alternate: the head holds one while
//! the other is being written, and it only lets go once the new picture is
//! finished, which is what makes the pair ping-pong in the register dumps.

use crate::codec::h263::parser::VOP_P;

/// `hdr_ref0` of `H263dParserImpl`.
#[derive(Default)]
pub struct Dpb {
    ref0: Option<usize>,
    ref0_coding: u32,
}

impl Dpb {
    pub fn new() -> Self {
        Self::default()
    }

    /// `sw136.sw_prev_pic_type`, which is set only when the forward reference of
    /// this picture was itself a P picture. With no head yet the C parser reports
    /// `H263_INVALID_VOP`, which is not a P picture.
    pub fn prev_pic_type(&self) -> bool {
        self.ref0.is_some() && self.ref0_coding == VOP_P
    }

    /// `mpp_h263_parser_setup_refer`: only an inter picture has a reference, and
    /// an intra picture is pointed at its own buffer instead.
    pub fn reference(&self, coding_type: u32) -> Option<usize> {
        if coding_type == VOP_P {
            self.ref0
        } else {
            None
        }
    }

    /// Whether a buffer is the head, and so cannot be handed to the next picture.
    pub fn is_live(&self, buffer: usize) -> bool {
        self.ref0 == Some(buffer)
    }

    /// `mpp_h263_parser_update_dpb`: the picture that was just decoded becomes
    /// the head and the one it displaces stops being a reference.
    pub fn update(&mut self, coding_type: u32, buffer: usize) {
        self.ref0 = Some(buffer);
        self.ref0_coding = coding_type;
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::h263::parser::VOP_I;

    #[test]
    fn an_intra_picture_has_no_reference() {
        let mut dpb = Dpb::new();
        assert_eq!(
            dpb.reference(VOP_P),
            None,
            "no head before the first picture"
        );
        dpb.update(VOP_I, 0);
        assert_eq!(dpb.reference(VOP_I), None, "an I picture never reads one");
        assert_eq!(dpb.reference(VOP_P), Some(0));
    }

    /// Two buffers alternate, because the head only lets go when the next
    /// picture is already decoded into the other one.
    #[test]
    fn the_head_moves_every_picture() {
        let mut dpb = Dpb::new();
        dpb.update(VOP_I, 0);
        assert!(dpb.is_live(0) && !dpb.is_live(1));
        dpb.update(VOP_P, 1);
        assert!(!dpb.is_live(0), "the I picture is no longer a reference");
        assert!(dpb.is_live(1));
        dpb.update(VOP_P, 0);
        assert!(dpb.is_live(0) && !dpb.is_live(1));
    }

    #[test]
    fn previous_picture_type_only_follows_a_p_reference() {
        let mut dpb = Dpb::new();
        assert!(!dpb.prev_pic_type(), "no head yet");
        dpb.update(VOP_I, 0);
        assert!(!dpb.prev_pic_type(), "an intra head");
        dpb.update(VOP_P, 1);
        assert!(dpb.prev_pic_type(), "an inter head");
        dpb.update(VOP_I, 0);
        assert!(!dpb.prev_pic_type(), "back to intra");
        dpb.reset();
        assert!(!dpb.prev_pic_type());
    }
}
