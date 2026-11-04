//! AV1 recursive filter-intra syntax and prediction.

use crate::reference::av1::cdf_defaults::{
    DEFAULT_FILTER_INTRA_CDF, DEFAULT_FILTER_INTRA_MODE_CDF,
};
use crate::reference::av1::cdf_state::{CdfState, CdfStateError};
use crate::reference::av1::intra::IntraMode;
use crate::reference::av1::partition::{block_size_index, BlockSize};
use crate::reference::av1::predict::IntraEdges;
use crate::reference::av1::symbol::SymbolDecoder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterIntraMode {
    Dc,
    V,
    H,
    D157,
    Paeth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterIntraInfo {
    pub mode: Option<FilterIntraMode>,
}

pub fn read_filter_intra(
    decoder: &mut SymbolDecoder<'_>,
    cdfs: &mut CdfState,
    size: BlockSize,
    y_mode: IntraMode,
    palette_size_y: usize,
    enabled: bool,
    disable_cdf_update: bool,
) -> Result<FilterIntraInfo, CdfStateError> {
    if !enabled
        || y_mode != IntraMode::Dc
        || palette_size_y != 0
        || size.width.max(size.height) > 32
    {
        return Ok(FilterIntraInfo { mode: None });
    }
    let use_filter = cdfs.read_symbol(
        decoder,
        &DEFAULT_FILTER_INTRA_CDF,
        block_size_index(size) * 3,
        3,
        disable_cdf_update,
    )? != 0;
    if !use_filter {
        return Ok(FilterIntraInfo { mode: None });
    }
    let symbol = cdfs.read_symbol(
        decoder,
        &DEFAULT_FILTER_INTRA_MODE_CDF,
        0,
        6,
        disable_cdf_update,
    )?;
    Ok(FilterIntraInfo {
        mode: Some(match symbol {
            0 => FilterIntraMode::Dc,
            1 => FilterIntraMode::V,
            2 => FilterIntraMode::H,
            3 => FilterIntraMode::D157,
            _ => FilterIntraMode::Paeth,
        }),
    })
}

pub fn filter_intra_direction(mode: FilterIntraMode) -> IntraMode {
    match mode {
        FilterIntraMode::Dc => IntraMode::Dc,
        FilterIntraMode::V => IntraMode::V,
        FilterIntraMode::H => IntraMode::H,
        FilterIntraMode::D157 => IntraMode::D157,
        FilterIntraMode::Paeth => IntraMode::Paeth,
    }
}

/// `Intra_Filter_Taps[INTRA_FILTER_MODES][8][7]`.
const TAPS: [[[i32; 7]; 8]; 5] = [
    [
        [-6, 10, 0, 0, 0, 12, 0],
        [-5, 2, 10, 0, 0, 9, 0],
        [-3, 1, 1, 10, 0, 7, 0],
        [-3, 1, 1, 2, 10, 5, 0],
        [-4, 6, 0, 0, 0, 2, 12],
        [-3, 2, 6, 0, 0, 2, 9],
        [-3, 2, 2, 6, 0, 2, 7],
        [-3, 1, 2, 2, 6, 3, 5],
    ],
    [
        [-10, 16, 0, 0, 0, 10, 0],
        [-6, 0, 16, 0, 0, 6, 0],
        [-4, 0, 0, 16, 0, 4, 0],
        [-2, 0, 0, 0, 16, 2, 0],
        [-10, 16, 0, 0, 0, 0, 10],
        [-6, 0, 16, 0, 0, 0, 6],
        [-4, 0, 0, 16, 0, 0, 4],
        [-2, 0, 0, 0, 16, 0, 2],
    ],
    [
        [-8, 8, 0, 0, 0, 16, 0],
        [-8, 0, 8, 0, 0, 16, 0],
        [-8, 0, 0, 8, 0, 16, 0],
        [-8, 0, 0, 0, 8, 16, 0],
        [-4, 4, 0, 0, 0, 0, 16],
        [-4, 0, 4, 0, 0, 0, 16],
        [-4, 0, 0, 4, 0, 0, 16],
        [-4, 0, 0, 0, 4, 0, 16],
    ],
    [
        [-2, 8, 0, 0, 0, 10, 0],
        [-1, 3, 8, 0, 0, 6, 0],
        [-1, 2, 3, 8, 0, 4, 0],
        [0, 1, 2, 3, 8, 2, 0],
        [-1, 4, 0, 0, 0, 3, 10],
        [-1, 3, 4, 0, 0, 4, 6],
        [-1, 2, 3, 4, 0, 4, 4],
        [-1, 2, 2, 3, 4, 3, 3],
    ],
    [
        [-12, 14, 0, 0, 0, 14, 0],
        [-10, 0, 14, 0, 0, 12, 0],
        [-9, 0, 0, 14, 0, 11, 0],
        [-8, 0, 0, 0, 14, 10, 0],
        [-10, 12, 0, 0, 0, 0, 14],
        [-9, 1, 12, 0, 0, 0, 12],
        [-8, 0, 0, 12, 0, 1, 11],
        [-7, 0, 0, 1, 12, 1, 9],
    ],
];

fn round2_signed(value: i32, n: u32) -> i32 {
    if value < 0 {
        -(((-value) + (1 << (n - 1))) >> n)
    } else {
        (value + (1 << (n - 1))) >> n
    }
}

/// `recursive intra prediction process`.
pub fn predict_filter_intra(
    mode: FilterIntraMode,
    edges: &IntraEdges,
    w: usize,
    h: usize,
    bit_depth: u32,
) -> Vec<u16> {
    assert!(w % 4 == 0 && h % 2 == 0);
    let max = (1i32 << bit_depth) - 1;
    let mut pred = vec![0u16; w * h];
    let w4 = w / 4;
    let h2 = h / 2;
    let taps = &TAPS[mode as usize];
    for i2 in 0..h2 {
        for j4 in 0..w4 {
            let mut p = [0i32; 7];
            for i in 0..7 {
                p[i] = if i < 5 {
                    if i2 == 0 {
                        if j4 * 4 + i == 0 {
                            i32::from(edges.top_left)
                        } else {
                            i32::from(edges.above[j4 * 4 + i - 1])
                        }
                    } else if j4 == 0 && i == 0 {
                        i32::from(edges.left[i2 * 2 - 1])
                    } else {
                        let row = i2 * 2 - 1;
                        let col = j4 * 4 + i - 1;
                        i32::from(pred[row * w + col])
                    }
                } else if j4 == 0 {
                    i32::from(edges.left[i2 * 2 + i - 5])
                } else {
                    let row = i2 * 2 + i - 5;
                    let col = j4 * 4 - 1;
                    i32::from(pred[row * w + col])
                };
            }
            for i1 in 0..2 {
                for j1 in 0..4 {
                    let kernel = &taps[(i1 << 2) + j1];
                    let sum: i32 = kernel.iter().zip(p).map(|(a, b)| a * b).sum();
                    pred[(i2 * 2 + i1) * w + j4 * 4 + j1] =
                        round2_signed(sum, 4).clamp(0, max) as u16;
                }
            }
        }
    }
    pred
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_intra_gating_and_symbols_are_persistent() {
        use crate::reference::av1::partition::BLOCK_SIZES;
        let mut cdfs = CdfState::new();
        let mut decoder = SymbolDecoder::init(&[0xffu8; 16]).unwrap();
        let info = read_filter_intra(
            &mut decoder,
            &mut cdfs,
            BLOCK_SIZES[6],
            IntraMode::Dc,
            0,
            true,
            false,
        )
        .unwrap();
        assert!(info.mode.is_some());
        let mut decoder = SymbolDecoder::init(&[0u8; 8]).unwrap();
        assert!(read_filter_intra(
            &mut decoder,
            &mut cdfs,
            BLOCK_SIZES[6],
            IntraMode::V,
            0,
            true,
            false,
        )
        .unwrap()
        .mode
        .is_none());
    }

    #[test]
    fn filter_intra_prediction_is_bounded_and_deterministic() {
        let edges = IntraEdges {
            above: vec![100, 110, 120, 130, 140, 150, 160, 170],
            left: vec![90, 80, 70, 60, 50, 40, 30, 20],
            top_left: 50,
            have_above: true,
            have_left: true,
            above_available: 4,
            left_available: 4,
        };
        for mode in [
            FilterIntraMode::Dc,
            FilterIntraMode::V,
            FilterIntraMode::H,
            FilterIntraMode::D157,
            FilterIntraMode::Paeth,
        ] {
            let a = predict_filter_intra(mode, &edges, 8, 8, 8);
            let b = predict_filter_intra(mode, &edges, 8, 8, 8);
            assert_eq!(a, b);
            assert!(a.iter().all(|v| *v <= 255));
        }
    }
}
