//! AV1 quantizer selection: `dc_q(b)` and `ac_q(b)` from the specification's
//! lookup tables.

use crate::reference::av1::quant_tables::{AC_QLOOKUP, DC_QLOOKUP};

fn lookup(table: &[[u16; 256]; 3], bit_depth: u32, b: i32) -> i32 {
    let row = ((bit_depth - 8) >> 1).min(2) as usize;
    let index = b.clamp(0, 255) as usize;
    i32::from(table[row][index])
}

/// `dc_q( b )`.
pub fn dc_q(bit_depth: u32, b: i32) -> i32 {
    lookup(&DC_QLOOKUP, bit_depth, b)
}

/// `ac_q( b )`.
pub fn ac_q(bit_depth: u32, b: i32) -> i32 {
    lookup(&AC_QLOOKUP, bit_depth, b)
}

/// Dequantize one coefficient.
pub fn dequantize(coefficient: i32, q: i32) -> i32 {
    coefficient * q
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_rows_start_as_the_specification_lists() {
        assert_eq!(dc_q(8, 0), 4);
        assert_eq!(dc_q(8, 1), 8);
        assert_eq!(dc_q(8, 2), 8);
        assert_eq!(ac_q(8, 0), 4);
        assert_eq!(ac_q(8, 1), 8);
        // Rows can coincide early (dc at index 0 is 4 for 8 and 10 bit) and
        // diverge immediately elsewhere (ac at index 1 is 8 for 8 bit, 13 for
        // 12 bit).
        assert_eq!(dc_q(10, 0), 4);
        assert_eq!(ac_q(12, 1), 13);
    }

    #[test]
    fn indices_are_clamped() {
        assert_eq!(dc_q(8, -1), dc_q(8, 0));
        assert_eq!(ac_q(8, 999), ac_q(8, 255));
    }

    #[test]
    fn dequantize_multiplies() {
        assert_eq!(dequantize(3, 11), 33);
        assert_eq!(dequantize(-4, 7), -28);
    }
}
