use rk_mpp_rs::codec::av1::assembler::{Av1PacketEvent, Av1PacketParser};
use rk_mpp_rs::codec::av1::parser::{parse_ivf, split_obus};
use rk_mpp_rs::codec::av1::picparams::{Av1PicParams, PicParamsPointers};
use rk_mpp_rs::hal::av1d383::{Frame, Session};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() != 3 {
        return Err("usage: av1d383_probe <in.ivf> <out.yuv>".into());
    }
    let data = std::fs::read(&args[1])?;
    let ivf = parse_ivf(&data)?;
    let packet = ivf.frames[0].data;
    let obus = split_obus(packet)?;
    let mut parser = Av1PacketParser::new();
    parser.begin_packet();
    let mut cursor = 0;
    let mut coded = None;
    while let Some(event) = parser.parse_next_event(&obus, &mut cursor)? {
        if let Av1PacketEvent::Coded(frame) = event {
            coded = Some(frame);
            break;
        }
    }
    let frame = coded.ok_or("first packet has no coded frame")?;
    let sequence = parser.sequence().ok_or("missing sequence header")?;
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
    let end = start + frame.stream_len as usize;
    let stream = packet
        .get(start..end)
        .ok_or("frame stream exceeds packet")?;
    let mut session = Session::open(params.width as usize, params.height as usize)?;
    let mut frame = Frame::new(params.width as usize, params.height as usize, 0)?;
    let yuv = session.decode(&params, stream, &mut frame, &[None; 8])?;
    std::fs::write(&args[2], yuv)?;
    println!(
        "decoded {}x{} status={:?} hardware_error={}",
        params.width, params.height, session.last_status, session.last_hardware_error
    );
    Ok(())
}
