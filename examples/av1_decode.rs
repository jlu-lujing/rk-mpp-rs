use rk_mpp_rs::codec::av1::decoder::Decoder;
use rk_mpp_rs::codec::av1::parser::parse_ivf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: av1_decode INPUT.ivf [OUTPUT.yuv]")?;
    let output = args.next().unwrap_or_else(|| "/tmp/av1_rust.yuv".into());
    let data = std::fs::read(input)?;
    let ivf = parse_ivf(&data)?;
    let mut decoder = Decoder::new()?;
    let mut yuv = Vec::new();
    let mut pictures = 0usize;

    for packet in &ivf.frames {
        for picture in decoder
            .decode_packet(packet.data)
            .map_err(|error| format!("AV1 packet decode failed: {error}"))?
        {
            let status = decoder.status();
            eprintln!(
                "picture={pictures} poc={} {}x{} err={:#x} hw=({:#x},{:#x},{:#x}) bytes={}",
                picture.poc,
                picture.width,
                picture.height,
                picture.err_info,
                status.0,
                status.1,
                status.2,
                picture.yuv.len(),
            );
            if picture.err_info != 0 {
                return Err(
                    format!("AV1 picture failed with error {:#x}", picture.err_info).into(),
                );
            }
            yuv.extend_from_slice(&picture.yuv);
            pictures += 1;
        }
    }

    std::fs::write(&output, &yuv)?;
    println!("decoded={pictures} bytes={} output={output}", yuv.len());
    Ok(())
}
