//! H.265 encode integration: the fixed-QP IDR parameter sets and the derived
//! register image, driven through the crate's public API.
//!
//! These run on the host - the parameter sets are pure software output and the
//! register image is pure derivation. The parameter sets are compared byte for
//! byte against streams the C encoder wrote on RK3588
//! (`tests/data/h265e_c_fixqp_qp26_*.265`); the board test that reproduces the
//! same comparison live is ignored and runs via `scripts/board_h265_qp_vs_mpp.sh`.

use rk_mpp_rs::codec::h265::encoder::{
    build_headers, build_pps, build_prefix, build_sps, EncoderConfig,
};
use rk_mpp_rs::codec::hevc::parser;
use rk_mpp_rs::hal::h265e::{build_registers, IdrJob};
use rk_mpp_rs::hal::h265e_regs;

/// The default 320x240 IDR's parameter sets, checked by feeding each NAL back
/// through the independent decoder parser. This is the byte-comparable gate for
/// M8: whatever the C encoder prepends to a fixed-QP IDR, our writer must produce
/// the same VPS/SPS/PPS.
#[test]
fn default_idr_parameter_sets_parse_back() {
    let config = EncoderConfig::new(320, 240);
    let headers = build_headers(&config).unwrap();

    // Three NAL units back to back, VPS first.
    assert_eq!(headers[0..4], [0, 0, 0, 1]);
    assert_eq!((headers[4] >> 1) & 0x3f, 32); // VPS
                                              // `42 01`: nal_unit_type 33 in the first byte, nuh_layer_id 0 and
                                              // nuh_temporal_id_plus1 1 in the second. `42 81` was layer_id 2 - the board's
                                              // SPS settled it.
    assert!(build_sps(&config)
        .unwrap()
        .starts_with(&[0, 0, 0, 1, 0x42, 0x01]));

    // Round-trip the SPS and PPS. The payload is passed through the parser's
    // escape stripper, because the writer emits an EBSP (Annex B) and the parser
    // reads an RBSP.
    let sps = build_sps(&config).unwrap();
    let parsed_sps = parser::parse_sps(&parser::ebsp_to_rbsp(&sps[6..])).unwrap();
    assert_eq!((parsed_sps.width, parsed_sps.height), (320, 240));
    assert!(parsed_sps.sao_enabled);
    assert!(!parsed_sps.amp_enabled);

    let pps = build_pps(&config).unwrap();
    let parsed_pps = parser::parse_pps(&parser::ebsp_to_rbsp(&pps[6..])).unwrap();
    assert_eq!(parsed_pps.init_qp, 26);
    // Fixed QP: `h265e_set_pps` leaves m_useDQP at 0.
    assert!(!parsed_pps.cu_qp_delta_enabled);
    assert_eq!(parsed_pps.log2_parallel_merge_level, 2);
}

/// The PPS byte golden for the default (fixed-QP) configuration, taken from the
/// oracle's own stream on RK3588 rather than hand-derived: `mpi_enc_test -t 16777220
/// -n 1 -rc 2 -qc 26:...` writes `44 01 c0 f1 80 9b 20`. The `f1` is
/// `cabac_init 1, ref_idx 1/1, init_qp 1, constrained 0, transform_skip 0,
/// cu_qp_delta 0` - the last field is the one a hard-wired `m_useDQP = 1` got
/// wrong, which produced `c0 f3 c0 4d 90` and shifted everything after it.
#[test]
fn default_pps_matches_the_c_bytes() {
    let pps = build_pps(&EncoderConfig::new(320, 240)).unwrap();
    assert_eq!(
        pps,
        vec![0, 0, 0, 1, 0x44, 0x01, 0xc0, 0xf1, 0x80, 0x9b, 0x20]
    );
}

/// The VPS/SPS/PPS prefix of a recorded oracle stream: everything up to the first
/// NAL that is not a parameter set (32/33/34). A plain start-code scan is safe
/// here because the writer escapes the payload, so `00 00 01` cannot occur inside
/// a NAL - that was not true before the escape landed.
fn parameter_set_prefix(stream: &[u8]) -> &[u8] {
    let mut search = 0;
    while let Some(pos) = stream[search..].windows(3).position(|w| w == [0, 0, 1]) {
        let code = search + pos;
        let nal = code + 3;
        let unit_type = (stream[nal] >> 1) & 0x3f;
        if (32..=34).contains(&unit_type) {
            search = nal;
            continue;
        }
        // Include a leading zero byte when the parameter sets used a four-byte
        // start code, so the prefix is exactly what `build_headers` writes.
        return &stream[..if code > 0 && stream[code - 1] == 0 {
            code - 1
        } else {
            code
        }];
    }
    stream
}

/// The complete parameter-set gate, on the host, against oracle streams recorded
/// on RK3588: `mpi_enc_test -i probe.nv12 -o out.265 -w W -h H -t 16777220 -f 0 -n 1
/// -rc 2 -qc 26:26:26:26:26 -v q`. Each fixture is the full 860/1618/3585-byte
/// stream (VPS, SPS, PPS, two SEIs and the IDR); only the prefix is compared here,
/// because the IDR payload needs the register-job template.
#[test]
fn parameter_sets_match_the_recorded_c_streams() {
    for (w, h, file) in [
        (320usize, 240usize, "h265e_c_fixqp_qp26_320x240.265"),
        (640, 480, "h265e_c_fixqp_qp26_640x480.265"),
        (1280, 720, "h265e_c_fixqp_qp26_1280x720.265"),
    ] {
        let path = format!("{}/tests/data/{file}", env!("CARGO_MANIFEST_DIR"));
        let c = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let rust = build_headers(&EncoderConfig::new(w, h)).unwrap();
        let prefix = parameter_set_prefix(&c);
        assert_eq!(
            rust.len(),
            prefix.len(),
            "{file}: parameter-set prefix length"
        );
        assert_eq!(rust, prefix, "{file}: parameter sets differ");
        // The reference's next NAL is an SEI (39), not another parameter set: the
        // prefix stopped on a real boundary rather than by running out of stream.
        let next = &c[prefix.len()..];
        let after_code = next.windows(3).position(|w| w == [0, 0, 1]).unwrap() + 3;
        assert_eq!((next[after_code] >> 1) & 0x3f, 39, "{file}: next NAL");
    }
}

/// The full software prefix - parameter sets and both SEIs - against the oracle's
/// access unit. The hardware's slice NAL is compared on the board
/// (`scripts/board_h265_idr_vs_mpp.sh`), because only a submitted job produces it.
#[test]
fn prefix_matches_the_recorded_c_access_unit_prefix() {
    for (w, h, file) in [
        (320usize, 240usize, "h265e_c_fixqp_qp26_320x240.265"),
        (640, 480, "h265e_c_fixqp_qp26_640x480.265"),
        (1280, 720, "h265e_c_fixqp_qp26_1280x720.265"),
    ] {
        let path = format!("{}/tests/data/{file}", env!("CARGO_MANIFEST_DIR"));
        let c = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let prefix = build_prefix(&EncoderConfig::new(w, h), 26, 60).unwrap();
        assert!(
            c.len() > prefix.len(),
            "{file}: the reference is only {} bytes",
            c.len()
        );
        assert_eq!(&c[..prefix.len()], &prefix[..], "{file}: prefix differs");
        // The next NAL is the IDR (19).
        let after = c[prefix.len()..].windows(4).position(|w| w == [0, 0, 0, 1]);
        let header = after.map(|i| prefix.len() + i + 4).unwrap_or(prefix.len());
        assert_eq!(
            (c[header] >> 1) & 0x3f,
            19,
            "{file}: next NAL is not an IDR"
        );
    }
}

/// A small size matrix: the coded picture is padded to the 8-sized minimum CU and
/// a conformance window carries the pad, which the parser resolves back to the
/// visible size. Non-multiples of 16 (322x242) are covered here since the header
/// path does not depend on the raw-frame seeking that limits the byte comparison.
#[test]
fn coded_size_matrix_round_trips() {
    for (w, h, vis_w, vis_h) in [
        (320usize, 240usize, 320usize, 240usize),
        (64, 64, 64, 64),
        (322, 242, 322, 242),
        (1280, 720, 1280, 720),
    ] {
        let config = EncoderConfig::new(w, h);
        let sps = build_sps(&config).unwrap();
        let parsed = parser::parse_sps(&parser::ebsp_to_rbsp(&sps[6..])).unwrap();
        assert_eq!(
            (parsed.width, parsed.height),
            (vis_w as u32, vis_h as u32),
            "{w}x{h}"
        );
    }
}

/// The derived register image for a fixed-QP IDR: the picture QP lands in
/// `enc_pic.pic_qp` exactly as `set_rc_regs` writes it, and the tuning windows are
/// still zero (no board template yet).
#[test]
fn derived_registers_carry_the_qp() {
    for qp in [20u32, 26, 32] {
        let mut job = IdrJob::reference(qp);
        job.qp_min = qp;
        job.qp_max = qp;
        let regs = build_registers(&job).unwrap();
        assert_eq!(
            regs.get(&h265e_regs::BASE_ENC_PIC_PIC_QP).unwrap(),
            qp,
            "pic_qp at QP {qp}"
        );
        assert_eq!(
            regs.get(&h265e_regs::H265_REG0236_SYNT_NAL_NAL_UNIT_TYPE)
                .unwrap(),
            19,
            "IDR_W_RADL"
        );
    }
}

/// The board comparison is a separate, single-session gate. It runs the reference
/// `mpi_enc_test -t <hevc> -rc 2` and checks the parameter-set prefix with
/// `scripts/h265_hdr_cmp.py`; the IDR payload stays PENDING until the recorded VEPU580
/// job template lands.
#[test]
#[ignore = "board-only: needs RK3588 and the source-compiled MPP reference"]
fn board_parameter_sets_match_the_c_encoder() {
    let status = std::process::Command::new("scripts/board_h265_qp_vs_mpp.sh")
        .status()
        .expect("run scripts/board_h265_qp_vs_mpp.sh");
    assert!(status.success(), "H.265 board parameter-set compare failed");
}

/// The submitted-IDR gate: runs `h265-idr` on the board against `mpi_enc_test` and
/// compares whole streams - parameter sets, both SEIs and the hardware's slice NAL.
#[test]
#[ignore = "board-only: needs RK3588, the MPP reference and /dev/mpp_service"]
fn board_idr_matches_the_c_encoder() {
    let status = std::process::Command::new("scripts/board_h265_idr_vs_mpp.sh")
        .env("ONLY", "320x240 640x480 1280x720")
        .status()
        .expect("run scripts/board_h265_idr_vs_mpp.sh");
    assert!(status.success(), "H.265 board IDR compare failed");
}

/// The GOP gate: an opening IDR plus P pictures, whole streams byte for byte.
#[test]
#[ignore = "board-only: needs RK3588, the MPP reference and /dev/mpp_service"]
fn board_gop_matches_the_c_encoder() {
    let status = std::process::Command::new("scripts/board_h265_gop_vs_mpp.sh")
        .env("ONLY", "320x240 640x480 1280x720")
        .env("FRAMES", "2")
        .status()
        .expect("run scripts/board_h265_gop_vs_mpp.sh");
    assert!(status.success(), "H.265 board GOP compare failed");
}

/// The CBR path with the C's own per-picture decisions.
#[test]
#[ignore = "board-only: needs RK3588, the MPP reference and /dev/mpp_service"]
fn board_cbr_path_matches_the_c_encoder() {
    let status = std::process::Command::new("scripts/board_h265_cbr_vs_mpp.sh")
        .env("SIZES", "320x240 640x480 1280x720")
        .status()
        .expect("run scripts/board_h265_cbr_vs_mpp.sh");
    assert!(status.success(), "H.265 board CBR compare failed");
}
