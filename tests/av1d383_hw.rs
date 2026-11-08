use std::path::PathBuf;

use rk_mpp_rs::codec::av1::assembler::{Av1PacketEvent, Av1PacketParser};
use rk_mpp_rs::codec::av1::parser::{parse_ivf, split_obus};
use rk_mpp_rs::codec::av1::picparams::{Av1PicParams, PicParamsPointers};
use rk_mpp_rs::hal::av1d383::Session;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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
    let actual = session.decode_intra(&params, stream).unwrap();
    let expected =
        std::fs::read(root().join("tests/data/av1_vdpu383_key_frame_320x240.yuv")).unwrap();
    assert!(!session.last_hardware_error);
    assert_eq!(actual, expected);
}
