//! VP8 reference-buffer bookkeeping (`vp8d_alloc_frame` / `vp8d_ref_update`).
//!
//! VP8 keeps three reference buffers - last, golden and alternate - and the
//! bitstream says what happens to each one of them after every picture: refresh
//! it from the picture just decoded, or make it an alias of another buffer. The
//! C decoder models that with reference counted `VP8Frame` objects, so `ref`
//! does not copy pixels, it makes two handles name the same surface. This is the
//! same bookkeeping with slot indices instead of heap objects; the frame buffers
//! themselves belong to [`super::decoder`], which frees them when a slot stops
//! being live.
//!
//! Slots are numbered from zero and the lowest free index is always taken, which
//! is what `mpp_buf_slot_get_unused` does over a fifteen-slot group.

use super::parser::Picture;

/// `mpp_buf_slot_setup(p->frame_slots, 15)`
pub const MAX_SLOTS: usize = 15;

/// The picture just decoded, held by nothing yet.
#[derive(Debug)]
struct Slot {
    /// Reference handles currently naming this surface.
    owners: u32,
    /// `invisible`: `show_frame` was zero.
    invisible: bool,
}

#[derive(Debug)]
pub struct Dpb {
    slots: Vec<Option<Slot>>,
    out: Option<usize>,
    last: Option<usize>,
    golden: Option<usize>,
    alternate: Option<usize>,
}

impl Default for Dpb {
    fn default() -> Self {
        Self::new()
    }
}

impl Dpb {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            out: None,
            last: None,
            golden: None,
            alternate: None,
        }
    }

    /// `vp8d_alloc_frame`: the slot the picture is decoded into.
    pub fn allocate(&mut self, show_frame: bool) -> Result<usize, String> {
        let free = self
            .slots
            .iter()
            .position(|slot| slot.is_none())
            .unwrap_or(self.slots.len());
        if free >= MAX_SLOTS {
            return Err(format!("VP8 holds more than {MAX_SLOTS} reference buffers"));
        }
        let slot = Slot {
            owners: 1,
            invisible: !show_frame,
        };
        if free == self.slots.len() {
            self.slots.push(Some(slot));
        } else {
            self.slots[free] = Some(slot);
        }
        self.out = Some(free);
        Ok(free)
    }

    /// `lst_fb_idx`, `gld_fb_idx` and `alt_fb_idx`, in that order. A buffer that
    /// holds nothing is `None`, which the caller reports as the `0x7f` index the
    /// hardware is given for it.
    pub fn references(&self) -> [Option<usize>; 3] {
        [self.last, self.golden, self.alternate]
    }

    /// `vp8d_ref_update`, in the reference's order: the alternate buffer first,
    /// because `copy_buffer_to_golden == 2` reads the alternate it may have just
    /// replaced. Returns the slots that stopped being live, in release order.
    pub fn update(&mut self, picture: &Picture) -> Vec<usize> {
        let mut released = Vec::new();

        // The alternate buffer is updated first, because `copy_buffer_to_golden
        // == 2` reads the alternate that step may have just replaced.
        if picture.copy_buffer_to_alternate != 0 {
            let source = if picture.copy_buffer_to_alternate == 1 {
                self.last
            } else {
                self.golden
            };
            let previous = self.alternate.take();
            self.release_slot(previous, &mut released);
            self.alternate = source;
            self.hold(self.alternate);
        }
        if picture.copy_buffer_to_golden != 0 {
            let source = if picture.copy_buffer_to_golden == 1 {
                self.last
            } else {
                self.alternate
            };
            let previous = self.golden.take();
            self.release_slot(previous, &mut released);
            self.golden = source;
            self.hold(self.golden);
        }
        if picture.refresh_golden {
            let previous = self.golden.take();
            self.release_slot(previous, &mut released);
            self.golden = self.out;
            self.hold(self.golden);
        }
        if picture.refresh_alternate {
            let previous = self.alternate.take();
            self.release_slot(previous, &mut released);
            self.alternate = self.out;
            self.hold(self.alternate);
        }
        if picture.refresh_last {
            let previous = self.last.take();
            self.release_slot(previous, &mut released);
            self.last = self.out;
            self.hold(self.last);
        }
        // The picture itself is released last; it stays alive through whichever
        // of the three buffers above took it.
        let out = self.out.take();
        self.release_slot(out, &mut released);
        released
    }

    fn hold(&mut self, slot: Option<usize>) {
        if let Some(index) = slot {
            if let Some(entry) = self.slots[index].as_mut() {
                entry.owners += 1;
            }
        }
    }

    fn release_slot(&mut self, index: Option<usize>, released: &mut Vec<usize>) {
        let Some(index) = index else { return };
        let Some(entry) = self.slots[index].as_mut() else {
            return;
        };
        entry.owners = entry.owners.saturating_sub(1);
        if entry.owners == 0 {
            self.slots[index] = None;
            released.push(index);
        }
    }

    /// Whether a frame buffer is still named by some handle.
    pub fn is_live(&self, slot: usize) -> bool {
        self.slots.get(slot).is_some_and(|entry| entry.is_some())
    }

    /// `invisible`: the picture went to a reference buffer, not to the display.
    pub fn is_invisible(&self, slot: usize) -> bool {
        self.slots
            .get(slot)
            .is_some_and(|entry| entry.as_ref().is_some_and(|slot| slot.invisible))
    }

    /// The handles at the end of the stream. A reference the next group still
    /// needs stays alive across an end-of-stream flush, so nothing is displayed
    /// later than it was decoded.
    pub fn flush(&mut self) -> Vec<usize> {
        let mut released = Vec::new();
        for handle in [self.out, self.last, self.golden, self.alternate] {
            self.release_slot(handle, &mut released);
        }
        self.out = None;
        self.last = None;
        self.golden = None;
        self.alternate = None;
        released
    }
}
