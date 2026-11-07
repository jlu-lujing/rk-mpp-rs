//! H.264 global/RPS tables consumed by the VDPU383 front-end.
//!
//! VDPU383 keeps the same parsed H.264 model as the RK3588 path, but packs the
//! SPS/PPS/DPB state into a 176-byte global block and a separate 128-byte RPS
//! block. Layout follows `vdpu38x_h264d_prepare_spspps` and
//! `hal_h264d_vdpu383.c::prepare_framerps`.

use super::dpb::{FrameStore, PictureOrderCount, RefListEntry};
use super::hw::BitWriter;
use super::parser::{Pps, SliceHeader, Sps};

pub const GLOBAL_CFG_BYTES: usize = 176;
pub const RPS_BYTES: usize = 128;

pub fn build_global_cfg(
    sps: &Sps,
    pps: &Pps,
    dpb_slots: &[Option<&FrameStore>],
    sh: &SliceHeader,
    order: PictureOrderCount,
) -> Vec<u8> {
    let mut w = BitWriter::new();
    let mbaff = !sps.frame_mbs_only && sps.mb_adaptive_frame_field && !sh.field_pic;
    let pic_height = if sh.field_pic {
        sps.coded_height / 2
    } else {
        sps.coded_height
    };

    // SPS
    w.put(sps.sps_id & 0xf, 4);
    w.put(sps.profile_idc as u32, 8);
    w.put(0, 1); // constraint_set3_flag
    w.put(sps.chroma_format_idc, 2);
    w.put(sps.bit_depth_luma - 8, 3);
    w.put(sps.bit_depth_chroma - 8, 3);
    w.put(0, 1); // qpprime_y_zero_transform_bypass_flag
    w.put(sps.log2_max_frame_num - 4, 4);
    w.put(sps.max_num_ref_frames, 5);
    w.put(sps.poc_type, 2);
    w.put(sps.log2_max_poc_lsb.saturating_sub(4), 4);
    w.put(sps.delta_pic_order_always_zero as u32, 1);
    w.put(sps.coded_width & 0xffff, 16);
    w.put(pic_height & 0xffff, 16);
    w.put(sps.frame_mbs_only as u32, 1);
    w.put(mbaff as u32, 1);
    w.put(sps.direct_8x8_inference as u32, 1);
    w.put(0, 1); // MVC extension disabled
    w.put(0, 22);

    // PPS
    w.put(pps.pps_id & 0xff, 8);
    w.put(pps.sps_id & 0x1f, 5);
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
    w.put(pps.second_chroma_qp_offset as u32, 5);
    w.put(0, 1); // scaling list is addressed separately

    let mut long_term = 0u32;
    for (index, slot) in dpb_slots.iter().copied().enumerate().take(16) {
        if slot.is_some_and(|slot| slot.long_term) {
            long_term |= 1 << index;
        }
    }
    w.put(long_term, 32);

    let (top, bottom) = order.hardware_fields(sh);
    w.put(sh.field_pic as u32, 1);
    w.put((sh.field_pic && sh.bottom_field) as u32, 1);
    w.put(top as u32, 32);
    w.put(bottom as u32, 32);

    for slot in dpb_slots.iter().copied().take(16) {
        let (top, bottom) = slot
            .map(|slot| (slot.top_poc, slot.bottom_poc))
            .unwrap_or((0, 0));
        w.put(top as u32, 32);
        w.put(bottom as u32, 32);
    }

    let mut field_and_top = 0u32;
    let mut bottom_and_colmv = 0u32;
    for (index, slot) in dpb_slots.iter().copied().enumerate().take(16) {
        let Some(slot) = slot else {
            continue;
        };
        if slot.field_coded {
            field_and_top |= 1 << index;
        }
        if slot.top_used {
            field_and_top |= 1 << (index + 16);
        }
        if slot.bottom_used {
            bottom_and_colmv |= 1 << index;
        }
        if slot.colmv_valid {
            bottom_and_colmv |= 1 << (index + 16);
        }
    }
    w.put(field_and_top, 32);
    w.put(bottom_and_colmv, 32);
    w.align(64);
    w.bytes_n(GLOBAL_CFG_BYTES)
}

pub fn build_rps(
    sps: &Sps,
    sh: &SliceHeader,
    dpb_slots: &[Option<&FrameStore>],
    lists: [&[RefListEntry]; 3],
) -> Vec<u8> {
    let mut w = BitWriter::new();
    let max_frame_num = 1u32 << sps.log2_max_frame_num;
    for slot in dpb_slots.iter().copied().take(16) {
        let frame_num = match slot {
            None => 0,
            Some(slot) if slot.long_term => slot.long_term_frame_idx,
            Some(slot) if slot.frame_num > sh.frame_num => {
                slot.frame_num.wrapping_sub(max_frame_num)
            }
            Some(slot) => slot.frame_num,
        };
        w.put(frame_num & 0xffff, 16);
    }
    w.put(0, 32); // MVC ref-layer ids
    for list in lists {
        for index in 0..32 {
            let value = match list.get(index) {
                Some(entry) => (entry.slot as u32 & 0x1f) | (1 << 4) | ((entry.bottom as u32) << 5),
                None => 0,
            };
            w.put(value, 7);
        }
    }
    w.align(128);
    w.bytes_n(RPS_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sps() -> Sps {
        Sps {
            profile_idc: 77,
            sps_id: 1,
            chroma_format_idc: 1,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            log2_max_frame_num: 4,
            poc_type: 0,
            log2_max_poc_lsb: 6,
            max_num_ref_frames: 4,
            width: 640,
            height: 480,
            coded_width: 640,
            coded_height: 480,
            height_map_units: 30,
            frame_mbs_only: true,
            direct_8x8_inference: true,
            ..Default::default()
        }
    }

    fn pps() -> Pps {
        Pps {
            pps_id: 2,
            sps_id: 1,
            num_ref_idx_l0: 2,
            num_ref_idx_l1: 1,
            entropy_coding: true,
            deblocking_filter_control: true,
            ..Default::default()
        }
    }

    #[test]
    fn global_cfg_is_the_vdpu383_fixed_window() {
        let out = build_global_cfg(
            &sps(),
            &pps(),
            &[],
            &SliceHeader::default(),
            PictureOrderCount {
                top: 4,
                bottom: 4,
                frame: 4,
            },
        );
        assert_eq!(out.len(), GLOBAL_CFG_BYTES);
        assert_eq!(out[0] & 0xf, 1, "sps_id");
        assert_eq!(out[1] & 0xf, 77 >> 4, "profile_idc high nibble");
        assert_eq!(out[0] >> 4, 77 & 0xf, "profile_idc low nibble");
    }

    #[test]
    fn rps_has_16_frame_nums_and_three_reference_lists() {
        let mut slots = [None; 16];
        let frame = FrameStore {
            frame_num: 3,
            poc: 3,
            top_poc: 3,
            bottom_poc: 3,
            long_term: false,
            long_term_frame_idx: 0,
            slot: 0,
            buf: 0,
            referenced: true,
            colmv_valid: true,
            field_coded: false,
            top_present: true,
            bottom_present: true,
            top_used: true,
            bottom_used: true,
            error: 0,
        };
        slots[0] = Some(&frame);
        let l0 = [RefListEntry {
            slot: 0,
            bottom: false,
        }];
        let out = build_rps(&sps(), &SliceHeader::default(), &slots, [&l0[..], &[], &[]]);
        assert_eq!(out.len(), RPS_BYTES);
        assert_eq!(&out[..8], &[0xf3, 0xff, 0, 0, 0, 0, 0, 0]);
    }
}
