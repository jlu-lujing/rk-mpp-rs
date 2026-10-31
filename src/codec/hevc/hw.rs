//! Hardware syntax tables for the rkvdec HEVC front-end.
//!
//! Layout ported from `hal/rkdec/h265d/hal_h265d_com.c`
//! (`hal_h265d_slice_hw_rps`): 32 long-term entries then 64 fixed-size
//! short-term RPS slots, bit-packed LSB-first exactly like `mpp_put_bits`.

use super::parser::{Pps, ShortTermRps, Sps};
use crate::codec::h264::hw::BitWriter;

/// One long-term reference entry in the SPS RPS table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LongTermRef {
    pub poc_lsb: u16,
    pub used: bool,
}

/// Total size of the RPS table: `fifo_len` 400 u64 words in the C code.
pub const RPS_TABLE_SIZE: usize = 400 * 8;

/// One SPS/PPS scaling-list table (`scalingFactor_t`).
pub const SCALING_LIST_TABLE_SIZE: usize = 1360;

/// One short-term set occupies a fixed 48 bytes: 8 header bits, 15
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

pub fn build_sps_rps_from_sps(sps: &Sps) -> Vec<u8> {
    let long_term: Vec<LongTermRef> = sps
        .long_term_refs_sps
        .iter()
        .map(|entry| LongTermRef {
            poc_lsb: entry.poc_lsb,
            used: entry.used,
        })
        .collect();
    build_sps_rps(&sps.short_term_ref_pic_sets, &long_term)
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
    w.put(
        if sps.pcm_enabled {
            sps.pcm_bit_depth_luma
        } else {
            0
        },
        4,
    );
    w.put(
        if sps.pcm_enabled {
            sps.pcm_bit_depth_chroma
        } else {
            0
        },
        4,
    );
    w.put(sps.pcm_loop_filter_disabled as u32, 1);
    w.put(sps.pcm_log2_diff_max_min_cb, 3);
    w.put(
        if sps.pcm_enabled {
            sps.pcm_log2_min_cb
        } else {
            0
        },
        3,
    );
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
        (log2_min_cb + sps.log2_diff_max_min_cb).saturating_sub(pps.diff_cu_qp_delta_depth),
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
    w.put(pps.loop_filter_across_slices as u32, 1);
    w.put(pps.loop_filter_across_tiles as u32, 1);
    w.put(pps.deblocking_filter_override_enabled as u32, 1);
    w.put(pps.pps_deblocking_filter_disabled as u32, 1);
    w.put(pps.beta_offset_div2 as u32, 4);
    w.put(pps.tc_offset_div2 as u32, 4);
    w.put(pps.lists_modification_present as u32, 1);
    w.put(pps.log2_parallel_merge_level, 3);
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

/// Build the vdpu34x scaling-list packet. MPP source:
/// `h265d_syntax.c::fill_scaling_lists`,
/// `hal_h265d_vdpu34x.c::hal_h265d_output_scalinglist_packet` and
/// `hal_h265d_com.c::hal_record_scaling_list`.
pub fn build_scaling_list_packet(sps: &Sps, pps: &Pps) -> Option<Vec<u8>> {
    if !sps.scaling_list_enabled {
        return None;
    }
    let mut scaling = pps
        .scaling_list
        .clone()
        .or_else(|| sps.scaling_list.clone())
        .unwrap_or_else(super::parser::ScalingList::default_hevc);
    if sps.chroma_format_idc == 3 {
        for matrix in [1usize, 2, 4, 5] {
            scaling.matrices[3][matrix] = scaling.matrices[2][matrix];
        }
        for matrix in [1usize, 2, 4, 5] {
            scaling.dc[1][matrix] = scaling.dc[0][matrix];
        }
    }

    let mut table = vec![0u8; SCALING_LIST_TABLE_SIZE];
    let mut offset = 0usize;

    // scalingfactor0: size 0 uses the parsed 4x4-copy table (flat 16 by
    // default), size 1 and 2 use all six matrices, and size 3 stores only
    // matrices 0 and 3 with a 128-byte hardware tail.
    for matrix in 0..6 {
        table[offset..offset + 16].copy_from_slice(&scaling.matrices[0][matrix][..16]);
        offset += 16;
    }
    for size in 1..3 {
        for matrix in 0..6 {
            table[offset..offset + 64].copy_from_slice(&scaling.matrices[size][matrix]);
            offset += 64;
        }
    }
    for matrix in [0usize, 3] {
        table[offset..offset + 64].copy_from_slice(&scaling.matrices[3][matrix]);
        offset += 192;
    }
    debug_assert_eq!(offset, 1248);

    // MPP stores each matrix transposed in place.
    for matrix in 0..6 {
        transpose_square(&mut table[matrix * 16..matrix * 16 + 16], 4);
    }
    for (start, count) in [(96usize, 6usize), (480, 6)] {
        for matrix in 0..count {
            let pos = start + matrix * 64;
            transpose_square(&mut table[pos..pos + 64], 8);
        }
    }
    for pos in [864usize, 1056] {
        transpose_square(&mut table[pos..pos + 64], 8);
    }

    // scalingfactor1: rotated 4x4 copy matrices.
    let mut rotated_4x4 = [0u8; 96];
    for matrix in 0..6 {
        let dst = &mut rotated_4x4[matrix * 16..matrix * 16 + 16];
        let copy_4x4 = &scaling.matrices[0][matrix][..16];
        for column in 0..4 {
            for row in 0..4 {
                dst[column * 4 + row] = copy_4x4[row * 4 + column];
            }
        }
        transpose_square(dst, 4);
    }
    table[1248..1344].copy_from_slice(&rotated_4x4);

    // scalingdc: six size-2 values followed by two padded size-3 values.
    table[1344..1350].copy_from_slice(&scaling.dc[0]);
    for (n, matrix) in [0usize, 3].into_iter().enumerate() {
        table[1350 + n * 3] = scaling.dc[1][matrix];
    }
    Some(table)
}

fn transpose_square(data: &mut [u8], size: usize) {
    debug_assert_eq!(data.len(), size * size);
    for row in 0..size {
        for column in row + 1..size {
            data.swap(row * size + column, column * size + row);
        }
    }
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
    let uniform_axis = |total: u32, count: u32| {
        (0..count)
            .map(|i| ((i + 1) * total) / count - (i * total) / count)
            .collect::<Vec<_>>()
    };
    let explicit_axis = |total: u32, count: u32, explicit: &[u32]| {
        if explicit.len() != count.saturating_sub(1) as usize {
            return None;
        }
        let used: u32 = explicit.iter().sum();
        let mut values = explicit.to_vec();
        values.push(total.saturating_sub(used));
        Some(values)
    };
    let cw = if pps.uniform_spacing {
        uniform_axis(pic_w, cols)
    } else {
        explicit_axis(pic_w, cols, &pps.tile_column_widths)
            .unwrap_or_else(|| uniform_axis(pic_w, cols))
    };
    let rh = if pps.uniform_spacing {
        uniform_axis(pic_h, rows)
    } else {
        explicit_axis(pic_h, rows, &pps.tile_row_heights)
            .unwrap_or_else(|| uniform_axis(pic_h, rows))
    };
    (cw, rh)
}

#[cfg(test)]
mod packet_tests {
    use super::*;
    use crate::codec::hevc::parser::{parse_pps, Pps, Sps};

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
        // The SPS fields occupy 120 bits, so they are padded to 128 bits with
        // ones and the PPS starts at byte 16.
        assert_eq!(pkt[15], 0xff);
        assert_eq!(pkt[16] & 0x3f, pps().pps_id as u8);
        assert_eq!(pkt.len(), 112);
    }

    #[test]
    fn pps_packet_preserves_mpp_filter_and_deblocking_fields() {
        let mut pps = pps();
        pps.loop_filter_across_slices = false;
        pps.loop_filter_across_tiles = false;
        pps.deblocking_filter_override_enabled = true;
        pps.pps_deblocking_filter_disabled = true;
        pps.beta_offset_div2 = -2;
        pps.tc_offset_div2 = -1;
        pps.lists_modification_present = true;
        pps.log2_parallel_merge_level = 4;

        let pkt = build_pps_packet(&sps(), &pps);

        assert_eq!(
            pkt[24], 0xec,
            "slice/tile loop-filter, deblocking flags and beta offset differ from MPP"
        );
        assert_eq!(
            pkt[25], 0x9f,
            "tc offset, list modification and merge level differ from MPP"
        );
    }

    #[test]
    fn wpp_pps_matches_mpp_packet_without_consuming_substream_flag() {
        // PPS NAL from the 720p x265 stream used by the board regression.
        // WPP is enabled and tiles are disabled. MPP does not consume the
        // standard pps_loop_filter_across_substreams_enabled_flag here.
        let pps = parse_pps(&[0xc1, 0x72, 0xb4, 0x62, 0x40]).unwrap();
        assert!(pps.entropy_coding_sync);
        assert!(!pps.tiles_enabled);

        let pkt = build_pps_packet(&sps(), &pps);
        assert_eq!(
            (pkt[24], pkt[25]),
            (0x03, 0x40),
            "MPP consumes the next PPS field here, not the WPP substream flag"
        );
    }

    #[test]
    fn default_scaling_list_matches_mpp_hardware_table() {
        let mut sps = sps();
        sps.scaling_list_enabled = true;
        sps.scaling_list = Some(crate::codec::hevc::parser::ScalingList::default_hevc());
        let table = build_scaling_list_packet(&sps, &pps()).unwrap();
        assert_eq!(table.len(), SCALING_LIST_TABLE_SIZE);
        let hash = table.iter().fold(0xcbf29ce484222325u64, |mut hash, byte| {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
            hash
        });
        assert_eq!(
            hash, 0xd1ca8a6af638f56a,
            "default scaling-list bytes differ from MPP"
        );
    }

    #[test]
    fn explicit_4x4_scaling_list_reaches_both_mpp_tables() {
        let mut sps = sps();
        sps.scaling_list_enabled = true;
        let mut scaling = crate::codec::hevc::parser::ScalingList::default_hevc();
        let matrix: [u8; 16] = std::array::from_fn(|i| 32 + i as u8);
        scaling.matrices[0][0][..16].copy_from_slice(&matrix);
        sps.scaling_list = Some(scaling);

        let table = build_scaling_list_packet(&sps, &pps()).unwrap();
        let transposed: [u8; 16] = std::array::from_fn(|i| matrix[(i % 4) * 4 + i / 4]);
        assert_eq!(table[..16], transposed);
        assert_eq!(table[1248..1264], matrix);
    }

    #[test]
    fn tile_geometry_covers_the_picture() {
        let (cw, rh) = tile_geometry(&sps(), &pps());
        assert_eq!(cw.iter().sum::<u32>(), (1280 + 63) / 64);
        assert_eq!(rh.iter().sum::<u32>(), (720 + 63) / 64);
    }

    #[test]
    fn nonuniform_tile_geometry_reaches_the_pps_packet() {
        let mut pps = pps();
        pps.tiles_enabled = true;
        pps.tile_columns = 3;
        pps.tile_rows = 2;
        pps.uniform_spacing = false;
        pps.tile_column_widths = vec![7, 9];
        pps.tile_row_heights = vec![5];

        let (columns, rows) = tile_geometry(&sps(), &pps);
        assert_eq!(columns, [7, 9, 4]);
        assert_eq!(rows, [5, 7]);

        let pkt = build_pps_packet(&sps(), &pps);
        let read_12 = |start: usize| -> Vec<u32> {
            (0..20)
                .map(|i| {
                    let bit = start + i * 12;
                    (0..12).fold(0u32, |value, offset| {
                        value
                            | ((((pkt[(bit + offset) / 8] >> ((bit + offset) % 8)) & 1) as u32)
                                << offset)
                    })
                })
                .collect()
        };
        // The SPS is 128 bits and the aligned PPS header ends at bit 256.
        // MPP stores each tile dimension minus one.
        assert_eq!(&read_12(256)[..3], [6, 8, 3]);
        assert_eq!(&read_12(256 + 20 * 12)[..2], [4, 6]);
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
    fn parsed_sps_long_term_refs_feed_the_hardware_table() {
        use crate::codec::hevc::parser::LongTermSpsRef;

        let sps = Sps {
            long_term_refs_sps: vec![
                LongTermSpsRef {
                    poc_lsb: 0x1234,
                    used: true,
                },
                LongTermSpsRef {
                    poc_lsb: 0xabcd,
                    used: false,
                },
            ],
            ..Default::default()
        };
        let table = build_sps_rps_from_sps(&sps);
        assert_eq!(
            &table[..8],
            &[0x34, 0x12, 0x01, 0x00, 0xcd, 0xab, 0x00, 0x00]
        );
    }

    #[test]
    fn parsed_sps_short_term_ref_sets_feed_the_hardware_table() {
        let sps = Sps {
            short_term_ref_pic_sets: vec![set(&[-1], &[2]), set(&[-3, -1], &[])],
            ..Default::default()
        };
        let table = build_sps_rps_from_sps(&sps);
        // The long-term block ends after 128 bytes. The first short-term set
        // has one negative and one positive picture.
        assert_eq!(table[128], 0x11);
        // Set 2 begins after the first fixed 48-byte slot.
        assert_eq!(table[176], 0x02);
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
