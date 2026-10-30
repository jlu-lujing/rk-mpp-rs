//! Hardware syntax tables for the rkvdec HEVC front-end.
//!
//! Layout ported from `hal/rkdec/h265d/hal_h265d_com.c`
//! (`hal_h265d_slice_hw_rps`): 32 long-term entries then 64 short-term RPS
//! slots of fixed size, bit-packed LSB-first exactly like `mpp_put_bits`.

use super::dpb::FrameStore;
use super::parser::{ShortTermRps, SliceHeader, Sps};
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
