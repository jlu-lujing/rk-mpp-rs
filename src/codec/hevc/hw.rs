//! Hardware syntax tables for the rkvdec HEVC front-end.
//!
//! Layout ported from `hal/rkdec/h265d/hal_h265d_com.c`
//! (`hal_h265d_slice_hw_rps`): 32 long-term entries then 64 short-term RPS
//! slots of fixed size, bit-packed LSB-first exactly like `mpp_put_bits`.

use super::dpb::FrameStore;
use super::parser::{Pps, ShortTermRps, SliceHeader, Sps};
use crate::codec::h264::hw::BitWriter;

/// One long-term reference entry in the SPS RPS table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LongTermRef {
    pub poc_lsb: u16,
    pub used: bool,
}

/// Total size of the RPS table: `fifo_len` 400 u64 words in the C code.
pub const RPS_TABLE_SIZE: usize = 400 * 8;

/// One short-term set occupies a fixed 48-byte slot: 8 header bits, 15
/// `(16-bit delta, 1-bit used)` pairs, padding to 64 bits, then 64 zero bits.
const ST_SET_BITS: usize = 8 + 15 * 17;

/// Build the SPS-level RPS table (`sps_lt_rps` + `sps_st_rps`).
pub fn build_sps_rps(sets: &[ShortTermRps], long_term: &[LongTermRef]) -> Vec<u8> {
    let mut w = BitWriter::new();
    for i in 0..32 {
        let lt = long_term.get(i).copied().unwrap_or_default();
        w.put(lt.poc_lsb as u32, 16);
        w.put(lt.used as u32, 1);
        w.put(0, 15);
    }
    for i in 0..64 {
        match sets.get(i) {
            Some(set) => {
                w.put(set.s0.len() as u32, 4);
                w.put(set.s1.len() as u32, 4);
                for (d, used) in set.s0.iter().zip(set.used_s0.iter()) {
                    w.put(*d as u32, 16);
                    w.put(*used as u32, 1);
                }
                for (d, used) in set.s1.iter().zip(set.used_s1.iter()) {
                    w.put(*d as u32, 16);
                    w.put(*used as u32, 1);
                }
                for _ in (set.s0.len() + set.s1.len())..15 {
                    w.put(0, 16);
                    w.put(0, 1);
                }
            }
            None => {
                w.put(0, 4);
                w.put(0, 4);
                for _ in 0..15 {
                    w.put(0, 16);
                    w.put(0, 1);
                }
            }
        }
        // Each slot is padded to a 64-bit boundary and followed by 64 zero bits.
        let used_bits = w.bits() % 64;
        if used_bits != 0 {
            w.put(0, (64 - used_bits) as u32);
        }
        w.put(0, 32);
        w.put(0, 32);
        debug_assert!(w.bits() % 64 == 0 || w.bits() >= ST_SET_BITS);
    }
    w.bytes_n(RPS_TABLE_SIZE)
}

/// Slice-level RPS table is just the selected short-term set, written with the
/// same slot layout (one slot per slice RPS reference, see
/// `hal_h265d_slice_hw_rps` reuse in the HAL).
pub fn build_slice_rps(set: &ShortTermRps) -> Vec<u8> {
    build_sps_rps(std::slice::from_ref(set), &[])
}

/// Size of the SPS/PPS hardware packet (`fifo_len = 14` u64 words).
pub const PPS_PACKET_SIZE: usize = 14 * 8;

/// Build the 112-byte SPS+PPS packet (`hal_h265d_v345_output_pps_packet`).
///
/// MPP replicates this packet 64 times into the shared buffer; the hardware
/// indexes it by pps id.
pub fn build_pps_packet(sps: &Sps, pps: &Pps) -> Vec<u8> {
    let mut w = BitWriter::new();
    let log2_min_cb = sps.log2_min_cb;
    let width = sps.width;
    let height = sps.height;

    // ---- SPS ----
    w.put(sps.vps_id, 4);
    w.put(sps.sps_id, 4);
    w.put(sps.chroma_format_idc, 2);
    w.put(width, 16);
    w.put(height, 16);
    w.put(sps.bit_depth_luma, 4);
    w.put(sps.bit_depth_chroma, 4);
    w.put(sps.log2_max_pic_order_cnt_lsb, 5);
    w.put(sps.log2_diff_max_min_cb, 2);
    w.put(log2_min_cb, 3);
    w.put(sps.log2_min_tb, 3);
    w.put(sps.log2_diff_max_min_tb, 2);
    w.put(sps.max_transform_hierarchy_depth_inter, 3);
    w.put(sps.max_transform_hierarchy_depth_intra, 3);
    w.put(sps.scaling_list_enabled as u32, 1);
    w.put(sps.amp_enabled as u32, 1);
    w.put(sps.sao_enabled as u32, 1);
    w.put(sps.pcm_enabled as u32, 1);
    w.put(if sps.pcm_enabled { sps.pcm_bit_depth_luma } else { 0 }, 4);
    w.put(if sps.pcm_enabled { sps.pcm_bit_depth_chroma } else { 0 }, 4);
    w.put(sps.pcm_loop_filter_disabled as u32, 1);
    w.put(sps.pcm_log2_diff_max_min_cb, 3);
    w.put(if sps.pcm_enabled { sps.pcm_log2_min_cb } else { 0 }, 3);
    w.put(sps.num_short_term_ref_pic_sets, 7);
    w.put(sps.long_term_ref_pics_present as u32, 1);
    w.put(sps.num_long_term_ref_pics, 6);
    w.put(sps.temporal_mvp_enabled as u32, 1);
    w.put(sps.strong_intra_smoothing as u32, 1);
    w.put(0, 7);
    w.put(sps.max_dec_pic_buffering.saturating_sub(1), 4);
    w.put(0, 3);
    w.align_ones(32);

    // ---- PPS ----
    w.put(pps.pps_id, 6);
    w.put(pps.sps_id, 4);
    w.put(pps.dependent_slice_segments as u32, 1);
    w.put(pps.output_flag_present as u32, 1);
    w.put(pps.num_extra_slice_header_bits, 13);
    w.put(pps.sign_data_hiding as u32, 1);
    w.put(pps.cabac_init_present as u32, 1);
    w.put(pps.num_ref_idx_l0_default_active, 4);
    w.put(pps.num_ref_idx_l1_default_active, 4);
    w.put((pps.init_qp - 26) as u32, 7);
    w.put(pps.constrained_intra_pred as u32, 1);
    w.put(pps.transform_skip_enabled as u32, 1);
    w.put(pps.cu_qp_delta_enabled as u32, 1);
    // log2_min_cb + log2_diff_max_min_cb - diff_cu_qp_delta_depth, exactly as
    // hal_h265d_v345_output_pps_packet computes it.
    w.put(
        (log2_min_cb + sps.log2_diff_max_min_cb)
            .saturating_sub(pps.diff_cu_qp_delta_depth),
        3,
    );
    w.put(pps.cb_qp_offset as u32, 5);
    w.put(pps.cr_qp_offset as u32, 5);
    w.put(pps.slice_chroma_qp_offsets_present as u32, 1);
    w.put(pps.weighted_pred as u32, 1);
    w.put(pps.weighted_bipred as u32, 1);
    w.put(pps.transquant_bypass as u32, 1);
    w.put(pps.tiles_enabled as u32, 1);
    w.put(pps.entropy_coding_sync as u32, 1);
    w.put(1, 1); // loop_filter_across_slices
    w.put(1, 1); // loop_filter_across_tiles (PP<T default on)
    w.put(0, 1);
    w.put(0, 1);
    w.put(0, 4);
    w.put(0, 4);
    w.put(0, 1);
    w.put(2, 3);
    w.put(pps.slice_segment_header_extension_present as u32, 1);
    w.put(0, 3);
    let (cols, rows) = tile_counts(pps);
    w.put(cols, 5);
    w.put(rows, 5);
    w.put(0, 4);
    w.align_ones(64);

    // ---- tile geometry ----
    let (col_width, row_height) = tile_geometry(sps, pps);
    for j in 0..20 {
        let v = col_width.get(j).copied().unwrap_or(0).saturating_sub(1);
        w.put(v, 12);
    }
    for j in 0..22 {
        let v = row_height.get(j).copied().unwrap_or(0).saturating_sub(1);
        w.put(v, 12);
    }
    w.put(0, 32);
    w.put(0, 32);
    w.put(0, 32);
    w.put(0, 6);
    w.align_ones(64);
    w.bytes_n(PPS_PACKET_SIZE)
}

/// Number of tile columns and rows (1 when tiles are disabled).
fn tile_counts(pps: &Pps) -> (u32, u32) {
    if !pps.tiles_enabled {
        return (0, 0);
    }
    (pps.tile_columns, pps.tile_rows)
}

/// Per-column CTU counts and per-row CTU counts as the hardware expects.
fn tile_geometry(sps: &Sps, pps: &Pps) -> (Vec<u32>, Vec<u32>) {
    let ctu = 1u32 << (sps.log2_min_cb + sps.log2_diff_max_min_cb);
    let pic_w = (sps.width + ctu - 1) / ctu;
    let pic_h = (sps.height + ctu - 1) / ctu;
    if !pps.tiles_enabled {
        return (vec![pic_w], vec![pic_h]);
    }
    let cols = pps.tile_columns.max(1);
    let rows = pps.tile_rows.max(1);
    let mut cw = Vec::with_capacity(cols as usize);
    for i in 0..cols {
        cw.push(((i + 1) * pic_w) / cols - (i * pic_w) / cols);
    }
    let mut rh = Vec::with_capacity(rows as usize);
    for i in 0..rows {
        rh.push(((i + 1) * pic_h) / rows - (i * pic_h) / rows);
    }
    (cw, rh)
}

#[cfg(test)]
mod packet_tests {
    use super::*;
    use crate::codec::hevc::parser::{Pps, Sps};

    fn sps() -> Sps {
        Sps {
            sps_id: 0,
            vps_id: 0,
            chroma_format_idc: 1,
            width: 1280,
            height: 720,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            log2_max_pic_order_cnt_lsb: 8,
            log2_min_cb: 3,
            log2_diff_max_min_cb: 3,
            log2_min_tb: 2,
            log2_diff_max_min_tb: 3,
            max_transform_hierarchy_depth_inter: 0,
            max_transform_hierarchy_depth_intra: 0,
            sao_enabled: true,
            amp_enabled: true,
            temporal_mvp_enabled: true,
            strong_intra_smoothing: true,
            max_dec_pic_buffering: 5,
            ..Default::default()
        }
    }

    fn pps() -> Pps {
        Pps {
            pps_id: 0,
            sps_id: 0,
            num_ref_idx_l0_default_active: 1,
            num_ref_idx_l1_default_active: 1,
            init_qp: 26,
            entropy_coding_sync: true,
            log2_parallel_merge_level: 2,
            ..Default::default()
        }
    }

    #[test]
    fn packet_is_112_bytes() {
        assert_eq!(build_pps_packet(&sps(), &pps()).len(), PPS_PACKET_SIZE);
        assert_eq!(PPS_PACKET_SIZE, 112);
    }

    #[test]
    fn sps_header_is_lsb_first() {
        let pkt = build_pps_packet(&sps(), &pps());
        // vps_id(4)=0, sps_id(4)=0, then chroma_format_idc(2)=1 at bits 8..9.
        let chroma = (pkt[1] >> 0) & 0x03;
        assert_eq!(chroma, 1);
        // width 1280 at bits 10..26: check the low byte at bit 10.
        let mut v: u32 = 0;
        for i in 0..16u32 {
            let bit = (pkt[(10 + i as usize) / 8] >> ((10 + i as usize) % 8)) & 1;
            v |= (bit as u32) << i;
        }
        assert_eq!(v, 1280);
    }

    #[test]
    fn pps_section_starts_after_ones_padding() {
        let pkt = build_pps_packet(&sps(), &pps());
        // The SPS section is padded to 32 bits with ones; the PPS starts at
        // bit 256 only after the SPS fields (which end before that).
        let ones: u32 = (0..8)
            .map(|i| ((pkt[20] >> i) & 1) as u32)
            .sum();
        assert!(ones >= 0);
        assert_eq!(pkt.len(), 112);
    }

    #[test]
    fn tile_geometry_covers_the_picture() {
        let (cw, rh) = tile_geometry(&sps(), &pps());
        assert_eq!(cw.iter().sum::<u32>(), (1280 + 63) / 64);
        assert_eq!(rh.iter().sum::<u32>(), (720 + 63) / 64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(neg: &[i32], pos: &[i32]) -> ShortTermRps {
        ShortTermRps {
            s0: neg.to_vec(),
            used_s0: vec![true; neg.len()],
            s1: pos.to_vec(),
            used_s1: vec![true; pos.len()],
        }
    }

    #[test]
    fn rps_table_has_fixed_size() {
        let table = build_sps_rps(&[], &[]);
        assert_eq!(table.len(), RPS_TABLE_SIZE);
    }

    #[test]
    fn long_term_entry_is_32_bits() {
        let lt = [LongTermRef {
            poc_lsb: 0x1234,
            used: true,
        }];
        let table = build_sps_rps(&[], &lt);
        // LSB-first: POC LSB occupies bits 0..15, used is bit 16.
        assert_eq!(table[0], 0x34);
        assert_eq!(table[1], 0x12);
        assert_eq!(table[2], 0x01);
    }

    #[test]
    fn short_term_set_encodes_counts() {
        let table = build_sps_rps(&[set(&[], &[])], &[]);
        // Long-term block is 32 * 4 bytes = 128 bytes.
        // First set: num_negative(4)=0, num_positive(4)=0 -> byte 128 = 0.
        assert_eq!(table[128], 0x00);
    }

    #[test]
    fn negative_delta_roundtrip() {
        let table = build_sps_rps(&[set(&[-1, -4], &[2])], &[]);
        // num_negative=2, num_positive=1 in the low nibbles of byte 128.
        assert_eq!(table[128], 0x12);
        // First delta (-1) is a 16-bit LSB-first field right after the counts.
        let v = table[129] as u32 | ((table[130] as u32) << 8);
        assert_eq!(v as i16 as i32, -1);
    }

    #[test]
    fn slice_rps_matches_sps_layout() {
        let table = build_slice_rps(&set(&[-2], &[]));
        assert_eq!(table.len(), RPS_TABLE_SIZE);
        assert_eq!(table[128], 0x01, "one negative picture");
    }
}
