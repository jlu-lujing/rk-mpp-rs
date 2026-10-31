use rk_mpp_rs::codec::jpeg::parser;
use rk_mpp_rs::hal::jpege::{ChromaFormat, EncoderConfig, InputFormat, Quality, Vepu2Jpeg};
use std::path::PathBuf;
use std::process::Command;

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |mut hash, byte| {
        hash ^= u64::from(*byte);
        hash.wrapping_mul(0x100000001b3)
    })
}

fn odd_nv12() -> Vec<u8> {
    const WIDTH: usize = 321;
    const HEIGHT: usize = 241;
    const HOR_STRIDE: usize = 328;
    const VER_STRIDE: usize = 256;

    let mut output = vec![16u8; HOR_STRIDE * VER_STRIDE];
    output.resize(HOR_STRIDE * VER_STRIDE * 3 / 2, 128);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            output[y * HOR_STRIDE + x] =
                (16 + (x * 200 / (WIDTH - 1) + y * 200 / (HEIGHT - 1)) / 2) as u8;
        }
    }

    let chroma = HOR_STRIDE * VER_STRIDE;
    for y in 0..HEIGHT.div_ceil(2) {
        for x in 0..WIDTH.div_ceil(2) {
            let offset = chroma + y * HOR_STRIDE + 2 * x;
            output[offset] = (48 + x * 128 / (WIDTH.div_ceil(2) - 1)) as u8;
            output[offset + 1] = (208 - y * 128 / (HEIGHT.div_ceil(2) - 1)) as u8;
        }
    }
    output
}

fn even_nv12(width: usize, height: usize, hor_stride: usize, ver_stride: usize) -> Vec<u8> {
    let mut output = vec![16u8; hor_stride * ver_stride];
    output.resize(hor_stride * ver_stride * 3 / 2, 128);
    for y in 0..height {
        for x in 0..width {
            output[y * hor_stride + x] =
                (16 + (x * 200 / (width - 1) + y * 200 / (height - 1)) / 2) as u8;
        }
    }

    let chroma = hor_stride * ver_stride;
    let chroma_width = width / 2;
    let chroma_height = height / 2;
    for y in 0..chroma_height {
        for x in 0..chroma_width {
            let offset = chroma + y * hor_stride + 2 * x;
            output[offset] = (48 + x * 128 / (chroma_width - 1)) as u8;
            output[offset + 1] = (208 - y * 128 / (chroma_height - 1)) as u8;
        }
    }
    output
}

fn yuyv422(width: usize, height: usize, hor_stride: usize, ver_stride: usize) -> Vec<u8> {
    assert_eq!(width & 1, 0);
    assert_eq!(hor_stride, width * 2);
    let mut output = vec![16u8; hor_stride * ver_stride];
    for y in 0..height {
        for pair in 0..width / 2 {
            let x = pair * 2;
            let luma0 = (16 + (x * 200 / (width - 1) + y * 200 / (height - 1)) / 2) as u8;
            let luma1 = (16 + ((x + 1) * 200 / (width - 1) + y * 200 / (height - 1)) / 2) as u8;
            let offset = y * hor_stride + pair * 4;
            output[offset] = luma0;
            output[offset + 1] = (48 + pair * 128 / (width / 2 - 1)) as u8;
            output[offset + 2] = luma1;
            output[offset + 3] = (208 - y * 128 / (height - 1)) as u8;
        }
    }
    output
}

fn uyvy422(width: usize, height: usize, hor_stride: usize, ver_stride: usize) -> Vec<u8> {
    let mut yuyv = yuyv422(width, height, hor_stride, ver_stride);
    for row in yuyv.chunks_exact_mut(hor_stride) {
        for pair in row.chunks_exact_mut(4) {
            pair.swap(0, 1);
            pair.swap(2, 3);
        }
    }
    yuyv
}

fn run_cli(
    chroma: &str,
    input_format: &str,
    width: usize,
    height: usize,
    input_data: &[u8],
) -> std::process::Output {
    let input = PathBuf::from(format!(
        "/tmp/rk-mpp-rs-jpeg-format-{}-{chroma}-{input_format}.yuv",
        std::process::id()
    ));
    let output = PathBuf::from(format!(
        "/tmp/rk-mpp-rs-jpeg-format-{}-{chroma}-{input_format}.jpg",
        std::process::id()
    ));
    std::fs::write(&input, input_data).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_rk-mpp-rs"))
        .args(["jpeg-enc"])
        .arg(&input)
        .arg(&output)
        .arg(width.to_string())
        .arg(height.to_string())
        .args(["10", chroma, input_format])
        .output()
        .unwrap();
    std::fs::remove_file(input).unwrap();
    result
}

fn encode_with_cli(
    chroma: &str,
    input_format: &str,
    width: usize,
    height: usize,
    input_data: &[u8],
) -> Vec<u8> {
    let output = PathBuf::from(format!(
        "/tmp/rk-mpp-rs-jpeg-format-{}-{chroma}-{input_format}.jpg",
        std::process::id()
    ));
    let result = run_cli(chroma, input_format, width, height, input_data);
    assert!(
        result.status.success(),
        "JPEG {chroma} encode failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let encoded = std::fs::read(&output).unwrap();
    std::fs::remove_file(output).unwrap();
    encoded
}

fn ffmpeg_decode(stream: &[u8], pixel_format: &str) -> Vec<u8> {
    let path = PathBuf::from(format!(
        "/tmp/rk-mpp-rs-jpeg-decode-{}.jpg",
        std::process::id()
    ));
    std::fs::write(&path, stream).unwrap();
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-xerror", "-i"])
        .arg(&path)
        .args([
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            pixel_format,
            "-",
        ])
        .output()
        .expect("FFmpeg must be installed for independent JPEG validation");
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "FFmpeg rejected JPEG: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn packed_to_planar420(source: &[u8], width: usize, height: usize, yuyv: bool) -> Vec<u8> {
    let (y0, u, y1, v) = if yuyv {
        (0usize, 1usize, 2usize, 3usize)
    } else {
        (1usize, 0usize, 3usize, 2usize)
    };
    let mut output = Vec::with_capacity(width * height * 3 / 2);
    for y in 0..height {
        for pair in 0..width / 2 {
            let offset = y * width * 2 + pair * 4;
            output.push(source[offset + y0]);
            output.push(source[offset + y1]);
        }
    }
    for byte in [u, v] {
        for y in (0..height).step_by(2) {
            for pair in 0..width / 2 {
                output.push(source[y * width * 2 + pair * 4 + byte]);
            }
        }
    }
    output
}

fn psnr(reference: &[u8], decoded: &[u8]) -> f64 {
    assert_eq!(reference.len(), decoded.len());
    let mse = reference
        .iter()
        .zip(decoded)
        .map(|(&a, &b)| {
            let error = f64::from(a) - f64::from(b);
            error * error
        })
        .sum::<f64>()
        / reference.len() as f64;
    10.0 * (255.0 * 255.0 / mse).log10()
}

fn psnr_planes_420(reference: &[u8], decoded: &[u8], width: usize, height: usize) -> [f64; 3] {
    let y_len = width * height;
    let chroma_len = width / 2 * height / 2;
    [
        psnr(&reference[..y_len], &decoded[..y_len]),
        psnr(
            &reference[y_len..y_len + chroma_len],
            &decoded[y_len..y_len + chroma_len],
        ),
        psnr(
            &reference[y_len + chroma_len..],
            &decoded[y_len + chroma_len..],
        ),
    ]
}

#[test]
#[ignore = "requires an RK3588 VEPU2 JPEG encoder and /dev/mpp_service access"]
fn odd_dimensions_encode_deterministically_and_read_back_after_dma_sync() {
    let input = odd_nv12();
    let config = EncoderConfig {
        quality: Quality::Quant(10),
        ..EncoderConfig::new(321, 241)
    };
    assert_eq!((config.hor_stride, config.ver_stride), (328, 256));

    let mut encoder = Vepu2Jpeg::open().unwrap();
    let first = encoder.encode_nv12(&input, config).unwrap();
    let second = encoder.encode_nv12(&input, config).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.len(), 20309);
    assert_eq!(fnv1a(&first), 0x3170_59eb_2161_cca2);
    assert_eq!(&first[first.len() - 2..], &[0xff, 0xd9]);

    let info = parser::parse(&first).unwrap();
    assert_eq!((info.width, info.height), (321, 241));
    assert!(!info.progressive);
}

#[test]
#[ignore = "requires an RK3588 VEPU2 JPEG encoder and /dev/mpp_service access"]
fn allocation_ver_stride_does_not_change_the_coded_picture() {
    let mut encoder = Vepu2Jpeg::open().unwrap();
    let compact = EncoderConfig {
        quality: Quality::Quant(10),
        ..EncoderConfig::new(320, 240)
    };
    let padded = EncoderConfig {
        ver_stride: 256,
        ..compact
    };

    let compact_stream = encoder
        .encode_nv12(&even_nv12(320, 240, 320, 240), compact)
        .unwrap();
    let padded_stream = encoder
        .encode_nv12(&even_nv12(320, 240, 320, 256), padded)
        .unwrap();
    assert_eq!(compact_stream, padded_stream);
}

#[test]
#[ignore = "requires an RK3588 VEPU2 JPEG encoder and /dev/mpp_service access"]
fn packed_yuv422_input_is_encoded_as_decodable_420_jpeg() {
    const WIDTH: usize = 320;
    const HEIGHT: usize = 240;
    let mut errors = Vec::new();
    for (input_format, source, yuyv) in [
        ("yuyv", yuyv422(WIDTH, HEIGHT, WIDTH * 2, HEIGHT), true),
        ("uyvy", uyvy422(WIDTH, HEIGHT, WIDTH * 2, HEIGHT), false),
    ] {
        let stream = encode_with_cli("420", input_format, WIDTH, HEIGHT, &source);
        let info = parser::parse(&stream).unwrap();
        assert_eq!(info.components.len(), 3);
        assert_eq!((info.components[0].h, info.components[0].v), (2, 2));
        assert_eq!((info.components[1].h, info.components[1].v), (1, 1));
        assert_eq!((info.components[2].h, info.components[2].v), (1, 1));

        let decoded = ffmpeg_decode(&stream, "yuv420p");
        let expected = packed_to_planar420(&source, WIDTH, HEIGHT, yuyv);
        let [y_psnr, u_psnr, v_psnr] = psnr_planes_420(&expected, &decoded, WIDTH, HEIGHT);
        if y_psnr <= 30.0 || u_psnr <= 30.0 || v_psnr <= 30.0 {
            errors.push(format!(
                "{input_format}: Y {y_psnr:.2} / U {u_psnr:.2} / V {v_psnr:.2} dB"
            ));
        }
    }
    assert!(
        errors.is_empty(),
        "packed YUV 420 PSNR failures: {errors:?}"
    );
}

#[test]
#[ignore = "requires an RK3588 VEPU2 JPEG encoder and /dev/mpp_service access"]
fn non_420_output_is_rejected_before_hardware_submission() {
    let source = odd_nv12();
    for chroma in ["gray", "422", "444"] {
        let result = run_cli(chroma, "nv12", 321, 241, &source);
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("unsupported on this hardware"),
            "unexpected {chroma} failure: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn config_rejects_non_420_output() {
    for chroma in [
        ChromaFormat::Gray,
        ChromaFormat::Yuv422,
        ChromaFormat::Yuv444,
    ] {
        let config = EncoderConfig {
            chroma,
            ..EncoderConfig::new(320, 240)
        };
        assert!(config.validate().is_err());
    }

    let yuyv = EncoderConfig::new_for_input(320, 240, InputFormat::Yuyv);
    assert_eq!(yuyv.chroma, ChromaFormat::Yuv420);
    assert_eq!(yuyv.hor_stride, 640);
    assert_eq!(yuyv.input_size().unwrap(), 640 * 240);

    let uyvy = EncoderConfig::new_for_input(320, 240, InputFormat::Uyvy);
    assert_eq!(uyvy.chroma, ChromaFormat::Yuv420);
    assert!(uyvy.validate().is_ok());
}
