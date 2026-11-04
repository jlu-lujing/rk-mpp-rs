//! AV1 block geometry and the partition syntax's context.
//!
//! The 22 `BLOCK_SIZES` entries are in the order the specification's
//! `Subsampled_Size[ BLOCK_SIZES ][ 2 ][ 2 ]` table lists them; each entry's
//! dimensions follow from its name (`BLOCK_16X32` is 16 wide, 32 high). The
//! partition context is spec 5.11.4:
//!
//! ```text
//! bsl   = Mi_Width_Log2[ bSize ]
//! above = AvailU && (Mi_Width_Log2[ MiSizes[ r - 1 ][ c ] ] < bsl )
//! left  = AvailL && ( Mi_Height_Log2[ MiSizes[ r ][ c - 1 ] ] < bsl )
//! ctx   = left * 2 + above
//! ```
//!
//! and the CDF is `TilePartitionW{8,16,32,64,128}Cdf[ ctx ]` chosen by `bsl`.
//! 4x4 blocks are never coded with a partition symbol.

use crate::reference::av1::cdf_defaults::{
    CdfTable, DEFAULT_PARTITION_W128_CDF, DEFAULT_PARTITION_W16_CDF, DEFAULT_PARTITION_W32_CDF,
    DEFAULT_PARTITION_W64_CDF, DEFAULT_PARTITION_W8_CDF,
};

/// One `BLOCK_SIZES` entry, in specification order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockSize {
    pub name: &'static str,
    pub width: u32,
    pub height: u32,
}

impl BlockSize {
    /// `Mi_Width_Log2`: `log2(width / MI_SIZE)`.
    pub fn mi_width_log2(self) -> u32 {
        (self.width / 4).trailing_zeros()
    }

    /// `Mi_Height_Log2`: `log2(height / MI_SIZE)`.
    pub fn mi_height_log2(self) -> u32 {
        (self.height / 4).trailing_zeros()
    }

    /// `Num_4x4_Blocks_Wide`.
    pub fn num_4x4_wide(self) -> u32 {
        self.width / 4
    }

    /// `Num_4x4_Blocks_High`.
    pub fn num_4x4_high(self) -> u32 {
        self.height / 4
    }

    /// `bSize < BLOCK_8X8`: only a 4x4 square is smaller.
    pub fn is_4x4(self) -> bool {
        self.width == 4 && self.height == 4
    }
}

/// `BLOCK_SIZES` in `Subsampled_Size` order.
pub static BLOCK_SIZES: [BlockSize; 22] = [
    BlockSize {
        name: "BLOCK_4X4",
        width: 4,
        height: 4,
    },
    BlockSize {
        name: "BLOCK_4X8",
        width: 4,
        height: 8,
    },
    BlockSize {
        name: "BLOCK_8X4",
        width: 8,
        height: 4,
    },
    BlockSize {
        name: "BLOCK_8X8",
        width: 8,
        height: 8,
    },
    BlockSize {
        name: "BLOCK_8X16",
        width: 8,
        height: 16,
    },
    BlockSize {
        name: "BLOCK_16X8",
        width: 16,
        height: 8,
    },
    BlockSize {
        name: "BLOCK_16X16",
        width: 16,
        height: 16,
    },
    BlockSize {
        name: "BLOCK_16X32",
        width: 16,
        height: 32,
    },
    BlockSize {
        name: "BLOCK_32X16",
        width: 32,
        height: 16,
    },
    BlockSize {
        name: "BLOCK_32X32",
        width: 32,
        height: 32,
    },
    BlockSize {
        name: "BLOCK_32X64",
        width: 32,
        height: 64,
    },
    BlockSize {
        name: "BLOCK_64X32",
        width: 64,
        height: 32,
    },
    BlockSize {
        name: "BLOCK_64X64",
        width: 64,
        height: 64,
    },
    BlockSize {
        name: "BLOCK_64X128",
        width: 64,
        height: 128,
    },
    BlockSize {
        name: "BLOCK_128X64",
        width: 128,
        height: 64,
    },
    BlockSize {
        name: "BLOCK_128X128",
        width: 128,
        height: 128,
    },
    BlockSize {
        name: "BLOCK_4X16",
        width: 4,
        height: 16,
    },
    BlockSize {
        name: "BLOCK_16X4",
        width: 16,
        height: 4,
    },
    BlockSize {
        name: "BLOCK_8X32",
        width: 8,
        height: 32,
    },
    BlockSize {
        name: "BLOCK_32X8",
        width: 32,
        height: 8,
    },
    BlockSize {
        name: "BLOCK_16X64",
        width: 16,
        height: 64,
    },
    BlockSize {
        name: "BLOCK_64X16",
        width: 64,
        height: 16,
    },
];

/// `Size_Group[ BLOCK_SIZES ]`, the reference implementation's table
/// (`av1/common/common_data.h`, `size_group_lookup`). It groups the square-ish
/// sizes for the `y_mode` CDF; the tall/wider-than-4 variants reuse the group
/// of their square size.
pub const SIZE_GROUP: [u8; 22] = [
    0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 0, 0, 1, 1, 2, 2,
];

/// Index of a size in [`BLOCK_SIZES`].
pub fn block_size_index(size: BlockSize) -> usize {
    BLOCK_SIZES
        .iter()
        .position(|entry| *entry == size)
        .expect("block size is one of BLOCK_SIZES")
}

/// `Size_Group[ MiSize ]`.
pub fn size_group(size: BlockSize) -> u8 {
    SIZE_GROUP[block_size_index(size)]
}

/// `TilePartitionW*Cdf` for a block size, or `None` for 4x4 (no symbol).
pub fn partition_cdf(size: BlockSize) -> Option<&'static CdfTable> {
    match size.mi_width_log2() {
        1 => Some(&DEFAULT_PARTITION_W8_CDF),
        2 => Some(&DEFAULT_PARTITION_W16_CDF),
        3 => Some(&DEFAULT_PARTITION_W32_CDF),
        4 => Some(&DEFAULT_PARTITION_W64_CDF),
        5 => Some(&DEFAULT_PARTITION_W128_CDF),
        _ => None,
    }
}

/// The partition syntax context, `left * 2 + above`.
pub fn partition_context(
    size: BlockSize,
    above: Option<BlockSize>,
    left: Option<BlockSize>,
) -> usize {
    let bsl = size.mi_width_log2();
    let above = above.is_some_and(|neighbour| neighbour.mi_width_log2() < bsl);
    let left = left.is_some_and(|neighbour| neighbour.mi_height_log2() < bsl);
    usize::from(left) * 2 + usize::from(above)
}

/// `enum PartitionType` in specification order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PartitionType {
    None = 0,
    Horz = 1,
    Vert = 2,
    Split = 3,
    HorzA = 4,
    HorzB = 5,
    VertA = 6,
    VertB = 7,
    Horz4 = 8,
    Vert4 = 9,
}

impl PartitionType {
    fn from_symbol(symbol: usize) -> Option<Self> {
        Some(match symbol {
            0 => Self::None,
            1 => Self::Horz,
            2 => Self::Vert,
            3 => Self::Split,
            4 => Self::HorzA,
            5 => Self::HorzB,
            6 => Self::VertA,
            7 => Self::VertB,
            8 => Self::Horz4,
            9 => Self::Vert4,
            _ => return None,
        })
    }
}

/// The 8x8 table only has `NONE/HORZ/VERT/SPLIT`; wider blocks add the
/// asymmetric and four-way partitions.
fn partition_symbol_count(size: BlockSize) -> Option<usize> {
    match size.mi_width_log2() {
        1 => Some(4),
        _ => Some(10),
    }
}

/// Decode one `partition` symbol (spec 5.11.4) with its context.
///
/// Returns `None` for 4x4, where the syntax has no partition symbol.
pub fn read_partition(
    decoder: &mut crate::reference::av1::symbol::SymbolDecoder<'_>,
    size: BlockSize,
    above: Option<BlockSize>,
    left: Option<BlockSize>,
) -> Result<Option<PartitionType>, crate::reference::av1::symbol::SymbolError> {
    let Some(table) = partition_cdf(size) else {
        return Ok(None);
    };
    let symbols = partition_symbol_count(size).unwrap_or(4);
    // Each context row is `symbols` probabilities plus the adaptation counter.
    let width = symbols + 1;
    debug_assert_eq!(table.values.len() % width, 0, "{} shape", table.name);
    let contexts = table.values.len() / width;
    let ctx = partition_context(size, above, left);
    debug_assert!(ctx < contexts);
    let mut cdf: Vec<u16> = table.values[ctx * width..ctx * width + symbols].to_vec();
    // The specification's arrays carry the adaptation counter as the last
    // entry; `read_symbol` expects it in place, so append one.
    cdf.push(0);
    let symbol = decoder.read_symbol(&mut cdf)?;
    Ok(PartitionType::from_symbol(symbol))
}

/// `split_or_horz` / `split_or_vert` CDF from the specification's CDF
/// derivation: a three-symbol array where the disallowed direction's mass is
/// folded into SPLIT. `partition_cdf` is the selected context's ten-value row
/// (`PARTITION_NONE .. PARTITION_VERT_4`).
pub fn split_or_cdf(partition_cdf: &[u16], size: BlockSize, vertical: bool) -> [u16; 3] {
    const SCALE: u32 = 1 << 15;
    let p = |symbol: usize| {
        let end = u32::from(partition_cdf[symbol]);
        let start = if symbol == 0 {
            0
        } else {
            u32::from(partition_cdf[symbol - 1])
        };
        end - start
    };
    let mut psum = if vertical {
        p(1) + p(3) + p(4) + p(5) + p(6) // HORZ + SPLIT + HORZ_A + HORZ_B + VERT_A
    } else {
        p(2) + p(3) + p(4) + p(6) + p(7) // VERT + SPLIT + HORZ_A + VERT_A + VERT_B
    };
    if !(size.width == 128 && size.height == 128) {
        psum += if vertical { p(8) } else { p(9) };
    }
    [(SCALE - psum) as u16, SCALE as u16, 0]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> BlockSize {
        *BLOCK_SIZES.iter().find(|size| size.name == name).unwrap()
    }

    #[test]
    fn block_sizes_follow_the_specification_order() {
        assert_eq!(BLOCK_SIZES.len(), 22);
        assert_eq!(BLOCK_SIZES[0].name, "BLOCK_4X4");
        assert_eq!(BLOCK_SIZES[15].name, "BLOCK_128X128");
        assert_eq!(BLOCK_SIZES[21].name, "BLOCK_64X16");
        let names: Vec<&str> = BLOCK_SIZES.iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            [
                "BLOCK_4X4",
                "BLOCK_4X8",
                "BLOCK_8X4",
                "BLOCK_8X8",
                "BLOCK_8X16",
                "BLOCK_16X8",
                "BLOCK_16X16",
                "BLOCK_16X32",
                "BLOCK_32X16",
                "BLOCK_32X32",
                "BLOCK_32X64",
                "BLOCK_64X32",
                "BLOCK_64X64",
                "BLOCK_64X128",
                "BLOCK_128X64",
                "BLOCK_128X128",
                "BLOCK_4X16",
                "BLOCK_16X4",
                "BLOCK_8X32",
                "BLOCK_32X8",
                "BLOCK_16X64",
                "BLOCK_64X16",
            ]
        );
    }

    #[test]
    fn dimensions_and_logs_match_the_names() {
        for size in BLOCK_SIZES {
            let digits: Vec<u32> = size
                .name
                .trim_start_matches("BLOCK_")
                .split('X')
                .map(|part| part.parse().unwrap())
                .collect();
            assert_eq!(
                (digits[0], digits[1]),
                (size.width, size.height),
                "{}",
                size.name
            );
            assert_eq!(size.mi_width_log2(), (size.width / 4).trailing_zeros());
            assert_eq!(size.mi_height_log2(), (size.height / 4).trailing_zeros());
        }
        assert_eq!(named("BLOCK_4X4").mi_width_log2(), 0);
        assert_eq!(named("BLOCK_64X16").mi_width_log2(), 4);
        assert_eq!(named("BLOCK_64X16").mi_height_log2(), 2);
    }

    #[test]
    fn partition_cdf_is_selected_by_the_width_log2() {
        assert!(partition_cdf(named("BLOCK_4X4")).is_none());
        assert_eq!(
            partition_cdf(named("BLOCK_8X8")).unwrap().name,
            "Default_Partition_W8_Cdf"
        );
        assert_eq!(
            partition_cdf(named("BLOCK_16X16")).unwrap().name,
            "Default_Partition_W16_Cdf"
        );
        assert_eq!(
            partition_cdf(named("BLOCK_128X128")).unwrap().name,
            "Default_Partition_W128_Cdf"
        );
    }

    #[test]
    fn partition_symbols_come_from_the_default_cdf() {
        use crate::reference::av1::symbol::SymbolDecoder;
        let size = named("BLOCK_8X8");
        // Values computed by an independent run of the specification
        // pseudocode over Default_Partition_W8_Cdf context 0.
        for (bytes, expected) in [
            ([0u8; 8], PartitionType::None),
            ([0xffu8; 8], PartitionType::Split),
            (
                [0x12, 0x34, 0x56, 0x78, 0x12, 0x34, 0x56, 0x78],
                PartitionType::None,
            ),
            ([0, 1, 2, 3, 4, 5, 6, 7], PartitionType::None),
        ] {
            let mut decoder = SymbolDecoder::init(&bytes).unwrap();
            assert_eq!(
                read_partition(&mut decoder, size, None, None).unwrap(),
                Some(expected)
            );
        }
        let mut decoder = SymbolDecoder::init(&[0u8; 8]).unwrap();
        assert_eq!(
            read_partition(&mut decoder, named("BLOCK_4X4"), None, None).unwrap(),
            None
        );
    }

    #[test]
    fn split_or_cdf_folds_the_disallowed_direction() {
        let row = &DEFAULT_PARTITION_W16_CDF.values[..10];
        let size = named("BLOCK_16X16");
        assert_eq!(split_or_cdf(row, size, false), [23417, 32768, 0]);
        assert_eq!(split_or_cdf(row, size, true), [21075, 32768, 0]);
    }

    #[test]
    fn split_or_cdf_drops_the_four_way_mass_at_128x128() {
        // Synthetic cumulative row with 1000 units in VERT_4 and HORZ_4.
        let row = [
            1000u16, 2000, 3000, 6000, 7000, 8000, 9000, 10000, 11000, 12000,
        ];
        let at_128 = named("BLOCK_128X128");
        let small = named("BLOCK_64X64");
        let horz_small = split_or_cdf(&row, small, false);
        let horz_large = split_or_cdf(&row, at_128, false);
        // The last term is VERT_4 = 12000 - 11000 = 1000 in the small case.
        assert_eq!(u32::from(horz_small[0]), 24768);
        assert_eq!(u32::from(horz_large[0]), u32::from(horz_small[0]) + 1000);
    }

    #[test]
    fn size_group_matches_the_reference_table() {
        let groups: Vec<u8> = BLOCK_SIZES.iter().map(|size| size_group(*size)).collect();
        assert_eq!(
            groups,
            [0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 0, 0, 1, 1, 2, 2]
        );
    }

    #[test]
    fn partition_context_counts_split_neighbours() {
        let size = named("BLOCK_16X16");
        // No neighbours at the picture edge.
        assert_eq!(partition_context(size, None, None), 0);
        // A smaller above neighbour sets the low bit.
        assert_eq!(partition_context(size, Some(named("BLOCK_8X8")), None), 1);
        // A smaller left neighbour sets the second bit.
        assert_eq!(partition_context(size, None, Some(named("BLOCK_8X8"))), 2);
        assert_eq!(
            partition_context(size, Some(named("BLOCK_8X8")), Some(named("BLOCK_8X8"))),
            3
        );
        // A neighbour at least as wide/high as the block does not.
        assert_eq!(
            partition_context(size, Some(named("BLOCK_16X16")), Some(named("BLOCK_16X16"))),
            0
        );
        assert_eq!(
            partition_context(size, Some(named("BLOCK_32X32")), Some(named("BLOCK_32X32"))),
            0
        );
    }
}
