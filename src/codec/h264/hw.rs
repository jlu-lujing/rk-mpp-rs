//! Hardware syntax tables for the rkvdec H.264 front-end.
//!
//! The decoder does not read the container syntax itself: the driver hands it
//! three bit-packed tables through `sw42` (SPS+PPS+DPB), `sw43` (frame RPS) and
//! the scan list, plus a cabac table buffer. The layout is defined by
//! `prepare_spspps`, `prepare_framerps` and `prepare_scanlist` in the official
//! `hal/rkdec/h264d/hal_h264d_rkv_reg.c`; this module reproduces it from our
//! own parsed structures.

use super::dpb::FrameStore;
use super::parser::{Pps, SliceHeader, Sps};

/// MSB-first bit packer with the same semantics as MPP's `mpp_put_bits`.
#[derive(Default)]
pub struct BitWriter {
    buf: Vec<u8>,
    bit: usize,
}

impl BitWriter {
    pub fn new() -> Self {
        BitWriter::default()
    }

    pub fn with_capacity(bytes: usize) -> Self {
        BitWriter {
            buf: vec![0; bytes],
            bit: 0,
        }
    }

    /// Write the low `n` bits of `val`, most significant first.
    pub fn put(&mut self, val: u32, n: u32) {
        for i in (0..n).rev() {
            let b = ((val >> i) & 1) as u8;
            let byte = self.bit / 8;
            if byte >= self.buf.len() {
                self.buf.push(0);
            }
            if b != 0 {
                self.buf[byte] |= 1 << (7 - (self.bit % 8));
            }
            self.bit += 1;
        }
    }

    /// Write `n` bits of all ones (`mpp_put_bits(&bp, -1, n)`).
    pub fn put_ones(&mut self, n: u32) {
        self.put(u32::MAX, n);
    }

    /// Pad with zeros to a multiple of `align` bits.
    pub fn align(&mut self, align: u32) {
        while self.bit % align as usize != 0 {
            self.put(0, 1);
        }
    }

    pub fn bits(&self) -> usize {
        self.bit
    }

    pub fn bytes(&mut self) -> Vec<u8> {
        self.buf.clone()
    }
}

/// SPS + PPS + DPB block, exactly 48 bytes as `VDPU34X_SPSPPS_UNIT_SIZE`.
///
/// Layout follows `prepare_spspps` in `hal_h264d_vdpu34x.c`: the SPS part is
/// 128-bit aligned, dimensions are 12-bit MB counts, the PPS part ends with a
/// zero scan-list address (the scan list has its own register), and the DPB
/// flags share one 32-bit word.
pub fn build_sps_pps(sps: &Sps, pps: &Pps, dpb_slots: &[Option<&FrameStore>]) -> Vec<u8> {
    let mut w = BitWriter::new();
    // SPS
    w.put_ones(13); // sps_id(4) | profile_idc(8) | constraint_set3(1)
    w.put(sps.chroma_format_idc, 2);
    w.put(sps.bit_depth_luma - 8, 3);
    w.put(sps.bit_depth_chroma - 8, 3);
    w.put(0, 1); // qpprime_y_zero_transform_bypass
    w.put(sps.log2_max_frame_num - 4, 4);
    w.put(sps.max_num_ref_frames, 5);
    w.put(sps.poc_type, 2);
    w.put(sps.log2_max_poc_lsb.saturating_sub(4), 4);
    w.put(0, 1); // delta_pic_order_always_zero_flag
    w.put((sps.width + 15) / 16, 12);
    w.put((sps.height + 15) / 16, 12);
    w.put(sps.frame_mbs_only as u32, 1);
    w.put(0, 1); // MbaffFrameFlag
    w.put(sps.direct_8x8_inference as u32, 1);
    // MVC extension: disabled
    w.put(1, 1); // mvc_extension_enable
    w.put(1, 2); // num_views_minus1 + 1
    w.put(0, 10);
    w.put(0, 10);
    w.put(0, 1);
    w.put(0, 10);
    w.put(0, 1);
    w.put(0, 10);
    w.put(0, 1);
    w.put(0, 10);
    w.put(0, 1);
    w.put(0, 10);
    w.align(128);

    // PPS
    w.put_ones(13); // pps_id(8) | sps_id(5)
    w.put(pps.entropy_coding as u32, 1);
    w.put(pps.bottom_field_pic_order as u32, 1);
    w.put(pps.num_ref_idx_l0 - 1, 5);
    w.put(pps.num_ref_idx_l1.saturating_sub(1), 5);
    w.put(pps.weighted_pred as u32, 1);
    w.put(pps.weighted_bipred_idc, 2);
    w.put((pps.pic_init_qp - 26) as u32, 7);
    w.put((pps.pic_init_qs - 26) as u32, 6);
    w.put(pps.chroma_qp_offset as u32, 5);
    w.put(pps.deblocking_filter_control as u32, 1);
    w.put(pps.constrained_intra_pred as u32, 1);
    w.put(pps.redundant_pic_cnt as u32, 1);
    w.put(pps.transform_8x8 as u32, 1);
    w.put(0, 5); // second_chroma_qp_index_offset
    w.put(0, 1); // scaling_list_enable_flag
    w.put(0, 32); // scan list address: has its own register

    // DPB: 16 long-term flags then 16 layer ids in one word
    let mut flags = 0u32;
    for i in 0..16 {
        let long = dpb_slots
            .get(i)
            .copied()
            .flatten()
            .map(|f| f.long_term)
            .unwrap_or(false);
        if long {
            flags |= 1 << i;
        }
    }
    w.put(flags, 32);
    w.align(64);
    w.bytes()
}

/// Frame RPS block: frame_num wrapping, then the three reference lists as
/// 7-bit `(index | valid << 4, bottom, voidx)` entries.
pub fn build_rps(
    sps: &Sps,
    sh: &SliceHeader,
    dpb_slots: &[Option<&FrameStore>],
    lists: [&[u8]; 3],
) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.align(128);
    let max_frame_num = 1u32 << sps.log2_max_frame_num;
    for i in 0..16 {
        let f = dpb_slots.get(i).copied().flatten();
        let wrap = match f {
            None => 0u32,
            Some(f) if f.long_term => f.frame_num,
            Some(f) => {
                if f.frame_num > sh.frame_num {
                    f.frame_num.wrapping_sub(max_frame_num)
                } else {
                    f.frame_num
                }
            }
        };
        w.put(wrap & 0xffff, 16);
    }
    w.put(0, 16); // NULL flags
    let mut layer = 0u32;
    for i in 0..16 {
        if dpb_slots.get(i).copied().flatten().is_some() {
            layer |= 1 << i;
        }
    }
    w.put(layer, 16);
    for list in lists.iter() {
        for i in 0..32 {
            let v = match list.get(i) {
                Some(&idx) => (idx as u32 & 0x1f) | (1 << 4),
                None => 0,
            };
            w.put(v, 7);
        }
    }
    w.align(128);
    w.bytes()
}

/// Scan list block: 6 × 4x4 lists then 2 × 8x8 lists, 8 bits per entry.
pub fn build_scanlist(scaling_4x4: Option<&[[u8; 16]; 6]>, scaling_8x8: Option<&[[u8; 64]; 2]>) -> Vec<u8> {
    let mut w = BitWriter::new();
    if let Some(l4) = scaling_4x4 {
        for list in l4 {
            for &v in list {
                w.put(v as u32, 8);
            }
        }
        if let Some(l8) = scaling_8x8 {
            for list in l8 {
                for &v in list {
                    w.put(v as u32, 8);
                }
            }
        }
    }
    w.bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::h264::parser::{Pps, Sps};

    fn sps() -> Sps {
        Sps {
            profile_idc: 77,
            level_idc: 31,
            chroma_format_idc: 1,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            log2_max_frame_num: 4,
            poc_type: 0,
            log2_max_poc_lsb: 6,
            max_num_ref_frames: 4,
            width: 1280,
            height: 720,
            frame_mbs_only: true,
            direct_8x8_inference: true,
            ..Default::default()
        }
    }

    fn pps() -> Pps {
        Pps {
            num_ref_idx_l0: 3,
            num_ref_idx_l1: 1,
            entropy_coding: true,
            weighted_pred: true,
            weighted_bipred_idc: 2,
            pic_init_qp: 26,
            pic_init_qs: 26,
            deblocking_filter_control: true,
            ..Default::default()
        }
    }

    #[test]
    fn bit_writer_is_msb_first() {
        let mut w = BitWriter::new();
        w.put(0b101, 3);
        w.put(0xff, 8);
        assert_eq!(w.bits(), 11);
        assert_eq!(w.bytes(), vec![0b1011_1111, 0b1110_0000]);
    }

    #[test]
    fn ones_and_align() {
        let mut w = BitWriter::new();
        w.put_ones(4);
        w.align(8);
        assert_eq!(w.bytes(), vec![0xf0]);
    }

    #[test]
    fn sps_pps_block_has_expected_size() {
        let sps = sps();
        let pps = pps();
        let block = build_sps_pps(&sps, &pps, &[]);
        assert_eq!(block.len(), 48, "VDPU34X_SPSPPS_UNIT_SIZE");
        assert_eq!(block.len() % 8, 0, "64-bit aligned");
    }

    #[test]
    fn sps_block_encodes_dimensions() {
        let sps = sps();
        let block = build_sps_pps(&sps, &pps(), &[]);
        // 13+2+3+3+1+4+5+2+4+1 = 38 bits before the width field (12 bits).
        let mut v = 0u32;
        for i in 38..50 {
            v = (v << 1) | ((block[i / 8] >> (7 - i % 8)) & 1) as u32;
        }
        assert_eq!(v, 80);
    }

    #[test]
    fn rps_marks_valid_entries() {
        let sps = sps();
        let sh = SliceHeader {
            frame_num: 3,
            slice_type: 5,
            ..Default::default()
        };
        let f0 = FrameStore {
            frame_num: 0,
            poc: 0,
            long_term: false,
            long_term_frame_idx: 0,
            slot: 0,
            buf: 0,
            referenced: true,
        };
        let slots = [Some(&f0), None, None, None];
        let block = build_rps(&sps, &sh, &slots, [&[0u8], &[], &[]]);
        assert_eq!(&block[..2], &[0, 0], "frame_num_wrap of slot 0");
        assert_eq!(block.len() % 16, 0, "128-bit aligned");
        assert!(block.len() <= 384, "VDPU34X_RPS_SIZE");
    }

    #[test]
    fn scanlist_is_empty_without_matrices() {
        assert!(build_scanlist(None, None).is_empty());
    }
}
