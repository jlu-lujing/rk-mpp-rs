//! AV1 residual layout: per-plane subsampled sizes and transform block order.
//!
//! The coefficient syntax itself lives in `coeff.rs`; this module supplies the
//! deterministic geometry before it is called. Intra blocks walk every plane in
//! raster order, with one transform block per `stepX`/`stepY` cell. Blocks
//! larger than 64x64 are split into 64x64 residual chunks first.

use crate::reference::av1::partition::{BlockSize, BLOCK_SIZES};
use crate::reference::av1::tx::{max_tx_size_rect, TxSize};

/// `Subsampled_Size[BLOCK_SIZES][2][2]`, encoded as `BLOCK_SIZES` indices.
const SUBSAMPLED_SIZE: [[[Option<u8>; 2]; 2]; 22] = [
    [[Some(0), Some(0)], [Some(0), Some(0)]],
    [[Some(1), Some(0)], [None, Some(0)]],
    [[Some(2), None], [Some(0), Some(0)]],
    [[Some(3), Some(2)], [Some(1), Some(0)]],
    [[Some(4), Some(3)], [None, Some(1)]],
    [[Some(5), None], [Some(3), Some(2)]],
    [[Some(6), Some(5)], [Some(4), Some(3)]],
    [[Some(7), Some(6)], [None, Some(4)]],
    [[Some(8), None], [Some(6), Some(5)]],
    [[Some(9), Some(8)], [Some(7), Some(6)]],
    [[Some(10), Some(9)], [None, Some(7)]],
    [[Some(11), None], [Some(9), Some(8)]],
    [[Some(12), Some(11)], [Some(10), Some(9)]],
    [[Some(13), Some(12)], [None, Some(10)]],
    [[Some(14), None], [Some(12), Some(11)]],
    [[Some(15), Some(14)], [Some(13), Some(12)]],
    [[Some(16), Some(1)], [None, Some(1)]],
    [[Some(17), None], [Some(2), Some(2)]],
    [[Some(18), Some(4)], [None, Some(16)]],
    [[Some(19), None], [Some(5), Some(17)]],
    [[Some(20), Some(7)], [None, Some(18)]],
    [[Some(21), None], [Some(8), Some(19)]],
];

fn size_index(size: BlockSize) -> usize {
    BLOCK_SIZES
        .iter()
        .position(|entry| *entry == size)
        .expect("block size is one of BLOCK_SIZES")
}

/// `get_plane_residual_size(subSize, plane)`, returning `None` for the
/// specification's `BLOCK_INVALID` cells.
pub fn residual_size(size: BlockSize, sub_x: u32, sub_y: u32) -> Option<BlockSize> {
    let index = SUBSAMPLED_SIZE[size_index(size)][(sub_x & 1) as usize][(sub_y & 1) as usize];
    index.map(|index| BLOCK_SIZES[index as usize])
}

/// `get_tx_size(plane, txSz)`. Chroma uses the residual block's maximum
/// transform, capped and converted to the 32x64/64x32/32x32 forms.
pub fn plane_tx_size(
    size: BlockSize,
    plane: usize,
    tx: TxSize,
    sub_x: u32,
    sub_y: u32,
) -> Option<TxSize> {
    if plane == 0 {
        return Some(tx);
    }
    let uv = max_tx_size_rect(residual_size(size, sub_x, sub_y)?);
    if uv.width() == 64 || uv.height() == 64 {
        if uv.width() == 16 {
            Some(TxSize::Tx16x32)
        } else if uv.height() == 16 {
            Some(TxSize::Tx32x16)
        } else {
            Some(TxSize::Tx32x32)
        }
    } else {
        Some(uv)
    }
}

/// One transform block to feed to prediction, coefficient decoding and
/// reconstruction. `x`/`y` are plane sample coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResidualTxBlock {
    pub plane: usize,
    pub x: usize,
    pub y: usize,
    pub tx: TxSize,
}

/// Enumerate the transform blocks of an intra-coded block in the specification's
/// chunk -> plane -> raster order.
#[allow(clippy::too_many_arguments)]
pub fn intra_residual_blocks(
    mi_row: usize,
    mi_col: usize,
    size: BlockSize,
    tx: TxSize,
    has_chroma: bool,
    lossless: bool,
    sub_x: u32,
    sub_y: u32,
    mi_cols: usize,
    mi_rows: usize,
) -> Vec<ResidualTxBlock> {
    let width_chunks = usize::max(1, size.width as usize / 64);
    let height_chunks = usize::max(1, size.height as usize / 64);
    let chunk_size = if width_chunks > 1 || height_chunks > 1 {
        BLOCK_SIZES[12]
    } else {
        size
    };
    let planes = if has_chroma { 3 } else { 1 };
    let mut out = Vec::new();
    for chunk_y in 0..height_chunks {
        for chunk_x in 0..width_chunks {
            for plane in 0..planes {
                let (sx, sy) = if plane == 0 { (0, 0) } else { (sub_x, sub_y) };
                let Some(plane_size) = residual_size(chunk_size, sx, sy) else {
                    continue;
                };
                let Some(tx_size) = plane_tx_size(chunk_size, plane, tx, sx, sy) else {
                    continue;
                };
                let tx_size = if lossless { TxSize::Tx4x4 } else { tx_size };
                let step_x = (tx_size.width() / 4) as usize;
                let step_y = (tx_size.height() / 4) as usize;
                let plane_w4 = (plane_size.width / 4) as usize;
                let plane_h4 = (plane_size.height / 4) as usize;
                let base_x = (mi_col >> sx) * 4;
                let base_y = (mi_row >> sy) * 4;
                let max_x = (mi_cols * 4) >> sx;
                let max_y = (mi_rows * 4) >> sy;
                let chunk_shift_x = (chunk_x << 4) >> sx;
                let chunk_shift_y = (chunk_y << 4) >> sy;
                for y in (0..plane_h4).step_by(step_y) {
                    for x in (0..plane_w4).step_by(step_x) {
                        let block_x = base_x + 4 * (x + chunk_shift_x);
                        let block_y = base_y + 4 * (y + chunk_shift_y);
                        if block_x >= max_x || block_y >= max_y {
                            continue;
                        }
                        out.push(ResidualTxBlock {
                            plane,
                            x: block_x,
                            y: block_y,
                            tx: tx_size,
                        });
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> BlockSize {
        *BLOCK_SIZES.iter().find(|entry| entry.name == name).unwrap()
    }

    #[test]
    fn subsampled_size_matches_the_spec_table_edges() {
        assert_eq!(residual_size(named("BLOCK_4X8"), 1, 0), None);
        assert_eq!(
            residual_size(named("BLOCK_4X8"), 1, 1),
            Some(named("BLOCK_4X4"))
        );
        assert_eq!(residual_size(named("BLOCK_8X16"), 1, 0), None);
        assert_eq!(
            residual_size(named("BLOCK_16X64"), 1, 1),
            Some(named("BLOCK_8X32"))
        );
    }

    #[test]
    fn chroma_tx_size_caps_64_tap_rectangles() {
        assert_eq!(
            plane_tx_size(named("BLOCK_128X128"), 1, TxSize::Tx64x64, 1, 1),
            Some(TxSize::Tx32x32)
        );
        assert_eq!(
            plane_tx_size(named("BLOCK_16X64"), 1, TxSize::Tx16x64, 1, 1),
            Some(TxSize::Tx8x32)
        );
        assert_eq!(
            plane_tx_size(named("BLOCK_64X16"), 1, TxSize::Tx64x16, 1, 1),
            Some(TxSize::Tx32x8)
        );
    }

    #[test]
    fn residual_layout_walks_luma_then_chroma() {
        let blocks = intra_residual_blocks(
            0,
            0,
            named("BLOCK_64X64"),
            TxSize::Tx32x32,
            true,
            false,
            1,
            1,
            16,
            16,
        );
        assert_eq!(
            blocks
                .iter()
                .map(|b| (b.plane, b.x, b.y, b.tx))
                .collect::<Vec<_>>(),
            [
                (0, 0, 0, TxSize::Tx32x32),
                (0, 32, 0, TxSize::Tx32x32),
                (0, 0, 32, TxSize::Tx32x32),
                (0, 32, 32, TxSize::Tx32x32),
                (1, 0, 0, TxSize::Tx32x32),
                (2, 0, 0, TxSize::Tx32x32),
            ]
        );
    }

    #[test]
    fn blocks_larger_than_64_are_emitted_as_chunks() {
        let blocks = intra_residual_blocks(
            0,
            0,
            named("BLOCK_128X128"),
            TxSize::Tx64x64,
            false,
            false,
            1,
            1,
            32,
            32,
        );
        eprintln!("{blocks:?}");
        assert_eq!(blocks.len(), 4);
        assert_eq!(
            blocks.iter().map(|b| (b.x, b.y)).collect::<Vec<_>>(),
            [(0, 0), (64, 0), (0, 64), (64, 64)]
        );
    }
}
