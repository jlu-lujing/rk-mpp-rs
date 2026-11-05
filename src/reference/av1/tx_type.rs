//! AV1 transform-type sets and the intra transform-type symbol.

use crate::reference::av1::cdf_defaults::{
    DEFAULT_INTER_TX_TYPE_SET1_CDF, DEFAULT_INTER_TX_TYPE_SET2_CDF, DEFAULT_INTER_TX_TYPE_SET3_CDF,
    DEFAULT_INTRA_TX_TYPE_SET1_CDF, DEFAULT_INTRA_TX_TYPE_SET2_CDF,
};
use crate::reference::av1::cdf_state::{CdfState, CdfStateError};
use crate::reference::av1::coeff::{tx_sqr_index, tx_sqr_up_index, TxClass};
use crate::reference::av1::idct::TxKind;
use crate::reference::av1::intra::UvMode;
use crate::reference::av1::scan::ScanDirection;
use crate::reference::av1::symbol::SymbolDecoder;
use crate::reference::av1::tx::TxSize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxSet {
    DctOnly,
    Intra1,
    Intra2,
    Inter1,
    Inter2,
    Inter3,
}

/// `enum TxType`. The first component is the vertical transform (UD), the
/// second is the horizontal transform (LR).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxType {
    DctDct,
    AdstDct,
    DctAdst,
    AdstAdst,
    FlipadstDct,
    DctFlipadst,
    FlipadstFlipadst,
    AdstFlipadst,
    FlipadstAdst,
    Idtx,
    VDct,
    HDct,
    VAdst,
    HAdst,
    VFlipadst,
    HFlipadst,
}

const INTRA_SET1: [TxType; 7] = [
    TxType::Idtx,
    TxType::DctDct,
    TxType::VDct,
    TxType::HDct,
    TxType::AdstAdst,
    TxType::AdstDct,
    TxType::DctAdst,
];

const INTRA_SET2: [TxType; 5] = [
    TxType::Idtx,
    TxType::DctDct,
    TxType::AdstAdst,
    TxType::AdstDct,
    TxType::DctAdst,
];

const INTER_SET1: [TxType; 16] = [
    TxType::Idtx,
    TxType::VDct,
    TxType::HDct,
    TxType::VAdst,
    TxType::HAdst,
    TxType::VFlipadst,
    TxType::HFlipadst,
    TxType::DctDct,
    TxType::AdstDct,
    TxType::DctAdst,
    TxType::FlipadstDct,
    TxType::DctFlipadst,
    TxType::AdstAdst,
    TxType::FlipadstFlipadst,
    TxType::AdstFlipadst,
    TxType::FlipadstAdst,
];

const INTER_SET2: [TxType; 12] = [
    TxType::Idtx,
    TxType::VDct,
    TxType::HDct,
    TxType::DctDct,
    TxType::AdstDct,
    TxType::DctAdst,
    TxType::FlipadstDct,
    TxType::DctFlipadst,
    TxType::AdstAdst,
    TxType::FlipadstFlipadst,
    TxType::AdstFlipadst,
    TxType::FlipadstAdst,
];

const INTER_SET3: [TxType; 2] = [TxType::Idtx, TxType::DctDct];

impl TxType {
    pub fn row_kind(self) -> TxKind {
        match self {
            Self::DctDct | Self::AdstDct | Self::FlipadstDct | Self::HDct => TxKind::Dct,
            Self::DctAdst | Self::AdstAdst | Self::FlipadstAdst | Self::HAdst => TxKind::Adst,
            Self::DctFlipadst | Self::AdstFlipadst | Self::FlipadstFlipadst | Self::HFlipadst => {
                TxKind::FlipAdst
            }
            Self::VFlipadst => TxKind::FlipAdst,
            Self::Idtx | Self::VDct | Self::VAdst => TxKind::Identity,
        }
    }

    pub fn col_kind(self) -> TxKind {
        match self {
            Self::DctDct | Self::DctAdst | Self::DctFlipadst | Self::VDct => TxKind::Dct,
            Self::AdstDct | Self::AdstAdst | Self::AdstFlipadst | Self::VAdst => TxKind::Adst,
            Self::FlipadstDct | Self::FlipadstAdst | Self::FlipadstFlipadst => TxKind::FlipAdst,
            Self::VFlipadst => TxKind::FlipAdst,
            Self::Idtx | Self::HDct | Self::HAdst | Self::HFlipadst => TxKind::Identity,
        }
    }

    pub fn class(self) -> TxClass {
        match self {
            Self::VDct | Self::VAdst | Self::VFlipadst => TxClass::Vertical,
            Self::HDct | Self::HAdst | Self::HFlipadst => TxClass::Horizontal,
            _ => TxClass::TwoD,
        }
    }

    pub fn scan_direction(self) -> ScanDirection {
        match self.class() {
            TxClass::Vertical => ScanDirection::Row,
            TxClass::Horizontal => ScanDirection::Column,
            TxClass::TwoD => ScanDirection::Default,
        }
    }
}

/// `get_tx_set(txSz)`; the caller supplies `reduced_tx_set` and whether the
/// block is inter-coded.
pub fn tx_set(tx: TxSize, is_inter: bool, reduced_tx_set: bool) -> TxSet {
    let square = tx_sqr_index(tx);
    let square_up = tx_sqr_up_index(tx);
    if square_up > 3 {
        return TxSet::DctOnly;
    }
    if is_inter {
        if reduced_tx_set || square_up == 3 {
            TxSet::Inter3
        } else if square == 2 {
            TxSet::Inter2
        } else {
            TxSet::Inter1
        }
    } else if square_up == 3 {
        TxSet::DctOnly
    } else if reduced_tx_set || square == 2 {
        TxSet::Intra2
    } else {
        TxSet::Intra1
    }
}

/// `is_tx_type_in_set`.
pub fn is_tx_type_in_set(set: TxSet, tx_type: TxType, is_inter: bool) -> bool {
    if !is_inter {
        return match set {
            TxSet::Intra1 => INTRA_SET1.contains(&tx_type),
            TxSet::Intra2 => INTRA_SET2.contains(&tx_type),
            _ => tx_type == TxType::DctDct,
        };
    }
    match set {
        TxSet::Inter1 => [
            TxType::Idtx,
            TxType::VDct,
            TxType::HDct,
            TxType::VAdst,
            TxType::HAdst,
            TxType::VFlipadst,
            TxType::HFlipadst,
            TxType::DctDct,
            TxType::AdstDct,
            TxType::DctAdst,
            TxType::FlipadstDct,
            TxType::DctFlipadst,
            TxType::AdstAdst,
            TxType::FlipadstFlipadst,
            TxType::AdstFlipadst,
            TxType::FlipadstAdst,
        ]
        .contains(&tx_type),
        TxSet::Inter2 => [
            TxType::Idtx,
            TxType::VDct,
            TxType::HDct,
            TxType::DctDct,
            TxType::AdstDct,
            TxType::DctAdst,
            TxType::FlipadstDct,
            TxType::DctFlipadst,
            TxType::AdstAdst,
            TxType::FlipadstFlipadst,
            TxType::AdstFlipadst,
            TxType::FlipadstAdst,
        ]
        .contains(&tx_type),
        _ => matches!(tx_type, TxType::Idtx | TxType::DctDct),
    }
}

/// `Mode_To_Txfm[UVMode]`.
pub fn uv_mode_tx_type(mode: UvMode) -> TxType {
    match mode {
        UvMode::Dc | UvMode::D45 | UvMode::Cfl => TxType::DctDct,
        UvMode::V | UvMode::D113 | UvMode::D67 | UvMode::SmoothV => TxType::AdstDct,
        UvMode::H | UvMode::D157 | UvMode::D203 | UvMode::SmoothH => TxType::DctAdst,
        UvMode::D135 | UvMode::Smooth | UvMode::Paeth => TxType::AdstAdst,
    }
}

/// Decode `intra_tx_type` from a persistent tile CDF.
pub fn read_intra_tx_type_with_cdf(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    tx: TxSize,
    q_index: u8,
    intra_dir: usize,
    reduced_tx_set: bool,
    disable_cdf_update: bool,
) -> Result<TxType, CdfStateError> {
    let set = tx_set(tx, false, reduced_tx_set);
    if set == TxSet::DctOnly || q_index == 0 {
        return Ok(TxType::DctDct);
    }
    let (table, square, width, count) = match set {
        TxSet::Intra1 => (&DEFAULT_INTRA_TX_TYPE_SET1_CDF, tx_sqr_index(tx), 8, 7),
        TxSet::Intra2 => (
            &DEFAULT_INTRA_TX_TYPE_SET2_CDF,
            tx_sqr_index(tx).min(2),
            6,
            5,
        ),
        _ => return Ok(TxType::DctDct),
    };
    debug_assert!(intra_dir < 13);
    let start = (square * 13 + intra_dir) * width;
    let symbol = cdfs.read_symbol(decoder, table, start, width, disable_cdf_update)?;
    debug_assert!(symbol < count);
    Ok(if set == TxSet::Intra1 {
        INTRA_SET1[symbol]
    } else {
        INTRA_SET2[symbol]
    })
}

/// Decode the inter transform type from the tile's persistent CDF state.
pub fn read_inter_tx_type_with_cdf(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    tx: TxSize,
    q_index: u8,
    reduced_tx_set: bool,
    disable_cdf_update: bool,
) -> Result<TxType, CdfStateError> {
    let set = tx_set(tx, true, reduced_tx_set);
    if set == TxSet::DctOnly || q_index == 0 {
        return Ok(TxType::DctDct);
    }
    match set {
        TxSet::Inter1 => {
            let start = tx_sqr_index(tx) * 17;
            Ok(INTER_SET1[cdfs.read_symbol(
                decoder,
                &DEFAULT_INTER_TX_TYPE_SET1_CDF,
                start,
                17,
                disable_cdf_update,
            )?])
        }
        TxSet::Inter2 => Ok(INTER_SET2[cdfs.read_symbol(
            decoder,
            &DEFAULT_INTER_TX_TYPE_SET2_CDF,
            0,
            13,
            disable_cdf_update,
        )?]),
        TxSet::Inter3 => {
            let start = tx_sqr_index(tx) * 3;
            Ok(INTER_SET3[cdfs.read_symbol(
                decoder,
                &DEFAULT_INTER_TX_TYPE_SET3_CDF,
                start,
                3,
                disable_cdf_update,
            )?])
        }
        TxSet::DctOnly | TxSet::Intra1 | TxSet::Intra2 => Ok(TxType::DctDct),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_sets_follow_size_and_inter_flags() {
        assert_eq!(tx_set(TxSize::Tx64x64, false, false), TxSet::DctOnly);
        assert_eq!(tx_set(TxSize::Tx32x32, false, false), TxSet::DctOnly);
        assert_eq!(tx_set(TxSize::Tx16x16, false, false), TxSet::Intra2);
        assert_eq!(tx_set(TxSize::Tx8x8, false, false), TxSet::Intra1);
        assert_eq!(tx_set(TxSize::Tx8x8, false, true), TxSet::Intra2);
        assert_eq!(tx_set(TxSize::Tx16x16, true, false), TxSet::Inter2);
        assert_eq!(tx_set(TxSize::Tx8x8, true, false), TxSet::Inter1);
        assert_eq!(tx_set(TxSize::Tx32x32, true, false), TxSet::Inter3);
    }

    #[test]
    fn inverse_tables_map_to_axis_kinds_and_scan_classes() {
        assert_eq!(
            (TxType::AdstDct.row_kind(), TxType::AdstDct.col_kind()),
            (TxKind::Dct, TxKind::Adst)
        );
        assert_eq!(
            (TxType::VDct.row_kind(), TxType::VDct.col_kind()),
            (TxKind::Identity, TxKind::Dct)
        );
        assert_eq!(TxType::VDct.class(), TxClass::Vertical);
        assert_eq!(TxType::HDct.class(), TxClass::Horizontal);
        assert_eq!(TxType::DctAdst.class(), TxClass::TwoD);
        assert_eq!(TxType::VDct.scan_direction(), ScanDirection::Row);
        assert_eq!(TxType::HDct.scan_direction(), ScanDirection::Column);
        assert_eq!(TxType::DctAdst.scan_direction(), ScanDirection::Default);
    }

    #[test]
    fn intra_symbols_are_members_of_their_sets() {
        for (set, table) in [
            (TxSet::Intra1, &INTRA_SET1[..]),
            (TxSet::Intra2, &INTRA_SET2[..]),
        ] {
            for tx_type in table {
                assert!(is_tx_type_in_set(set, *tx_type, false));
            }
        }
        assert!(!is_tx_type_in_set(TxSet::Intra2, TxType::VDct, false));
        assert!(matches!(TxType::DctDct.row_kind(), TxKind::Dct));
    }
}
