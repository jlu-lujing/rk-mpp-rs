//! Packet-level AV1 frame assembly.
//!
//! MPP keeps a frame header alive between `OBU_FRAME_HEADER` and the tile
//! groups that complete the picture. This state machine mirrors that boundary
//! and preserves every tile's byte range for the hardware packet.

use std::ops::Range;

use super::cdf::Av1Cdf;
use super::parser::{
    obu_in_operating_point, parse_frame_header, parse_frame_obu, parse_sequence_header,
    parse_tile_group_obu, split_obus, Av1Error, FrameDecodeState, FrameHeader, Obu, ObuType,
    ReferenceFrame, SequenceHeader, TileGroupAccumulator, MAX_TILES,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Av1CodedFrame {
    pub header: FrameHeader,
    pub cdf: Av1Cdf,
    pub references_at_header: [ReferenceFrame; 8],
    /// Start of the C-equivalent split frame inside the original packet.
    pub frame_start: u32,
    /// Length of the C-equivalent split frame, including its trailing OBU.
    pub stream_len: u32,
    /// Tile-size header offset relative to `frame_start`.
    pub frame_tag_size: u32,
    /// Absolute byte ranges in the packet passed to `parse_packet`.
    pub tile_ranges: [Option<Range<usize>>; MAX_TILES],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Av1PacketEvent {
    ShowExisting(FrameHeader),
    Coded(Av1CodedFrame),
}

#[derive(Debug, Clone)]
struct PendingFrame {
    header: FrameHeader,
    state_after_header: Box<FrameDecodeState>,
    frame_start: usize,
    header_payload: Vec<u8>,
    tiles: TileGroupAccumulator,
    frame_tag_offset: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Av1PacketParser {
    sequence: Option<SequenceHeader>,
    state: Box<FrameDecodeState>,
    pending: Option<PendingFrame>,
    frame_start: usize,
}

impl Av1PacketParser {
    pub fn new() -> Self {
        Self {
            sequence: None,
            state: Box::new(FrameDecodeState::new()),
            pending: None,
            frame_start: 0,
        }
    }

    pub fn sequence(&self) -> Option<&SequenceHeader> {
        self.sequence.as_ref()
    }

    pub fn frame_state(&self) -> &FrameDecodeState {
        &self.state
    }

    /// Start consuming one temporal unit with [`Self::parse_next_event`].
    ///
    /// The caller must apply the hardware-written CDFs before asking for the
    /// next event. MPP parses and submits AV1 frames one at a time for exactly
    /// this reason: a later frame header may depend on the CDF refreshed by an
    /// earlier frame in the same temporal unit.
    pub fn begin_packet(&mut self) {
        if self.pending.is_none() {
            self.frame_start = 0;
        }
    }

    /// Consume OBUs until the next complete frame event is available.
    ///
    /// `obus` must have been split from one packet and `cursor` must start at
    /// zero after [`Self::begin_packet`]. Each OBU is committed atomically.
    pub fn parse_next_event(
        &mut self,
        obus: &[Obu<'_>],
        cursor: &mut usize,
    ) -> Result<Option<Av1PacketEvent>, Av1Error> {
        while *cursor < obus.len() {
            let obu = &obus[*cursor];
            let mut next = self.clone();
            let next_event = next.parse_obu(obu)?;
            *cursor += 1;
            *self = next;
            if next_event.is_some() {
                return Ok(next_event);
            }
        }
        Ok(None)
    }

    /// Import the CDF image written by AV1D_VPU after a completed job.
    ///
    /// MPP applies this through `av1d_parser_update()` before parsing the next
    /// primary-reference frame. Skipping it leaves every following frame with
    /// the header-time probability table instead of the entropy-updated table.
    pub fn apply_hardware_cdf(
        &mut self,
        refresh_frame_flags: u8,
        frame_is_intra: bool,
        hardware: &[u8],
    ) {
        self.state
            .cdfs
            .apply_hardware_update(refresh_frame_flags, frame_is_intra, hardware);
    }

    /// Parse one temporal unit and return all complete frame events in it.
    ///
    /// State changes are committed only when the entire packet succeeds.
    pub fn parse_packet(&mut self, packet: &[u8]) -> Result<Vec<Av1PacketEvent>, Av1Error> {
        let obus = split_obus(packet)?;
        let mut next = self.clone();
        next.begin_packet();
        let mut cursor = 0;
        let mut events = Vec::new();
        while let Some(event) = next.parse_next_event(&obus, &mut cursor)? {
            events.push(event);
        }
        *self = next;
        Ok(events)
    }

    fn parse_obu(&mut self, obu: &Obu<'_>) -> Result<Option<Av1PacketEvent>, Av1Error> {
        let mut events = Vec::new();
        let operating_point_idc = self
            .sequence
            .as_ref()
            .and_then(|sequence| sequence.operating_points.first())
            .map_or(0, |operating_point| operating_point.idc);
        if !obu_in_operating_point(obu, operating_point_idc) {
            return Ok(None);
        }

        match obu.obu_type {
            ObuType::SequenceHeader => {
                if self.pending.is_some() {
                    return Err(Av1Error::InvalidValue(
                        "sequence header before frame completion",
                    ));
                }
                self.sequence = Some(parse_sequence_header(obu.payload)?);
            }
            ObuType::Frame => {
                self.parse_frame_obu(obu, &mut events)?;
            }
            ObuType::FrameHeader | ObuType::RedundantFrameHeader => {
                self.parse_frame_header_obu(obu, &mut events)?;
            }
            ObuType::TileGroup => {
                self.parse_tile_group_obu(obu, &mut events)?;
            }
            ObuType::TemporalDelimiter
            | ObuType::Metadata
            | ObuType::TileList
            | ObuType::Padding => {}
            _ => {}
        }
        debug_assert!(events.len() <= 1);
        Ok(events.pop())
    }

    fn required_sequence(&self) -> Result<&SequenceHeader, Av1Error> {
        self.sequence
            .as_ref()
            .ok_or(Av1Error::InvalidValue("missing sequence header"))
    }

    fn parse_frame_obu(
        &mut self,
        obu: &Obu<'_>,
        events: &mut Vec<Av1PacketEvent>,
    ) -> Result<(), Av1Error> {
        if self.pending.is_some() {
            return Err(Av1Error::InvalidValue(
                "OBU_FRAME before tile groups completed",
            ));
        }
        let mut state = (*self.state).clone();
        let frame = parse_frame_obu(obu, self.required_sequence()?, &mut state)?;
        let mut tiles = TileGroupAccumulator::new();
        tiles.insert_group(
            frame.tile_data,
            &frame.header,
            frame.tile_group,
            obu.start + frame.tag_size,
        )?;
        let tile_ranges = tiles.finish(&frame.header)?;
        let cdf = state.active_cdf;
        let references_at_header = state.references_at_header;
        let frame_tag_offset = u32::try_from(obu.start + frame.tag_size).unwrap_or(u32::MAX);
        let frame_start = u32::try_from(self.frame_start).unwrap_or(u32::MAX);
        let frame_end = u32::try_from(obu.start + obu.raw.len()).unwrap_or(u32::MAX);
        let frame_tag_size = frame_tag_offset.saturating_sub(frame_start);
        *self.state = state;
        events.push(Av1PacketEvent::Coded(Av1CodedFrame {
            header: frame.header,
            cdf,
            references_at_header,
            frame_start,
            stream_len: frame_end.saturating_sub(frame_start),
            frame_tag_size,
            tile_ranges,
        }));
        self.frame_start = obu.start + obu.raw.len();
        Ok(())
    }

    fn parse_frame_header_obu(
        &mut self,
        obu: &Obu<'_>,
        events: &mut Vec<Av1PacketEvent>,
    ) -> Result<(), Av1Error> {
        if let Some(pending) = &self.pending {
            if obu.obu_type != ObuType::RedundantFrameHeader {
                return Err(Av1Error::InvalidValue("repeated frame header"));
            }
            if !bits_match(
                &pending.header_payload,
                obu.payload,
                pending.header.header_bits,
            ) {
                return Err(Av1Error::InvalidValue("redundant frame header differs"));
            }
            return Ok(());
        }

        let mut state = (*self.state).clone();
        let header = parse_frame_header(obu.payload, self.required_sequence()?, &mut state)?;
        if header.show_existing_frame {
            *self.state = state;
            events.push(Av1PacketEvent::ShowExisting(header));
            return Ok(());
        }

        let header_bytes = header.header_bits.div_ceil(8);
        let header_payload = obu
            .payload
            .get(..header_bytes)
            .ok_or(Av1Error::InvalidValue("truncated frame header"))?
            .to_vec();
        self.pending = Some(PendingFrame {
            header,
            state_after_header: Box::new(state),
            frame_start: self.frame_start,
            header_payload,
            tiles: TileGroupAccumulator::new(),
            frame_tag_offset: None,
        });
        Ok(())
    }

    fn parse_tile_group_obu(
        &mut self,
        obu: &Obu<'_>,
        events: &mut Vec<Av1PacketEvent>,
    ) -> Result<(), Av1Error> {
        let mut pending = self
            .pending
            .take()
            .ok_or(Av1Error::InvalidValue("missing frame header"))?;
        let group = parse_tile_group_obu(obu, &pending.header)?;
        let frame_tag_offset = u32::try_from(obu.start + group.tag_size).unwrap_or(u32::MAX);
        pending.frame_tag_offset.get_or_insert(frame_tag_offset);
        pending.tiles.insert_group(
            group.tile_data,
            &pending.header,
            group.tile_group,
            obu.start + group.tag_size,
        )?;

        let num_tiles = pending
            .header
            .tile_cols
            .checked_mul(pending.header.tile_rows)
            .ok_or(Av1Error::InvalidValue("tile count"))?;
        if group.tile_group.tg_end + 1 != num_tiles {
            self.pending = Some(pending);
            return Ok(());
        }

        let tile_ranges = pending.tiles.finish(&pending.header)?;
        let cdf = pending.state_after_header.active_cdf;
        let references_at_header = pending.state_after_header.references_at_header;
        let frame_start = u32::try_from(pending.frame_start).unwrap_or(u32::MAX);
        let frame_end = u32::try_from(obu.start + obu.raw.len()).unwrap_or(u32::MAX);
        let frame_tag_size = pending
            .frame_tag_offset
            .unwrap_or(frame_start)
            .saturating_sub(frame_start);
        self.state = pending.state_after_header;
        events.push(Av1PacketEvent::Coded(Av1CodedFrame {
            header: pending.header,
            cdf,
            references_at_header,
            frame_start,
            stream_len: frame_end.saturating_sub(frame_start),
            frame_tag_size,
            tile_ranges,
        }));
        self.frame_start = obu.start + obu.raw.len();
        Ok(())
    }
}

impl Default for Av1PacketParser {
    fn default() -> Self {
        Self::new()
    }
}

fn bits_match(expected: &[u8], actual: &[u8], bits: usize) -> bool {
    if actual.len().saturating_mul(8) < bits {
        return false;
    }
    for bit in 0..bits {
        let expected_bit = expected[bit / 8] >> (7 - bit % 8) & 1;
        let actual_bit = actual[bit / 8] >> (7 - bit % 8) & 1;
        if expected_bit != actual_bit {
            return false;
        }
    }
    true
}
