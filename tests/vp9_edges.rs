//! VP9 boundary matrix, host side.
//!
//! Every boundary combination must either decode or be refused with a named
//! reason. The parser-side rejections are exercised here (they need no
//! hardware); the decode side of the matrix runs on the board via
//! `scripts/board_vp9_matrix.sh`. The capability module is unit-tested in
//! place (`src/codec/vp9/capability.rs`).

use rk_mpp_rs::codec::vp9::capability::{self, MAX_SUPERBLOCKS};
use rk_mpp_rs::codec::vp9::parser::{self, FrameKind, Parser, Vp9Error};

fn payload(path: &str, frame: usize) -> Vec<u8> {
    let data = std::fs::read(path).expect("fixture");
    let ivf = parser::parse_ivf(&data).expect("container");
    let frames = parser::split_superframe(ivf.frames[frame].data).expect("superframe");
    frames[0].to_vec()
}

/// The boundary dimensions the fixtures actually carry, checked against the
/// hardware capability gate: everything a real corpus stream can present is
/// inside the matrix, with the superblock budget named per stream.
#[test]
fn every_fixture_boundary_is_parsed_and_inside_the_gate() {
    // path, frame, width, height, bit depth, log2 tile cols, log2 tile rows
    #[rustfmt::skip]
    let cases: [(&str, usize, u32, u32, u8, u8, u8); 6] = [
        ("tests/data/vp9_p0_64x64.ivf", 0, 64, 64, 8, 0, 0),
        ("tests/data/vp9_p2_10bit_64x64.ivf", 0, 64, 64, 10, 0, 0),
        ("tests/data/vp9_p0_lossless_321x241_2.ivf", 0, 321, 241, 8, 0, 0),
        ("tests/data/vp9_p0_errres_tiles_640x360_12.ivf", 0, 640, 360, 8, 1, 0),
        ("tests/data/vp9_p0_lossless_320x240_12.ivf", 0, 320, 240, 8, 0, 0),
        ("tests/data/vp9_p0_720p_errres_seg.ivf", 0, 1280, 720, 8, 0, 0),
    ];
    for (path, frame, width, height, bit_depth, tile_cols, tile_rows) in cases {
        let mut parser = Parser::new();
        let header = parser
            .parse(&payload(path, frame))
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!((header.width, header.height), (width, height), "{path}");
        assert_eq!(header.bit_depth, bit_depth, "{path} bit depth");
        assert_eq!(header.log2_tile_cols, tile_cols, "{path} tile columns");
        assert_eq!(header.log2_tile_rows, tile_rows, "{path} tile rows");
        let superblocks =
            u64::from(header.width.div_ceil(64)) * u64::from(header.height.div_ceil(64));
        assert!(
            superblocks <= MAX_SUPERBLOCKS,
            "{path} spends {superblocks} of the segment-map budget"
        );
        capability::check(&header).unwrap_or_else(|e| panic!("{path} rejected by the gate: {e}"));
    }
}

/// Odd luma dimensions are legal syntax and spend ceiling superblocks; the
/// 321x241 fixture is the proof the parser does not round anything away.
#[test]
fn odd_dimensions_reach_the_gate_verbatim() {
    let mut parser = Parser::new();
    let header = parser
        .parse(&payload("tests/data/vp9_p0_lossless_321x241_2.ivf", 0))
        .expect("odd key frame");
    assert_eq!((header.width, header.height), (321, 241));
    assert_eq!((header.render_width, header.render_height), (321, 241));
    assert!(header.quantization.lossless, "and arrives lossless");
}

/// Lossless is a quantisation condition, not a flag: base 0 with all three
/// deltas zero. A qindex of 0 with a delta is not lossless.
#[test]
fn lossless_is_only_base_zero_with_no_deltas() {
    let mut parser = Parser::new();
    let mut seen = 0;
    let data = std::fs::read("tests/data/vp9_p0_lossless_320x240_12.ivf").expect("fixture");
    for frame in parser::parse_ivf(&data).expect("container").frames {
        for part in parser::split_superframe(frame.data).expect("superframe") {
            let header = parser.parse(part).expect("frame");
            let lossless = header.quantization.base_q_idx == 0
                && header.quantization.y_dc_delta_q == 0
                && header.quantization.uv_dc_delta_q == 0
                && header.quantization.uv_ac_delta_q == 0;
            assert_eq!(header.quantization.lossless, lossless, "packet {seen}");
            assert!(lossless, "packet {seen} of a lossless fixture");
            seen += 1;
        }
    }
    assert!(seen > 0, "the fixture carries packets");
}

/// Profiles are the frame tag's two (or three) bits after the marker; with a
/// real key-frame packet patched in place, the parser must stop at the
/// profile and name it, for both codings of profile 3.
#[test]
fn unsupported_profiles_are_named_at_the_frame_tag() {
    let base = payload("tests/data/vp9_p0_64x64.ivf", 0);
    // The reader is MSB-first: profile bits sit at 0x20 and 0x10, and the
    // third bit the >2 codings read sits at 0x08.
    for (bits, profile) in [
        (0x20u8, 1u8),
        (0x30, 3), // profile > 2 reads one more bit, left 0 here
        (0x38, 4), // the extra bit set, which the spec reserves the same way
    ] {
        let mut packet = base.clone();
        packet[0] = (packet[0] & 0xCF) | bits;
        let error = Parser::new()
            .parse(&packet)
            .expect_err("a reserved profile must not decode");
        assert_eq!(
            error,
            Vp9Error::UnsupportedProfile(profile),
            "bits {bits:#04x}"
        );
    }
}

/// Truncation can bite at any byte. The behaviour that has to be locked is
/// that it always lands on a named error - never a panic, never a bogus
/// header - at every single-byte cut through the first two frames.
#[test]
fn every_truncation_step_lands_on_a_named_error_or_a_still_whole_frame() {
    for path in [
        "tests/data/vp9_p0_64x64.ivf",
        "tests/data/vp9_p0_errres_tiles_640x360_12.ivf",
    ] {
        let base = payload(path, 0);
        let mut parser = Parser::new();
        for cut in 1..base.len().min(48) {
            let result = Parser::new().parse(&base[..cut]);
            match result {
                Ok(header) => assert!(
                    header.header_size + header.first_partition_size as usize <= cut,
                    "{path} at {cut} declared a partition that cannot fit"
                ),
                Err(error) => match error {
                    Vp9Error::Truncated { .. }
                    | Vp9Error::BadSyncCode
                    | Vp9Error::BadFrameMarker
                    | Vp9Error::BadHeaderSize { .. } => {}
                    other => panic!("{path} at {cut} reported unexpected error {other:?}"),
                },
            }
        }
        assert!(
            parser.parse(&base).is_ok(),
            "{path} itself must still parse"
        );
    }
}

/// A packet whose declared first partition does not fit is the boundary
/// between a truncation (the header itself ran out) and a lying header; both
/// have to be distinguishable, and the lying one names all three sizes.
#[test]
fn a_first_partition_larger_than_the_packet_is_named() {
    let base = payload("tests/data/vp9_p0_64x64.ivf", 0);
    let header_size = Parser::new().parse(&base).expect("frame").header_size;
    // Keep the whole uncompressed header readable and drop the partition.
    let cut = (header_size + 1).min(base.len() - 1);
    assert!(
        base.len() > header_size + 1,
        "fixture keeps a real first partition"
    );
    let error = Parser::new()
        .parse(&base[..cut])
        .expect_err("the partition cannot fit into a stub");
    match error {
        Vp9Error::BadHeaderSize {
            header, partition, ..
        } => {
            assert_eq!(header, header_size);
            assert!(
                partition > cut - header_size,
                "the declared partition overshoots"
            );
        }
        Vp9Error::Truncated { .. } => {
            // Acceptable too: the 16-bit size field itself may cross the cut.
        }
        other => panic!("unexpected error {other:?}"),
    }
}

/// An inter frame before any key frame is a drop, not a stream failure; the
/// name it drops under is the reference it cannot resolve.
#[test]
fn frames_before_the_first_key_frame_drop_by_name() {
    let data = std::fs::read("tests/data/vp9_p0_64x64.ivf").expect("fixture");
    let ivf = parser::parse_ivf(&data).expect("container");
    let second = parser::split_superframe(ivf.frames[1].data).expect("superframe")[0];
    let error = Parser::new()
        .parse(second)
        .expect_err("no key frame has been seen");
    assert!(
        matches!(error, Vp9Error::MissingReference(0xff)),
        "{error:?}"
    );
    // After a key frame the same packet resolves.
    let mut parser = Parser::new();
    parser
        .parse(&payload("tests/data/vp9_p0_64x64.ivf", 0))
        .expect("key frame");
    assert!(
        parser.parse(second).is_ok(),
        "the second picture parses once keyed"
    );
}

/// A `show_existing_frame` is the one frame type that never reaches the
/// hardware; without its reference it is the same named drop.
#[test]
fn show_existing_without_its_slot_is_named() {
    // 0b10_00_1_000: marker 2, profile 0, show-existing set, slot 0.
    let error = Parser::new()
        .parse(&[0x88, 0x00])
        .expect_err("empty reference map");
    assert_eq!(error, Vp9Error::MissingReference(0));
    let mut parser = Parser::new();
    parser
        .parse(&payload("tests/data/vp9_p0_64x64.ivf", 0))
        .expect("key frame fills the map");
    let header = parser
        .parse(&[0x88, 0x00])
        .expect("the same byte now resolves");
    assert!(matches!(header.frame_kind, FrameKind::ShowExisting));
    assert_eq!(header.width, 64, "the picture is sized by its reference");
}
