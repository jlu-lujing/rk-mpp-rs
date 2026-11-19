use std::collections::BTreeSet;
use std::io::Write;

use rk_mpp_rs::codec::av1::assembler::{Av1PacketEvent, Av1PacketParser};
use rk_mpp_rs::codec::av1::parser::{parse_ivf, split_obus};
use rk_mpp_rs::codec::av1::picparams::{Av1PicParams, PicParamsPointers};
use rk_mpp_rs::hal::av1d383::{Frame, Session};

const MAX_FRAMES: usize = 24;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() != 3 {
        return Err("usage: av1d383_sequence_probe <in.ivf> <out.yuv>".into());
    }
    let data = std::fs::read(&args[1])?;
    let ivf = parse_ivf(&data)?;
    let mut parser = Av1PacketParser::new();
    let mut session: Option<Session> = None;
    let mut frames: Vec<Option<Frame>> = (0..MAX_FRAMES).map(|_| None).collect();
    let mut reference_slots: [Option<usize>; 8] = [None; 8];
    let mut output = std::io::BufWriter::new(std::fs::File::create(&args[2])?);
    let mut output_frames = 0usize;

    for packet in &ivf.frames {
        parser.begin_packet();
        let obus = split_obus(packet.data)?;
        let mut cursor = 0;
        while let Some(event) = parser.parse_next_event(&obus, &mut cursor)? {
            match event {
                Av1PacketEvent::ShowExisting(header) => {
                    let slot = header.frame_to_show_map_idx as usize;
                    let index = reference_slots[slot].ok_or("show-existing slot is empty")?;
                    let frame = frames[index]
                        .as_mut()
                        .ok_or("released show-existing frame")?;
                    output.write_all(&frame.read_planar(Vec::new())?)?;
                    output_frames += 1;
                    for slot in 0..8 {
                        if header.refresh_frame_flags & (1 << slot) != 0 {
                            reference_slots[slot] = Some(index);
                        }
                    }
                }
                Av1PacketEvent::Coded(coded) => {
                    let sequence = parser.sequence().ok_or("missing sequence header")?;
                    let width = coded.header.width as usize;
                    let height = coded.header.height as usize;
                    let session_ref = match session.as_mut() {
                        Some(session) => session,
                        None => session.insert(Session::open(width, height)?),
                    };

                    let live = reference_slots
                        .iter()
                        .flatten()
                        .copied()
                        .collect::<BTreeSet<_>>();
                    let current_index = (0..MAX_FRAMES)
                        .find(|index| !live.contains(index))
                        .ok_or("AV1 frame texture pool exhausted")?;
                    if frames[current_index]
                        .as_ref()
                        .is_none_or(|frame| frame.texture_index as usize != current_index)
                    {
                        frames[current_index] = Some(Frame::new(
                            width,
                            height,
                            current_index as u8,
                            u32::from(sequence.bit_depth),
                        )?);
                    }

                    let texture_slots = std::array::from_fn(|slot| {
                        reference_slots[slot]
                            .and_then(|index| frames[index].as_ref())
                            .map_or(0xff, |frame| frame.texture_index)
                    });
                    let params = Av1PicParams::build(
                        sequence,
                        &coded.header,
                        &coded.references_at_header,
                        &coded.cdf,
                        &coded.tile_ranges,
                        coded.frame_start,
                        coded.frame_tag_size,
                        current_index as u8,
                        &texture_slots,
                        PicParamsPointers::default(),
                    );
                    let start = coded.frame_start as usize;
                    let stream = &packet.data[start..start + coded.stream_len as usize];
                    let mut current = frames[current_index].take().unwrap();
                    let references: [Option<&Frame>; 8] = std::array::from_fn(|slot| {
                        reference_slots[slot].and_then(|index| frames[index].as_ref())
                    });
                    let yuv = session_ref.decode(
                        &params,
                        stream,
                        &mut current,
                        &references,
                        Vec::new(),
                    )?;
                    frames[current_index] = Some(current);

                    for slot in 0..8 {
                        if coded.header.refresh_frame_flags & (1 << slot) != 0 {
                            reference_slots[slot] = Some(current_index);
                        }
                    }
                    if coded.header.show_frame {
                        output.write_all(&yuv)?;
                        output_frames += 1;
                    }
                }
            }
        }
    }

    output.flush()?;
    let session = session.ok_or("no coded AV1 frame")?;
    println!(
        "decoded {} frames status={:?} hardware_error={}",
        output_frames, session.last_status, session.last_hardware_error
    );
    Ok(())
}
