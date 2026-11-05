//! Stateful AV1 tile entry and partition traversal.
//!
//! Partition symbols and block payloads are interleaved in the bitstream. The
//! traversal therefore calls a [`TileBlockSink`] as soon as a leaf block is
//! known; the sink receives the same `SymbolDecoder` and `CdfState`, so it can
//! consume block modes and residual syntax before the next partition symbol.

use crate::reference::av1::cdf_state::{CdfState, CdfStateError};
use crate::reference::av1::frame_header::FrameHeader;
use crate::reference::av1::partition::{read_partition_with_cdf_bounded, BlockSize, PartitionType};
use crate::reference::av1::symbol::{SymbolDecoder, SymbolError};
use crate::reference::av1::tile::{walk_partitions, DecodedBlock, PartitionSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileBounds {
    pub tile: usize,
    pub tile_row: usize,
    pub tile_col: usize,
    pub mi_row_start: usize,
    pub mi_row_end: usize,
    pub mi_col_start: usize,
    pub mi_col_end: usize,
}

impl TileBounds {
    pub fn mi_rows(self) -> usize {
        self.mi_row_end - self.mi_row_start
    }

    pub fn mi_cols(self) -> usize {
        self.mi_col_end - self.mi_col_start
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TileEntropyError {
    InvalidTile(usize),
    InvalidGeometry,
    Cdf(CdfStateError),
    Symbol(SymbolError),
    Unsupported(&'static str),
    Block {
        row: usize,
        col: usize,
        size: &'static str,
        bit_position: usize,
        source: Box<TileEntropyError>,
    },
    ExitSymbol {
        bit_position: usize,
        state: (u32, u32, i32),
        error: SymbolError,
    },
}

impl From<CdfStateError> for TileEntropyError {
    fn from(error: CdfStateError) -> Self {
        Self::Cdf(error)
    }
}

impl From<SymbolError> for TileEntropyError {
    fn from(error: SymbolError) -> Self {
        Self::Symbol(error)
    }
}

/// Map a row-major tile id to its MI bounds.
pub fn tile_bounds(frame: &FrameHeader, tile: usize) -> Result<TileBounds, TileEntropyError> {
    let count = frame
        .tile_cols
        .checked_mul(frame.tile_rows)
        .filter(|count| *count > 0)
        .ok_or(TileEntropyError::InvalidGeometry)?;
    if tile >= count
        || frame.mi_col_starts.len() != frame.tile_cols + 1
        || frame.mi_row_starts.len() != frame.tile_rows + 1
    {
        return Err(TileEntropyError::InvalidTile(tile));
    }
    let tile_row = tile / frame.tile_cols;
    let tile_col = tile % frame.tile_cols;
    let bounds = TileBounds {
        tile,
        tile_row,
        tile_col,
        mi_row_start: frame.mi_row_starts[tile_row],
        mi_row_end: frame.mi_row_starts[tile_row + 1],
        mi_col_start: frame.mi_col_starts[tile_col],
        mi_col_end: frame.mi_col_starts[tile_col + 1],
    };
    if bounds.mi_rows() == 0 || bounds.mi_cols() == 0 {
        return Err(TileEntropyError::InvalidGeometry);
    }
    Ok(bounds)
}

/// Consumer called in bitstream order for every leaf block.
pub trait TileBlockSink {
    /// Consume syntax that belongs to a whole superblock before its partition
    /// tree. AV1 loop-restoration units are decoded here.
    fn decode_superblock(
        &mut self,
        _block: DecodedBlock,
        _decoder: &mut SymbolDecoder<'_>,
        _cdfs: &mut CdfState,
    ) -> Result<(), TileEntropyError> {
        Ok(())
    }

    fn decode_block(
        &mut self,
        block: DecodedBlock,
        decoder: &mut SymbolDecoder<'_>,
        cdfs: &mut CdfState,
    ) -> Result<(), TileEntropyError>;
}

/// A sink that only records traversal blocks.
#[derive(Debug, Clone, Default)]
pub struct NoopBlockSink(pub usize);

impl TileBlockSink for NoopBlockSink {
    fn decode_block(
        &mut self,
        _block: DecodedBlock,
        _decoder: &mut SymbolDecoder<'_>,
        _cdfs: &mut CdfState,
    ) -> Result<(), TileEntropyError> {
        self.0 += 1;
        Ok(())
    }
}

struct StatefulTile<'a, S> {
    decoder: SymbolDecoder<'a>,
    cdfs: CdfState,
    sizes: Vec<Option<BlockSize>>,
    bounds: TileBounds,
    local_cols: usize,
    local_rows: usize,
    blocks: Vec<DecodedBlock>,
    sink: S,
    error: Option<TileEntropyError>,
    disable_cdf_update: bool,
}

impl<S: TileBlockSink> PartitionSource for StatefulTile<'_, S> {
    fn begin_superblock(&mut self, row: usize, col: usize, size: BlockSize) {
        if self.error.is_some() {
            return;
        }
        let global = DecodedBlock {
            row: row + self.bounds.mi_row_start,
            col: col + self.bounds.mi_col_start,
            size,
        };
        if let Err(source) = self
            .sink
            .decode_superblock(global, &mut self.decoder, &mut self.cdfs)
        {
            self.error = Some(TileEntropyError::Block {
                row: global.row,
                col: global.col,
                size: global.size.name,
                bit_position: self.decoder.position(),
                source: Box::new(source),
            });
        }
    }

    fn choose(
        &mut self,
        row: usize,
        col: usize,
        size: BlockSize,
        _above: Option<BlockSize>,
        _left: Option<BlockSize>,
    ) -> PartitionType {
        if self.error.is_some() {
            return PartitionType::None;
        }
        let above = (row > 0)
            .then(|| self.sizes[(row - 1) * self.local_cols + col])
            .flatten();
        let left = (col > 0)
            .then(|| self.sizes[row * self.local_cols + col - 1])
            .flatten();
        let half = (size.num_4x4_wide() / 2) as usize;
        let has_rows = row + half < self.local_rows;
        let has_cols = col + half < self.local_cols;
        let choice = read_partition_with_cdf_bounded(
            &mut self.decoder,
            &mut self.cdfs,
            size,
            above,
            left,
            has_rows,
            has_cols,
            self.disable_cdf_update,
        );
        match choice {
            Ok(Some(partition)) => partition,
            Ok(None) => PartitionType::None,
            Err(error) => {
                self.error = Some(error.into());
                PartitionType::None
            }
        }
    }

    fn record(&mut self, block: DecodedBlock) {
        if self.error.is_some() {
            return;
        }
        let w = (block.size.width / 4) as usize;
        let h = (block.size.height / 4) as usize;
        for row in block.row..block.row + h {
            for col in block.col..block.col + w {
                if row < self.local_rows && col < self.local_cols {
                    self.sizes[row * self.local_cols + col] = Some(block.size);
                }
            }
        }
        let global = DecodedBlock {
            row: block.row + self.bounds.mi_row_start,
            col: block.col + self.bounds.mi_col_start,
            size: block.size,
        };
        self.blocks.push(global);
        if let Err(source) = self
            .sink
            .decode_block(global, &mut self.decoder, &mut self.cdfs)
        {
            self.error = Some(TileEntropyError::Block {
                row: global.row,
                col: global.col,
                size: global.size.name,
                bit_position: self.decoder.position(),
                source: Box::new(source),
            });
        }
    }
}

/// Result returned after traversing one tile.
#[derive(Debug)]
pub struct TileDecodeResult<S> {
    pub bounds: TileBounds,
    pub blocks: Vec<DecodedBlock>,
    pub cdfs: CdfState,
    pub sink: S,
}

/// Decode a tile's partition tree and invoke `sink` for every leaf block.
pub fn decode_tile_partitions<S: TileBlockSink>(
    payload: &[u8],
    frame: &FrameHeader,
    tile: usize,
    sb_size: u32,
    disable_cdf_update: bool,
    verify_exit: bool,
    sink: S,
) -> Result<TileDecodeResult<S>, TileEntropyError> {
    decode_tile_partitions_with_cdfs(
        payload,
        frame,
        tile,
        sb_size,
        disable_cdf_update,
        verify_exit,
        CdfState::new(),
        sink,
    )
}

/// Decode a tile starting from a saved frame CDF state.
#[allow(clippy::too_many_arguments)]
pub fn decode_tile_partitions_with_cdfs<S: TileBlockSink>(
    payload: &[u8],
    frame: &FrameHeader,
    tile: usize,
    sb_size: u32,
    disable_cdf_update: bool,
    verify_exit: bool,
    cdfs: CdfState,
    sink: S,
) -> Result<TileDecodeResult<S>, TileEntropyError> {
    let bounds = tile_bounds(frame, tile)?;
    let local_cols = bounds.mi_cols();
    let local_rows = bounds.mi_rows();
    let decoder = SymbolDecoder::init(payload)?;
    let mut state = StatefulTile {
        decoder,
        cdfs,
        sizes: vec![None; local_cols * local_rows],
        bounds,
        local_cols,
        local_rows,
        blocks: Vec::new(),
        sink,
        error: None,
        disable_cdf_update,
    };
    let _ = walk_partitions(local_cols, local_rows, sb_size, &mut state);
    if let Some(error) = state.error {
        return Err(error);
    }
    if verify_exit {
        state
            .decoder
            .exit_symbol()
            .map_err(|error| TileEntropyError::ExitSymbol {
                bit_position: state.decoder.position(),
                state: state.decoder.debug_state(),
                error,
            })?;
    }
    Ok(TileDecodeResult {
        bounds,
        blocks: state.blocks,
        cdfs: state.cdfs,
        sink: state.sink,
    })
}

/// Sanity check used by the tile entry tests.
pub fn block_cover_count(blocks: &[DecodedBlock], bounds: TileBounds) -> Vec<usize> {
    let mut count = vec![0usize; bounds.mi_rows() * bounds.mi_cols()];
    for block in blocks {
        let w = (block.size.width / 4) as usize;
        let h = (block.size.height / 4) as usize;
        for row in block.row..block.row + h {
            for col in block.col..block.col + w {
                if row >= bounds.mi_row_end || col >= bounds.mi_col_end {
                    continue;
                }
                let local =
                    (row - bounds.mi_row_start) * bounds.mi_cols() + (col - bounds.mi_col_start);
                count[local] += 1;
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::frame_header::{parse_frame_obu, FrameHeaderState};
    use crate::reference::av1::obu::{parse_ivf, parse_obus, ObuType};
    use crate::reference::av1::seq_header::{parse_sequence_header_obu, SequenceHeader};

    fn tiles() -> (SequenceHeader, FrameHeader, Vec<u8>) {
        let data = include_bytes!("../../../tests/data/av1_tiles_320x240_10.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut sequence = None;
        for packet in &ivf.frames {
            for obu in parse_obus(packet.payload).unwrap() {
                match obu.header.obu_type {
                    ObuType::SequenceHeader => {
                        sequence = Some(parse_sequence_header_obu(&obu, None).unwrap());
                    }
                    ObuType::Frame => {
                        let parts = parse_frame_obu(
                            &obu,
                            sequence.as_ref().unwrap(),
                            &FrameHeaderState::default(),
                        )
                        .unwrap();
                        return (
                            sequence.unwrap(),
                            parts.header,
                            parts.tile_group_payload.to_vec(),
                        );
                    }
                    _ => {}
                }
            }
        }
        panic!("no tile fixture frame");
    }

    #[test]
    fn tile_bounds_follow_row_major_tile_ids() {
        let (_, frame, _) = tiles();
        assert_eq!((frame.tile_cols, frame.tile_rows), (2, 2));
        let starts = [(0, 0, 0), (0, 1, 48), (1, 0, 0), (1, 1, 48)];
        for (tile, (tile_row, tile_col, mi_col_start)) in starts.into_iter().enumerate() {
            let bounds = tile_bounds(&frame, tile).unwrap();
            assert_eq!(
                (bounds.tile_row, bounds.tile_col, bounds.mi_col_start),
                (tile_row, tile_col, mi_col_start)
            );
        }
        assert!(tile_bounds(&frame, 4).is_err());
    }

    #[test]
    fn stateful_partition_walk_covers_each_tile_once() {
        let (sequence, frame, payload) = tiles();
        let group =
            crate::reference::av1::tile_group::parse_tile_group(&payload, &frame, 0).unwrap();
        let sb_size = if sequence.use_128x128_superblock {
            128
        } else {
            64
        };
        for tile in 0..4 {
            let range = &group.tiles[tile].bytes;
            let result = decode_tile_partitions(
                &payload[range.clone()],
                &frame,
                tile,
                sb_size,
                false,
                false,
                NoopBlockSink::default(),
            )
            .unwrap();
            let count = block_cover_count(&result.blocks, result.bounds);
            assert!(
                count.iter().all(|value| *value == 1),
                "tile {tile}: {count:?}"
            );
            assert_eq!(result.sink.0, result.blocks.len());
        }
    }
}
