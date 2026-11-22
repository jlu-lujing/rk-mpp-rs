use std::mem::{align_of, offset_of, size_of};
use std::path::PathBuf;

use rk_mpp_rs::codec::av1::cdf::{
    coefficient_context, Av1Cdf, Av1CdfStore, Av1Cdfs, Av1MvCdfs, MV_CDF_OFFSET, MV_CDF_SIZE,
    REF_FRAME_COUNT,
};
use rk_mpp_rs::codec::av1::parser::{parse_ivf, split_obus};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/av1_cdf")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

#[test]
fn cdf_layout_matches_the_c_board_abi() {
    assert_eq!(size_of::<Av1Cdfs>(), 12_256);
    assert_eq!(align_of::<Av1Cdfs>(), 2);
    assert_eq!(offset_of!(Av1Cdfs, mv_cdf), MV_CDF_OFFSET);
    assert_eq!(MV_CDF_OFFSET, 2_192);
    assert_eq!(size_of::<Av1MvCdfs>(), MV_CDF_SIZE);
    assert_eq!(MV_CDF_SIZE, 138);
    assert_eq!(offset_of!(Av1Cdfs, obmc_cdf), 2_330);
    assert_eq!(offset_of!(Av1Cdfs, dummy0), 4_036);
    assert_eq!(offset_of!(Av1Cdfs, palette_y_color_index_cdf), 4_064);
    assert_eq!(offset_of!(Av1Cdfs, tx_type_intra0_cdf), 4_640);
    assert_eq!(offset_of!(Av1Cdfs, txb_skip_cdf), 6_144);
    assert_eq!(offset_of!(Av1Cdfs, coeff_br_cdf), 10_544);
    assert_eq!(offset_of!(Av1Cdfs, dummy2), 12_224);
}

#[test]
fn q_index_buckets_match_c_mpp_at_every_boundary() {
    for (q_index, context) in [
        (0, 0),
        (20, 0),
        (21, 1),
        (60, 1),
        (61, 2),
        (96, 2),
        (120, 2),
        (121, 3),
    ] {
        assert_eq!(coefficient_context(q_index), context);
    }
}

#[test]
fn every_representative_q_context_matches_the_c_snapshot_byte_for_byte() {
    for (q_index, name) in [
        (0, "cdf_q000.bin"),
        (21, "cdf_q021.bin"),
        (61, "cdf_q061.bin"),
        (121, "cdf_q121.bin"),
    ] {
        let expected = fixture(name);
        let actual = Av1Cdfs::default_for_q_index(q_index);
        assert_eq!(
            actual.as_bytes(),
            expected.as_slice(),
            "AV1 CDF mismatch for q_index={q_index} ({name})"
        );
    }
}

#[test]
fn intrabc_mv_defaults_match_the_c_snapshot() {
    let expected = fixture("cdf_ndvc.bin");
    let actual = Av1MvCdfs::default_intrabc();
    assert_eq!(actual.as_bytes(), expected.as_slice());
}

#[test]
fn reference_slots_copy_both_regular_and_intrabc_cdfs() {
    let cdf = Av1Cdf::new(
        Av1Cdfs::default_for_q_index(61),
        Av1MvCdfs::default_intrabc(),
    );
    let mut store = Av1CdfStore::new();
    store.store(0b0010_0100, &cdf);

    assert_eq!(store.load(2), &cdf);
    assert_eq!(store.load(5), &cdf);
    assert_eq!(store.load(1), &Av1Cdf::empty());
}

#[test]
fn show_existing_key_frame_copies_the_selected_slot_to_all_slots() {
    let cdf = Av1Cdf::new(
        Av1Cdfs::default_for_q_index(121),
        Av1MvCdfs::default_intrabc(),
    );
    let mut store = Av1CdfStore::new();
    store.store(1 << 6, &cdf);
    store.copy_key_frame_to_all_slots(6);

    for slot in 0..REF_FRAME_COUNT {
        assert_eq!(store.load(slot), &cdf, "slot {slot}");
    }
}

#[test]
fn intra_hardware_update_keeps_intrabc_mv_outside_the_regular_mv_region() {
    let original = Av1Cdf::new(
        Av1Cdfs::default_for_q_index(21),
        Av1MvCdfs::default_intrabc(),
    );
    let mut store = Av1CdfStore::new();
    store.store(1, &original);

    let mut hardware = vec![0u8; size_of::<Av1Cdfs>()];
    for (index, byte) in hardware.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(37).wrapping_add(11);
    }
    store.apply_hardware_update(1, true, &hardware);

    let updated = store.load(0);
    assert_eq!(
        &updated.regular.as_bytes()[..MV_CDF_OFFSET],
        &hardware[..MV_CDF_OFFSET]
    );
    assert_eq!(
        &updated.regular.as_bytes()[MV_CDF_OFFSET..MV_CDF_OFFSET + MV_CDF_SIZE],
        &original.regular.as_bytes()[MV_CDF_OFFSET..MV_CDF_OFFSET + MV_CDF_SIZE]
    );
    assert_eq!(
        updated.intrabc_mv.as_bytes(),
        &hardware[MV_CDF_OFFSET..MV_CDF_OFFSET + MV_CDF_SIZE]
    );
    assert_eq!(
        &updated.regular.as_bytes()[MV_CDF_OFFSET + MV_CDF_SIZE..],
        &hardware[MV_CDF_OFFSET + MV_CDF_SIZE..]
    );
}

#[test]
fn inter_hardware_update_replaces_the_whole_regular_cdf() {
    let original = Av1Cdf::new(
        Av1Cdfs::default_for_q_index(21),
        Av1MvCdfs::default_intrabc(),
    );
    let mut store = Av1CdfStore::new();
    store.store(1, &original);

    let hardware: Vec<u8> = (0..size_of::<Av1Cdfs>())
        .map(|index| (index as u8).wrapping_mul(13).wrapping_add(7))
        .collect();
    store.apply_hardware_update(1, false, &hardware);

    assert_eq!(store.load(0).regular.as_bytes(), hardware);
    assert_eq!(store.load(0).intrabc_mv, original.intrabc_mv);
}

#[test]
fn each_frame_uses_the_cdf_updated_by_the_previous_hardware_job() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/av1_320x240_10.ivf");
    let data = std::fs::read(path).expect("read AV1 fixture");
    let ivf = parse_ivf(&data).expect("parse IVF");
    let mut parser = rk_mpp_rs::codec::av1::assembler::Av1PacketParser::new();

    parser
        .parse_packet(ivf.frames[0].data)
        .expect("parse key frame");
    let hardware: Vec<u8> = (0..12_256)
        .map(|index| (index as u8).wrapping_mul(37).wrapping_add(11))
        .collect();
    parser.apply_hardware_cdf(0xff, true, &hardware);

    let obus = split_obus(ivf.frames[1].data).expect("split inter-frame packet");
    parser.begin_packet();
    let mut cursor = 0;
    let mut checked_primary_reference = false;
    while let Some(event) = parser
        .parse_next_event(&obus, &mut cursor)
        .expect("parse next frame event")
    {
        let rk_mpp_rs::codec::av1::assembler::Av1PacketEvent::Coded(frame) = event else {
            continue;
        };

        if frame.header.order_hint == 2 {
            assert_eq!(frame.header.primary_ref_frame, 4);
            let source_slot =
                frame.header.ref_frame_idx[frame.header.primary_ref_frame as usize] as usize;
            assert_eq!(source_slot, 2);
            let expected = parser.frame_state().cdfs.load(source_slot);
            assert_eq!(
                &frame.cdf, expected,
                "order_hint={} primary_ref_frame={} source_slot={source_slot}",
                frame.header.order_hint, frame.header.primary_ref_frame
            );
            assert_ne!(
                frame.cdf.regular.as_bytes(),
                Av1Cdfs::default_for_q_index(u32::from(frame.header.base_q_idx)).as_bytes()
            );
            checked_primary_reference = true;
        }

        let frame_is_intra = matches!(
            frame.header.frame_type,
            rk_mpp_rs::codec::av1::parser::FrameType::Key
                | rk_mpp_rs::codec::av1::parser::FrameType::IntraOnly
        );
        let hardware: Vec<u8> = (0..12_256)
            .map(|index| {
                (index as u8)
                    .wrapping_mul(37)
                    .wrapping_add(frame.header.order_hint)
            })
            .collect();
        parser.apply_hardware_cdf(frame.header.refresh_frame_flags, frame_is_intra, &hardware);
    }
    assert!(checked_primary_reference);
}
