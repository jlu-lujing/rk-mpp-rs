use std::mem::{align_of, offset_of, size_of};
use std::path::PathBuf;

use rk_mpp_rs::codec::av1::assembler::{Av1PacketEvent, Av1PacketParser};
use rk_mpp_rs::codec::av1::cdf::{Av1Cdf, Av1Cdfs, Av1MvCdfs};
use rk_mpp_rs::codec::av1::global_motion::WarpedMotion;
use rk_mpp_rs::codec::av1::parser::{
    parse_sequence_header, split_obus, FrameHeader, FrameType, GlobalMotion, ReferenceFrame,
    MAX_TILES,
};
use rk_mpp_rs::codec::av1::picparams::{Av1PicParams, PicParamsPointers, PIC_PARAMS_SIZE};

fn snapshot(index: usize) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/av1_picparams")
        .join(format!("picparams_{index:03}.bin"));
    std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

#[test]
fn picparams_layout_matches_the_c_board_abi() {
    assert_eq!(PIC_PARAMS_SIZE, 3_240);
    assert_eq!(size_of::<Av1PicParams>(), PIC_PARAMS_SIZE);
    assert_eq!(align_of::<Av1PicParams>(), 8);
    assert_eq!(offset_of!(Av1PicParams, width), 2);
    assert_eq!(offset_of!(Av1PicParams, coding), 24);
    assert_eq!(offset_of!(Av1PicParams, tiles), 32);
    assert_eq!(offset_of!(Av1PicParams, format), 1_324);
    assert_eq!(offset_of!(Av1PicParams, loop_filter), 1_334);
    assert_eq!(offset_of!(Av1PicParams, segmentation), 1_408);
    assert_eq!(offset_of!(Av1PicParams, film_grain), 1_680);
    assert_eq!(offset_of!(Av1PicParams, ref_frame_idx), 1_852);
    assert_eq!(offset_of!(Av1PicParams, frame_refs), 1_912);
    assert_eq!(offset_of!(Av1PicParams, frame_ref_state), 2_776);
    assert_eq!(offset_of!(Av1PicParams, ref_frame_sign_bias), 3_160);
    assert_eq!(offset_of!(Av1PicParams, upscaled_width), 3_188);
    assert_eq!(offset_of!(Av1PicParams, cdfs), 3_216);
    assert_eq!(offset_of!(Av1PicParams, tile_cols_log2), 3_232);
}

#[test]
fn every_multi_tile_hardware_job_matches_the_c_picparams_snapshot() {
    let data = std::fs::read("tests/data/av1_tiles_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let mut parser = Av1PacketParser::new();
    let mut coded_index = 0;

    for packet in &ivf.frames {
        for event in parser.parse_packet(packet.data).unwrap() {
            let Av1PacketEvent::Coded(frame) = event else {
                continue;
            };
            let expected = snapshot(coded_index);
            let current_slot =
                u16_at(&expected, offset_of!(Av1PicParams, curr_pic_texture_index)) as u8;
            let mut reference_texture_slots = [0x7f; 8];
            for logical in 0..7 {
                let source = frame.header.ref_frame_idx[logical] as usize;
                reference_texture_slots[source] =
                    expected[offset_of!(Av1PicParams, frame_refs) + logical * 108 + 48] as u8;
            }
            for (slot, value) in reference_texture_slots.iter_mut().enumerate() {
                let mapped = expected[offset_of!(Av1PicParams, ref_frame_map_texture_index) + slot];
                if mapped != 0xff {
                    *value = mapped;
                }
            }
            let pointers = PicParamsPointers {
                cdfs: u64_at(&expected, offset_of!(Av1PicParams, cdfs)),
                intrabc_cdfs: u64_at(&expected, offset_of!(Av1PicParams, cdfs_ndvc)),
            };
            let sequence = parser.sequence().unwrap();
            let actual = Av1PicParams::build(
                sequence,
                &frame.header,
                &frame.references_at_header,
                &frame.cdf,
                &frame.tile_ranges,
                frame.frame_start,
                frame.frame_tag_size,
                current_slot,
                &reference_texture_slots,
                pointers,
            );

            if let Some(offset) = actual
                .as_bytes()
                .iter()
                .zip(&expected)
                .position(|(actual, expected)| actual != expected)
            {
                panic!(
                    "coded frame {coded_index}, order_hint={}, PicParams byte {offset}: \
                     Rust {}, C MPP {} (frame_tag_size={}, first_tile={:?})",
                    frame.header.order_hint,
                    actual.as_bytes()[offset],
                    expected[offset],
                    frame.frame_tag_size,
                    frame.tile_ranges[0],
                );
            }
            coded_index += 1;
        }
    }

    assert_eq!(coded_index, 11);
}

#[test]
fn global_motion_parameters_match_the_c_mpp_conversion() {
    let cases = [
        (
            GlobalMotion {
                identity: false,
                rotation_zoom: false,
                translation: true,
                parameters: [2, 1, 0, 0, 0, 0],
            },
            1,
            [8192, -8192, 65536, 0, 0, 65536],
            [1, -1, 0, 0, 0, 0],
            [0; 4],
        ),
        (
            GlobalMotion {
                identity: false,
                rotation_zoom: true,
                translation: false,
                parameters: [4, 3, 2, 0, 0, 0],
            },
            2,
            [2048, -2048, 65538, 0, 0, 65538],
            [2, -2, 1, 0, 0, 0],
            [0; 4],
        ),
        (
            GlobalMotion {
                identity: false,
                rotation_zoom: true,
                translation: false,
                parameters: [0, 0, 194, 0, 0, 0],
            },
            2,
            [0, 0, 65730, 0, 0, 65730],
            [0, 0, 97, 0, 0, 0],
            [192, 0, 0, 192],
        ),
        (
            GlobalMotion {
                identity: false,
                rotation_zoom: false,
                translation: false,
                parameters: [2, 1, 3, 1, 2, 0],
            },
            3,
            [1024, -1024, 65532, -2, 2, 65536],
            [1, -1, -2, -1, 1, 0],
            [0; 4],
        ),
    ];
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let obus = split_obus(ivf.frames[0].data).unwrap();
    let sequence = parse_sequence_header(obus[1].payload).unwrap();

    for (global_motion, expected_type, expected_matrix, expected_values, expected_shear) in cases {
        let mut header = FrameHeader {
            frame_type: FrameType::Inter,
            width: 320,
            height: 240,
            upscaled_width: 320,
            render_width: 320,
            render_height: 240,
            primary_ref_frame: 7,
            allow_high_precision_mv: true,
            ..FrameHeader::default()
        };
        header.global_motion[1] = global_motion;

        let reference = ReferenceFrame {
            valid: true,
            upscaled_width: 320,
            frame_width: 320,
            frame_height: 240,
            render_width: 320,
            render_height: 240,
            order_hint: 1,
            ..ReferenceFrame::default()
        };
        let references = [reference; 8];
        let cdf = Av1Cdf::new(Av1Cdfs::default(), Av1MvCdfs::default());
        let params = Av1PicParams::build(
            &sequence,
            &header,
            &references,
            &cdf,
            &[const { None }; MAX_TILES],
            0,
            0,
            0,
            &[0x7f; 8],
            PicParamsPointers::default(),
        );

        let pic_reference = params.frame_refs[0];
        assert_eq!(pic_reference.wminvalid, 0);
        assert_eq!(usize::from(pic_reference.wmtype), expected_type);
        assert_eq!(pic_reference.wmmat, expected_matrix);
        assert_eq!(pic_reference.wmmat_val, expected_values);
        assert_eq!(
            [
                pic_reference.alpha,
                pic_reference.beta,
                pic_reference.gamma,
                pic_reference.delta,
            ]
            .map(|value| value as i16 as i32),
            expected_shear
        );
    }
}

#[test]
fn global_motion_reads_its_previous_value_from_the_primary_reference() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let obus = split_obus(ivf.frames[0].data).unwrap();
    let sequence = parse_sequence_header(obus[1].payload).unwrap();
    let mut header = FrameHeader {
        frame_type: FrameType::Inter,
        width: 320,
        height: 240,
        upscaled_width: 320,
        render_width: 320,
        render_height: 240,
        primary_ref_frame: 0,
        allow_high_precision_mv: true,
        ..FrameHeader::default()
    };
    header.global_motion[1] = GlobalMotion {
        identity: false,
        rotation_zoom: false,
        translation: true,
        parameters: [1, 1, 0, 0, 0, 0],
    };

    let mut reference = ReferenceFrame {
        valid: true,
        upscaled_width: 320,
        frame_width: 320,
        frame_height: 240,
        render_width: 320,
        render_height: 240,
        order_hint: 1,
        ..ReferenceFrame::default()
    };
    reference.warped_motion[1] = WarpedMotion {
        wmtype: 1,
        wmmat: [8192, -8192, 65536, 0, 0, 65536],
        wmmat_val: [1, -1, 0, 0, 0, 0],
        ..WarpedMotion::identity()
    };
    let references = [reference; 8];
    let cdf = Av1Cdf::new(Av1Cdfs::default(), Av1MvCdfs::default());
    let params = Av1PicParams::build(
        &sequence,
        &header,
        &references,
        &cdf,
        &[const { None }; MAX_TILES],
        0,
        0,
        0,
        &[0x7f; 8],
        PicParamsPointers::default(),
    );

    assert_eq!(
        params.frame_refs[0].wmmat,
        [16384, -16384, 65536, 0, 0, 65536]
    );
    assert_eq!(params.frame_refs[0].wmmat_val, [2, -2, 0, 0, 0, 0]);
}
