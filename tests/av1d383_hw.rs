use std::collections::BTreeSet;
use std::path::PathBuf;

use rk_mpp_rs::codec::av1::assembler::{Av1PacketEvent, Av1PacketParser};
use rk_mpp_rs::codec::av1::parser::{parse_ivf, split_obus};
use rk_mpp_rs::codec::av1::picparams::{Av1PicParams, PicParamsPointers};
use rk_mpp_rs::hal::av1d383::{Frame, Session};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn decode_vdpu383_sequence(path: &str) -> (usize, u32, Vec<u8>, bool) {
    const MAX_FRAMES: usize = 24;

    let data = std::fs::read(root().join(path)).unwrap();
    let ivf = parse_ivf(&data).unwrap();
    let mut parser = Av1PacketParser::new();
    let mut session: Option<Session> = None;
    let mut frames: Vec<Option<Frame>> = (0..MAX_FRAMES).map(|_| None).collect();
    let mut reference_slots: [Option<usize>; 8] = [None; 8];
    let mut output = Vec::new();
    let mut output_frames = 0;
    let mut bit_depth = 0;

    for packet in &ivf.frames {
        parser.begin_packet();
        let obus = split_obus(packet.data).unwrap();
        let mut cursor = 0;
        while let Some(event) = parser.parse_next_event(&obus, &mut cursor).unwrap() {
            match event {
                Av1PacketEvent::ShowExisting(header) => {
                    let slot = header.frame_to_show_map_idx as usize;
                    let index = reference_slots[slot].unwrap();
                    output
                        .extend_from_slice(&frames[index].as_mut().unwrap().read_planar().unwrap());
                    output_frames += 1;
                    for slot in 0..8 {
                        if header.refresh_frame_flags & (1 << slot) != 0 {
                            reference_slots[slot] = Some(index);
                        }
                    }
                }
                Av1PacketEvent::Coded(coded) => {
                    let sequence = parser.sequence().unwrap();
                    bit_depth = u32::from(sequence.bit_depth);
                    let width = coded.header.width as usize;
                    let height = coded.header.height as usize;
                    let session = session.get_or_insert(Session::open(width, height).unwrap());
                    let live = reference_slots
                        .iter()
                        .flatten()
                        .copied()
                        .collect::<BTreeSet<_>>();
                    let current_index =
                        (0..MAX_FRAMES).find(|index| !live.contains(index)).unwrap();
                    if frames[current_index]
                        .as_ref()
                        .is_none_or(|frame| frame.texture_index as usize != current_index)
                    {
                        frames[current_index] = Some(
                            Frame::new(width, height, current_index as u8, bit_depth).unwrap(),
                        );
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
                    let yuv = session
                        .decode(&params, stream, &mut current, &references)
                        .unwrap();
                    frames[current_index] = Some(current);
                    for slot in 0..8 {
                        if coded.header.refresh_frame_flags & (1 << slot) != 0 {
                            reference_slots[slot] = Some(current_index);
                        }
                    }
                    if coded.header.show_frame {
                        output.extend_from_slice(&yuv);
                        output_frames += 1;
                    }
                }
            }
        }
    }

    let hardware_error = session.unwrap().last_hardware_error;
    (output_frames, bit_depth, output, hardware_error)
}

#[test]
#[ignore = "requires RK3576 VDPU383 and /dev/mpp_service"]
fn first_vdpu383_av1_keyframe_matches_c_mpp() {
    let data = std::fs::read(root().join("tests/data/av1_tiles_320x240_10.ivf")).unwrap();
    let ivf = parse_ivf(&data).unwrap();
    let packet = ivf.frames[0].data;
    let obus = split_obus(packet).unwrap();
    let mut parser = Av1PacketParser::new();
    parser.begin_packet();
    let mut cursor = 0;
    let mut coded = None;
    while let Some(event) = parser.parse_next_event(&obus, &mut cursor).unwrap() {
        if let Av1PacketEvent::Coded(frame) = event {
            coded = Some(frame);
            break;
        }
    }
    let frame = coded.unwrap();
    let sequence = parser.sequence().unwrap();
    let params = Av1PicParams::build(
        sequence,
        &frame.header,
        &frame.references_at_header,
        &frame.cdf,
        &frame.tile_ranges,
        frame.frame_start,
        frame.frame_tag_size,
        0,
        &[0xff; 8],
        PicParamsPointers::default(),
    );
    let start = frame.frame_start as usize;
    let stream = &packet[start..start + frame.stream_len as usize];
    let mut session = Session::open(params.width as usize, params.height as usize).unwrap();
    let mut output = Frame::new(
        params.width as usize,
        params.height as usize,
        0,
        u32::from(params.bitdepth),
    )
    .unwrap();
    let actual = session
        .decode(&params, stream, &mut output, &[None; 8])
        .unwrap();
    let expected =
        std::fs::read(root().join("tests/data/av1_vdpu383_key_frame_320x240.yuv")).unwrap();
    assert!(!session.last_hardware_error);
    assert_eq!(actual, expected);
}

#[test]
#[ignore = "requires RK3576 VDPU383 and /dev/mpp_service"]
fn full_vdpu383_av1_sequence_matches_c_mpp() {
    let (frames, bit_depth, output, hardware_error) =
        decode_vdpu383_sequence("tests/data/av1_tiles_320x240_10.ivf");
    assert!(!hardware_error);
    assert_eq!(frames, 10);
    assert_eq!(bit_depth, 8);
    let expected =
        std::fs::read(root().join("tests/data/av1_vdpu383_sequence_320x240.yuv")).unwrap();
    assert_eq!(output, expected);
}

#[test]
#[ignore = "requires RK3576 VDPU383 and /dev/mpp_service"]
fn ten_bit_vdpu383_av1_sequence_matches_c_mpp() {
    let (frames, bit_depth, output, hardware_error) =
        decode_vdpu383_sequence("tests/data/film_grain_10bit.ivf");
    assert!(!hardware_error);
    assert_eq!(frames, 10);
    assert_eq!(bit_depth, 10);
    assert_eq!(output.len(), 10 * 320 * 240 * 3);
    assert_eq!(fnv1a(&output), 0x460f2d59098e4320);
}
