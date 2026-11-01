//! H.265 encode integration: the fixed-QP IDR parameter sets and the derived
//! register image, driven through the crate's public API.
//!
//! These run on the host - the parameter sets are pure software output and the
//! register image is pure derivation. The board-only test that compares them
//! against the reference C encoder is ignored and runs via
//! `scripts/board_h265_qp_vs_mpp.sh`.

use rk_mpp_rs::codec::h265::encoder::{build_headers, build_pps, build_sps, EncoderConfig};
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
    assert!(build_sps(&config)
        .unwrap()
        .starts_with(&[0, 0, 0, 1, 0x42, 0x81]));

    // Round-trip the SPS and PPS.
    let sps = build_sps(&config).unwrap();
    let parsed_sps = parser::parse_sps(&sps[6..]).unwrap();
    assert_eq!((parsed_sps.width, parsed_sps.height), (320, 240));
    assert!(parsed_sps.sao_enabled);
    assert!(!parsed_sps.amp_enabled);

    let pps = build_pps(&config).unwrap();
    let parsed_pps = parser::parse_pps(&pps[6..]).unwrap();
    assert_eq!(parsed_pps.init_qp, 26);
    assert!(parsed_pps.cu_qp_delta_enabled);
    assert_eq!(parsed_pps.log2_parallel_merge_level, 2);
}

/// The PPS byte golden for the default configuration - hand-derived from
/// h265e_set_pps + h265e_pps_write, and independent of the requested QP.
#[test]
fn default_pps_matches_the_c_bytes() {
    let pps = build_pps(&EncoderConfig::new(320, 240)).unwrap();
    assert_eq!(
        pps,
        vec![0, 0, 0, 1, 0x44, 0x01, 0xc0, 0xf3, 0xc0, 0x4d, 0x90]
    );
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
        let parsed = parser::parse_sps(&sps[6..]).unwrap();
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
#[ignore = "board-only: needs t6 and the source-compiled MPP reference"]
fn board_parameter_sets_match_the_c_encoder() {
    let status = std::process::Command::new("scripts/board_h265_qp_vs_mpp.sh")
        .status()
        .expect("run scripts/board_h265_qp_vs_mpp.sh");
    assert!(status.success(), "H.265 board parameter-set compare failed");
}
