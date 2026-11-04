//! AV1 coefficient scan selection (`get_scan`).
//!
//! AV1 has no scan larger than 32x32: `TX_16X64` uses the 16x32 scan,
//! `TX_64X16` the 32x16 one, and any transform whose square-up is 64x64
//! (`TX_64X64`, `TX_32X64`, `TX_64X32`) uses the 32x32 scan. The remaining
//! choice is direction: identity transforms use the default scan, vertical
//! transforms the row-major one and horizontal transforms the column-major one.

use crate::reference::av1::scan_defaults::{
    ScanTable, DEFAULT_SCAN_16X16, DEFAULT_SCAN_16X32, DEFAULT_SCAN_16X4, DEFAULT_SCAN_16X8,
    DEFAULT_SCAN_32X16, DEFAULT_SCAN_32X32, DEFAULT_SCAN_32X8, DEFAULT_SCAN_4X16, DEFAULT_SCAN_4X4,
    DEFAULT_SCAN_4X8, DEFAULT_SCAN_8X16, DEFAULT_SCAN_8X32, DEFAULT_SCAN_8X4, DEFAULT_SCAN_8X8,
    MCOL_SCAN_16X16, MCOL_SCAN_16X4, MCOL_SCAN_16X8, MCOL_SCAN_4X16, MCOL_SCAN_4X4, MCOL_SCAN_4X8,
    MCOL_SCAN_8X16, MCOL_SCAN_8X4, MCOL_SCAN_8X8, MROW_SCAN_16X16, MROW_SCAN_16X4, MROW_SCAN_16X8,
    MROW_SCAN_4X16, MROW_SCAN_4X4, MROW_SCAN_4X8, MROW_SCAN_8X16, MROW_SCAN_8X4, MROW_SCAN_8X8,
};
use crate::reference::av1::tx::TxSize;

/// Which family `get_scan` should pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanDirection {
    Default,
    Row,
    Column,
}

fn default_scan(tx: TxSize) -> &'static ScanTable {
    match tx {
        TxSize::Tx4x4 => &DEFAULT_SCAN_4X4,
        TxSize::Tx4x8 => &DEFAULT_SCAN_4X8,
        TxSize::Tx8x4 => &DEFAULT_SCAN_8X4,
        TxSize::Tx8x8 => &DEFAULT_SCAN_8X8,
        TxSize::Tx8x16 => &DEFAULT_SCAN_8X16,
        TxSize::Tx16x8 => &DEFAULT_SCAN_16X8,
        TxSize::Tx16x16 => &DEFAULT_SCAN_16X16,
        TxSize::Tx16x32 => &DEFAULT_SCAN_16X32,
        TxSize::Tx32x16 => &DEFAULT_SCAN_32X16,
        TxSize::Tx4x16 => &DEFAULT_SCAN_4X16,
        TxSize::Tx16x4 => &DEFAULT_SCAN_16X4,
        TxSize::Tx8x32 => &DEFAULT_SCAN_8X32,
        TxSize::Tx32x8 => &DEFAULT_SCAN_32X8,
        // 64-wide transforms fall back to the 32x32 scan.
        TxSize::Tx32x32 | TxSize::Tx64x64 | TxSize::Tx32x64 | TxSize::Tx64x32 => {
            &DEFAULT_SCAN_32X32
        }
        TxSize::Tx16x64 => &DEFAULT_SCAN_16X32,
        TxSize::Tx64x16 => &DEFAULT_SCAN_32X16,
    }
}

fn row_scan(tx: TxSize) -> &'static ScanTable {
    match tx {
        TxSize::Tx4x4 => &MROW_SCAN_4X4,
        TxSize::Tx4x8 => &MROW_SCAN_4X8,
        TxSize::Tx8x4 => &MROW_SCAN_8X4,
        TxSize::Tx8x8 => &MROW_SCAN_8X8,
        TxSize::Tx8x16 => &MROW_SCAN_8X16,
        TxSize::Tx16x8 => &MROW_SCAN_16X8,
        TxSize::Tx16x16 => &MROW_SCAN_16X16,
        TxSize::Tx4x16 => &MROW_SCAN_4X16,
        TxSize::Tx16x4 => &MROW_SCAN_16X4,
        _ => default_scan(tx),
    }
}

fn column_scan(tx: TxSize) -> &'static ScanTable {
    match tx {
        TxSize::Tx4x4 => &MCOL_SCAN_4X4,
        TxSize::Tx4x8 => &MCOL_SCAN_4X8,
        TxSize::Tx8x4 => &MCOL_SCAN_8X4,
        TxSize::Tx8x8 => &MCOL_SCAN_8X8,
        TxSize::Tx8x16 => &MCOL_SCAN_8X16,
        TxSize::Tx16x8 => &MCOL_SCAN_16X8,
        TxSize::Tx16x16 => &MCOL_SCAN_16X16,
        TxSize::Tx4x16 => &MCOL_SCAN_4X16,
        TxSize::Tx16x4 => &MCOL_SCAN_16X4,
        _ => default_scan(tx),
    }
}

/// `get_scan( txSz )` with the direction already chosen from `PlaneTxType`.
pub fn get_scan(tx: TxSize, direction: ScanDirection) -> &'static ScanTable {
    match direction {
        ScanDirection::Default => default_scan(tx),
        ScanDirection::Row => row_scan(tx),
        ScanDirection::Column => column_scan(tx),
    }
}

/// Convert a specification scan position to the row-major coefficient buffer
/// layout used by the decoder. The specification tables index \((row, col)\),
/// while libaom's transform buffers and context functions advance by columns.
pub fn scan_position(tx: TxSize, position: u16) -> u16 {
    let width = tx.width().min(32);
    let height = tx.height().min(32);
    let position = u32::from(position);
    let row = position / width;
    let col = position - row * width;
    (col * height + row) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_scans_follow_the_direction() {
        assert_eq!(
            get_scan(TxSize::Tx4x4, ScanDirection::Default).name,
            "Default_Scan_4x4"
        );
        assert_eq!(
            get_scan(TxSize::Tx4x4, ScanDirection::Row).name,
            "Mrow_Scan_4x4"
        );
        assert_eq!(
            get_scan(TxSize::Tx4x4, ScanDirection::Column).name,
            "Mcol_Scan_4x4"
        );
        assert_eq!(
            get_scan(TxSize::Tx16x16, ScanDirection::Row).name,
            "Mrow_Scan_16x16"
        );
    }

    #[test]
    fn wide_transforms_fall_back_to_the_32_scan() {
        assert_eq!(
            get_scan(TxSize::Tx64x64, ScanDirection::Default).name,
            "Default_Scan_32x32"
        );
        assert_eq!(
            get_scan(TxSize::Tx32x64, ScanDirection::Default).name,
            "Default_Scan_32x32"
        );
        assert_eq!(
            get_scan(TxSize::Tx64x32, ScanDirection::Default).name,
            "Default_Scan_32x32"
        );
        assert_eq!(
            get_scan(TxSize::Tx16x64, ScanDirection::Default).name,
            "Default_Scan_16x32"
        );
        assert_eq!(
            get_scan(TxSize::Tx64x16, ScanDirection::Default).name,
            "Default_Scan_32x16"
        );
    }

    #[test]
    fn every_transform_has_a_scan_with_the_right_length() {
        for tx in [
            TxSize::Tx4x4,
            TxSize::Tx8x8,
            TxSize::Tx16x16,
            TxSize::Tx32x32,
            TxSize::Tx64x64,
            TxSize::Tx4x8,
            TxSize::Tx8x4,
            TxSize::Tx8x16,
            TxSize::Tx16x8,
            TxSize::Tx16x32,
            TxSize::Tx32x16,
            TxSize::Tx32x64,
            TxSize::Tx64x32,
            TxSize::Tx4x16,
            TxSize::Tx16x4,
            TxSize::Tx8x32,
            TxSize::Tx32x8,
            TxSize::Tx16x64,
            TxSize::Tx64x16,
        ] {
            let (w, h) = tx.dimensions();
            let expected = (w.min(32) * h.min(32)) as usize;
            assert_eq!(
                get_scan(tx, ScanDirection::Default).order.len(),
                expected,
                "{tx:?}"
            );
        }
    }

    #[test]
    fn scan_positions_use_the_transposed_coefficient_layout() {
        let scan = get_scan(TxSize::Tx8x8, ScanDirection::Default);
        let positions: Vec<u16> = scan
            .order
            .iter()
            .copied()
            .map(|position| scan_position(TxSize::Tx8x8, position))
            .collect();
        assert_eq!(&positions[..9], &[0, 8, 1, 2, 9, 16, 24, 17, 10]);
    }
}
