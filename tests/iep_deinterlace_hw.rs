use std::process::Command;

use rk_mpp_rs::sdk::{Codec, Decoder, DecoderConfig, Packet, PacketFlags};
use rk_mpp_rs::service::iep_session::{DeinterlaceConfig, IepDeinterlace};

fn run_binary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rk-mpp-rs"))
        .args(args)
        .output()
        .expect("run rk-mpp-rs")
}

fn generate_padded_h264_fixture() -> Vec<u8> {
    let path = format!("/tmp/rk-mpp-rs-padded-stride-{}.264", std::process::id());
    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=50x34:rate=30",
            "-frames:v",
            "3",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-profile:v",
            "baseline",
            "-pix_fmt",
            "yuv420p",
            "-g",
            "1",
        ])
        .arg(&path)
        .status()
        .expect("run ffmpeg for padded H.264 fixture");
    assert!(
        status.success(),
        "ffmpeg could not generate padded H.264 fixture"
    );
    let stream = std::fs::read(&path).unwrap();
    std::fs::remove_file(path).ok();
    stream
}

/// The capability survey records client 28 as binding but has never driven a
/// job through it. This is the first on-board assertion: opening the session,
/// announcing `IEP_CLIENT_TYPE`, and the two commands the deinterlace path
/// needs must all answer. Run with `cargo test -- --ignored` on RK3588.
#[test]
#[ignore = "requires an RK3588 IEP2 driver and /dev/mpp_service access"]
fn iep_client_28_binds_and_supports_the_deinterlace_commands() {
    let out = run_binary(&["iep-probe"]);
    let log = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "iep-probe failed: {log}");
    assert!(
        log.contains("session init (INIT_CLIENT_TYPE):         binds"),
        "client 28 did not bind:\n{log}"
    );
    assert!(
        log.contains("POLL_HW_FINISH:                          yes"),
        "POLL_HW_FINISH missing:\n{log}"
    );
}

/// Drive one I5O2 deinterlace end to end: a synthetic three-frame YUV420-SP
/// input must yield two reconstructed frames of exactly the input frame size.
/// Byte-exactness against the C `iep2_test` is the shell script's job; this
/// test only proves the ioctl sequence completes and the hardware writes out.
#[test]
#[ignore = "requires an RK3588 IEP2 driver and /dev/dma_heap access"]
fn iep_deinterlace_writes_two_frames_of_input_size() {
    let (w, h) = (320u32, 240u32);
    let frame = (w as usize) * (h as usize) * 3 / 2;
    let pid = std::process::id();
    let in_path = format!("/tmp/rk-mpp-rs-iep-in-{pid}.yuv");
    let out_path = format!("/tmp/rk-mpp-rs-iep-out-{pid}.yuv");

    // prev, current, next: three flat frames differing only in luma level.
    let mut data = Vec::with_capacity(frame * 3);
    for n in 0..3u8 {
        let level = 40u8 + n * 30;
        data.extend_from_slice(&vec![level; frame]);
    }
    std::fs::write(&in_path, &data).unwrap();

    let run = run_binary(&[
        "iep-dei",
        &in_path,
        &out_path,
        &w.to_string(),
        &h.to_string(),
        "TFF",
    ]);
    let log = String::from_utf8_lossy(&run.stdout);
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "iep-dei failed: stdout={log} stderr={err}"
    );

    let out_bytes = std::fs::read(&out_path).unwrap();
    std::fs::remove_file(&in_path).ok();
    std::fs::remove_file(&out_path).ok();
    assert_eq!(
        out_bytes.len(),
        frame * 2,
        "two deinterlaced frames expected"
    );
}

#[test]
#[ignore = "requires RK3588 rkvdec, IEP2 and /dev/dma_heap access"]
fn iep_consumes_decoder_dmabuf_frames_without_copying_through_cpu() {
    let mut decoder =
        Decoder::new(DecoderConfig::new(Codec::H264).with_dmabuf_output(true)).unwrap();
    let stream = include_bytes!("data/h264_i32_cavlc.264");
    decoder
        .submit_packet(Packet::new(stream).with_flags(PacketFlags::EOS))
        .unwrap();
    let frame = decoder.receive_frame().expect("decoded H.264 frame");
    let y = frame.plane(0).unwrap();
    let cfg = DeinterlaceConfig {
        width: frame.width(),
        height: frame.height(),
        hor_stride: y.stride,
        ver_stride: y.data.len() / y.stride,
        ..DeinterlaceConfig::default()
    };
    let mut iep = IepDeinterlace::open(cfg).unwrap();
    for index in 0..3 {
        iep.import_src_frame(index, &frame).unwrap();
    }
    iep.run().unwrap();
    assert!(iep.dst_frame(0).iter().any(|sample| *sample != 0));
}

#[test]
#[ignore = "requires RK3588 rkvdec, IEP2 and /dev/dma_heap access"]
fn iep_decoder_import_matches_explicit_padded_sources() {
    let mut decoder =
        Decoder::new(DecoderConfig::new(Codec::H264).with_dmabuf_output(true)).unwrap();
    let stream = generate_padded_h264_fixture();
    decoder
        .submit_packet(Packet::new(&stream).with_flags(PacketFlags::EOS))
        .unwrap();
    let mut frames = Vec::new();
    while let Some(frame) = decoder.receive_frame() {
        frames.push(frame);
    }
    assert!(!frames.is_empty(), "fixture decoded no frames");

    let first = &frames[0];
    let width = first.width();
    let height = first.height();
    let y = first.plane(0).unwrap();
    let stride = y.stride;
    let ver_stride = y.data.len() / stride;
    assert!(
        stride > width,
        "fixture must expose padded stride: width={width}, stride={stride}"
    );
    let sources = [
        first,
        frames.get(1).unwrap_or(first),
        frames.get(2).unwrap_or(first),
    ];

    let padded_cfg = DeinterlaceConfig {
        width,
        height,
        hor_stride: stride,
        ver_stride,
        ..DeinterlaceConfig::default()
    };
    let mut padded = IepDeinterlace::open(padded_cfg).unwrap();
    for (index, frame) in sources.iter().enumerate() {
        let source = &frame.data()[..stride * ver_stride + stride * ver_stride.div_ceil(2)];
        let slot = padded.src_frame_mut(index);
        assert!(source.len() <= slot.len());
        slot.sync_write_begin().unwrap();
        slot.as_mut_slice()[..source.len()].copy_from_slice(source);
        padded.flush_src(index).unwrap();
    }
    padded.run().unwrap();
    let padded_output: Vec<Vec<u8>> = (0..2)
        .map(|index| padded.dst_frame_compact(index).unwrap())
        .collect();

    let imported_cfg = DeinterlaceConfig {
        width,
        height,
        hor_stride: width,
        ver_stride: height,
        ..DeinterlaceConfig::default()
    };
    let mut imported = IepDeinterlace::open(imported_cfg).unwrap();
    for (index, frame) in sources.iter().enumerate() {
        imported.import_src_frame(index, frame).unwrap();
    }
    assert_eq!(imported.hor_stride(), stride);
    assert_eq!(imported.ver_stride(), ver_stride);
    imported.run().unwrap();

    for (index, expected) in padded_output.iter().enumerate() {
        let actual = imported.dst_frame_compact(index).unwrap();
        assert_eq!(actual.len(), expected.len());
        if actual != *expected {
            let differing = actual
                .iter()
                .zip(expected)
                .filter(|(left, right)| left != right)
                .count();
            let first = actual
                .iter()
                .zip(expected)
                .position(|(left, right)| left != right)
                .unwrap();
            panic!("IEP output slot {index} differs at {differing} samples; first byte {first}");
        }
        assert!(actual.iter().any(|sample| *sample != 0));
    }
}
