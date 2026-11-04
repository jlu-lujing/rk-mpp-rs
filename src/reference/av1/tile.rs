//! AV1 tile traversal: the geometry half of `decode_partition`.
//!
//! The symbol decoding lives in `partition.rs`; this module walks the
//! superblock grid the way the specification's recursive `decode_partition`
//! does and reports every decoded block position/size. The partition decision
//! is a callback, so the traversal can be checked for exact, non-overlapping
//! coverage without an entropy-coded tile.

use crate::reference::av1::partition::{BlockSize, PartitionType, BLOCK_SIZES};

/// The block a partition produces at this level, or `None` when the size
/// cannot carry that partition (e.g. a 4x4 HORZ).
pub fn subsize(partition: PartitionType, size: BlockSize) -> Option<BlockSize> {
    if size.width != size.height {
        return None;
    }
    let (w, h) = match partition {
        PartitionType::None => (size.width, size.height),
        PartitionType::Horz | PartitionType::HorzA | PartitionType::HorzB => {
            (size.width, size.height / 2)
        }
        PartitionType::Vert | PartitionType::VertA | PartitionType::VertB => {
            (size.width / 2, size.height)
        }
        PartitionType::Split => (size.width / 2, size.height / 2),
        PartitionType::Horz4 => (size.width, size.height / 4),
        PartitionType::Vert4 => (size.width / 4, size.height),
    };
    BLOCK_SIZES
        .iter()
        .copied()
        .find(|entry| entry.width == w && entry.height == h)
}

/// A decoded block: its top-left 4x4 position and size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedBlock {
    pub row: usize,
    pub col: usize,
    pub size: BlockSize,
}

/// Walk every superblock and report the blocks `decode_partition` would visit.
///
/// `choose` receives `(row, col, size, above, left)` and returns the partition
/// for that level; `above`/`left` are `None` when the neighbouring position is
/// outside the picture.
pub fn walk_partitions<F>(
    mi_cols: usize,
    mi_rows: usize,
    sb_size: u32,
    mut choose: F,
) -> Vec<DecodedBlock>
where
    F: FnMut(usize, usize, BlockSize, Option<BlockSize>, Option<BlockSize>) -> PartitionType,
{
    let sb = BLOCK_SIZES
        .iter()
        .copied()
        .find(|size| size.width == sb_size && size.height == sb_size)
        .expect("superblock size");
    let sb_mi = (sb_size / 4) as usize;
    let mut blocks = Vec::new();
    let mut row = 0;
    while row < mi_rows {
        let mut col = 0;
        while col < mi_cols {
            visit(row, col, sb, mi_cols, mi_rows, &mut choose, &mut blocks);
            col += sb_mi;
        }
        row += sb_mi;
    }
    blocks
}

fn visit<F>(
    row: usize,
    col: usize,
    size: BlockSize,
    mi_cols: usize,
    mi_rows: usize,
    choose: &mut F,
    blocks: &mut Vec<DecodedBlock>,
) where
    F: FnMut(usize, usize, BlockSize, Option<BlockSize>, Option<BlockSize>) -> PartitionType,
{
    if row >= mi_rows || col >= mi_cols {
        return;
    }
    let half = (size.num_4x4_wide() / 2) as usize;
    let has_rows = row + half < mi_rows;
    let has_cols = col + half < mi_cols;
    let partition = if size.width == 4 {
        PartitionType::None
    } else {
        let above = (row > 0).then_some(size);
        let left = (col > 0).then_some(size);
        choose(row, col, size, above, left)
    };
    let Some(sub) = subsize(partition, size) else {
        blocks.push(DecodedBlock { row, col, size });
        return;
    };
    let split = subsize(PartitionType::Split, size).unwrap_or(sub);
    let block = |r: usize, c: usize, s: BlockSize, blocks: &mut Vec<DecodedBlock>| {
        if r < mi_rows && c < mi_cols {
            blocks.push(DecodedBlock {
                row: r,
                col: c,
                size: s,
            });
        }
    };
    match partition {
        PartitionType::None => block(row, col, sub, blocks),
        PartitionType::Horz => {
            block(row, col, sub, blocks);
            if has_rows {
                block(row + half, col, sub, blocks);
            }
        }
        PartitionType::Vert => {
            block(row, col, sub, blocks);
            if has_cols {
                block(row, col + half, sub, blocks);
            }
        }
        PartitionType::Split => {
            visit(row, col, sub, mi_cols, mi_rows, choose, blocks);
            if has_cols {
                visit(row, col + half, sub, mi_cols, mi_rows, choose, blocks);
            }
            if has_rows {
                visit(row + half, col, sub, mi_cols, mi_rows, choose, blocks);
                if has_cols {
                    visit(
                        row + half,
                        col + half,
                        sub,
                        mi_cols,
                        mi_rows,
                        choose,
                        blocks,
                    );
                }
            }
        }
        PartitionType::HorzA => {
            block(row, col, split, blocks);
            block(row, col + half, split, blocks);
            block(row + half, col, sub, blocks);
        }
        PartitionType::HorzB => {
            block(row, col, sub, blocks);
            block(row + half, col, split, blocks);
            block(row + half, col + half, split, blocks);
        }
        PartitionType::VertA => {
            block(row, col, split, blocks);
            block(row + half, col, split, blocks);
            block(row, col + half, sub, blocks);
        }
        PartitionType::VertB => {
            block(row, col, sub, blocks);
            block(row, col + half, split, blocks);
            block(row + half, col + half, split, blocks);
        }
        PartitionType::Horz4 => {
            let quarter = (size.num_4x4_high() / 4) as usize;
            for i in 0..4 {
                block(row + i * quarter, col, sub, blocks);
            }
        }
        PartitionType::Vert4 => {
            let quarter = (size.num_4x4_wide() / 4) as usize;
            for i in 0..4 {
                block(row, col + i * quarter, sub, blocks);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coverage(blocks: &[DecodedBlock], mi_cols: usize, mi_rows: usize) -> Vec<usize> {
        let mut count = vec![0usize; mi_cols * mi_rows];
        for block in blocks {
            let w = (block.size.width / 4) as usize;
            let h = (block.size.height / 4) as usize;
            for r in block.row..block.row + h {
                for c in block.col..block.col + w {
                    assert!(r < mi_rows && c < mi_cols, "{block:?} out of bounds");
                    count[r * mi_cols + c] += 1;
                }
            }
        }
        count
    }

    #[test]
    fn all_none_covers_each_superblock_once() {
        let blocks = walk_partitions(32, 16, 64, |_, _, _, _, _| PartitionType::None);
        let count = coverage(&blocks, 32, 16);
        assert!(count.iter().all(|v| *v == 1), "{count:?}");
        assert!(blocks.iter().all(|b| b.size.width == 64));
    }

    #[test]
    fn all_split_covers_each_4x4_once() {
        let blocks = walk_partitions(32, 16, 64, |_, _, _, _, _| PartitionType::Split);
        let count = coverage(&blocks, 32, 16);
        assert!(count.iter().all(|v| *v == 1), "{count:?}");
        assert_eq!(blocks.len(), 32 * 16);
        assert!(blocks
            .iter()
            .all(|b| b.size.width == 4 && b.size.height == 4));
    }

    #[test]
    fn mixed_partitions_still_tile_the_picture() {
        let blocks = walk_partitions(48, 48, 64, |row, col, size, _, _| {
            if size.width == 128 {
                PartitionType::Split
            } else if (row + col) % 2 == 0 {
                PartitionType::Horz
            } else {
                PartitionType::Vert
            }
        });
        let count = coverage(&blocks, 48, 48);
        assert!(count.iter().all(|v| *v == 1), "overlap or hole");
    }

    #[test]
    fn subsize_matches_the_square_partition_table() {
        let size = BLOCK_SIZES[6]; // BLOCK_16X16
        assert_eq!(
            subsize(PartitionType::None, size).unwrap().name,
            "BLOCK_16X16"
        );
        assert_eq!(
            subsize(PartitionType::Horz, size).unwrap().name,
            "BLOCK_16X8"
        );
        assert_eq!(
            subsize(PartitionType::Vert, size).unwrap().name,
            "BLOCK_8X16"
        );
        assert_eq!(
            subsize(PartitionType::Split, size).unwrap().name,
            "BLOCK_8X8"
        );
        assert_eq!(
            subsize(PartitionType::Horz4, size).unwrap().name,
            "BLOCK_16X4"
        );
        assert_eq!(
            subsize(PartitionType::Vert4, size).unwrap().name,
            "BLOCK_4X16"
        );
        assert!(subsize(PartitionType::Horz, BLOCK_SIZES[0]).is_none());
    }
}
