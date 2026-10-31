use rk_mpp_rs::codec::h264::encoder::EncoderConfig;
use rk_mpp_rs::codec::h264::split_annex_b;
use rk_mpp_rs::hal::h264e::{probe_nv12, Vepu580H264, REFERENCE_HEIGHT, REFERENCE_WIDTH};
use std::path::PathBuf;
use std::process::Command;

const HEADERS: &[u8] = &[
    0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xc0, 0x1e, 0x8d, 0x8d, 0x50, 0xa0, 0xf9, 0x08, 0x00, 0x00,
    0x03, 0x00, 0x08, 0x00, 0x00, 0x03, 0x01, 0xe4, 0x78, 0x44, 0x23, 0x50, 0x00, 0x00, 0x00, 0x01,
    0x68, 0xce, 0x31, 0xb2,
];

const MULTI_SIZE_GOLDENS: &[(usize, usize, &[(u8, usize, u64)])] = &[
    (
        64,
        64,
        &[
            (7, 27, 0x1cb6_b5d7_a916_cf59),
            (8, 8, 0x4d13_8e40_8a89_af33),
            (5, 171, 0x14dd_33ae_863c_7927),
        ],
    ),
    (
        320,
        240,
        &[
            (7, 28, 0x4775_6f8d_f2be_3302),
            (8, 8, 0x4d13_8e40_8a89_af33),
            (5, 825, 0xf815_b3c7_11ff_0107),
        ],
    ),
    (
        640,
        480,
        &[
            (7, 27, 0xc409_5ca0_055d_ac10),
            (8, 8, 0x4d13_8e40_8a89_af33),
            (5, 2167, 0x64cb_740f_355a_6c20),
        ],
    ),
    (
        1280,
        720,
        &[
            (7, 29, 0x5628_0ed3_2e70_a563),
            (8, 8, 0x4d13_8e40_8a89_af33),
            (5, 6064, 0x0245_8073_705d_318e),
        ],
    ),
    (
        322,
        242,
        &[
            (7, 30, 0x9835_0a7b_8347_ba69),
            (8, 8, 0x4d13_8e40_8a89_af33),
            (5, 911, 0x16c4_5884_6d8c_ab85),
        ],
    ),
    (
        1366,
        768,
        &[
            (7, 30, 0xe58c_f7fa_571b_c155),
            (8, 8, 0x4d13_8e40_8a89_af33),
            (5, 7014, 0x70bd_466e_e737_8748),
        ],
    ),
];

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |mut hash, byte| {
        hash ^= u64::from(*byte);
        hash.wrapping_mul(0x100000001b3)
    })
}

fn encode(input: &[u8], width: usize, height: usize) -> Vec<u8> {
    let config = EncoderConfig::new(width, height);
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

#[test]
#[ignore = "requires an RK3588 RKVENCP/RKVENC H.264 encoder and /dev/mpp_service access"]
fn baseline_idr_is_bit_exact_deterministic_and_decodes_at_high_quality() {
    let input = probe_nv12(REFERENCE_WIDTH, REFERENCE_HEIGHT);
    let first = encode(&input, REFERENCE_WIDTH, REFERENCE_HEIGHT);
    let second = encode(&input, REFERENCE_WIDTH, REFERENCE_HEIGHT);

    assert_eq!(first, second, "H.264 output must be deterministic");
    assert_eq!(first.len(), 861);
    assert_eq!(fnv1a(&first), 0x99c1_8249_c32b_28f8);
    assert_eq!(&first[..HEADERS.len()], HEADERS);

    let nals = split_annex_b(&first);
    assert_eq!(nals.len(), 3);
    assert_eq!(nals[0].unit_type, 7);
    assert_eq!(nals[1].unit_type, 8);
    assert_eq!(nals[2].unit_type, 5);
    assert_eq!(nals[2].len, 825);
    assert_eq!(
        fnv1a(&first[nals[2].offset..nals[2].offset + nals[2].len]),
        0xf815_b3c7_11ff_0107
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

        let nals = split_annex_b(&stream);
        assert_eq!(
            nals.len(),
            goldens.len(),
            "{width}x{height} emitted unexpected NAL units"
        );
        for (nal, &(unit_type, len, hash)) in nals.iter().zip(goldens) {
            assert_eq!(nal.unit_type, unit_type, "{width}x{height} NAL type");
            assert_eq!(nal.len, len, "{width}x{height} NAL length");
            assert_eq!(
                fnv1a(&stream[nal.offset..nal.offset + nal.len]),
                hash,
                "{width}x{height} NAL type {unit_type} differs from C MPP"
            );
        }

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
