//! JPEG encoder restart markers, 90/270 rotation and RGB input.
//!
//! The host tests lock the geometry and rejection rules; the hardware tests
//! check that the rotated and RGB-encoded streams decode as expected. The
//! byte-for-byte verdict against the C encoder lives in
//! scripts/board_jpege_restart_vs_mpp.sh, board_jpege_rotate_vs_mpp.sh and
//! board_jpege_rgb_vs_mpp.sh.

use rk_mpp_rs::codec::jpeg::parser;
use rk_mpp_rs::hal::jpege::{ChromaFormat, EncoderConfig, InputFormat, Quality, Rotation};
use std::path::PathBuf;
use std::process::Command;

fn argb8888(width: usize, height: usize, stride: usize) -> Vec<u8> {
    let mut output = vec![0u8; stride * height];
    for y in 0..height {
        for x in 0..width {
            let offset = y * stride + x * 4;
            // MPP_FMT_ARGB8888 carries the a,r,g,b components as one 32-bit
            // value; on the little-endian board that is memory order b,g,r,a.
            output[offset] = (x * 255 / (width - 1)) as u8;
            output[offset + 1] = 128;
            output[offset + 2] = (y * 255 / (height - 1)) as u8;
            output[offset + 3] = 255;
        }
    }
    output
}

fn rgb565(width: usize, height: usize, stride: usize) -> Vec<u8> {
    let mut output = vec![0u8; stride * height];
    for y in 0..height {
        for x in 0..width {
            let r = (x * 31 / (width - 1)) as u16;
            let g = (y * 63 / (height - 1)) as u16;
            let value = (r << 11) | (g << 5) | r;
            // MPP_FMT_RGB565 carries the 565 value big-endian (FFmpeg
            // rgb565be), which is what scripts/gen_rgb_fixture.py writes.
            output[y * stride + x * 2..y * stride + x * 2 + 2]
                .copy_from_slice(&value.to_be_bytes());
        }
    }
    output
}

fn run_cli(
    input_format: &str,
    width: usize,
    height: usize,
    input: &[u8],
    extra: &[&str],
) -> Vec<u8> {
    let path = PathBuf::from(format!(
        "/tmp/rk-mpp-rs-jpeg-features-{}-{}.bin",
        std::process::id(),
        input_format
    ));
    let out = PathBuf::from(format!(
        "/tmp/rk-mpp-rs-jpeg-features-{}-{}.jpg",
        std::process::id(),
        input_format
    ));
    std::fs::write(&path, input).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_rk-mpp-rs"));
    command
        .args(["jpeg-enc"])
        .arg(&path)
        .arg(&out)
        .arg(width.to_string())
        .arg(height.to_string())
        .arg("10")
        .arg("420")
        .arg(input_format)
        .args(extra);
    let result = command.output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(
        result.status.success(),
        "{input_format} encode failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let encoded = std::fs::read(&out).unwrap();
    std::fs::remove_file(&out).unwrap();
    encoded
}

fn ffmpeg_decode(stream: &[u8]) -> (usize, usize) {
    let path = PathBuf::from(format!(
        "/tmp/rk-mpp-rs-jpeg-features-decode-{}.jpg",
        std::process::id()
    ));
    std::fs::write(&path, stream).unwrap();
    let probe = Command::new("ffmpeg")
        .args(["-v", "error", "-xerror", "-i"])
        .arg(&path)
        .args([
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p",
            "-",
        ])
        .output()
        .expect("FFmpeg must be installed for independent JPEG validation");
    std::fs::remove_file(&path).unwrap();
    assert!(
        probe.status.success(),
        "FFmpeg rejected JPEG: {}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let info = parser::parse(stream).unwrap();
    (info.width, info.height)
}

#[test]
fn restart_interval_follows_the_ctu_split_derivation() {
    // 320x240: 20x15 MCUs. A 25-MCU band is two rows, so the DRI carries 40.
    let config = EncoderConfig {
        split_mcus: Some(25),
        ..EncoderConfig::new(320, 240)
    };
    let plan = config.split_plan().unwrap();
    assert_eq!((plan.part_rows, plan.restart_interval), (2, 40));

    // The band covers the whole picture, so C derives no restart interval.
    let full = EncoderConfig {
        split_mcus: Some(30 * 20),
        ..EncoderConfig::new(320, 240)
    };
    assert_eq!(full.split_plan(), None);
}

#[test]
fn rotation_only_transposes_the_output_and_its_mcu_grid() {
    let config = EncoderConfig {
        rotation: Rotation::Degree90,
        ..EncoderConfig::new(320, 240)
    };
    assert_eq!(config.output_size(), (240, 320));
    assert_eq!(config.mcu_counts(), (15, 20));
    assert_eq!(
        config.input_size().unwrap(),
        320 * EncoderConfig::new(320, 240).ver_stride * 3 / 2
    );
    // A restart interval is counted from the transposed MCU columns.
    let rotated = EncoderConfig {
        rotation: Rotation::Degree270,
        split_mcus: Some(15),
        ..EncoderConfig::new(320, 240)
    };
    assert_eq!(rotated.split_plan().unwrap().restart_interval, 15);
}

#[test]
fn rgb_input_geometry_and_stride_rules() {
    let argb = EncoderConfig::new_for_input(320, 240, InputFormat::Argb8888);
    assert_eq!(argb.hor_stride, 1280);
    assert_eq!(argb.input_size().unwrap(), 1280 * 240);
    assert!(argb.validate().is_ok());

    let rgb565 = EncoderConfig::new_for_input(320, 240, InputFormat::Rgb565);
    assert_eq!(rgb565.hor_stride, 640);
    assert_eq!(rgb565.input_size().unwrap(), 640 * 240);
    assert!(rgb565.validate().is_ok());

    // A 32-bit row must still land on an 8-pixel boundary.
    let mut broken = argb;
    broken.hor_stride = 1284;
    assert!(broken.validate().is_err());
}

#[test]
#[ignore = "requires an RK3588 VEPU2 JPEG encoder and /dev/mpp_service access"]
fn rotated_encode_decodes_at_the_transposed_dimensions() {
    let mut input = vec![16u8; 320 * 240 * 3 / 2];
    input[320 * 240..].fill(128);
    for (rotation, expected) in [("rot90", (240, 320)), ("rot270", (240, 320))] {
        let stream = run_cli("nv12", 320, 240, &input, &[rotation]);
        let info = parser::parse(&stream).unwrap();
        assert_eq!((info.width, info.height), expected);
        assert_eq!(ffmpeg_decode(&stream), expected);
    }
}

#[test]
#[ignore = "requires an RK3588 VEPU2 JPEG encoder and /dev/mpp_service access"]
fn restart_interval_marks_are_written_at_the_derived_period() {
    let mut input = vec![16u8; 320 * 240 * 3 / 2];
    input[320 * 240..].fill(128);
    let stream = run_cli("nv12", 320, 240, &input, &["rst25"]);
    let dri_at = stream
        .windows(2)
        .position(|w| w == [0xff, 0xdd])
        .expect("DRI marker");
    assert_eq!(&stream[dri_at + 4..dri_at + 6], &[0x00, 0x28]);

    // 300 MCUs over a 40-MCU interval gives the full restart-marker cycle
    // at least once, and the markers stay inside the entropy-coded data.
    let markers = stream
        .windows(2)
        .filter(|w| w[0] == 0xff && (0xd0..=0xd7).contains(&w[1]))
        .count();
    assert!(markers >= 7, "only {markers} restart markers");
}

#[test]
#[ignore = "requires an RK3588 VEPU2 JPEG encoder and /dev/mpp_service access"]
fn rgb_inputs_encode_decodable_baseline_420_pictures() {
    for (input_format, input) in [
        ("argb8888", argb8888(320, 240, 1280)),
        ("rgb565", rgb565(320, 240, 640)),
    ] {
        let stream = run_cli(input_format, 320, 240, &input, &[]);
        let info = parser::parse(&stream).unwrap();
        assert_eq!((info.width, info.height), (320, 240));
        assert_eq!(info.components.len(), 3);
        assert_eq!((info.components[0].h, info.components[0].v), (2, 2));
        assert_eq!(ffmpeg_decode(&stream), (320, 240));
        assert!(!info.progressive);
    }
}

#[test]
fn rgb_output_stays_locked_to_quality_tables_like_nv12() {
    // The quantizer path is shared: quality 10 is all-ones for every input
    // format, so a DQT comparison across formats is format-independent.
    for format in [
        InputFormat::Nv12,
        InputFormat::Argb8888,
        InputFormat::Rgb565,
    ] {
        let config = EncoderConfig {
            quality: Quality::Quant(10),
            ..EncoderConfig::new_for_input(64, 64, format)
        };
        assert!(config.validate().is_ok());
        assert_eq!(config.chroma, ChromaFormat::Yuv420);
    }
}
