//! MPEG-2 reference picture bookkeeping: the three-head rotation of
//! `m2vd_alloc_frame` / `m2v_update_ref_frame` / `m2vd_parser_flush`.
//!
//! MPEG-2 never needs a general DPB. At any moment one picture is being
//! decoded and at most two earlier pictures stay alive as references, so the
//! official decoder just rotates three heads over a buffer pool:
//!
//! * `ref0` holds the most recent reference picture, which is the *backward*
//!   reference of the next B picture;
//! * `ref1` holds the one before it, the *forward* reference of a B picture;
//! * `cur` is the picture being decoded.
//!
//! A picture becomes displayable when it stops being the newest reference
//! (`ref0` at the next reference picture) or straight away for a B picture,
//! which is never referenced. That is what puts the stream back in display
//! order without a reorder buffer.

use super::parser::{PIC_B, PIC_I};

/// 1 = top field, 2 = bottom field, 3 = frame (`M2VDPicStruct`).
pub const STRUCT_TOP_FIELD: u32 = 1;
pub const STRUCT_BOTTOM_FIELD: u32 = 2;
pub const STRUCT_FRAME: u32 = 3;

/// One of the three heads. `buffer` is an index into the frame pool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Head {
    pub buffer: Option<usize>,
    /// Coding type of the picture in this head, `NO_PICTURE` before the first.
    pub coding: u32,
    /// `M2V_OUT_FLAG`: the picture has not reached the display queue yet.
    pub marked: bool,
    /// The picture, or one of its references, was damaged.
    pub error: bool,
}

/// No picture has been decoded into this head yet.
pub const NO_PICTURE: u32 = 0;

#[derive(Debug, Default)]
pub struct Dpb {
    pub cur: Head,
    pub ref0: Head,
    pub ref1: Head,
    /// `ref_frame_cnt`: the number of reference pictures decoded so far. A B
    /// picture needs two of them.
    pub reference_count: u32,
}

/// The field that opens a frame picture: a frame picture itself, or the first
/// field of a field-coded pair. `m2vd_alloc_frame` takes a fresh buffer here.
pub fn starts_new_picture(picture_structure: u32, top_field_first: bool) -> bool {
    match picture_structure {
        STRUCT_FRAME => true,
        STRUCT_TOP_FIELD => top_field_first,
        STRUCT_BOTTOM_FIELD => !top_field_first,
        _ => true,
    }
}

/// The field that finishes a frame picture. `m2v_update_ref_frame` rotates the
/// heads and hands the picture to the display queue here, one field later than
/// [`starts_new_picture`], which is when both halves are in the buffer.
pub fn completes_picture(picture_structure: u32, top_field_first: bool) -> bool {
    match picture_structure {
        STRUCT_FRAME => true,
        STRUCT_TOP_FIELD => !top_field_first,
        STRUCT_BOTTOM_FIELD => top_field_first,
        _ => true,
    }
}

impl Dpb {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_live(&self, buffer: usize) -> bool {
        [self.cur, self.ref0, self.ref1]
            .iter()
            .any(|head| head.buffer == Some(buffer))
    }

    /// The picture being decoded, when it already has a buffer.
    pub fn current(&self) -> Option<usize> {
        self.cur.buffer
    }

    /// `m2vd_alloc_frame`, called when [`starts_new_picture`] opens a frame
    /// picture. The caller picks a buffer that is not [`Dpb::is_live`]; the
    /// closing field joins the buffer its partner already owns and is reached
    /// through the address offset instead.
    ///
    /// When the previous picture never got its partner field, that half-written
    /// buffer is orphaned by the new allocation. It comes back as `Some` so the
    /// caller can report it as damaged instead of dropping it on the floor,
    /// which is what the C parser does with `miss match field order`.
    pub fn allocate(&mut self, coding: u32, buffer: usize) -> Option<usize> {
        let orphan = if self.cur.coding == PIC_B {
            self.cur.buffer.take()
        } else {
            None
        };
        self.cur = Head {
            buffer: Some(buffer),
            coding,
            marked: true,
            error: false,
        };
        orphan
    }

    /// Reference buffers in the order the register file wants them: sw131,
    /// sw148, sw134, sw135. A B picture reads forward from `ref1` and backward
    /// from `ref0`; every other picture reads forward from `ref0` and points
    /// the backward pair at itself, which is what the C HAL does.
    pub fn references(&self, picture_structure: u32, top_field_first: bool) -> [Option<usize>; 4] {
        let forward = self.ref1.buffer;
        let backward = self.ref0.buffer;
        let current = self.cur.buffer;
        // Missing references fall back the way `m2vd_convert_to_dxva` does, so
        // an intra picture drives all four slots with its own buffer.
        let backward = backward.or(current);
        let forward = forward.or(backward);
        if self.cur.coding == PIC_B {
            [forward, forward, backward, backward]
        } else if starts_new_picture(picture_structure, top_field_first) {
            [backward, backward, current, current]
        } else {
            // The closing field takes the partner field out of the current
            // buffer, so one of the forward slots has to point at it.
            match picture_structure {
                STRUCT_TOP_FIELD => [backward, current, current, current],
                _ => [current, backward, current, current],
            }
        }
    }

    /// True when the error status of the references must poison this picture.
    /// Intra pictures never inherit an error, matching `m2vd_convert_to_dxva`.
    pub fn inherits_reference_errors(&self) -> bool {
        self.cur.coding != PIC_I
    }

    /// A B picture cannot be decoded before two reference pictures exist.
    pub fn accepts_picture(&self, coding: u32) -> bool {
        coding != PIC_B || self.reference_count >= 2
    }

    /// Mark the current picture as damaged, either through its references or
    /// through a failed hardware job.
    pub fn mark_current_error(&mut self) {
        self.cur.error = true;
    }

    /// True when the current picture has no valid reference. Mirrors the
    /// `error_info` the C parser writes onto the output frame.
    pub fn current_error(&self) -> bool {
        self.cur.error
    }

    /// Error state of the buffers the current picture references.
    pub fn references_in_error(&self) -> bool {
        [self.ref0, self.ref1].iter().any(|head| head.error)
    }

    /// `m2v_update_ref_frame` for one completed picture, returning the buffers
    /// that the display queue takes in order. `release` returns a buffer to the
    /// pool; a displayed buffer leaves the decoder, everything else stays.
    pub fn update(&mut self, dominant: bool, release: &mut dyn FnMut(usize)) -> Vec<usize> {
        let mut output = Vec::new();
        if !dominant {
            return output;
        }
        if self.cur.coding == PIC_B {
            if let Some(buffer) = self.cur.buffer.take() {
                output.push(buffer);
            }
            self.cur = Head::default();
            return output;
        }
        if self.cur.coding != NO_PICTURE {
            self.reference_count += 1;
            if self.ref0.marked {
                if let Some(buffer) = self.ref0.buffer {
                    output.push(buffer);
                }
                self.ref0.marked = false;
            }
            // Displaying a reference picture does not retire it: MPEG-2 B
            // pictures still read the second-newest reference, so only the
            // picture that falls out of `ref1` gives its buffer back.
            if let Some(buffer) = self.ref1.buffer.take() {
                release(buffer);
            }
            self.ref1 = self.ref0;
            self.ref0 = self.cur;
            self.cur = Head::default();
        }
        output
    }

    /// `m2v_update_ref_frame` with `force = 1`, then the flush: the trailing
    /// reference picture is the last thing the display queue takes.
    pub fn flush(&mut self, release: &mut dyn FnMut(usize)) -> Vec<usize> {
        let mut output = self.update(true, release);
        if self.cur.coding != PIC_B && self.cur.coding != NO_PICTURE {
            // A trailing field picture that never found its partner still owns
            // the buffer that the rotation just moved into `cur`.
            if let Some(buffer) = self.cur.buffer.take() {
                release(buffer);
            }
        }
        if self.ref0.marked {
            if let Some(buffer) = self.ref0.buffer {
                output.push(buffer);
            }
            self.ref0.marked = false;
        }
        if let Some(buffer) = self.ref0.buffer.take() {
            release(buffer);
        }
        if let Some(buffer) = self.ref1.buffer.take() {
            release(buffer);
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::m2v::parser::{PIC_B, PIC_P};

    /// `(coding type, picture structure, top_field_first)` for the first GOP of
    /// `tests/data/m2v_320x240_12.m2v`, which `ffprobe` names IPBPB.
    const IPBPB: [(u32, u32, bool); 5] = [
        (PIC_I, STRUCT_FRAME, false),
        (PIC_P, STRUCT_FRAME, false),
        (PIC_B, STRUCT_FRAME, false),
        (PIC_P, STRUCT_FRAME, false),
        (PIC_B, STRUCT_FRAME, false),
    ];

    /// A pool that only grows when every allocated buffer is in use, which is
    /// how the frame buffer set in `Decoder` behaves.
    #[derive(Default)]
    struct Pool {
        free: Vec<usize>,
        allocated: usize,
    }

    impl Pool {
        /// Hand out a free buffer that no reference picture is reading, which is
        /// the rule `Decoder::reserve_buffer` applies.
        fn take(&mut self, dpb: &Dpb) -> usize {
            if let Some(position) = self.free.iter().rposition(|buffer| !dpb.is_live(*buffer)) {
                return self.free.remove(position);
            }
            let buffer = self.allocated;
            self.allocated += 1;
            buffer
        }

        fn give(&mut self, buffer: usize) {
            self.free.push(buffer);
        }
    }

    /// Walk the fixture through the rotation and report what the display queue
    /// took, in order, together with the reference list of every picture.
    fn drive(pictures: &[(u32, u32, bool)]) -> (Vec<u32>, usize, Vec<[Option<usize>; 4]>) {
        let mut dpb = Dpb::new();
        let mut pool = Pool::default();
        let mut coding_of = vec![NO_PICTURE; 16];
        let mut displayed = Vec::new();
        let mut references = Vec::new();
        for &(coding, structure, top_field_first) in pictures {
            let buffer = pool.take(&dpb);
            coding_of[buffer] = coding;
            dpb.allocate(coding, buffer);
            references.push(dpb.references(structure, top_field_first));
            let mut release = |buffer: usize| pool.give(buffer);
            for buffer in dpb.update(completes_picture(structure, top_field_first), &mut release) {
                displayed.push(coding_of[buffer]);
                pool.give(buffer);
            }
        }
        let mut release = |buffer: usize| pool.give(buffer);
        for buffer in dpb.flush(&mut release) {
            displayed.push(coding_of[buffer]);
        }
        (displayed, pool.allocated, references)
    }

    #[test]
    fn ipbpb_displays_in_temporal_order_over_three_buffers() {
        let (displayed, buffers, references) = drive(&IPBPB);
        // Decode order is IPBPB; display order reorders the B pictures, which is
        // what the reference rotation is there to do.
        assert_eq!(
            displayed,
            vec![PIC_I, PIC_B, PIC_P, PIC_B, PIC_P],
            "display order"
        );
        // One picture being decoded plus two references is the whole working set.
        assert!(buffers <= 3, "MPEG-2 needed {buffers} frame buffers");

        // An intra picture has no reference yet, so every slot points at itself.
        assert_eq!(references[0][0], references[0][1]);
        assert_eq!(references[0][2], references[0][3]);
        assert_eq!(references[0][0], references[0][2]);
        // The first P reads the I picture in the forward direction only.
        assert_eq!(references[1][0], references[0][0], "P forward reference");
        assert_eq!(
            references[1][2], references[1][3],
            "unused pair is the current picture"
        );
        // The first B reads the I picture forward and the P picture backward.
        assert_eq!(references[2][0], references[0][0], "B forward reference");
        assert_eq!(references[2][1], references[2][0], "forward pair repeats");
        assert_eq!(references[2][3], references[2][2], "backward pair repeats");
        assert_ne!(
            references[2][0], references[2][2],
            "B reads two different pictures"
        );
    }

    #[test]
    fn a_field_picture_pair_shares_one_buffer() {
        // A top-field-first pair, then the frame picture that follows it.
        let pictures = [
            (PIC_I, STRUCT_TOP_FIELD, true),
            (PIC_I, STRUCT_BOTTOM_FIELD, true),
            (PIC_P, STRUCT_FRAME, false),
        ];
        let mut dpb = Dpb::new();
        let mut pool = Pool::default();
        let mut fields = Vec::new();
        for &(coding, structure, top_field_first) in &pictures {
            let buffer = if starts_new_picture(structure, top_field_first) {
                let buffer = pool.take(&dpb);
                dpb.allocate(coding, buffer);
                buffer
            } else {
                dpb.current().expect("the partner field owns a buffer")
            };
            fields.push(buffer);
            let mut release = |buffer: usize| pool.give(buffer);
            dpb.update(completes_picture(structure, top_field_first), &mut release);
        }
        assert_eq!(fields[0], fields[1], "both fields write one frame buffer");
        assert_ne!(
            fields[0], fields[2],
            "the next picture takes another buffer"
        );
    }

    #[test]
    fn a_b_picture_needs_two_references() {
        let dpb = Dpb::new();
        assert!(!dpb.accepts_picture(PIC_B));
        assert!(dpb.accepts_picture(PIC_I));
        assert!(dpb.accepts_picture(PIC_P));
    }

    #[test]
    fn reference_errors_reach_inter_pictures_only() {
        let mut dpb = Dpb::new();
        let mut pool = Pool::default();
        dpb.allocate(PIC_I, pool.take(&dpb));
        assert!(
            !dpb.inherits_reference_errors(),
            "intra pictures stand alone"
        );
        let mut release = |buffer: usize| pool.give(buffer);
        dpb.update(true, &mut release);

        dpb.ref0.error = true;
        dpb.allocate(PIC_P, pool.take(&dpb));
        assert!(dpb.inherits_reference_errors());
        assert!(dpb.references_in_error());
        dpb.mark_current_error();
        assert!(dpb.current_error());
    }
}
