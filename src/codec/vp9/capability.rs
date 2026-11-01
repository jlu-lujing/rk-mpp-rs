//! Explicit hardware-boundary decisions for VP9 pictures.
//!
//! The boundary matrix is only useful if every combination either decodes or
//! is refused *with a reason*, instead of failing somewhere three layers
//! later. The parser already refuses what the syntax says is invalid
//! (profiles 1/3, bad markers, reference size ratios outside the spec's
//! 1/2..16x window, missing references). This module is the hardware side of
//! the same question, decisions taken from `hal_vp9d_vdpu34x.c` and its
//! register file:
//!
//! * the two ping-pong segment-map halves are allocated once at
//!   `MAX_SEGMAP_SIZE_ALIGN_TO_4K * 2` (`hal_vp9d_vdpu34x.c:125`), sized by
//!   `MAX_SEGMAP_SIZE` = 73728 bytes = 2304 superblock entries of 32 bytes,
//!   which `hal_vp9d_com.h:66` documents as covering 4096x2304. C lets a larger frame overrun that buffer; the hardware writes
//!   past its own segment map. Here the same frame is refused before the job,
//!   because our `reset_segment_maps` would otherwise index the buffer out of
//!   bounds - the same overflow, as a panic rather than as memory corruption,
//!   which is not a better answer. This is a documented divergence from C in
//!   the strict sense: C does not reject, C corrupts.
//! * the output DMA geometry (`frame_layout`, `colmv_size`) is a runtime
//!   allocation on dma-heap and fails with an OS error past what the board can
//!   hand out, so no separate ceiling is invented here; the segment-map check
//!   is the only static one the C code itself proves.
//!
//! Everything a real stream can still hit - 1x1 and other odd sizes, a single
//! superblock, the full 4096x2304 envelope, all frame types, four tile rows
//! (MPP's parser caps `log2_tile_rows` at two read bits, so eight-row tiling
//! cannot even be expressed), lossless, segmentation - is accepted, and the
//! acceptance is locked by the tests around the parser and register file.

use std::fmt;

use super::parser::Header;

/// Superblocks of 64x64 the fixed segment-map buffers are sized for:
/// `MAX_SEGMAP_SIZE / SEGMENT_MAP_BYTES_PER_CTU` = 73728 / 32, i.e. the
/// 64x36 superblock grid of the documented 4096x2304 envelope.
pub const MAX_SUPERBLOCKS: u64 = 2_304;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryError {
    /// The frame needs more segment-map bytes than the two ping-pong halves
    /// hold, so neither the hardware write nor the software clear fits.
    SegmentMapOverflow { superblocks: u64, capacity: u64 },
}

impl fmt::Display for BoundaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BoundaryError::SegmentMapOverflow {
                superblocks,
                capacity,
            } => write!(
                f,
                "VP9 frame needs {superblocks} superblock map entries, capacity is {capacity}"
            ),
        }
    }
}

impl std::error::Error for BoundaryError {}

/// Decide whether the rkvdec (vdpu34x) job can be built for this picture.
pub fn check(header: &Header) -> Result<(), BoundaryError> {
    // The C buffer comment counts superblocks of the *coded* size, and
    // `segment_map_len` uses the same ceiling rounding, so the budget check
    // uses it too: a 4097-wide frame spends 65 columns per row, not 64.
    let superblocks = u64::from(header.width.div_ceil(64)) * u64::from(header.height.div_ceil(64));
    if superblocks > MAX_SUPERBLOCKS {
        return Err(BoundaryError::SegmentMapOverflow {
            superblocks,
            capacity: MAX_SUPERBLOCKS,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::vp9::parser::{FrameKind, Header, LoopFilter, Quantization};

    fn sized(width: u32, height: u32) -> Header {
        Header {
            profile: crate::codec::vp9::parser::Profile::Profile0,
            frame_kind: FrameKind::Key,
            show_frame: true,
            error_resilient_mode: false,
            intra_only: false,
            refresh_frame_flags: 0xff,
            width,
            height,
            render_width: width,
            render_height: height,
            bit_depth: 8,
            ref_frame_idx: [0, 0, 0],
            ref_frame_sign_bias: [false; 3],
            frame_to_show_map_idx: None,
            refresh_frame_context: false,
            reset_frame_context: 0,
            allow_high_precision_mv: false,
            interp_filter: 0,
            use_prev_in_find_mv_refs: false,
            frame_context_idx: 0,
            frame_parallel_mode: false,
            loop_filter: LoopFilter::default(),
            quantization: Quantization::default(),
            segmentation: crate::codec::vp9::parser::Segmentation::default(),
            mvscale: [[16, 16]; 3],
            log2_tile_cols: 0,
            log2_tile_rows: 0,
            first_partition_size: 0,
            header_size: 0,
        }
    }

    #[test]
    fn the_full_documented_envelope_is_accepted() {
        // 4096x2304 is the size MAX_SEGMAP_SIZE's comment in the C header
        // computes for; the corner of the budget has to stay inside.
        let header = sized(4096, 2304);
        assert_eq!(superblocks_of(&header), MAX_SUPERBLOCKS);
        assert!(check(&header).is_ok(), "4096x2304 is the budget corner");
    }

    #[test]
    fn one_superblock_more_than_the_budget_is_refused_by_name() {
        // 65 columns by 36 rows clears the 2304-entry budget by 36.
        let header = sized(64 * 65, 64 * 35 + 1);
        assert!(superblocks_of(&header) > MAX_SUPERBLOCKS);
        let error = check(&header).expect_err("over budget must not decode");
        assert!(matches!(
            error,
            BoundaryError::SegmentMapOverflow {
                capacity: MAX_SUPERBLOCKS,
                ..
            }
        ));
        assert!(error.to_string().contains("capacity is 2304"));
    }

    #[test]
    fn odd_and_small_sizes_spend_superblocks_by_ceiling() {
        for (width, height, superblocks) in [
            (1u32, 1u32, 1u64),
            (64, 64, 1),
            (65, 1, 2),
            (321, 241, 6 * 4),
            (4096, 2304, MAX_SUPERBLOCKS),
        ] {
            let header = sized(width, height);
            assert_eq!(superblocks_of(&header), superblocks, "{width}x{height}");
            assert!(check(&header).is_ok(), "{width}x{height} inside budget");
        }
    }

    fn superblocks_of(header: &Header) -> u64 {
        u64::from(header.width.div_ceil(64)) * u64::from(header.height.div_ceil(64))
    }
}
