//! RK3576 VDPU383 AV1 decoder front-end.
//!
//! The syntax parser and reference-slot policy are shared with the RK3588
//! decoder; only the hardware session and frame layout differ.

use std::collections::BTreeSet;
use std::io;

use crate::codec::av1::assembler::{Av1CodedFrame, Av1PacketEvent, Av1PacketParser};
use crate::codec::av1::decoder::Picture;
use crate::codec::av1::parser::{split_obus, FrameHeader};
use crate::codec::av1::picparams::{Av1PicParams, PicParamsPointers};
use crate::hal::av1d383::{Frame, Session};

const MAX_FRAMES: usize = 24;

#[derive(Debug, Clone, Copy)]
struct Reference {
    frame: usize,
    poc: u32,
}

pub struct Decoder {
    parser: Av1PacketParser,
    session: Option<Session>,
    frames: Vec<Option<Frame>>,
    references: [Option<Reference>; 8],
    width: usize,
    height: usize,
    bit_depth: u32,
    next_poc: u32,
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            parser: Av1PacketParser::new(),
            session: None,
            frames: (0..MAX_FRAMES).map(|_| None).collect(),
            references: [None; 8],
            width: 0,
            height: 0,
            bit_depth: 0,
            next_poc: 1,
        }
    }

    pub fn status(&self) -> (u32, u32, u32) {
        self.session
            .as_ref()
            .map(|session| session.last_status)
            .unwrap_or((0, 0, 0))
    }

    pub fn recover_session(&mut self) -> io::Result<()> {
        if let Some(session) = self.session.as_mut() {
            session.recover_session()?;
        }
        Ok(())
    }

    /// Decode one temporal unit and return every displayed picture in it.
    pub fn decode_packet(&mut self, packet: &[u8]) -> Result<Vec<Picture>, String> {
        let obus = split_obus(packet).map_err(|error| error.to_string())?;
        self.parser.begin_packet();
        let mut cursor = 0;
        let mut output = Vec::new();
        while let Some(event) = self
            .parser
            .parse_next_event(&obus, &mut cursor)
            .map_err(|error| error.to_string())?
        {
            match event {
                Av1PacketEvent::ShowExisting(header) => {
                    output.push(self.show_existing(&header)?);
                }
                Av1PacketEvent::Coded(frame) => {
                    if let Some(picture) = self.decode_frame(packet, &frame)? {
                        output.push(picture);
                    }
                }
            }
        }
        Ok(output)
    }

    fn decode_frame(
        &mut self,
        packet: &[u8],
        coded: &Av1CodedFrame,
    ) -> Result<Option<Picture>, String> {
        let sequence = self
            .parser
            .sequence()
            .cloned()
            .ok_or("missing AV1 sequence header")?;
        let width = coded.header.width as usize;
        let height = coded.header.height as usize;
        let bit_depth = u32::from(sequence.bit_depth);
        self.ensure_geometry(width, height, bit_depth)?;

        let poc = self.next_poc;
        self.next_poc = if self.next_poc == u32::MAX {
            1
        } else {
            self.next_poc + 1
        };
        let live = self
            .references
            .iter()
            .flatten()
            .map(|reference| reference.frame)
            .collect::<BTreeSet<_>>();
        let frame_index = self
            .frames
            .iter()
            .enumerate()
            .find_map(|(index, frame)| (!live.contains(&index) && frame.is_none()).then_some(index))
            .or_else(|| {
                self.frames.iter().enumerate().find_map(|(index, frame)| {
                    (!live.contains(&index) && frame.is_some()).then_some(index)
                })
            })
            .ok_or("VDPU383 AV1 DPB exhausted all frame slots")?;
        if self.frames[frame_index]
            .as_ref()
            .is_none_or(|frame| frame.texture_index as usize != frame_index)
        {
            self.frames[frame_index] = Some(
                Frame::new(width, height, frame_index as u8, bit_depth)
                    .map_err(|error| format!("VDPU383 AV1 frame: {error}"))?,
            );
        }

        let texture_slots = std::array::from_fn(|slot| {
            self.references[slot]
                .and_then(|reference| self.frames[reference.frame].as_ref())
                .map_or(0xff, |frame| frame.texture_index)
        });
        let params = Av1PicParams::build(
            &sequence,
            &coded.header,
            &coded.references_at_header,
            &coded.cdf,
            &coded.tile_ranges,
            coded.frame_start,
            coded.frame_tag_size,
            frame_index as u8,
            &texture_slots,
            PicParamsPointers::default(),
        );
        let start = coded.frame_start as usize;
        let end = start
            .checked_add(coded.stream_len as usize)
            .filter(|end| *end <= packet.len())
            .ok_or("AV1 split frame exceeds packet")?;
        let mut current = self.frames[frame_index].take().unwrap();
        let references: [Option<&Frame>; 8] = std::array::from_fn(|slot| {
            self.references[slot].and_then(|reference| self.frames[reference.frame].as_ref())
        });
        let yuv = self
            .session
            .as_mut()
            .unwrap()
            .decode(&params, &packet[start..end], &mut current, &references)
            .map_err(|error| error.to_string());
        self.frames[frame_index] = Some(current);
        let yuv = yuv?;
        for slot in 0..8 {
            if coded.header.refresh_frame_flags & (1 << slot) != 0 {
                self.references[slot] = Some(Reference {
                    frame: frame_index,
                    poc,
                });
            }
        }
        if !coded.header.show_frame {
            return Ok(None);
        }
        Ok(Some(Picture {
            width,
            height,
            bit_depth,
            poc,
            yuv,
            err_info: 0,
        }))
    }

    fn show_existing(&mut self, header: &FrameHeader) -> Result<Picture, String> {
        let slot = header.frame_to_show_map_idx as usize;
        let reference = self.references[slot]
            .ok_or_else(|| format!("show-existing AV1 reference slot {slot} is empty"))?;
        let frame = self.frames[reference.frame]
            .as_mut()
            .ok_or("show-existing AV1 frame was released")?;
        let yuv = frame.read_planar().map_err(|error| error.to_string())?;
        for slot in 0..8 {
            if header.refresh_frame_flags & (1 << slot) != 0 {
                self.references[slot] = Some(reference);
            }
        }
        Ok(Picture {
            width: header.upscaled_width as usize,
            height: header.height as usize,
            bit_depth: self.bit_depth,
            poc: reference.poc,
            yuv,
            err_info: 0,
        })
    }

    fn ensure_geometry(
        &mut self,
        width: usize,
        height: usize,
        bit_depth: u32,
    ) -> Result<(), String> {
        if width == 0 || height == 0 {
            return Err("invalid AV1 frame geometry".to_string());
        }
        if self.session.is_some()
            && self.width == width
            && self.height == height
            && self.bit_depth == bit_depth
        {
            return Ok(());
        }
        self.session = Some(
            Session::open(width, height).map_err(|error| format!("open VDPU383 AV1: {error}"))?,
        );
        for frame in &mut self.frames {
            *frame = None;
        }
        self.references = [None; 8];
        self.width = width;
        self.height = height;
        self.bit_depth = bit_depth;
        Ok(())
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}
