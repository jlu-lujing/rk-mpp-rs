//! Decode one H.264 stream through CPU and DMA-BUF frame output, then scale
//! both directly from strided NV12 and require identical RGB model input.

use std::path::PathBuf;

use rk_mpp_rs::scale::Scaler;
use rk_mpp_rs::sdk::{Codec, Decoder, DecoderConfig, Packet, PacketFlags, PixelFormat};

fn decode(data: &[u8], dmabuf: bool) -> rk_mpp_rs::sdk::Frame {
    let config = DecoderConfig::new(Codec::H264)
        .with_output_format(PixelFormat::Nv12)
        .with_dmabuf_output(dmabuf);
    let mut decoder = Decoder::new(config).unwrap();
    let packet = Packet::new(data).with_flags(PacketFlags::EOS);
    decoder.submit_packet(packet).unwrap();
    decoder
        .receive_frame()
        .expect("H.264 stream did not produce a frame")
}

fn main() {
    let args = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    assert_eq!(args.len(), 2, "usage: sdk_decode_scale <in.264> <out.rgb>");
    let data = std::fs::read(&args[0]).unwrap();
    let cpu = decode(&data, false);
    let dma = decode(&data, true);
    assert!(dma.uses_dmabuf());
    assert!(!cpu.uses_dmabuf());
    assert_eq!((cpu.width(), cpu.height()), (dma.width(), dma.height()));

    let mut cpu_rgb = vec![0u8; 640 * 640 * 3];
    let mut dma_rgb = vec![0u8; 640 * 640 * 3];
    let cpu_geometry = cpu
        .letterbox_nv12(&mut Scaler::new(640, 640), &mut cpu_rgb)
        .unwrap();
    let dma_geometry = dma
        .letterbox_nv12(&mut Scaler::new(640, 640), &mut dma_rgb)
        .unwrap();
    assert_eq!(
        (
            cpu_geometry.pad_left,
            cpu_geometry.pad_top,
            cpu_geometry.new_w,
            cpu_geometry.new_h,
        ),
        (
            dma_geometry.pad_left,
            dma_geometry.pad_top,
            dma_geometry.new_w,
            dma_geometry.new_h,
        )
    );
    assert_eq!(dma_rgb, cpu_rgb);
    std::fs::write(&args[1], &dma_rgb).unwrap();
    println!(
        "PASS dma-buf={} {}x{} -> {}x{} pad={},{}",
        dma.uses_dmabuf(),
        dma.width(),
        dma.height(),
        dma_geometry.new_w,
        dma_geometry.new_h,
        dma_geometry.pad_left,
        dma_geometry.pad_top,
    );
}
