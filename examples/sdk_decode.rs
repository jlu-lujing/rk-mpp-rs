use std::fs;
use std::io::Write;

use rk_mpp_rs::sdk::{Codec, Decoder, DecoderConfig, Packet, PacketFlags, PixelFormat};

fn parse_codec(name: &str) -> Option<(Codec, bool)> {
    match name {
        "h264" => Some((Codec::H264, false)),
        "mjpeg" | "jpeg" => Some((Codec::Mjpeg, false)),
        "avs" | "avsplus" => Some((Codec::Avs, false)),
        "avs-vproc" => Some((Codec::Avs, true)),
        _ => None,
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        return Err(format!(
            "usage: {} <h264|mjpeg|avs|avs-vproc> <input> <output.yuv>",
            args[0]
        ));
    }
    let (codec, vproc) =
        parse_codec(&args[1]).ok_or_else(|| format!("unsupported example codec: {}", args[1]))?;
    let input = fs::read(&args[2]).map_err(|error| format!("{}: {error}", args[2]))?;
    let mut config = DecoderConfig::new(codec).with_output_format(PixelFormat::I420);
    if vproc {
        config = config.with_vproc_output(true);
    }
    let mut decoder = Decoder::new(config).map_err(|error| error.to_string())?;
    let mut output =
        std::io::BufWriter::new(fs::File::create(&args[3]).map_err(|error| error.to_string())?);

    let mut offset = 0usize;
    let mut frames = 0usize;
    while offset < input.len() {
        let end = (offset + 64 * 1024).min(input.len());
        let flags = if end == input.len() {
            PacketFlags::EOS
        } else {
            PacketFlags::empty()
        };
        decoder
            .submit_packet(Packet::new(&input[offset..end]).with_flags(flags))
            .map_err(|error| error.to_string())?;
        offset = end;
        while let Some(frame) = decoder.receive_frame() {
            for plane in 0..3 {
                if let Some(plane) = frame.plane(plane) {
                    output
                        .write_all(plane.data)
                        .map_err(|error| error.to_string())?;
                }
            }
            frames += 1;
        }
    }
    decoder.flush().map_err(|error| error.to_string())?;
    while let Some(frame) = decoder.receive_frame() {
        for plane in 0..3 {
            if let Some(plane) = frame.plane(plane) {
                output
                    .write_all(plane.data)
                    .map_err(|error| error.to_string())?;
            }
        }
        frames += 1;
    }
    output.flush().map_err(|error| error.to_string())?;
    println!("decoded {frames} frames");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
