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

/// Source of partition decisions plus the block-size grid they build.
///
/// The grid lets a symbol-driven source derive the above/left contexts the way
/// `decode_partition` does; the constant source in the tests ignores it.
pub trait PartitionSource {
    fn choose(
        &mut self,
        row: usize,
        col: usize,
        size: BlockSize,
        above: Option<BlockSize>,
        left: Option<BlockSize>,
    ) -> PartitionType;
    fn record(&mut self, block: DecodedBlock);
}

/// A source that always returns the same partition.
pub struct ConstantPartition(pub PartitionType);

impl PartitionSource for ConstantPartition {
    fn choose(
        &mut self,
        _row: usize,
        _col: usize,
        _size: BlockSize,
        _above: Option<BlockSize>,
        _left: Option<BlockSize>,
    ) -> PartitionType {
        self.0
    }

    fn record(&mut self, _block: DecodedBlock) {}
}

/// Walk every superblock and report the blocks `decode_partition` would visit.
pub fn walk_partitions<S: PartitionSource>(
    mi_cols: usize,
    mi_rows: usize,
    sb_size: u32,
    source: &mut S,
) -> Vec<DecodedBlock> {
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
            visit(row, col, sb, mi_cols, mi_rows, source, &mut blocks);
            col += sb_mi;
        }
        row += sb_mi;
    }
    blocks
}

fn push_block<S: PartitionSource>(
    source: &mut S,
    blocks: &mut Vec<DecodedBlock>,
    row: usize,
    col: usize,
    size: BlockSize,
) {
    let block = DecodedBlock { row, col, size };
    blocks.push(block);
    source.record(block);
}

fn visit<S: PartitionSource>(
    row: usize,
    col: usize,
    size: BlockSize,
    mi_cols: usize,
    mi_rows: usize,
    source: &mut S,
    blocks: &mut Vec<DecodedBlock>,
) {
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
        source.choose(row, col, size, above, left)
    };
    let Some(sub) = subsize(partition, size) else {
        push_block(source, blocks, row, col, size);
        return;
    };
    let split = subsize(PartitionType::Split, size).unwrap_or(sub);
    match partition {
        PartitionType::None => push_block(source, blocks, row, col, sub),
        PartitionType::Horz => {
            push_block(source, blocks, row, col, sub);
            if has_rows {
                push_block(source, blocks, row + half, col, sub);
            }
        }
        PartitionType::Vert => {
            push_block(source, blocks, row, col, sub);
            if has_cols {
                push_block(source, blocks, row, col + half, sub);
            }
        }
        PartitionType::Split => {
            visit(row, col, sub, mi_cols, mi_rows, source, blocks);
            if has_cols {
                visit(row, col + half, sub, mi_cols, mi_rows, source, blocks);
            }
            if has_rows {
                visit(row + half, col, sub, mi_cols, mi_rows, source, blocks);
                if has_cols {
                    visit(
                        row + half,
                        col + half,
                        sub,
                        mi_cols,
                        mi_rows,
                        source,
                        blocks,
                    );
                }
            }
        }
        PartitionType::HorzA => {
            push_block(source, blocks, row, col, split);
            push_block(source, blocks, row, col + half, split);
            push_block(source, blocks, row + half, col, sub);
        }
        PartitionType::HorzB => {
            push_block(source, blocks, row, col, sub);
            push_block(source, blocks, row + half, col, split);
            push_block(source, blocks, row + half, col + half, split);
        }
        PartitionType::VertA => {
            push_block(source, blocks, row, col, split);
            push_block(source, blocks, row + half, col, split);
            push_block(source, blocks, row, col + half, sub);
        }
        PartitionType::VertB => {
            push_block(source, blocks, row, col, sub);
            push_block(source, blocks, row, col + half, split);
            push_block(source, blocks, row + half, col + half, split);
        }
        PartitionType::Horz4 => {
            let quarter = (size.num_4x4_high() / 4) as usize;
            for i in 0..4 {
                if row + i * quarter < mi_rows {
                    push_block(source, blocks, row + i * quarter, col, sub);
                }
            }
        }
        PartitionType::Vert4 => {
            let quarter = (size.num_4x4_wide() / 4) as usize;
            for i in 0..4 {
                if col + i * quarter < mi_cols {
                    push_block(source, blocks, row, col + i * quarter, sub);
                }
            }
        }
    }
}

/// A partition source driven by the tile's symbol stream.
pub struct SymbolTile<'a> {
    decoder: crate::reference::av1::symbol::SymbolDecoder<'a>,
    sizes: Vec<Option<BlockSize>>,
    mi_cols: usize,
}

impl<'a> SymbolTile<'a> {
    pub fn new(
        data: &'a [u8],
        mi_cols: usize,
        mi_rows: usize,
    ) -> Result<Self, crate::reference::av1::symbol::SymbolError> {
        Ok(Self {
            decoder: crate::reference::av1::symbol::SymbolDecoder::init(data)?,
            sizes: vec![None; mi_cols * mi_rows],
            mi_cols,
        })
    }
}

impl PartitionSource for SymbolTile<'_> {
    fn choose(
        &mut self,
        row: usize,
        col: usize,
        size: BlockSize,
        _above: Option<BlockSize>,
        _left: Option<BlockSize>,
    ) -> PartitionType {
        // `decode_partition` derives the context from the recorded MiSizes.
        let above = (row > 0)
            .then(|| self.sizes[(row - 1) * self.mi_cols + col])
            .flatten();
        let left = (col > 0)
            .then(|| self.sizes[row * self.mi_cols + col - 1])
            .flatten();
        crate::reference::av1::partition::read_partition(&mut self.decoder, size, above, left)
            .ok()
            .flatten()
            .unwrap_or(PartitionType::None)
    }

    fn record(&mut self, block: DecodedBlock) {
        let w = (block.size.width / 4) as usize;
        let h = (block.size.height / 4) as usize;
        for r in block.row..block.row + h {
            for c in block.col..block.col + w {
                if r * self.mi_cols + c < self.sizes.len() {
                    self.sizes[r * self.mi_cols + c] = Some(block.size);
                }
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
        let blocks = walk_partitions(32, 16, 64, &mut ConstantPartition(PartitionType::None));
        let count = coverage(&blocks, 32, 16);
        assert!(count.iter().all(|v| *v == 1), "{count:?}");
        assert!(blocks.iter().all(|b| b.size.width == 64));
    }

    #[test]
    fn all_split_covers_each_4x4_once() {
        let blocks = walk_partitions(32, 16, 64, &mut ConstantPartition(PartitionType::Split));
        let count = coverage(&blocks, 32, 16);
        assert!(count.iter().all(|v| *v == 1), "{count:?}");
        assert_eq!(blocks.len(), 32 * 16);
        assert!(blocks
            .iter()
            .all(|b| b.size.width == 4 && b.size.height == 4));
    }

    #[test]
    fn mixed_partitions_still_tile_the_picture() {
        struct Alternating;
        impl PartitionSource for Alternating {
            fn choose(
                &mut self,
                row: usize,
                col: usize,
                size: BlockSize,
                _above: Option<BlockSize>,
                _left: Option<BlockSize>,
            ) -> PartitionType {
                if size.width == 128 {
                    PartitionType::Split
                } else if (row + col) % 2 == 0 {
                    PartitionType::Horz
                } else {
                    PartitionType::Vert
                }
            }
            fn record(&mut self, _block: DecodedBlock) {}
        }
        let blocks = walk_partitions(48, 48, 64, &mut Alternating);
        let count = coverage(&blocks, 48, 48);
        assert!(count.iter().all(|v| *v == 1), "overlap or hole");
    }

    #[test]
    fn symbol_driven_walk_still_tiles_the_picture() {
        for data in [
            vec![0u8; 256],
            vec![0xffu8; 256],
            (0..256).map(|v| v as u8).collect(),
        ] {
            let mut tile = SymbolTile::new(&data, 32, 32).unwrap();
            let blocks = walk_partitions(32, 32, 64, &mut tile);
            let count = coverage(&blocks, 32, 32);
            assert!(count.iter().all(|v| *v == 1), "overlap or hole");
        }
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
