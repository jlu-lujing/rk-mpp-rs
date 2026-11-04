//! AV1 tile-group syntax and byte-range extraction.
//!
//! `parse_frame_obu()` stops after `uncompressed_header()` and
//! `byte_alignment()`; the returned tail begins with `tile_group_obu()`.
//! This module consumes that header and turns the size-prefixed payload into
//! one byte range per tile. The last tile in a group has no explicit size and
//! extends to the end of the group payload.

use std::ops::Range;

use crate::reference::av1::bitstream::{BitReader, BitstreamError};
use crate::reference::av1::frame_header::{tile_log2, FrameHeader};

/// `MAX_TILE_COLS * MAX_TILE_ROWS` is larger, but the AV1 conformance bitstream
/// limit is 512 tiles per frame.
pub const MAX_TILES: usize = 512;

/// Header fields of `tile_group_obu()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileGroupHeader {
    pub tile_start_and_end_present_flag: bool,
    pub tg_start: usize,
    pub tg_end: usize,
}

/// One tile's byte range. The range is relative to the input tile-group
/// payload plus the caller-supplied absolute base offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileRange {
    pub tile: usize,
    pub bytes: Range<usize>,
}

/// A parsed tile-group header and its tile ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTileGroup {
    pub header: TileGroupHeader,
    /// Byte offset from the start of the tile-group payload to the first size
    /// byte or tile payload.
    pub data_offset: usize,
    pub tiles: Vec<TileRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileGroupError {
    Bitstream(BitstreamError),
    NoTiles,
    TooManyTiles(usize),
    InvalidGroupRange {
        start: usize,
        end: usize,
        count: usize,
    },
    TruncatedTileSize {
        offset: usize,
        size_bytes: usize,
    },
    TileSizeOverflow,
    TruncatedTileData {
        offset: usize,
        size: usize,
        available: usize,
    },
    MissingTilePayload {
        tile: usize,
    },
    DuplicateTile(usize),
    IncompleteTiles {
        present: usize,
        expected: usize,
    },
}

impl From<BitstreamError> for TileGroupError {
    fn from(error: BitstreamError) -> Self {
        Self::Bitstream(error)
    }
}

impl core::fmt::Display for TileGroupError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

fn tile_count(frame: &FrameHeader) -> Result<usize, TileGroupError> {
    let count = frame
        .tile_cols
        .checked_mul(frame.tile_rows)
        .ok_or(TileGroupError::NoTiles)?;
    if count == 0 {
        return Err(TileGroupError::NoTiles);
    }
    if count > MAX_TILES {
        return Err(TileGroupError::TooManyTiles(count));
    }
    Ok(count)
}

/// Parse `tile_start_and_end_present_flag`, `tg_start`, `tg_end` and the
/// following `byte_alignment()`.
pub fn parse_tile_group_header(
    payload: &[u8],
    frame: &FrameHeader,
) -> Result<(TileGroupHeader, usize), TileGroupError> {
    let count = tile_count(frame)?;
    let mut reader = BitReader::new(payload);
    let present = count > 1 && reader.read_bit()? != 0;
    let (start, end) = if present {
        let bits = tile_log2(1, frame.tile_cols) + tile_log2(1, frame.tile_rows);
        let start = reader.read_f(bits)? as usize;
        let end = reader.read_f(bits)? as usize;
        if start > end || end >= count {
            return Err(TileGroupError::InvalidGroupRange { start, end, count });
        }
        (start, end)
    } else {
        (0, count - 1)
    };
    reader.align_to_byte()?;
    Ok((
        TileGroupHeader {
            tile_start_and_end_present_flag: present,
            tg_start: start,
            tg_end: end,
        },
        reader.position() / 8,
    ))
}

/// Split one tile group's payload into tile byte ranges. `absolute_base` is the
/// position of `payload`'s first byte relative to the caller's enclosing
/// packet or OBU; ranges use that coordinate system.
pub fn parse_tile_ranges(
    payload: &[u8],
    frame: &FrameHeader,
    group: TileGroupHeader,
    absolute_base: usize,
) -> Result<Vec<TileRange>, TileGroupError> {
    let count = tile_count(frame)?;
    if group.tg_start > group.tg_end || group.tg_end >= count {
        return Err(TileGroupError::InvalidGroupRange {
            start: group.tg_start,
            end: group.tg_end,
            count,
        });
    }
    let size_bytes = frame.tile_size_bytes;

    let mut cursor = 0usize;
    let mut tiles = Vec::with_capacity(group.tg_end - group.tg_start + 1);
    for tile in group.tg_start..=group.tg_end {
        if tile == group.tg_end {
            let start = cursor
                .checked_add(absolute_base)
                .ok_or(TileGroupError::TileSizeOverflow)?;
            let end = absolute_base
                .checked_add(payload.len())
                .ok_or(TileGroupError::TileSizeOverflow)?;
            if end <= start {
                return Err(TileGroupError::MissingTilePayload { tile });
            }
            tiles.push(TileRange {
                tile,
                bytes: start..end,
            });
            break;
        }

        if !(1..=4).contains(&size_bytes) {
            return Err(TileGroupError::TruncatedTileSize {
                offset: cursor,
                size_bytes,
            });
        }
        let size_end = cursor
            .checked_add(size_bytes)
            .filter(|end| *end <= payload.len())
            .ok_or(TileGroupError::TruncatedTileSize {
                offset: cursor,
                size_bytes,
            })?;
        let mut encoded = 0u32;
        for (i, byte) in payload[cursor..size_end].iter().enumerate() {
            encoded |= u32::from(*byte) << (i * 8);
        }
        let size = usize::try_from(encoded)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or(TileGroupError::TileSizeOverflow)?;
        cursor = size_end;
        let start = cursor
            .checked_add(absolute_base)
            .ok_or(TileGroupError::TileSizeOverflow)?;
        let data_end = cursor
            .checked_add(size)
            .filter(|end| *end <= payload.len())
            .ok_or(TileGroupError::TruncatedTileData {
                offset: cursor,
                size,
                available: payload.len().saturating_sub(cursor),
            })?;
        tiles.push(TileRange {
            tile,
            bytes: start
                ..absolute_base
                    .checked_add(data_end)
                    .ok_or(TileGroupError::TileSizeOverflow)?,
        });
        cursor = data_end;
    }
    Ok(tiles)
}

/// Parse the complete tile-group header and its ranges.
pub fn parse_tile_group(
    payload: &[u8],
    frame: &FrameHeader,
    absolute_base: usize,
) -> Result<ParsedTileGroup, TileGroupError> {
    let (header, data_offset) = parse_tile_group_header(payload, frame)?;
    let data = &payload[data_offset..];
    let tiles = parse_tile_ranges(
        data,
        frame,
        header,
        absolute_base
            .checked_add(data_offset)
            .ok_or(TileGroupError::TileSizeOverflow)?,
    )?;
    Ok(ParsedTileGroup {
        header,
        data_offset,
        tiles,
    })
}

/// Accumulate disjoint groups that together cover a frame.
#[derive(Debug, Clone)]
pub struct TileGroupAccumulator {
    ranges: Vec<Option<Range<usize>>>,
}

impl TileGroupAccumulator {
    pub fn new(tile_count: usize) -> Result<Self, TileGroupError> {
        if tile_count == 0 {
            return Err(TileGroupError::NoTiles);
        }
        if tile_count > MAX_TILES {
            return Err(TileGroupError::TooManyTiles(tile_count));
        }
        Ok(Self {
            ranges: vec![None; tile_count],
        })
    }

    pub fn insert(&mut self, tiles: Vec<TileRange>) -> Result<(), TileGroupError> {
        for tile in tiles {
            let Some(slot) = self.ranges.get_mut(tile.tile) else {
                return Err(TileGroupError::InvalidGroupRange {
                    start: tile.tile,
                    end: tile.tile,
                    count: self.ranges.len(),
                });
            };
            if slot.replace(tile.bytes).is_some() {
                return Err(TileGroupError::DuplicateTile(tile.tile));
            }
        }
        Ok(())
    }

    pub fn finish(self) -> Result<Vec<Range<usize>>, TileGroupError> {
        let present = self.ranges.iter().filter(|range| range.is_some()).count();
        if present != self.ranges.len() {
            return Err(TileGroupError::IncompleteTiles {
                present,
                expected: self.ranges.len(),
            });
        }
        Ok(self.ranges.into_iter().flatten().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::frame_header::{parse_frame_obu, FrameHeaderState};
    use crate::reference::av1::obu::{parse_ivf, parse_obus, ObuType};
    use crate::reference::av1::seq_header::parse_sequence_header_obu;

    fn frame(cols: usize, rows: usize, size_bytes: usize) -> FrameHeader {
        FrameHeader {
            tile_cols: cols,
            tile_rows: rows,
            tile_size_bytes: size_bytes,
            ..FrameHeader::default()
        }
    }

    #[test]
    fn size_bytes_split_the_group_and_the_last_tile_extends_to_the_end() {
        let frame = frame(3, 1, 1);
        let group = TileGroupHeader {
            tile_start_and_end_present_flag: false,
            tg_start: 0,
            tg_end: 2,
        };
        let payload = [2, 0xaa, 0xbb, 0xcc, 1, 0xdd, 0xee, 0xff];
        let tiles = parse_tile_ranges(&payload, &frame, group, 0).unwrap();
        assert_eq!(tiles[0].bytes, 1..4);
        assert_eq!(tiles[1].bytes, 5..7);
        assert_eq!(tiles[2].bytes, 7..8);
    }

    #[test]
    fn group_header_reads_start_end_and_alignment() {
        let frame = frame(2, 2, 1);
        // flag=1, start=10b=2, end=11b=3, then five zero alignment bits.
        let payload = [0b11011000, 0xaa];
        let (group, offset) = parse_tile_group_header(&payload, &frame).unwrap();
        assert_eq!(
            group,
            TileGroupHeader {
                tile_start_and_end_present_flag: true,
                tg_start: 2,
                tg_end: 3,
            }
        );
        assert_eq!(offset, 1);
    }

    #[test]
    fn single_tile_group_has_no_bit_header() {
        let frame = frame(1, 1, 1);
        let (group, offset) = parse_tile_group_header(&[0xaa, 0xbb], &frame).unwrap();
        assert_eq!(group.tg_start, 0);
        assert_eq!(group.tg_end, 0);
        assert_eq!(offset, 0);
    }

    #[test]
    fn malformed_groups_are_named() {
        let frame = frame(2, 2, 1);
        assert!(matches!(
            parse_tile_group_header(&[0xf0, 0], &frame),
            Err(TileGroupError::InvalidGroupRange { .. })
        ));
        let group = TileGroupHeader {
            tile_start_and_end_present_flag: false,
            tg_start: 0,
            tg_end: 1,
        };
        assert!(matches!(
            parse_tile_ranges(&[3, 0xaa], &frame, group, 0),
            Err(TileGroupError::TruncatedTileData { .. })
        ));
    }

    #[test]
    fn accumulator_rejects_duplicates_and_incomplete_frames() {
        let mut accumulator = TileGroupAccumulator::new(2).unwrap();
        let first = TileRange {
            tile: 0,
            bytes: 0..1,
        };
        let second = TileRange {
            tile: 1,
            bytes: 1..2,
        };
        accumulator.insert(vec![first.clone()]).unwrap();
        assert_eq!(
            accumulator.insert(vec![first.clone()]),
            Err(TileGroupError::DuplicateTile(0))
        );
        assert!(matches!(
            accumulator.clone().finish(),
            Err(TileGroupError::IncompleteTiles {
                present: 1,
                expected: 2
            })
        ));
        accumulator.insert(vec![second]).unwrap();
        assert_eq!(accumulator.clone().finish().unwrap(), vec![0..1, 1..2]);
    }

    fn first_frame(
        data: &[u8],
    ) -> (
        crate::reference::av1::seq_header::SequenceHeader,
        FrameHeader,
        Vec<u8>,
        usize,
    ) {
        let ivf = parse_ivf(data).unwrap();
        let mut sequence = None;
        for packet in &ivf.frames {
            for obu in parse_obus(packet.payload).unwrap() {
                if obu.header.obu_type == ObuType::SequenceHeader {
                    sequence = Some(parse_sequence_header_obu(&obu, None).unwrap());
                }
                if obu.header.obu_type == ObuType::Frame {
                    let sequence = sequence.as_ref().unwrap();
                    let state = FrameHeaderState::default();
                    let parts = parse_frame_obu(&obu, sequence, &state).unwrap();
                    let tail = parts.tile_group_payload.to_vec();
                    return (sequence.clone(), parts.header, tail, parts.header_bytes);
                }
            }
        }
        panic!("no frame");
    }

    #[test]
    fn real_one_tile_fixture_has_one_complete_range() {
        let (_, header, tail, _) =
            first_frame(include_bytes!("../../../tests/data/av1_320x240_10.ivf"));
        let group = parse_tile_group(&tail, &header, 0).unwrap();
        assert_eq!(group.header.tg_start, 0);
        assert_eq!(group.header.tg_end, 0);
        assert_eq!(group.data_offset, 0);
        assert_eq!(group.tiles.len(), 1);
        assert_eq!(group.tiles[0].bytes, 0..tail.len());
    }

    #[test]
    fn real_two_by_two_fixture_splits_every_tile() {
        let (_, header, tail, _) = first_frame(include_bytes!(
            "../../../tests/data/av1_tiles_320x240_10.ivf"
        ));
        assert_eq!((header.tile_cols, header.tile_rows), (2, 2));
        let group = parse_tile_group(&tail, &header, 0).unwrap();
        assert_eq!(group.tiles.len(), 4);
        assert_eq!(
            group.tiles[0].bytes.start,
            group.data_offset + header.tile_size_bytes
        );
        for pair in group.tiles[..3].windows(2) {
            assert_eq!(
                pair[1].bytes.start,
                pair[0].bytes.end + header.tile_size_bytes
            );
        }
        assert_eq!(group.tiles[2].bytes.end, group.tiles[3].bytes.start);
        assert_eq!(group.tiles[3].bytes.end, tail.len());
    }
}
