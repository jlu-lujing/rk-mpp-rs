// Board-only: an RK3588 VEPU580 H.264 encoder and /dev/mpp_service.
//
// The goldens are the C reference's own NAL units, captured with
// `tools/h264e_probe.c` at fixed QP 26 and `mpi_enc_test`'s default rate triple
// (`w*h*30/8 : 1500000 : 2500000`). Every NAL is byte-identical to that
// reference except the `uuid_version` SEI: its payload is the build identity of
// whichever encoder wrote it (C embeds its git log line, we embed our crate
// version), so it is checked for framing only - the rule
// `scripts/compare_streams.py` already applies to H.265. The `uuid_rc_cfg` SEI
// is compared byte for byte.
//
// Two and three live sessions in one process were measured at 64x64, 320x240 and
// 640x480: one distinct stream each, identical to the serial result. The earlier
// comment here blamed `setup_ext_line_bufs` for cross-session damage; that
// function only allocates per-context buffers, and these sizes never reach the
// >4096 extension path that uses them.

use rk_mpp_rs::codec::h264::encoder::EncoderConfig;
use rk_mpp_rs::codec::h264::split_annex_b;
use rk_mpp_rs::hal::h264e::{probe_nv12, Vepu580H264, REFERENCE_HEIGHT, REFERENCE_WIDTH};
use std::path::PathBuf;
use std::process::Command;

/// `(picture width, height, stable NALs)` where each stable NAL is
/// `(index in the stream, unit type, length, FNV-1a)`.
///
/// Index 2 - the version SEI - is deliberately absent: its payload is the
/// encoder's build identity, not a property of the encode. Index 3 is the
/// `uuid_rc_cfg` SEI and is compared like any other NAL.
const MULTI_SIZE_GOLDENS: &[(usize, usize, &[(usize, u8, usize, u64)])] = &[
    (
        64,
        64,
        &[
            (0, 7, 27, 0x1cb6b5d7a916cf59),
            (1, 8, 8, 0x4d138e408a89af33),
            (3, 6, 91, 0x7958b309dd391d31),
            (4, 5, 171, 0x14dd33ae863c7927),
        ],
    ),
    (
        320,
        240,
        &[
            (0, 7, 28, 0x47756f8df2be3302),
            (1, 8, 8, 0x4d138e408a89af33),
            (3, 6, 92, 0x08c75296f0712a01),
            (4, 5, 825, 0xf815b3c711ff0107),
        ],
    ),
    (
        640,
        480,
        &[
            (0, 7, 27, 0xc4095ca0055dac10),
            (1, 8, 8, 0x4d138e408a89af33),
            (3, 6, 93, 0xcf5eca59cf05a5a9),
            (4, 5, 2167, 0x64cb740f355a6c20),
        ],
    ),
    (
        1280,
        720,
        &[
            (0, 7, 29, 0x56280ed32e70a563),
            (1, 8, 8, 0x4d138e408a89af33),
            (3, 6, 93, 0x8e8fd772023610d0),
            (4, 5, 6064, 0x02458073705d318e),
        ],
    ),
    (
        1366,
        768,
        &[
            (0, 7, 30, 0xe58cf7fa571bc155),
            (1, 8, 8, 0x4d138e408a89af33),
            (3, 6, 93, 0xb97510c1824266bb),
            (4, 5, 7014, 0x70bd466ee7378748),
        ],
    ),
];

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |mut hash, byte| {
        hash ^= u64::from(*byte);
        hash.wrapping_mul(0x100000001b3)
    })
}

/// The harness configuration the goldens were captured with: `mpi_enc_test`'s
/// 60-picture GOP and its own default rate target (`w * h * 30 / 8`, bounds
/// 1.5-2.5 Mbps). A fixed-QP picture does not spend them, but both SEIs print
/// them, so the library defaults are not the reference configuration.
fn encode(input: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut config = EncoderConfig::new(width, height);
    config.gop = 60;
    config.bps_target = rk_mpp_rs::codec::h264::encoder::default_bps_target(width, height, 30);
    let mut encoder = Vepu580H264::open(config).unwrap();
    encoder.encode_idr(input).unwrap().annex_b
}

fn ffmpeg_decode(stream: &[u8]) -> Vec<u8> {
    let path = PathBuf::from(format!(
        "/tmp/rk-mpp-rs-h264-encode-{}-{:016x}.264",
        std::process::id(),
        fnv1a(stream)
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
            "yuv420p",
            "-",
        ])
        .output()
        .expect("FFmpeg is required for independent H.264 validation");
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "FFmpeg rejected the H.264 stream: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
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
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0 * 255.0 / mse).log10()
    }
}

fn nv12_planes(nv12: &[u8], width: usize, height: usize) -> [Vec<u8>; 3] {
    let stride = (width + 15) & !15;
    let ver_stride = (height + 15) & !15;
    let uv_offset = stride * ver_stride;
    let mut planes = [Vec::new(), Vec::new(), Vec::new()];
    for y in 0..height {
        let row = y * stride;
        planes[0].extend_from_slice(&nv12[row..row + width]);
    }
    for y in 0..height / 2 {
        let row = uv_offset + y * stride;
        for x in 0..width / 2 {
            let offset = row + x * 2;
            planes[1].push(nv12[offset]);
            planes[2].push(nv12[offset + 1]);
        }
    }
    planes
}

/// Five NAL units: SPS, PPS, version SEI, rc-cfg SEI, IDR. Everything except the
/// version SEI must equal the C reference exactly.
fn assert_reference_nals(
    stream: &[u8],
    width: usize,
    height: usize,
    goldens: &[(usize, u8, usize, u64)],
) {
    let nals = split_annex_b(stream);
    assert_eq!(nals.len(), 5, "{width}x{height} NAL count");
    assert_eq!(nals[2].unit_type, 6, "{width}x{height} version SEI type");
    let version = &stream[nals[2].offset..nals[2].offset + nals[2].len];
    // `split_annex_b` keeps the start code, and this encoder writes four bytes.
    let header = if version[2] == 1 { 3 } else { 4 };
    assert_eq!(
        version[header + 1],
        5,
        "user_data_unregistered payload type"
    );
    assert!(
        (40..=130).contains(&version.len()),
        "{width}x{height} version SEI framing: {} bytes",
        version.len()
    );
    for &(index, unit_type, len, hash) in goldens {
        let nal = &nals[index];
        assert_eq!(
            nal.unit_type, unit_type,
            "{width}x{height} NAL {index} type"
        );
        assert_eq!(nal.len, len, "{width}x{height} NAL {index} length");
        assert_eq!(
            fnv1a(&stream[nal.offset..nal.offset + nal.len]),
            hash,
            "{width}x{height} NAL {index} (type {unit_type}) differs from C MPP"
        );
    }
}

fn reference_goldens() -> &'static [(usize, u8, usize, u64)] {
    MULTI_SIZE_GOLDENS
        .iter()
        .find(|(width, height, _)| *width == REFERENCE_WIDTH && *height == REFERENCE_HEIGHT)
        .map(|(_, _, goldens)| *goldens)
        .expect("reference size has goldens")
}

#[test]
#[ignore = "requires an RK3588 RKVENCP/RKVENC H.264 encoder and /dev/mpp_service access"]
fn concurrent_live_sessions_match_the_serial_reference() {
    for &(width, height) in &[(64, 64), (320, 240), (640, 480)] {
        let (_, _, goldens) = MULTI_SIZE_GOLDENS
            .iter()
            .find(|(w, h, _)| *w == width && *h == height)
            .expect("size must have C goldens");
        let input = probe_nv12(width, height);
        let serial = encode(&input, width, height);
        assert_reference_nals(&serial, width, height, goldens);

        for sessions in [2, 3] {
            let streams: Vec<Vec<u8>> = std::thread::scope(|scope| {
                let threads: Vec<_> = (0..sessions)
                    .map(|_| scope.spawn(|| encode(&input, width, height)))
                    .collect();
                threads
                    .into_iter()
                    .map(|thread| thread.join().unwrap())
                    .collect()
            });
            for stream in &streams {
                assert_eq!(
                    stream, &serial,
                    "{sessions} live {width}x{height} sessions must not interfere"
                );
            }
        }
    }
}

#[test]
#[ignore = "requires an RK3588 RKVENCP/RKVENC H.264 encoder and /dev/mpp_service access"]
fn baseline_idr_is_bit_exact_deterministic_and_decodes_at_high_quality() {
    let input = probe_nv12(REFERENCE_WIDTH, REFERENCE_HEIGHT);
    let first = encode(&input, REFERENCE_WIDTH, REFERENCE_HEIGHT);
    let second = encode(&input, REFERENCE_WIDTH, REFERENCE_HEIGHT);

    assert_eq!(first, second, "H.264 output must be deterministic");
    assert_reference_nals(
        &first,
        REFERENCE_WIDTH,
        REFERENCE_HEIGHT,
        reference_goldens(),
    );

    let decoded = ffmpeg_decode(&first);
    assert_eq!(decoded.len(), REFERENCE_WIDTH * REFERENCE_HEIGHT * 3 / 2);
    let expected = nv12_planes(&input, REFERENCE_WIDTH, REFERENCE_HEIGHT);
    let luma = REFERENCE_WIDTH * REFERENCE_HEIGHT;
    let chroma = luma / 4;
    let planes = [
        &decoded[..luma],
        &decoded[luma..luma + chroma],
        &decoded[luma + chroma..],
    ];
    let scores: [f64; 3] = std::array::from_fn(|i| psnr(&expected[i], planes[i]));
    assert!(
        scores.iter().all(|score| *score >= 45.0),
        "decoded PSNR is below 45 dB: {scores:?}"
    );
}

#[test]
#[ignore = "requires an RK3588 RKVENCP/RKVENC H.264 encoder and /dev/mpp_service access"]
fn baseline_idr_matches_mpp_goldens_across_dimensions() {
    for &(width, height, goldens) in MULTI_SIZE_GOLDENS {
        let input = probe_nv12(width, height);
        let stream = encode(&input, width, height);
        assert_eq!(
            stream,
            encode(&input, width, height),
            "{width}x{height} output must be deterministic"
        );

        assert_reference_nals(&stream, width, height, goldens);

        let decoded = ffmpeg_decode(&stream);
        assert_eq!(
            decoded.len(),
            width * height * 3 / 2,
            "{width}x{height} independent decoder output size"
        );
        let expected = nv12_planes(&input, width, height);
        let luma = width * height;
        let chroma = luma / 4;
        let planes = [
            &decoded[..luma],
            &decoded[luma..luma + chroma],
            &decoded[luma + chroma..],
        ];
        let scores: [f64; 3] = std::array::from_fn(|i| psnr(&expected[i], planes[i]));
        assert!(
            scores.iter().all(|score| *score >= 45.0),
            "{width}x{height} decoded PSNR is below 45 dB: {scores:?}"
        );
    }
}
