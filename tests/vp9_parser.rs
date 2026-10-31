use rk_mpp_rs::codec::vp9::parser::{FrameKind, Parser, Profile, Vp9Error};

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bit: u8,
}

impl BitWriter {
    fn put(&mut self, value: u32, width: u8) {
        for shift in (0..width).rev() {
            if self.bit == 0 {
                self.bytes.push(0);
            }
            if (value >> shift) & 1 != 0 {
                let last = self.bytes.len() - 1;
                self.bytes[last] |= 1 << (7 - self.bit);
            }
            self.bit = (self.bit + 1) & 7;
        }
    }

    fn align(&mut self) {
        while self.bit != 0 {
            self.put(0, 1);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        self.align();
        self.bytes
    }
}

fn keyframe_header(profile: u8, bit_depth_minus_8: u8, width: u32, height: u32) -> Vec<u8> {
    let mut writer = BitWriter::default();
    writer.put(2, 2);
    writer.put((profile & 1) as u32, 1);
    writer.put((profile >> 1) as u32, 1);
    writer.put(0, 1); // show_existing_frame
    writer.put(0, 1); // key frame
    writer.put(1, 1); // show_frame
    writer.put(1, 1); // error_resilient_mode
    writer.put(0x498342, 24);
    if profile == 2 {
        writer.put(((bit_depth_minus_8 - 2) >> 1) as u32, 1);
    }
    writer.put(1, 3); // color_space
    writer.put(0, 1); // color_range
    writer.put(width - 1, 16);
    writer.put(height - 1, 16);
    writer.put(0, 1); // render_size_present
    writer.put(0, 2); // frame_context_idx
    writer.put(0, 6); // loop_filter_level
    writer.put(0, 3); // sharpness_level
    writer.put(0, 1); // mode_ref_delta_enabled
    writer.put(100, 8); // base_qindex
    writer.put(0, 1);
    writer.put(0, 1);
    writer.put(0, 1);
    writer.put(0, 1); // segmentation_enabled
    let sb_cols = width.div_ceil(64);
    let mut max_log2 = 0u32;
    while (sb_cols >> max_log2) >= 4 {
        max_log2 += 1;
    }
    for _ in 0..max_log2.saturating_sub(1) {
        writer.put(0, 1); // log2_tile_cols == 0
    }
    writer.put(0, 1); // log2_tile_rows == 0
    writer.put(2, 16); // first_partition_size
    writer.align();
    writer.bytes.extend_from_slice(&[0, 0]);
    writer.finish()
}

fn inter_header() -> Vec<u8> {
    let mut writer = BitWriter::default();
    writer.put(2, 2);
    writer.put(0, 1); // profile bit 0
    writer.put(0, 1); // profile bit 1
    writer.put(0, 1); // show_existing_frame
    writer.put(1, 1); // inter frame
    writer.put(1, 1); // show_frame
    writer.put(1, 1); // error_resilient_mode
    writer.put(0x02, 8); // refresh_frame_flags
    for ref_idx in 0..3 {
        writer.put(ref_idx, 3);
        writer.put(0, 1); // sign_bias
    }
    writer.put(1, 1); // last-frame size
    writer.put(0, 1); // render_size_present
    writer.put(0, 1); // allow_high_precision_mv
    writer.put(0, 1); // interp_filter is fixed
    writer.put(0, 2); // eight-tap smooth
    writer.put(1, 2); // frame_context_idx
    writer.put(0, 6); // loop_filter_level
    writer.put(0, 3); // sharpness_level
    writer.put(0, 1); // mode_ref_delta_enabled
    writer.put(100, 8); // base_qindex
    writer.put(0, 1);
    writer.put(0, 1);
    writer.put(0, 1);
    writer.put(0, 1); // segmentation_enabled
    writer.put(0, 1); // log2_tile_rows == 0
    writer.put(2, 16); // first_partition_size
    writer.align();
    writer.bytes.extend_from_slice(&[0, 0]);
    writer.finish()
}

fn profile_two_intra_only_header() -> Vec<u8> {
    let mut writer = BitWriter::default();
    writer.put(2, 2);
    writer.put(0, 1); // profile_low_bit
    writer.put(1, 1); // profile_high_bit
    writer.put(0, 1); // show_existing_frame
    writer.put(1, 1); // inter frame with intra_only set
    writer.put(0, 1); // show_frame
    writer.put(1, 1); // error_resilient_mode
    writer.put(1, 1); // intra_only
    writer.put(0x498342, 24);
    writer.put(0, 1); // 10-bit
    writer.put(1, 3); // color_space
    writer.put(0, 1); // color_range
    writer.put(0x01, 8); // refresh_frame_flags
    writer.put(1919, 16);
    writer.put(1079, 16);
    writer.put(1, 1); // render size differs from frame size
    writer.put(1279, 16);
    writer.put(719, 16);
    writer.put(0, 2); // frame_context_idx
    writer.put(0, 6); // loop_filter_level
    writer.put(0, 3); // sharpness_level
    writer.put(0, 1); // mode_ref_delta_enabled
    writer.put(100, 8); // base_qindex
    writer.put(0, 1);
    writer.put(0, 1);
    writer.put(0, 1);
    writer.put(0, 1); // segmentation_enabled
    writer.put(0, 1); // log2_tile_cols == 0
    writer.put(0, 1); // log2_tile_cols == 0
    writer.put(0, 1); // log2_tile_rows == 0
    writer.put(2, 16); // first_partition_size
    writer.align();
    writer.bytes.extend_from_slice(&[0, 0]);
    writer.finish()
}

fn keyframe_header_with_unupdated_segmentation_probabilities() -> Vec<u8> {
    let mut writer = BitWriter::default();
    writer.put(2, 2);
    writer.put(0, 1); // profile_low_bit
    writer.put(0, 1); // profile_high_bit
    writer.put(0, 1); // show_existing_frame
    writer.put(0, 1); // key frame
    writer.put(1, 1); // show_frame
    writer.put(1, 1); // error_resilient_mode
    writer.put(0x498342, 24);
    writer.put(1, 3); // color_space
    writer.put(0, 1); // color_range
    writer.put(63, 16);
    writer.put(63, 16);
    writer.put(0, 1); // render_size_present
    writer.put(0, 2); // frame_context_idx
    writer.put(0, 6); // loop_filter_level
    writer.put(0, 3); // sharpness_level
    writer.put(0, 1); // mode_ref_delta_enabled
    writer.put(100, 8); // base_qindex
    writer.put(0, 1);
    writer.put(0, 1);
    writer.put(0, 1);
    writer.put(1, 1); // segmentation_enabled
    writer.put(1, 1); // update_map
    for _ in 0..7 {
        writer.put(0, 1); // tree probability is not updated
    }
    writer.put(1, 1); // temporal_update
    for _ in 0..3 {
        writer.put(0, 1); // predictor probability is not updated
    }
    writer.put(0, 1); // update_data
    writer.put(0, 1); // log2_tile_cols == 0
    writer.put(0, 1); // log2_tile_rows == 0
    writer.put(2, 16); // first_partition_size
    writer.align();
    writer.bytes.extend_from_slice(&[0, 0]);
    writer.finish()
}

fn show_existing_header(map_index: u8) -> Vec<u8> {
    let mut writer = BitWriter::default();
    writer.put(2, 2); // frame_marker
    writer.put(0, 1); // profile_low_bit
    writer.put(0, 1); // profile_high_bit
    writer.put(1, 1); // show_existing_frame
    writer.put(map_index as u32, 3);
    writer.finish()
}

#[test]
fn splits_a_superframe_index_in_frame_order() {
    let first = [0x82, 0x01, 0x02, 0x03];
    let second = [0x84, 0x11, 0x12, 0x13, 0x14];
    let marker = 0xc1;
    let mut packet = Vec::new();
    packet.extend_from_slice(&first);
    packet.extend_from_slice(&second);
    packet.extend_from_slice(&[marker, first.len() as u8, second.len() as u8, marker]);

    let frames = rk_mpp_rs::codec::vp9::parser::split_superframe(&packet).unwrap();
    assert_eq!(frames, vec![first.as_slice(), second.as_slice()]);
}

#[test]
fn reads_multi_byte_superframe_sizes_little_endian() {
    let first = vec![0x82; 300];
    let second = vec![0x84; 2];
    let marker = 0xc9;
    let mut packet = Vec::new();
    packet.extend_from_slice(&first);
    packet.extend_from_slice(&second);
    packet.extend_from_slice(&[marker, 0x2c, 0x01, 0x02, 0x00, marker]);

    let frames = rk_mpp_rs::codec::vp9::parser::split_superframe(&packet).unwrap();
    assert_eq!(frames, vec![first.as_slice(), second.as_slice()]);
}

#[test]
fn show_existing_frame_reuses_the_selected_reference_geometry() {
    let mut parser = Parser::new();
    parser.parse(&keyframe_header(0, 0, 321, 241)).unwrap();

    let header = parser.parse(&show_existing_header(5)).unwrap();

    assert_eq!(header.frame_kind, FrameKind::ShowExisting);
    assert_eq!(header.frame_to_show_map_idx, Some(5));
    assert_eq!(
        (header.width, header.height, header.bit_depth),
        (321, 241, 8)
    );
    assert!(header.show_frame);
}

#[test]
fn show_existing_frame_without_a_reference_is_rejected() {
    let mut parser = Parser::new();

    assert!(matches!(
        parser.parse(&show_existing_header(0)),
        Err(Vp9Error::MissingReference(0))
    ));
}

#[test]
fn rejects_a_superframe_index_with_an_oversized_frame() {
    let marker = 0xc1;
    let packet = [0x82, 0x01, marker, 10, 1, marker];
    assert!(matches!(
        rk_mpp_rs::codec::vp9::parser::split_superframe(&packet),
        Err(Vp9Error::BadSuperframe)
    ));
}

#[test]
fn parses_a_profile_zero_keyframe() {
    let mut parser = Parser::new();
    let header = parser.parse(&keyframe_header(0, 0, 64, 64)).unwrap();

    assert_eq!(header.profile, Profile::Profile0);
    assert_eq!(header.frame_kind, FrameKind::Key);
    assert_eq!((header.width, header.height), (64, 64));
    assert_eq!(header.bit_depth, 8);
    assert!(header.show_frame);
    assert_eq!(header.refresh_frame_flags, 0xff);
    assert_eq!(header.first_partition_size, 2);
    assert_eq!((header.log2_tile_cols, header.log2_tile_rows), (0, 0));
}

#[test]
fn unupdated_segmentation_probabilities_keep_mpp_sentinel_values() {
    let mut parser = Parser::new();
    let header = parser
        .parse(&keyframe_header_with_unupdated_segmentation_probabilities())
        .unwrap();

    assert_eq!(header.segmentation.tree_probs, [0xff; 7]);
    assert_eq!(header.segmentation.pred_probs, [0xff; 3]);
}

#[test]
fn parses_a_profile_two_10_bit_keyframe() {
    let mut parser = Parser::new();
    let header = parser.parse(&keyframe_header(2, 2, 1920, 1080)).unwrap();

    assert_eq!(header.profile, Profile::Profile2);
    assert_eq!(header.bit_depth, 10);
    assert_eq!((header.width, header.height), (1920, 1080));
}

#[test]
fn parses_a_profile_two_ten_bit_intra_only_frame() {
    let mut parser = Parser::new();
    parser.parse(&keyframe_header(2, 2, 1920, 1080)).unwrap();
    let header = parser.parse(&profile_two_intra_only_header()).unwrap();

    assert_eq!(header.profile, Profile::Profile2);
    assert_eq!(header.frame_kind, FrameKind::IntraOnly);
    assert!(!header.show_frame);
    assert!(header.error_resilient_mode);
    assert_eq!(header.bit_depth, 10);
    assert_eq!((header.width, header.height), (1920, 1080));
    assert_eq!((header.render_width, header.render_height), (1280, 720));
}

#[test]
fn rejects_an_unsupported_profile() {
    let mut parser = Parser::new();
    let header = keyframe_header(1, 0, 64, 64);
    assert!(matches!(
        parser.parse(&header),
        Err(Vp9Error::UnsupportedProfile(1))
    ));
}

#[test]
fn inter_frame_inherits_the_last_reference_size() {
    let mut parser = Parser::new();
    parser.parse(&keyframe_header(0, 0, 64, 64)).unwrap();
    let header = parser.parse(&inter_header()).unwrap();

    assert_eq!(header.frame_kind, FrameKind::Inter);
    assert_eq!((header.width, header.height), (64, 64));
    assert_eq!(header.refresh_frame_flags, 0x02);
    assert_eq!(header.ref_frame_idx, [0, 1, 2]);
    assert_eq!(header.frame_context_idx, 0);
    assert!(header.frame_parallel_mode);
}

#[test]
fn parses_every_frame_in_a_real_profile_zero_ivf() {
    let bytes = include_bytes!("data/vp9_p0_64x64.ivf");
    let ivf = rk_mpp_rs::codec::vp9::parser::parse_ivf(bytes).unwrap();
    assert_eq!(ivf.width, 64);
    assert_eq!(ivf.height, 64);
    assert_eq!(ivf.frames.len(), 12);

    let mut parser = Parser::new();
    for (index, frame) in ivf.frames.iter().enumerate() {
        let payloads = rk_mpp_rs::codec::vp9::parser::split_superframe(frame.data).unwrap();
        assert_eq!(payloads.len(), 1, "frame {index}");
        let header = parser
            .parse(payloads[0])
            .unwrap_or_else(|error| panic!("frame {index}: {error}"));
        assert_eq!((header.width, header.height), (64, 64));
        assert_eq!(header.bit_depth, 8);
        if index == 0 || index == 6 {
            assert_eq!(header.frame_kind, FrameKind::Key);
        } else {
            assert_eq!(header.frame_kind, FrameKind::Inter);
        }
        if index == 1 {
            assert!(!header.error_resilient_mode);
            assert_eq!(header.refresh_frame_flags, 1);
            assert_eq!(header.ref_frame_idx, [0, 1, 2]);
            assert_eq!((header.render_width, header.render_height), (64, 64));
            assert!(header.allow_high_precision_mv);
            assert_eq!(header.interp_filter, 4);
            assert!(header.refresh_frame_context);
            assert!(header.frame_parallel_mode);
            assert_eq!(header.frame_context_idx, 0);
            assert_eq!(header.loop_filter.level, 8);
            assert!(header.loop_filter.mode_ref_delta_enabled);
            assert!(!header.loop_filter.mode_ref_delta_update);
            assert_eq!(header.quantization.base_q_idx, 128);
            assert!(!header.segmentation.enabled);
            assert_eq!((header.log2_tile_cols, header.log2_tile_rows), (0, 0));
            assert_eq!(header.first_partition_size, 7);
            assert_eq!(header.header_size, 10);
        }
    }
}

#[test]
fn parses_every_frame_in_a_real_profile_two_ten_bit_ivf() {
    let bytes = include_bytes!("data/vp9_p2_10bit_64x64.ivf");
    let ivf = rk_mpp_rs::codec::vp9::parser::parse_ivf(bytes).unwrap();
    assert_eq!(ivf.width, 64);
    assert_eq!(ivf.height, 64);
    assert_eq!(ivf.frames.len(), 12);

    let mut parser = Parser::new();
    for (index, frame) in ivf.frames.iter().enumerate() {
        let payloads = rk_mpp_rs::codec::vp9::parser::split_superframe(frame.data).unwrap();
        assert_eq!(payloads.len(), 1, "frame {index}");
        let header = parser
            .parse(payloads[0])
            .unwrap_or_else(|error| panic!("frame {index}: {error}"));
        assert_eq!(header.profile, Profile::Profile2);
        assert_eq!((header.width, header.height), (64, 64));
        assert_eq!(header.bit_depth, 10);
    }
}

#[test]
fn rejects_a_truncated_ivf_payload() {
    let mut bytes = include_bytes!("data/vp9_p0_64x64.ivf").to_vec();
    bytes.truncate(bytes.len() - 3);
    assert!(rk_mpp_rs::codec::vp9::parser::parse_ivf(&bytes).is_err());
}
