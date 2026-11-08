use std::fs;
use std::io::Write;

use rk_mpp_rs::encoder::{Encoder, EncoderConfig, InputFrame};
use rk_mpp_rs::sdk::{Codec, PixelFormat};

fn parse_codec(name: &str) -> Option<Codec> {
    match name {
        "h264" => Some(Codec::H264),
        "h265" => Some(Codec::H265),
        "jpeg" | "mjpeg" => Some(Codec::Mjpeg),
        _ => None,
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 7 {
        return Err(format!(
            "usage: {} <h264|h265|jpeg> <input.nv12> <output> <width> <height> <qp> [gop]",
            args[0]
        ));
    }
    let codec =
        parse_codec(&args[1]).ok_or_else(|| format!("unsupported example codec: {}", args[1]))?;
    let width = args[4]
        .parse::<usize>()
        .map_err(|error| format!("width: {error}"))?;
    let height = args[5]
        .parse::<usize>()
        .map_err(|error| format!("height: {error}"))?;
    let qp = args[6]
        .parse::<u8>()
        .map_err(|error| format!("qp/quality: {error}"))?;
    let gop = args
        .get(7)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(if codec == Codec::Mjpeg { 1 } else { 6 });

    let mut config = EncoderConfig::new(codec, width, height)
        .with_input_format(PixelFormat::Nv12)
        .with_qp(qp)
        .with_gop(gop);
    if codec == Codec::Mjpeg {
        config = config.with_jpeg_quality(u32::from(qp));
    }
    let mut encoder = Encoder::new(config).map_err(|error| error.to_string())?;
    let input = fs::read(&args[2]).map_err(|error| format!("{}: {error}", args[2]))?;
    let frame_size = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .map(|bytes| bytes / 2)
        .ok_or_else(|| "input frame size overflow".to_string())?;
    let frames = if codec == Codec::Mjpeg {
        1
    } else {
        input.len() / frame_size
    };
    if frames == 0 || input.len() < frames * frame_size {
        return Err(format!(
            "{}: need at least one {width}x{height} NV12 frame",
            args[2]
        ));
    }

    let mut output =
        std::io::BufWriter::new(fs::File::create(&args[3]).map_err(|error| error.to_string())?);
    let mut packets = 0usize;
    for index in 0..frames {
        let frame = InputFrame::new(&input[index * frame_size..(index + 1) * frame_size])
            .with_pts(index as i64);
        encoder
            .submit_frame(frame)
            .map_err(|error| error.to_string())?;
        while let Some(packet) = encoder.receive_packet() {
            output
                .write_all(packet.data())
                .map_err(|error| error.to_string())?;
            packets += 1;
        }
    }
    encoder.flush();
    while let Some(packet) = encoder.receive_packet() {
        output
            .write_all(packet.data())
            .map_err(|error| error.to_string())?;
        packets += 1;
    }
    output.flush().map_err(|error| error.to_string())?;
    println!("encoded {frames} frames into {packets} packets");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
