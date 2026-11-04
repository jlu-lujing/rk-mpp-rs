//! AV1 coefficient-context tables, generated from the published specification.
//!
//! `tools/gen_av1_coeff_tables.py`; `Coeff_Base_Pos_Ctx_Offset` has the
//! specification's SIG_COEF_CONTEXTS_2D (26) folded in as 26/31/36.
//!
//! FNV-1a 0xba59c65f642764e7 over the offsets, positions and reference deltas.

use crate::reference::av1::tx::TxSize;

/// `Adjusted_Tx_Size[ TX_SIZES_ALL ]`.
pub static ADJUSTED_TX_SIZE: [TxSize; 19] = [
    TxSize::Tx4x4,
    TxSize::Tx8x8,
    TxSize::Tx16x16,
    TxSize::Tx32x32,
    TxSize::Tx32x32,
    TxSize::Tx4x8,
    TxSize::Tx8x4,
    TxSize::Tx8x16,
    TxSize::Tx16x8,
    TxSize::Tx16x32,
    TxSize::Tx32x16,
    TxSize::Tx32x32,
    TxSize::Tx32x32,
    TxSize::Tx4x16,
    TxSize::Tx16x4,
    TxSize::Tx8x32,
    TxSize::Tx32x8,
    TxSize::Tx16x32,
    TxSize::Tx32x16,
];

/// `Coeff_Base_Ctx_Offset[ TX_SIZES_ALL ][ 5 ][ 5 ]` (flat).
pub static COEFF_BASE_CTX_OFFSET: [u8; 475] = [
    0, 1, 6, 6, 0, 1, 6, 6, 21, 0, 6, 6, 21, 21, 0, 6, 21, 21, 21, 0, 0, 0, 0, 0, 0, 0, 1, 6, 6,
    21, 1, 6, 6, 21, 21, 6, 6, 21, 21, 21, 6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 0, 1, 6, 6, 21,
    1, 6, 6, 21, 21, 6, 6, 21, 21, 21, 6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 0, 1, 6, 6, 21, 1, 6,
    6, 21, 21, 6, 6, 21, 21, 21, 6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 0, 1, 6, 6, 21, 1, 6, 6,
    21, 21, 6, 6, 21, 21, 21, 6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 0, 11, 11, 11, 0, 11, 11, 11,
    11, 0, 6, 6, 21, 21, 0, 6, 21, 21, 21, 0, 21, 21, 21, 21, 0, 0, 16, 6, 6, 21, 16, 16, 6, 21,
    21, 16, 16, 21, 21, 21, 16, 16, 21, 21, 21, 0, 0, 0, 0, 0, 0, 11, 11, 11, 11, 11, 11, 11, 11,
    11, 6, 6, 21, 21, 21, 6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 0, 16, 6, 6, 21, 16, 16, 6, 21,
    21, 16, 16, 21, 21, 21, 16, 16, 21, 21, 21, 16, 16, 21, 21, 21, 0, 11, 11, 11, 11, 11, 11, 11,
    11, 11, 6, 6, 21, 21, 21, 6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 0, 16, 6, 6, 21, 16, 16, 6,
    21, 21, 16, 16, 21, 21, 21, 16, 16, 21, 21, 21, 16, 16, 21, 21, 21, 0, 11, 11, 11, 11, 11, 11,
    11, 11, 11, 6, 6, 21, 21, 21, 6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 0, 16, 6, 6, 21, 16, 16,
    6, 21, 21, 16, 16, 21, 21, 21, 16, 16, 21, 21, 21, 16, 16, 21, 21, 21, 0, 11, 11, 11, 0, 11,
    11, 11, 11, 0, 6, 6, 21, 21, 0, 6, 21, 21, 21, 0, 21, 21, 21, 21, 0, 0, 16, 6, 6, 21, 16, 16,
    6, 21, 21, 16, 16, 21, 21, 21, 16, 16, 21, 21, 21, 0, 0, 0, 0, 0, 0, 11, 11, 11, 11, 11, 11,
    11, 11, 11, 6, 6, 21, 21, 21, 6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 0, 16, 6, 6, 21, 16, 16,
    6, 21, 21, 16, 16, 21, 21, 21, 16, 16, 21, 21, 21, 16, 16, 21, 21, 21, 0, 11, 11, 11, 11, 11,
    11, 11, 11, 11, 6, 6, 21, 21, 21, 6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 0, 16, 6, 6, 21, 16,
    16, 6, 21, 21, 16, 16, 21, 21, 21, 16, 16, 21, 21, 21, 16, 16, 21, 21, 21,
];

/// `Coeff_Base_Pos_Ctx_Offset[ 3 ]`.
pub static COEFF_BASE_POS_CTX_OFFSET: [u8; 3] = [26, 31, 36];

/// `Sig_Ref_Diff_Offset[ 3 ][ SIG_REF_DIFF_OFFSET_NUM ][ 2 ]` (flat).
pub static SIG_REF_DIFF_OFFSET: [i8; 30] = [
    0, 1, 1, 0, 1, 1, 0, 2, 2, 0, 0, 1, 1, 0, 0, 2, 0, 3, 0, 4, 0, 1, 1, 0, 2, 0, 3, 0, 4, 0,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extract_has_the_specification_shapes() {
        assert_eq!(ADJUSTED_TX_SIZE.len(), 19);
        assert_eq!(COEFF_BASE_CTX_OFFSET.len(), 19 * 5 * 5);
        assert_eq!(COEFF_BASE_POS_CTX_OFFSET, [26, 31, 36]);
        assert_eq!(SIG_REF_DIFF_OFFSET.len(), 3 * 5 * 2);
        assert_eq!(COEFF_BASE_CTX_OFFSET[..5], [0, 1, 6, 6, 0]);
        assert_eq!(ADJUSTED_TX_SIZE[4], TxSize::Tx32x32);
        assert_eq!(ADJUSTED_TX_SIZE[18], TxSize::Tx32x16);
    }
}
