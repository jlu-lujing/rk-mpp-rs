use std::path::Path;

use rk_mpp_rs::codec::vp9::hw::{build_prob_default, build_prob_flag_delta, build_probe};
use rk_mpp_rs::codec::vp9::parser::{parse_ivf, split_superframe, Parser};

#[test]
fn profile_zero_hardware_probability_images_match_c_mpp() {
    assert_hardware_probabilities_match(include_bytes!("data/vp9_p0_64x64.ivf"), "vp9_p0_hw");
}

#[test]
fn profile_two_hardware_probability_images_match_c_mpp() {
    assert_hardware_probabilities_match(include_bytes!("data/vp9_p2_10bit_64x64.ivf"), "vp9_p2_hw");
}

fn assert_hardware_probabilities_match(ivf_bytes: &[u8], fixture_dir: &str) {
    let ivf = parse_ivf(ivf_bytes).unwrap();
    let mut parser = Parser::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(fixture_dir);

    for (index, frame) in ivf.frames.iter().enumerate() {
        let payloads = split_superframe(frame.data).unwrap();
        let parsed = parser
            .parse_frame(payloads[0])
            .unwrap_or_else(|error| panic!("frame {index}: {error}"));
        let compressed = parsed.compressed_header.as_ref().unwrap();

        let expected_update = std::fs::read(root.join(format!("update_{index:02}.bin"))).unwrap();
        let actual_update = build_prob_flag_delta(&parsed.header, compressed);
        assert_bytes(
            &actual_update,
            &expected_update,
            index,
            "probability flag/delta",
        );

        let expected_probe = std::fs::read(root.join(format!("probe_{index:02}.bin"))).unwrap();
        let actual_probe = build_probe(&parsed.header, compressed);
        assert_bytes(&actual_probe, &expected_probe, index, "probe");

        let default_path = root.join(format!("default_{index:02}.bin"));
        if default_path.exists() {
            let expected_default = std::fs::read(default_path).unwrap();
            let actual_default = build_prob_default(&parsed.header, compressed);
            assert_bytes(&actual_default, &expected_default, index, "default");
        }
    }
}

fn assert_bytes(actual: &[u8], expected: &[u8], frame: usize, label: &str) {
    assert_eq!(
        expected.len(),
        actual.len(),
        "frame {frame}: invalid C {label} fixture size"
    );
    if let Some(offset) = actual
        .iter()
        .zip(expected)
        .position(|(actual, expected)| actual != expected)
    {
        let start = offset.saturating_sub(8);
        let end = (offset + 16).min(actual.len());
        panic!(
            "frame {frame}, {label} byte {offset}: Rust {}, C MPP {}\n\
             Rust: {:02x?}\nC MPP: {:02x?}",
            actual[offset],
            expected[offset],
            &actual[start..end],
            &expected[start..end],
        );
    }
}
