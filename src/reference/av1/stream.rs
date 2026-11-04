//! Stateful AV1 stream dispatch from IVF packets through frame/tile syntax.
//!
//! This layer intentionally stops at the tile byte ranges. It is the handoff
//! point for the future tile entropy/reconstruction driver: all preceding
//! state (sequence header, frame header, reference refresh, tile grouping) is
//! processed in stream order.

use crate::reference::av1::frame_header::{
    parse_frame_header_obu, parse_frame_obu, FrameHeader, FrameHeaderError, FrameHeaderState,
};
use crate::reference::av1::obu::{parse_obus, ObuError, ObuType};
use crate::reference::av1::seq_header::{
    parse_sequence_header_obu, SeqHeaderError, SequenceHeader,
};
use crate::reference::av1::tile_group::{parse_tile_group, ParsedTileGroup, TileGroupError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamError {
    Obu(ObuError),
    Sequence(SeqHeaderError),
    Frame(FrameHeaderError),
    TileGroup(TileGroupError),
    MissingSequence,
    MissingFrameHeader,
    ShowExistingInFrameObu,
}

impl From<ObuError> for StreamError {
    fn from(error: ObuError) -> Self {
        Self::Obu(error)
    }
}

impl From<SeqHeaderError> for StreamError {
    fn from(error: SeqHeaderError) -> Self {
        Self::Sequence(error)
    }
}

impl From<FrameHeaderError> for StreamError {
    fn from(error: FrameHeaderError) -> Self {
        Self::Frame(error)
    }
}

impl From<TileGroupError> for StreamError {
    fn from(error: TileGroupError) -> Self {
        Self::TileGroup(error)
    }
}

impl core::fmt::Display for StreamError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// A tile group belonging to a decoded frame header. `payload` is the byte
/// slice whose absolute-local ranges are in `layout.tiles`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameTiles<'a> {
    pub header: FrameHeader,
    pub payload: &'a [u8],
    pub layout: ParsedTileGroup,
}

/// Events emitted from one IVF packet, in bitstream order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent<'a> {
    Sequence(SequenceHeader),
    ShowExisting(FrameHeader),
    CodedFrame(FrameTiles<'a>),
}

/// AV1 OBU/frame/tile state carried across IVF packets.
#[derive(Debug, Clone)]
pub struct ReferenceStream {
    sequence: Option<SequenceHeader>,
    state: FrameHeaderState,
    pending_header: Option<FrameHeader>,
}

impl Default for ReferenceStream {
    fn default() -> Self {
        Self::new()
    }
}

impl ReferenceStream {
    pub fn new() -> Self {
        Self {
            sequence: None,
            state: FrameHeaderState::default(),
            pending_header: None,
        }
    }

    pub fn sequence(&self) -> Option<&SequenceHeader> {
        self.sequence.as_ref()
    }

    pub fn state(&self) -> &FrameHeaderState {
        &self.state
    }

    /// Parse one IVF packet. OBU ordering is preserved and events borrow only
    /// from `packet`.
    pub fn parse_packet<'a>(
        &mut self,
        packet: &'a [u8],
    ) -> Result<Vec<StreamEvent<'a>>, StreamError> {
        let obus = parse_obus(packet)?;
        let mut events = Vec::new();
        for obu in obus {
            match obu.header.obu_type {
                ObuType::TemporalDelimiter => {
                    self.pending_header = None;
                }
                ObuType::SequenceHeader => {
                    let sequence = parse_sequence_header_obu(&obu, None)?;
                    self.sequence = Some(sequence.clone());
                    events.push(StreamEvent::Sequence(sequence));
                }
                ObuType::Frame => {
                    let sequence = self.sequence.as_ref().ok_or(StreamError::MissingSequence)?;
                    let parts = parse_frame_obu(&obu, sequence, &self.state)?;
                    if parts.header.show_existing_frame {
                        return Err(StreamError::ShowExistingInFrameObu);
                    }
                    self.state.apply_frame(&parts.header);
                    let layout = parse_tile_group(parts.tile_group_payload, &parts.header, 0)?;
                    events.push(StreamEvent::CodedFrame(FrameTiles {
                        header: parts.header,
                        payload: parts.tile_group_payload,
                        layout,
                    }));
                    self.pending_header = None;
                }
                ObuType::FrameHeader => {
                    let sequence = self.sequence.as_ref().ok_or(StreamError::MissingSequence)?;
                    let header = parse_frame_header_obu(&obu, sequence, &self.state)?;
                    if header.show_existing_frame {
                        self.state.apply_frame(&header);
                        events.push(StreamEvent::ShowExisting(header));
                    } else {
                        self.state.apply_frame(&header);
                        self.pending_header = Some(header);
                    }
                }
                ObuType::RedundantFrameHeader => {
                    // A redundant header is identical to the preceding coded
                    // header. The entropy decoder will compare it later.
                }
                ObuType::TileGroup => {
                    let header = self
                        .pending_header
                        .take()
                        .ok_or(StreamError::MissingFrameHeader)?;
                    let layout = parse_tile_group(obu.payload, &header, 0)?;
                    events.push(StreamEvent::CodedFrame(FrameTiles {
                        header,
                        payload: obu.payload,
                        layout,
                    }));
                }
                _ => {}
            }
        }
        Ok(events)
    }

    /// Parse every packet in an IVF payload.
    pub fn parse_ivf<'a>(
        &mut self,
        ivf: &'a crate::reference::av1::obu::Ivf<'a>,
    ) -> Result<Vec<Vec<StreamEvent<'a>>>, StreamError> {
        ivf.frames
            .iter()
            .map(|frame| self.parse_packet(frame.payload))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::obu::parse_ivf;

    fn coded_headers(events: &[Vec<StreamEvent<'_>>]) -> Vec<FrameHeader> {
        events
            .iter()
            .flatten()
            .filter_map(|event| match event {
                StreamEvent::CodedFrame(frame) => Some(frame.header.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn one_tile_fixture_reaches_all_coded_frames_and_tiles() {
        let data = include_bytes!("../../../tests/data/av1_320x240_10.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let events = stream.parse_ivf(&ivf).unwrap();
        assert_eq!(
            events
                .iter()
                .flatten()
                .filter(|event| matches!(event, StreamEvent::Sequence(_)))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .flatten()
                .filter(|event| matches!(event, StreamEvent::ShowExisting(_)))
                .count(),
            3
        );
        let coded = coded_headers(&events);
        assert_eq!(coded.len(), 11);
        assert_eq!(
            coded.iter().map(|h| h.order_hint).collect::<Vec<_>>(),
            [0, 8, 4, 2, 1, 3, 6, 5, 7, 8, 9]
        );
        for event in events.iter().flatten() {
            if let StreamEvent::CodedFrame(frame) = event {
                assert_eq!(frame.layout.tiles.len(), 1);
                assert_eq!(frame.layout.tiles[0].bytes, 0..frame.payload.len());
            }
        }
    }

    #[test]
    fn two_by_two_fixture_exposes_four_tile_ranges_for_each_coded_frame() {
        let data = include_bytes!("../../../tests/data/av1_tiles_320x240_10.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let events = stream.parse_ivf(&ivf).unwrap();
        let coded = events
            .iter()
            .flatten()
            .filter_map(|event| match event {
                StreamEvent::CodedFrame(frame) => Some(frame),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(coded.len(), 11);
        for frame in coded {
            assert_eq!((frame.header.tile_cols, frame.header.tile_rows), (2, 2));
            assert_eq!(frame.layout.tiles.len(), 4);
            assert_eq!(frame.layout.tiles[3].bytes.end, frame.payload.len());
        }
    }

    #[test]
    fn codec_lossless_stream_has_pending_header_state_reset_by_delimiter() {
        let data = include_bytes!("../../../tests/data/av1_320x240_10.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let events_0 = stream.parse_packet(ivf.frames[0].payload).unwrap();
        assert!(matches!(events_0[0], StreamEvent::Sequence(_)));
        assert!(stream.pending_header.is_none());
    }
}
