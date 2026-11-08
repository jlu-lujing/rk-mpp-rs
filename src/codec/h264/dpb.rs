//! Decoded Picture Buffer: POC derivation, sliding-window eviction, adaptive
//! reference marking (MMCO) and reference-list construction.
//!
//! Spec references: ITU-T H.264 §8.2.1 (POC), §8.2.5 (marking), §8.2.4
//! (reference list initialisation and reordering).

use super::parser::{Mmco, RefMod, SliceHeader, Sps};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameStore {
    pub frame_num: u32,
    /// Frame POC used to order pictures for display.
    pub poc: i32,
    /// Top-field and bottom-field POCs written to the rkvdec registers.
    pub top_poc: i32,
    pub bottom_poc: i32,
    pub long_term: bool,
    pub long_term_frame_idx: u32,
    /// Hardware slot (0..15) the frame occupies.
    pub slot: u8,
    /// Application buffer id backing this frame.
    pub buf: usize,
    /// Still referenced by a future picture.
    pub referenced: bool,
    /// The picture was decoded with colmv compression, so its colmv buffer is
    /// valid for later motion-vector prediction.
    pub colmv_valid: bool,
    /// The store was formed from field-coded pictures.
    pub field_coded: bool,
    /// Which fields have been decoded into this frame store's buffer.
    pub top_present: bool,
    pub bottom_present: bool,
    /// Which fields remain available as references.
    pub top_used: bool,
    pub bottom_used: bool,
    /// MPP-style frame error mask. A nonzero value makes this an invalid
    /// reference for every later picture that selects it.
    pub error: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceFlags {
    pub field_coded: bool,
    pub top_used: bool,
    pub bottom_used: bool,
    pub colmv_valid: bool,
}

impl FrameStore {
    pub fn reference_flags(&self) -> ReferenceFlags {
        ReferenceFlags {
            field_coded: self.field_coded,
            top_used: self.top_used,
            bottom_used: self.bottom_used,
            colmv_valid: self.colmv_valid,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureOrderCount {
    pub top: i32,
    pub bottom: i32,
    /// Display-order POC (`min(top, bottom)` for a frame picture).
    pub frame: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefListEntry {
    pub slot: u8,
    pub bottom: bool,
}

impl PictureOrderCount {
    /// rkvdec stores zero for the absent field of a field-coded picture.
    pub fn hardware_fields(&self, sh: &SliceHeader) -> (i32, i32) {
        if !sh.field_pic {
            (self.top, self.bottom)
        } else if sh.bottom_field {
            (0, self.bottom)
        } else {
            (self.top, 0)
        }
    }
}

#[derive(Debug)]
pub struct Dpb {
    frames: Vec<FrameStore>,
    /// Maximum number of reference pictures, excluding the picture currently
    /// being decoded. MPP applies the same limit in
    /// `sliding_window_memory_management`.
    pub max_size: usize,
    pub long_term_max: u32,
    prev_poc_msb: i32,
    prev_poc_lsb: i32,
    prev_frame_num: u32,
    prev_frame_num_offset: u32,
    last_ref_frame_num: Option<u32>,
    started: bool,
}

impl Dpb {
    pub fn new(max_num_ref_frames: u32, long_term_max: u32) -> Dpb {
        let size = max_num_ref_frames.clamp(1, 16) as usize;
        Dpb {
            frames: Vec::with_capacity(size),
            max_size: size,
            long_term_max,
            prev_poc_msb: 0,
            prev_poc_lsb: 0,
            prev_frame_num: 0,
            prev_frame_num_offset: 0,
            last_ref_frame_num: None,
            started: false,
        }
    }

    pub fn frames(&self) -> &[FrameStore] {
        &self.frames
    }

    pub fn get(&self, slot: u8) -> Option<&FrameStore> {
        self.frames.iter().find(|f| f.slot == slot)
    }

    /// Reference table order consumed by rkvdec: short-term pictures first,
    /// then long-term pictures by `long_term_frame_idx` (§8.2.4 / MPP
    /// `prepare_init_dpb_info`). RPS entries index this compact table.
    pub fn reference_table(&self) -> Vec<&FrameStore> {
        let mut table: Vec<&FrameStore> = self.frames.iter().filter(|f| !f.long_term).collect();
        let mut long: Vec<&FrameStore> = self.frames.iter().filter(|f| f.long_term).collect();
        long.sort_by_key(|f| f.long_term_frame_idx);
        table.extend(long);
        table.truncate(16);
        table
    }

    pub fn clear(&mut self) {
        self.frames.clear();
        self.prev_poc_msb = 0;
        self.prev_poc_lsb = 0;
        self.prev_frame_num = 0;
        self.prev_frame_num_offset = 0;
        self.last_ref_frame_num = None;
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
        }
    }

    /// §8.2.1.1 picture order count for POC types 0, 1 and 2.
    pub fn compute_poc(
        &self,
        sh: &SliceHeader,
        sps: &Sps,
        idr: bool,
        frame_num_offset: u32,
    ) -> i32 {
        self.compute_picture_order(sh, sps, idr, frame_num_offset)
            .frame
    }

    /// Compute both field counts required by the hardware. A progressive
    /// frame has equal top/bottom values; an MBAFF frame usually differs.
    pub fn compute_picture_order(
        &self,
        sh: &SliceHeader,
        sps: &Sps,
        idr: bool,
        frame_num_offset: u32,
    ) -> PictureOrderCount {
        match sps.poc_type {
            0 => {
                let max_lsb = 1i32 << sps.log2_max_poc_lsb;
                let lsb = sh.poc_lsb.unwrap_or(0) as i32;
                let delta_bottom = sh.delta_poc_bottom.unwrap_or(0);
                let top = if idr || !self.started {
                    lsb
                } else {
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
                    prev_msb + lsb
                };
                let bottom = top + delta_bottom;
                let frame = if sh.field_pic {
                    if sh.bottom_field {
                        bottom
                    } else {
                        top
                    }
                } else {
                    top.min(bottom)
                };
                PictureOrderCount { top, bottom, frame }
            }
            1 => {
                let mut abs_frame_num = if sps.offset_for_ref_frame.is_empty() {
                    0
                } else {
                    frame_num_offset + sh.frame_num
                };
                if sh.nal_ref_idc == 0 && abs_frame_num > 0 {
                    abs_frame_num -= 1;
                }
                let expected_cycle = sps.offset_for_ref_frame.iter().sum::<i32>();
                let mut expected = if abs_frame_num == 0 {
                    0
                } else {
                    let cycle_count = (abs_frame_num - 1) / sps.offset_for_ref_frame.len() as u32;
                    let frame_in_cycle =
                        (abs_frame_num - 1) % sps.offset_for_ref_frame.len() as u32;
                    cycle_count as i32 * expected_cycle
                        + sps.offset_for_ref_frame[..=frame_in_cycle as usize]
                            .iter()
                            .sum::<i32>()
                };
                if sh.nal_ref_idc == 0 {
                    expected += sps.offset_for_non_ref_pic;
                }
                let delta0 = sh.delta_poc[0].unwrap_or(0);
                let delta1 = sh.delta_poc[1].unwrap_or(0);
                let top = expected + delta0;
                let bottom = top + sps.offset_for_top_to_bottom_field + delta1;
                let frame = if sh.field_pic {
                    if sh.bottom_field {
                        expected + sps.offset_for_top_to_bottom_field + delta0
                    } else {
                        top
                    }
                } else {
                    top.min(bottom)
                };
                PictureOrderCount { top, bottom, frame }
            }
            _ => {
                let top = if idr {
                    0
                } else {
                    let non_ref = i32::from(sh.nal_ref_idc == 0);
                    2 * (frame_num_offset + sh.frame_num) as i32 - non_ref
                };
                PictureOrderCount {
                    top,
                    bottom: top,
                    frame: top,
                }
            }
        }
    }

    /// Advance the POC/marking state after a picture was stored.
    pub fn update_poc_state(&mut self, sh: &SliceHeader, sps: &Sps, poc: i32) {
        self.update_picture_order_state(
            sh,
            sps,
            PictureOrderCount {
                top: poc,
                bottom: poc,
                frame: poc,
            },
        );
    }

    pub fn update_picture_order_state(
        &mut self,
        sh: &SliceHeader,
        sps: &Sps,
        order: PictureOrderCount,
    ) {
        self.started = true;
        self.prev_frame_num = sh.frame_num;
        if sps.poc_type == 0 && sh.nal_ref_idc != 0 {
            if let Some(lsb) = sh.poc_lsb {
                let msb = order.top - lsb as i32;
                self.prev_poc_msb = if msb < 0 { 0 } else { msb };
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

    /// Make room before inserting the current reference picture. Sliding
    /// window is used only in sliding-window marking mode; adaptive marking
    /// inserts first and then removes exactly the pictures named by MMCO.
    pub fn prepare_reference_insertion(
        &mut self,
        adaptive_ref_pic_buffering: bool,
    ) -> Option<FrameStore> {
        if adaptive_ref_pic_buffering {
            None
        } else {
            self.sliding_window()
        }
    }

    /// §8.2.5.4 adaptive marking.
    pub fn apply_mmco(
        &mut self,
        ops: &[Mmco],
        curr_frame_num: u32,
        _curr_long_term_idx: u32,
        max_frame_num: u32,
    ) {
        for op in ops {
            match *op {
                Mmco::UnmarkShort(delta) => {
                    let target = curr_frame_num.wrapping_sub(delta + 1) % max_frame_num;
                    if let Some(i) = self
                        .frames
                        .iter()
                        .position(|f| !f.long_term && f.frame_num % max_frame_num == target)
                    {
                        self.frames.remove(i);
                    }
                }
                Mmco::UnmarkLong(pic_num) => {
                    if let Some(i) = self
                        .frames
                        .iter()
                        .position(|f| f.long_term && f.long_term_frame_idx == pic_num)
                    {
                        self.frames.remove(i);
                    }
                }
                Mmco::MakeLong { delta, idx } => {
                    self.frames
                        .retain(|f| !f.long_term || f.long_term_frame_idx != idx);
                    let target = curr_frame_num.wrapping_sub(delta + 1) % max_frame_num;
                    if let Some(i) = self
                        .frames
                        .iter()
                        .position(|f| !f.long_term && f.frame_num % max_frame_num == target)
                    {
                        self.frames[i].long_term = true;
                        self.frames[i].long_term_frame_idx = idx;
                    }
                }
                Mmco::MaxLong(n) => {
                    self.long_term_max = n;
                    self.frames
                        .retain(|f| !f.long_term || f.long_term_frame_idx < n);
                }
                Mmco::UnmarkAll => {
                    self.frames.clear();
                }
                Mmco::MakeLongCurrent(idx) => {
                    self.frames
                        .retain(|f| !f.long_term || f.long_term_frame_idx != idx);
                    if let Some(i) = self.frames.iter().rposition(|f| {
                        !f.long_term
                            && f.frame_num % max_frame_num == curr_frame_num % max_frame_num
                    }) {
                        self.frames[i].long_term = true;
                        self.frames[i].long_term_frame_idx = idx;
                    }
                }
            }
        }
    }

    pub fn push(&mut self, frame: FrameStore) {
        self.frames.retain(|f| f.slot != frame.slot);
        if frame.referenced {
            self.last_ref_frame_num = Some(frame.frame_num);
        }
        self.frames.push(frame);
    }

    /// MPP `check_dpb_discontinuous`: an allowed missing reference must be
    /// detected from the frame_num gap even when the remaining DPB can still
    /// materialise a syntactically valid fallback list.
    pub fn frame_num_gap(&self, sh: &SliceHeader, sps: &Sps) -> bool {
        if sh.is_i() || sps.gaps_in_frame_num_allowed {
            return false;
        }
        let Some(last) = self.last_ref_frame_num else {
            return false;
        };
        let max_frame_num = 1u32 << sps.log2_max_frame_num;
        sh.frame_num != last.wrapping_add(1) % max_frame_num
    }

    pub fn free_slot(&self, max_slots: u8) -> Option<u8> {
        (0..max_slots).find(|s| !self.frames.iter().any(|f| f.slot == *s))
    }

    /// H.264 list 0 as MPP materialises it for P slices. The hardware RPS also
    /// consumes this table for B pictures, in addition to B lists 0 and 1.
    pub fn build_p_ref_list(&self, sh: &SliceHeader, log2_max_frame_num: u32) -> Vec<u8> {
        let max_frame_num = 1u32 << log2_max_frame_num;
        let mut short: Vec<&FrameStore> = self.frames.iter().filter(|f| !f.long_term).collect();
        short.sort_by_key(|f| {
            std::cmp::Reverse(signed_pic_num(
                f.frame_num % max_frame_num,
                sh.frame_num,
                max_frame_num,
            ))
        });
        let mut long: Vec<&FrameStore> = self.frames.iter().filter(|f| f.long_term).collect();
        long.sort_by_key(|f| f.long_term_frame_idx);

        let mut list = Vec::with_capacity(short.len() + long.len());
        list.extend(short.iter().map(|f| f.slot));
        list.extend(long.iter().map(|f| f.slot));
        list.truncate(32);
        list
    }

    pub fn build_p_ref_list_entries(
        &self,
        sh: &SliceHeader,
        log2_max_frame_num: u32,
    ) -> Vec<RefListEntry> {
        let max_frame_num = 1u32 << log2_max_frame_num;
        let mut short: Vec<&FrameStore> = self.frames.iter().filter(|f| !f.long_term).collect();
        short.sort_by_key(|f| {
            std::cmp::Reverse(signed_pic_num(
                f.frame_num % max_frame_num,
                sh.frame_num,
                max_frame_num,
            ))
        });
        let mut long: Vec<&FrameStore> = self.frames.iter().filter(|f| f.long_term).collect();
        long.sort_by_key(|f| f.long_term_frame_idx);

        field_entries_for_frames(short.into_iter().chain(long), sh)
    }

    /// §8.2.4 initial reference lists, then the signalled reordering.
    ///
    /// `log2_max_frame_num` comes from the active SPS and is needed to wrap
    /// the frame numbers used by `modification_of_pic_nums_idc`.
    pub fn build_ref_lists(
        &self,
        sh: &SliceHeader,
        poc: i32,
        log2_max_frame_num: u32,
    ) -> (Vec<u8>, Vec<u8>) {
        let (mut l0, mut l1) = self.build_initial_ref_lists(sh, poc);

        // Keep every DPB picture available to the modification syntax. The
        // hardware consumes at most 32 RPS entries, regardless of the active
        // count advertised by the slice header.
        l0.truncate(32);
        l1.truncate(32);
        let max_frame_num = 1u32 << log2_max_frame_num;
        reorder(
            &mut l0,
            &sh.mod_l0,
            sh.frame_num,
            max_frame_num,
            &self.frames,
        );
        reorder(
            &mut l1,
            &sh.mod_l1,
            sh.frame_num,
            max_frame_num,
            &self.frames,
        );
        (l0, l1)
    }

    /// Check the same reference-list conditions as MPP
    /// `check_refer_picture_lists`: explicitly reordered lists validate every
    /// named picture, while unmodified lists validate only the active refs.
    pub fn reference_list_error(
        &self,
        sh: &SliceHeader,
        poc: i32,
        log2_max_frame_num: u32,
    ) -> bool {
        if sh.is_i() {
            return false;
        }
        let (mut l0, mut l1) = self.build_initial_ref_lists(sh, poc);
        l0.truncate(32);
        l1.truncate(32);
        let max_frame_num = 1u32 << log2_max_frame_num;
        let mut error = false;

        if sh.mod_l0.is_empty() {
            error |= self.active_list_error(&l0, sh.ref_idx_l0_active);
        } else {
            error |= reorder(
                &mut l0,
                &sh.mod_l0,
                sh.frame_num,
                max_frame_num,
                &self.frames,
            );
        }

        if sh.is_b() {
            if sh.mod_l1.is_empty() {
                error |= self.active_list_error(&l1, sh.ref_idx_l1_active);
            } else {
                error |= reorder(
                    &mut l1,
                    &sh.mod_l1,
                    sh.frame_num,
                    max_frame_num,
                    &self.frames,
                );
            }
        }
        error
    }

    fn active_list_error(&self, list: &[u8], active: u32) -> bool {
        list.iter()
            .take(active as usize)
            .filter_map(|slot| self.get(*slot))
            .any(|frame| frame.error != 0)
    }

    /// MPP also materialises both B reference lists for P and I pictures.
    /// Its hardware RPS always reads `RefPicList[1]` and `RefPicList[2]` from
    /// those auxiliary B lists, even though only list 0 is used for prediction.
    pub fn build_auxiliary_b_ref_lists(
        &self,
        sh: &SliceHeader,
        poc: i32,
        log2_max_frame_num: u32,
    ) -> (Vec<u8>, Vec<u8>) {
        let (mut l0, mut l1) = self.build_initial_b_ref_lists(poc);
        l0.truncate(32);
        l1.truncate(32);
        if sh.is_b() {
            let max_frame_num = 1u32 << log2_max_frame_num;
            reorder(
                &mut l0,
                &sh.mod_l0,
                sh.frame_num,
                max_frame_num,
                &self.frames,
            );
            reorder(
                &mut l1,
                &sh.mod_l1,
                sh.frame_num,
                max_frame_num,
                &self.frames,
            );
        }
        (l0, l1)
    }

    pub fn build_auxiliary_b_ref_list_entries(
        &self,
        sh: &SliceHeader,
        poc: i32,
        log2_max_frame_num: u32,
    ) -> (Vec<RefListEntry>, Vec<RefListEntry>) {
        if sh.field_pic && sh.mod_l0.is_empty() && sh.mod_l1.is_empty() {
            let (frames0, frames1) = self.build_initial_b_frame_lists(poc);
            let l0 = field_entries_for_frames(frames0, sh);
            let mut l1 = field_entries_for_frames(frames1, sh);
            if l0.len() > 1 && l0 == l1 {
                l1.swap(0, 1);
            }
            return (l0, l1);
        }
        let (l0, l1) = self.build_auxiliary_b_ref_lists(sh, poc, log2_max_frame_num);
        (
            self.field_entries_for_slots(&l0, sh),
            self.field_entries_for_slots(&l1, sh),
        )
    }

    fn field_entries_for_slots(&self, slots: &[u8], sh: &SliceHeader) -> Vec<RefListEntry> {
        let mut entries = Vec::with_capacity(slots.len() * 2);
        entries.extend(field_entries_for_frames(
            slots.iter().filter_map(|&slot| self.get(slot)),
            sh,
        ));
        entries
    }

    fn build_initial_b_ref_lists(&self, poc: i32) -> (Vec<u8>, Vec<u8>) {
        // §8.2.4.2.3: L0 takes past/non-future pictures in descending POC
        // order, then future pictures in ascending POC order. L1 is the
        // mirror image. This is also the order MPP writes into its
        // DXVA RefPicList[1] and RefPicList[2] tables.
        let (frames0, frames1) = self.build_initial_b_frame_lists(poc);
        let l0: Vec<u8> = frames0.into_iter().map(|f| f.slot).collect();
        let mut l1: Vec<u8> = frames1.into_iter().map(|f| f.slot).collect();
        if l0.len() > 1 && l0 == l1 {
            l1.swap(0, 1);
        }
        (l0, l1)
    }

    fn build_initial_ref_lists(&self, sh: &SliceHeader, poc: i32) -> (Vec<u8>, Vec<u8>) {
        if sh.is_i() {
            return (Vec::new(), Vec::new());
        }
        if sh.is_b() {
            return self.build_initial_b_ref_lists(poc);
        }

        let mut short: Vec<&FrameStore> = self.frames.iter().filter(|f| !f.long_term).collect();
        let mut long: Vec<&FrameStore> = self.frames.iter().filter(|f| f.long_term).collect();
        short.sort_by(|a, b| b.frame_num.cmp(&a.frame_num));
        long.sort_by_key(|f| f.long_term_frame_idx);
        let l0 = short.into_iter().chain(long).map(|f| f.slot).collect();
        (l0, Vec::new())
    }

    fn build_initial_b_frame_lists(&self, poc: i32) -> (Vec<&FrameStore>, Vec<&FrameStore>) {
        let short: Vec<&FrameStore> = self.frames.iter().filter(|f| !f.long_term).collect();
        let mut long: Vec<&FrameStore> = self.frames.iter().filter(|f| f.long_term).collect();
        long.sort_by_key(|f| f.long_term_frame_idx);
        let mut greater: Vec<&FrameStore> = short.iter().copied().filter(|f| f.poc > poc).collect();
        greater.sort_by_key(|f| f.poc);
        let mut less: Vec<&FrameStore> = short.iter().copied().filter(|f| f.poc <= poc).collect();
        less.sort_by(|a, b| b.poc.cmp(&a.poc));

        let l0: Vec<&FrameStore> = less
            .iter()
            .chain(greater.iter())
            .chain(long.iter())
            .copied()
            .collect();
        let l1: Vec<&FrameStore> = greater
            .iter()
            .chain(less.iter())
            .chain(long.iter())
            .copied()
            .collect();
        (l0, l1)
    }
}

fn field_entries_for_frames<'a>(
    frames: impl IntoIterator<Item = &'a FrameStore>,
    sh: &SliceHeader,
) -> Vec<RefListEntry> {
    let frames: Vec<&FrameStore> = frames.into_iter().collect();
    if !sh.field_pic {
        return frames
            .into_iter()
            .map(|frame| RefListEntry {
                slot: frame.slot,
                bottom: false,
            })
            .take(32)
            .collect();
    }

    let mut entries = Vec::with_capacity(frames.len() * 2);
    let mut top = 0usize;
    let mut bottom = 0usize;
    while top < frames.len() || bottom < frames.len() {
        for parity_bottom in [sh.bottom_field, !sh.bottom_field] {
            let cursor = if parity_bottom { &mut bottom } else { &mut top };
            while *cursor < frames.len() {
                let frame = frames[*cursor];
                *cursor += 1;
                let present = if parity_bottom {
                    frame.bottom_present
                } else {
                    frame.top_present
                };
                if present {
                    entries.push(RefListEntry {
                        slot: frame.slot,
                        bottom: parity_bottom,
                    });
                    break;
                }
            }
        }
    }
    entries.truncate(32);
    entries
}

/// §8.2.4.2.3: apply `modification_of_pic_nums_idc` operations in order.
///
/// `list` holds DPB slots in initial-list order and is modified in place.
/// `curr_frame_num` is the slice's `frame_num`, `max_frame_num` is
/// `1 << log2_max_frame_num`. A modification naming a picture that is not in
/// the DPB leaves the list unchanged.
pub fn reorder(
    list: &mut Vec<u8>,
    mods: &[RefMod],
    curr_frame_num: u32,
    max_frame_num: u32,
    frames: &[FrameStore],
) -> bool {
    let mut insert_at = 0usize;
    let mut pic_num_pred = curr_frame_num;
    let mut error = false;
    for m in mods {
        let target = match *m {
            RefMod::End => break,
            RefMod::Sub(d) => {
                pic_num_pred = pic_num_pred.wrapping_sub(d + 1) % max_frame_num;
                let pic_num = signed_pic_num(pic_num_pred, curr_frame_num, max_frame_num);
                find_slot(frames, |f| {
                    !f.long_term
                        && signed_pic_num(
                            f.frame_num % max_frame_num,
                            curr_frame_num,
                            max_frame_num,
                        ) == pic_num
                })
            }
            RefMod::Add(d) => {
                pic_num_pred = (pic_num_pred + d + 1) % max_frame_num;
                let pic_num = signed_pic_num(pic_num_pred, curr_frame_num, max_frame_num);
                find_slot(frames, |f| {
                    !f.long_term
                        && signed_pic_num(
                            f.frame_num % max_frame_num,
                            curr_frame_num,
                            max_frame_num,
                        ) == pic_num
                })
            }
            RefMod::LongTerm(pic_num) => find_slot(frames, |f| {
                f.long_term
                    && if f.field_coded {
                        (f.top_used && 2 * f.long_term_frame_idx == pic_num)
                            || (f.bottom_used && 2 * f.long_term_frame_idx + 1 == pic_num)
                    } else {
                        f.long_term_frame_idx == pic_num
                    }
            }),
        };
        if let Some(slot) = target {
            if frames.iter().any(|f| f.slot == slot && f.error != 0) {
                error = true;
            }
            if let Some(pos) = list.iter().position(|&s| s == slot) {
                let v = list.remove(pos);
                list.insert(insert_at.min(list.len()), v);
                insert_at += 1;
            }
        } else {
            error = true;
        }
    }
    error
}

fn signed_pic_num(pic_num: u32, curr_frame_num: u32, max_frame_num: u32) -> i32 {
    if pic_num > curr_frame_num {
        pic_num as i32 - max_frame_num as i32
    } else {
        pic_num as i32
    }
}

fn find_slot(frames: &[FrameStore], pred: impl Fn(&FrameStore) -> bool) -> Option<u8> {
    frames.iter().find(|f| pred(*f)).map(|f| f.slot)
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
            top_poc: poc,
            bottom_poc: poc,
            long_term: false,
            long_term_frame_idx: 0,
            slot,
            buf: slot as usize,
            referenced: true,
            colmv_valid: true,
            field_coded: false,
            top_present: true,
            bottom_present: true,
            top_used: true,
            bottom_used: true,
            error: 0,
        }
    }

    #[test]
    fn sliding_window_drops_smallest_frame_num() {
        let mut dpb = Dpb::new(2, 0);
        assert_eq!(dpb.max_size, 2);
        dpb.push(frame(0, 0, 0));
        assert!(dpb.sliding_window().is_none());
        dpb.push(frame(1, 1, 2));
        let evicted = dpb.sliding_window().unwrap();
        assert_eq!(evicted.frame_num, 0);
        assert_eq!(dpb.frames().len(), 1);
        dpb.push(frame(2, 2, 4));
        assert_eq!(dpb.frames().len(), 2);
    }

    #[test]
    fn reference_capacity_is_limited_by_sps_max_num_ref_frames() {
        let mut dpb = Dpb::new(2, 0);
        dpb.push(frame(0, 0, 0));
        dpb.push(frame(1, 1, 2));

        let evicted = dpb
            .sliding_window()
            .expect("the oldest reference must be evicted before adding another one");

        assert_eq!(evicted.frame_num, 0);
        assert_eq!(dpb.frames().len(), 1);
    }

    #[test]
    fn adaptive_marking_does_not_preemptively_slide_the_window() {
        let mut dpb = Dpb::new(2, 0);
        dpb.push(frame(0, 0, 0));
        dpb.push(frame(1, 1, 2));

        assert!(dpb.prepare_reference_insertion(true).is_none());
        assert_eq!(dpb.frames().len(), 2);

        let evicted = dpb.prepare_reference_insertion(false).unwrap();
        assert_eq!(evicted.frame_num, 0);
        assert_eq!(dpb.frames().len(), 1);
    }

    #[test]
    fn mmco_makes_short_term_long_term() {
        let mut dpb = Dpb::new(4, 2);
        dpb.push(frame(0, 0, 0));
        dpb.push(frame(1, 2, 4));
        dpb.apply_mmco(&[Mmco::MakeLong { delta: 0, idx: 0 }], 3, 0, 16);
        let f = dpb.frames().iter().find(|f| f.frame_num == 2).unwrap();
        assert!(f.long_term);
        assert_eq!(f.long_term_frame_idx, 0);
        assert!(
            !dpb.frames()
                .iter()
                .find(|f| f.frame_num == 0)
                .unwrap()
                .long_term
        );
    }

    #[test]
    fn mmco_make_long_replaces_existing_long_term_index() {
        let mut dpb = Dpb::new(4, 2);
        let mut old_long = frame(0, 9, 18);
        old_long.long_term = true;
        old_long.long_term_frame_idx = 0;
        dpb.push(old_long);
        dpb.push(frame(1, 30, 60));

        dpb.apply_mmco(&[Mmco::MakeLong { delta: 0, idx: 0 }], 31, 0, 32);

        assert!(dpb.frames().iter().all(|f| f.frame_num != 9));
        let replacement = dpb.frames().iter().find(|f| f.frame_num == 30).unwrap();
        assert!(replacement.long_term);
        assert_eq!(replacement.long_term_frame_idx, 0);
        assert_eq!(
            dpb.frames()
                .iter()
                .filter(|f| f.long_term && f.long_term_frame_idx == 0)
                .count(),
            1
        );
    }

    #[test]
    fn mmco_make_current_long_marks_and_replaces_current_index() {
        let mut dpb = Dpb::new(4, 3);
        let mut old_long = frame(0, 1, 2);
        old_long.long_term = true;
        old_long.long_term_frame_idx = 2;
        dpb.push(old_long);
        dpb.push(frame(1, 3, 6));

        dpb.apply_mmco(&[Mmco::MakeLongCurrent(2)], 3, 2, 16);

        assert!(dpb.frames().iter().all(|f| f.frame_num != 1));
        let current = dpb.frames().iter().find(|f| f.frame_num == 3).unwrap();
        assert!(current.long_term);
        assert_eq!(current.long_term_frame_idx, 2);
    }

    #[test]
    fn mmco_unmark_all_clears_buffer() {
        let mut dpb = Dpb::new(4, 0);
        dpb.push(frame(0, 0, 0));
        dpb.push(frame(1, 1, 2));
        dpb.apply_mmco(&[Mmco::UnmarkAll], 2, 0, 16);
        assert!(dpb.frames().is_empty());
    }

    #[test]
    fn mmco_unmark_short_uses_difference_of_pic_nums() {
        let mut dpb = Dpb::new(4, 0);
        dpb.push(frame(0, 0, 0));
        dpb.push(frame(1, 1, 6));
        dpb.push(frame(2, 2, 2));

        // CurrPicNum is 4: MMCO-1(delta 3) targets frame_num 0 and
        // MMCO-1(delta 1) targets frame_num 2. Frame_num 1 must survive.
        dpb.apply_mmco(&[Mmco::UnmarkShort(3), Mmco::UnmarkShort(1)], 4, 0, 8);

        assert_eq!(dpb.frames()[0].frame_num, 1);
        assert_eq!(dpb.frames().len(), 1);
    }

    #[test]
    fn mmco_make_long_handles_frame_num_wrap() {
        let mut dpb = Dpb::new(4, 2);
        dpb.push(frame(0, 15, 30));
        dpb.push(frame(1, 0, 32));

        // CurrPicNum is 1. delta 1 selects PicNum -1, which wraps to fn 15.
        dpb.apply_mmco(&[Mmco::MakeLong { delta: 1, idx: 0 }], 1, 0, 16);

        assert!(dpb.frames()[0].long_term);
        assert!(!dpb.frames()[1].long_term);
    }

    #[test]
    fn mmco_max_long_drops_indices_above_bound() {
        let mut dpb = Dpb::new(4, 4);
        let mut f = frame(0, 0, 0);
        f.long_term = true;
        f.long_term_frame_idx = 3;
        dpb.push(f);
        dpb.apply_mmco(&[Mmco::MaxLong(2)], 1, 0, 16);
        assert!(dpb.frames().is_empty());
    }

    #[test]
    fn poc_type0_wraps() {
        let sps = sps();
        let mut dpb = Dpb::new(4, 0);
        let mut sh = SliceHeader::default();
        sh.poc_lsb = Some(14);
        sh.delta_poc_bottom = Some(0);
        sh.nal_ref_idc = 1;
        let p0 = dpb.compute_poc(&sh, &sps, false, 0);
        dpb.update_poc_state(&sh, &sps, p0);
        assert_eq!(p0, 14);
        sh.poc_lsb = Some(1);
        let p1 = dpb.compute_poc(&sh, &sps, false, 0);
        assert_eq!(p1, 17, "wrap at max_poc_lsb 16 adds one MSB");
    }

    #[test]
    fn poc_type0_frame_keeps_top_and_bottom_field_counts() {
        let sps = sps();
        let dpb = Dpb::new(4, 0);
        let mut sh = SliceHeader::default();
        sh.poc_lsb = Some(1);
        sh.delta_poc_bottom = Some(-1);

        let order = dpb.compute_picture_order(&sh, &sps, true, 0);

        assert_eq!(order.top, 1);
        assert_eq!(order.bottom, 0);
        assert_eq!(order.frame, 0);
    }

    #[test]
    fn top_field_picture_has_no_bottom_field_poc() {
        let order = PictureOrderCount {
            top: 6,
            bottom: 7,
            frame: 6,
        };
        let sh = SliceHeader {
            field_pic: true,
            bottom_field: false,
            ..Default::default()
        };

        assert_eq!(order.hardware_fields(&sh), (6, 0));
    }

    #[test]
    fn bottom_field_picture_has_no_top_field_poc() {
        let order = PictureOrderCount {
            top: 6,
            bottom: 7,
            frame: 7,
        };
        let sh = SliceHeader {
            field_pic: true,
            bottom_field: true,
            ..Default::default()
        };

        assert_eq!(order.hardware_fields(&sh), (0, 7));
    }

    #[test]
    fn poc_type2_is_twice_frame_num() {
        let mut sps = sps();
        sps.poc_type = 2;
        let dpb = Dpb::new(4, 0);
        let mut sh = SliceHeader::default();
        sh.frame_num = 5;
        sh.nal_ref_idc = 1;
        assert_eq!(dpb.compute_poc(&sh, &sps, false, 0), 10);
    }

    #[test]
    fn poc_type1_applies_cycle_offsets_and_field_deltas() {
        let mut sps = sps();
        sps.poc_type = 1;
        sps.offset_for_ref_frame = vec![1, 3];
        sps.offset_for_non_ref_pic = -1;
        sps.offset_for_top_to_bottom_field = 2;
        let dpb = Dpb::new(4, 0);
        let sh = SliceHeader {
            frame_num: 3,
            nal_ref_idc: 1,
            delta_poc: [Some(2), Some(-1)],
            ..Default::default()
        };

        let order = dpb.compute_picture_order(&sh, &sps, false, 0);

        assert_eq!(order.top, 7);
        assert_eq!(order.bottom, 8);
        assert_eq!(order.frame, 7);
    }

    #[test]
    fn poc_type1_non_reference_uses_previous_cycle_slot() {
        let mut sps = sps();
        sps.poc_type = 1;
        sps.offset_for_ref_frame = vec![1, 3];
        sps.offset_for_non_ref_pic = -1;
        sps.offset_for_top_to_bottom_field = 2;
        let dpb = Dpb::new(4, 0);
        let sh = SliceHeader {
            frame_num: 3,
            ..Default::default()
        };

        let order = dpb.compute_picture_order(&sh, &sps, false, 0);

        assert_eq!(order.top, 3);
        assert_eq!(order.bottom, 5);
        assert_eq!(order.frame, 3);
    }

    #[test]
    fn poc_type2_includes_frame_num_offset_and_non_reference_bias() {
        let mut sps = sps();
        sps.poc_type = 2;
        let dpb = Dpb::new(4, 0);
        let sh = SliceHeader {
            frame_num: 2,
            ..Default::default()
        };

        assert_eq!(dpb.compute_poc(&sh, &sps, false, 16), 35);
        assert_eq!(dpb.compute_poc(&sh, &sps, true, 16), 0);
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
        let (l0, l1) = d.build_ref_lists(&sh, 8, 4);
        assert_eq!(l0, vec![2, 1, 0]);
        assert!(l1.is_empty());
        let _ = dpb;
    }

    #[test]
    fn top_field_reference_list_interleaves_top_and_bottom_fields() {
        let mut d = Dpb::new(4, 0);
        d.push(frame(3, 0, 0));
        let sh = SliceHeader {
            slice_type: 5,
            field_pic: true,
            bottom_field: false,
            ..Default::default()
        };

        assert_eq!(
            d.build_p_ref_list_entries(&sh, 4),
            vec![
                RefListEntry {
                    slot: 3,
                    bottom: false,
                },
                RefListEntry {
                    slot: 3,
                    bottom: true,
                },
            ]
        );
    }

    #[test]
    fn bottom_field_reference_list_starts_with_bottom_fields() {
        let mut d = Dpb::new(4, 0);
        d.push(frame(3, 0, 0));
        let sh = SliceHeader {
            slice_type: 5,
            field_pic: true,
            bottom_field: true,
            ..Default::default()
        };

        assert_eq!(
            d.build_p_ref_list_entries(&sh, 4),
            vec![
                RefListEntry {
                    slot: 3,
                    bottom: true,
                },
                RefListEntry {
                    slot: 3,
                    bottom: false,
                },
            ]
        );
    }

    #[test]
    fn bottom_field_p_list_uses_independent_field_cursors() {
        let mut d = Dpb::new(4, 0);
        d.push(frame(0, 0, 0));
        d.push(frame(1, 1, 6));
        let mut top_only = frame(2, 2, 12);
        top_only.field_coded = true;
        top_only.bottom_present = false;
        top_only.bottom_used = false;
        d.push(top_only);
        let sh = SliceHeader {
            slice_type: 5,
            frame_num: 2,
            field_pic: true,
            bottom_field: true,
            ..Default::default()
        };

        assert_eq!(
            d.build_p_ref_list_entries(&sh, 4),
            vec![
                RefListEntry {
                    slot: 1,
                    bottom: true,
                },
                RefListEntry {
                    slot: 2,
                    bottom: false,
                },
                RefListEntry {
                    slot: 0,
                    bottom: true,
                },
                RefListEntry {
                    slot: 1,
                    bottom: false,
                },
                RefListEntry {
                    slot: 0,
                    bottom: false,
                },
            ]
        );
    }

    #[test]
    fn top_only_reference_does_not_claim_a_decoded_bottom_field() {
        let mut d = Dpb::new(4, 0);
        let mut f = frame(3, 0, 0);
        f.field_coded = true;
        f.bottom_present = false;
        f.bottom_used = false;
        d.push(f);
        let sh = SliceHeader {
            slice_type: 5,
            field_pic: true,
            ..Default::default()
        };

        assert_eq!(
            d.build_p_ref_list_entries(&sh, 4),
            vec![RefListEntry {
                slot: 3,
                bottom: false,
            }]
        );
    }

    #[test]
    fn p_slice_auxiliary_b_lists_match_mpp_rps_semantics() {
        let mut d = Dpb::new(4, 0);
        d.push(frame(0, 0, 0));
        d.push(frame(1, 1, 6));
        d.push(frame(2, 2, 2));
        let mut sh = SliceHeader::default();
        sh.slice_type = 5; // P
        sh.frame_num = 3;
        sh.mod_l0 = vec![RefMod::Sub(2), RefMod::End];

        let (b0, b1) = d.build_auxiliary_b_ref_lists(&sh, 4, 8);

        assert_eq!(b0, vec![2, 0, 1]);
        assert_eq!(b1, vec![1, 2, 0]);
        assert_eq!(
            d.build_ref_lists(&sh, 4, 8).0,
            vec![0, 2, 1],
            "the P-only list 0 reordering must not alter the auxiliary lists"
        );
    }

    #[test]
    fn auxiliary_lists_use_field_entries_for_field_pictures() {
        let mut d = Dpb::new(4, 0);
        d.push(frame(3, 0, 0));
        let sh = SliceHeader {
            slice_type: 6,
            field_pic: true,
            bottom_field: true,
            ..Default::default()
        };

        let (l0, _) = d.build_auxiliary_b_ref_list_entries(&sh, 1, 4);

        assert_eq!(
            l0,
            vec![
                RefListEntry {
                    slot: 3,
                    bottom: true,
                },
                RefListEntry {
                    slot: 3,
                    bottom: false,
                },
            ]
        );
    }

    #[test]
    fn bottom_field_b_lists_expand_fields_before_breaking_identical_lists() {
        let mut d = Dpb::new(4, 0);
        d.push(frame(0, 0, 0));
        d.push(frame(1, 1, 6));
        let mut top_only = frame(2, 2, 12);
        top_only.field_coded = true;
        top_only.bottom_present = false;
        top_only.bottom_used = false;
        d.push(top_only);
        let sh = SliceHeader {
            slice_type: 6,
            frame_num: 2,
            field_pic: true,
            bottom_field: true,
            ..Default::default()
        };

        let (l0, l1) = d.build_auxiliary_b_ref_list_entries(&sh, 13, 4);

        assert_eq!(
            l0,
            vec![
                RefListEntry {
                    slot: 1,
                    bottom: true,
                },
                RefListEntry {
                    slot: 2,
                    bottom: false,
                },
                RefListEntry {
                    slot: 0,
                    bottom: true,
                },
                RefListEntry {
                    slot: 1,
                    bottom: false,
                },
                RefListEntry {
                    slot: 0,
                    bottom: false,
                },
            ]
        );
        assert_eq!(
            l1,
            vec![
                RefListEntry {
                    slot: 2,
                    bottom: false,
                },
                RefListEntry {
                    slot: 1,
                    bottom: true,
                },
                RefListEntry {
                    slot: 0,
                    bottom: true,
                },
                RefListEntry {
                    slot: 1,
                    bottom: false,
                },
                RefListEntry {
                    slot: 0,
                    bottom: false,
                },
            ]
        );
    }

    #[test]
    fn b_slice_initial_lists_interleave_by_poc() {
        // Current picture POC 5; refs at POC 0, 2, 4, 6, 8.
        let mut d = Dpb::new(5, 0);
        for (i, poc) in [0, 2, 4, 6, 8].iter().enumerate() {
            d.push(frame(i as u8, i as u32, *poc));
        }
        let mut sh = SliceHeader::default();
        sh.slice_type = 6; // B
        sh.ref_idx_l0_active = 5;
        sh.ref_idx_l1_active = 5;
        let (l0, l1) = d.build_ref_lists(&sh, 5, 8);
        // L0: POC <= 5 descending (4, 2, 0), then POC > 5 ascending (6, 8).
        assert_eq!(l0, vec![2, 1, 0, 3, 4]);
        // L1: POC > 5 ascending (6, 8), then POC <= 5 descending (4, 2, 0).
        assert_eq!(l1, vec![3, 4, 2, 1, 0]);
    }

    #[test]
    fn p_list_is_evaluated_before_b_lists_for_the_hardware_rps() {
        // MPP fills hardware list 0 from the P-list even for a B picture.
        let mut d = Dpb::new(4, 0);
        d.push(frame(0, 0, 0));
        d.push(frame(1, 1, 6));
        d.push(frame(2, 2, 2));
        let mut sh = SliceHeader::default();
        sh.slice_type = 6; // B
        sh.frame_num = 3;
        sh.ref_idx_l0_active = 3;
        sh.ref_idx_l1_active = 3;
        let (b0, b1) = d.build_ref_lists(&sh, 4, 8);
        assert_eq!(b0, vec![2, 0, 1], "POC 2, 0, then future 6");
        assert_eq!(b1, vec![1, 2, 0], "future 6, then POC 2, 0");
        assert_eq!(
            d.build_p_ref_list(&sh, 8),
            vec![2, 1, 0],
            "frame_num descending"
        );
    }

    #[test]
    fn reorder_moves_a_frame_to_the_front() {
        // Frames: fn 0,1,2 in slots 0,1,2. Initial L0 = [2,1,0].
        // Modification "Sub(1)" selects frame_num 1 and moves it to position 0.
        let frames = [frame(0, 0, 0), frame(1, 1, 2), frame(2, 2, 4)];
        let mut list = vec![2u8, 1, 0];
        reorder(&mut list, &[RefMod::Sub(1), RefMod::End], 3, 16, &frames);
        assert_eq!(list, vec![1, 2, 0]);
    }

    #[test]
    fn reorder_selects_field_long_term_by_encoded_pic_num() {
        let mut long = frame(1, 2, 4);
        long.long_term = true;
        long.long_term_frame_idx = 1;
        long.field_coded = true;
        let frames = [frame(0, 3, 6), long];
        let mut list = vec![0u8, 1];

        reorder(
            &mut list,
            &[RefMod::LongTerm(2), RefMod::End],
            4,
            8,
            &frames,
        );

        assert_eq!(
            list,
            vec![1, 0],
            "a top field uses 2 * long_term_frame_idx as long_term_pic_num"
        );
    }

    #[test]
    fn reorder_reports_missing_pictures() {
        let frames = [frame(0, 0, 0)];
        let mut list = vec![0u8];
        // Sub(5) selects frame_num -3 (wraps), which is not in the DPB.
        assert!(reorder(
            &mut list,
            &[RefMod::Sub(5), RefMod::End],
            2,
            16,
            &frames
        ));
        assert_eq!(list, vec![0]);
    }

    #[test]
    fn reorder_reports_an_errored_reference() {
        let mut errored = frame(0, 0, 0);
        errored.error = 0x10;
        let frames = [errored];
        let mut list = vec![0u8];

        assert!(reorder(
            &mut list,
            &[RefMod::Sub(0), RefMod::End],
            1,
            16,
            &frames
        ));
    }

    #[test]
    fn active_reference_error_marks_the_current_picture() {
        let mut d = Dpb::new(2, 0);
        let mut errored = frame(0, 0, 0);
        errored.error = 0x10;
        d.push(errored);
        let sh = SliceHeader {
            slice_type: 5,
            frame_num: 1,
            ref_idx_l0_active: 1,
            ..Default::default()
        };

        assert!(d.reference_list_error(&sh, 2, 4));

        d.frames[0].error = 0;
        assert!(!d.reference_list_error(&sh, 2, 4));
    }

    #[test]
    fn inactive_reference_error_does_not_mark_the_current_picture() {
        let mut d = Dpb::new(2, 0);
        let mut older = frame(0, 0, 0);
        older.error = 0x10;
        d.push(older);
        d.push(frame(1, 1, 2));
        let sh = SliceHeader {
            slice_type: 5,
            frame_num: 2,
            ref_idx_l0_active: 1,
            ..Default::default()
        };

        assert!(!d.reference_list_error(&sh, 4, 4));
    }

    #[test]
    fn frame_num_gap_marks_a_non_i_picture_when_disallowed() {
        let mut d = Dpb::new(2, 0);
        d.push(frame(0, 0, 0));
        let mut sps = sps();
        sps.gaps_in_frame_num_allowed = false;
        let mut sh = SliceHeader {
            slice_type: 5,
            frame_num: 2,
            ..Default::default()
        };

        assert!(d.frame_num_gap(&sh, &sps));
        sh.frame_num = 1;
        assert!(!d.frame_num_gap(&sh, &sps));
        sps.gaps_in_frame_num_allowed = true;
        sh.frame_num = 2;
        assert!(!d.frame_num_gap(&sh, &sps));
    }

    #[test]
    fn reorder_handles_frame_num_wrap() {
        let frames = [frame(0, 14, 28), frame(1, 15, 30), frame(2, 0, 32)];
        let mut list = vec![2u8, 1, 0];
        // current frame_num 1 (after wrap), Sub(1) -> frame_num 15.
        reorder(&mut list, &[RefMod::Sub(1), RefMod::End], 1, 16, &frames);
        assert_eq!(list, vec![1, 2, 0]);
    }

    #[test]
    fn reorder_updates_prediction_for_each_operation() {
        // Each PicNum follows the previous operation, not the current frame
        // number. Sub(0), Sub(0) therefore selects frame_num 3 then 2.
        let frames = [frame(0, 1, 2), frame(1, 2, 4), frame(2, 3, 6)];
        let mut list = vec![2u8, 1, 0];
        reorder(
            &mut list,
            &[RefMod::Sub(0), RefMod::Sub(0), RefMod::End],
            4,
            16,
            &frames,
        );
        assert_eq!(list, vec![2, 1, 0]);
    }

    #[test]
    fn identical_b_lists_swap_the_first_l1_entries() {
        let mut d = Dpb::new(4, 0);
        d.push(frame(0, 0, 4));
        d.push(frame(1, 1, 6));
        let mut sh = SliceHeader::default();
        sh.slice_type = 6; // B
        sh.frame_num = 2;
        sh.ref_idx_l0_active = 2;
        sh.ref_idx_l1_active = 2;
        let (l0, l1) = d.build_ref_lists(&sh, 10, 4);
        assert_eq!(l0, vec![1, 0]);
        assert_eq!(l1, vec![0, 1]);
    }

    #[test]
    fn reorder_can_select_a_reference_beyond_the_active_count() {
        let mut d = Dpb::new(5, 0);
        for slot in 0..4 {
            d.push(frame(slot, slot as u32, slot as i32 * 2));
        }
        let mut sh = SliceHeader::default();
        sh.slice_type = 5; // P
        sh.frame_num = 4;
        sh.ref_idx_l0_active = 1;
        sh.mod_l0 = vec![RefMod::Sub(3), RefMod::End];
        let (l0, _) = d.build_ref_lists(&sh, 8, 4);
        assert_eq!(l0[0], 0, "Sub(3) selects frame_num 0");
    }

    #[test]
    fn reorder_is_a_noop_without_modifications() {
        let frames = [frame(0, 0, 0), frame(1, 1, 2)];
        let mut list = vec![1u8, 0];
        reorder(&mut list, &[], 2, 16, &frames);
        assert_eq!(list, vec![1, 0]);
    }

    #[test]
    fn long_term_ref_is_appended_after_short_term() {
        let mut d = Dpb::new(4, 2);
        d.push(frame(0, 0, 0));
        d.push(frame(1, 1, 2));
        // Current frame_num is 2; MakeLong{delta: 0} targets frame_num 1.
        d.apply_mmco(&[Mmco::MakeLong { delta: 0, idx: 0 }], 2, 0, 16);
        let mut sh = SliceHeader::default();
        sh.slice_type = 5; // P
        sh.ref_idx_l0_active = 2;
        let (l0, _) = d.build_ref_lists(&sh, 4, 8);
        assert_eq!(l0, vec![0, 1], "short-term first, long-term after");
    }

    #[test]
    fn field_store_reports_top_only_reference_state() {
        let mut f = frame(2, 0, 4);
        f.field_coded = true;
        f.top_present = true;
        f.bottom_present = false;
        f.top_used = true;
        f.bottom_used = false;

        assert_eq!(
            f.reference_flags(),
            ReferenceFlags {
                field_coded: true,
                top_used: true,
                bottom_used: false,
                colmv_valid: true,
            }
        );
    }
}
