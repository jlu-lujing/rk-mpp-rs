//! Smooth-intra weight tables from the AV1 specification.
//!
//! Generated from the specification's `Sm_Weights_Tx_*` arrays (124 weights,
//! FNV-1a 0x255be4e0964b9b30).

pub static SM_WEIGHTS_TX_4X4: [u8; 4] = [255, 149, 85, 64];

pub static SM_WEIGHTS_TX_8X8: [u8; 8] = [255, 197, 146, 105, 73, 50, 37, 32];

pub static SM_WEIGHTS_TX_16X16: [u8; 16] = [
    255, 225, 196, 170, 145, 123, 102, 84, 68, 54, 43, 33, 26, 20, 17, 16,
];

pub static SM_WEIGHTS_TX_32X32: [u8; 32] = [
    255, 240, 225, 210, 196, 182, 169, 157, 145, 133, 122, 111, 101, 92, 83, 74, 66, 59, 52, 45,
    39, 34, 29, 25, 21, 17, 14, 12, 10, 9, 8, 8,
];

pub static SM_WEIGHTS_TX_64X64: [u8; 64] = [
    255, 248, 240, 233, 225, 218, 210, 203, 196, 189, 182, 176, 169, 163, 156, 150, 144, 138, 133,
    127, 121, 116, 111, 106, 101, 96, 91, 86, 82, 77, 73, 69, 65, 61, 57, 54, 50, 47, 44, 41, 38,
    35, 32, 29, 27, 25, 22, 20, 18, 16, 15, 13, 12, 10, 9, 8, 7, 6, 6, 5, 5, 4, 4, 4,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_start_and_end_where_the_specification_does() {
        assert_eq!(SM_WEIGHTS_TX_4X4, [255, 149, 85, 64]);
        assert_eq!(SM_WEIGHTS_TX_8X8[..3], [255, 197, 146]);
        assert_eq!(SM_WEIGHTS_TX_64X64[0], 255);
        assert_eq!(SM_WEIGHTS_TX_64X64[63], 4);
    }
}
