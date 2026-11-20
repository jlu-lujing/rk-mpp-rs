use rk_mpp_rs::codec::av1::assembler::{Av1PacketEvent, Av1PacketParser};
use rk_mpp_rs::codec::av1::cdf::Av1Cdfs;
use rk_mpp_rs::codec::av1::parser::{
    encode_obu, obu_in_operating_point, parse_frame_header, parse_frame_obu, parse_sequence_header,
    parse_tile_group_layout, parse_tile_group_obu, relative_distance, split_obus, FrameDecodeState,
    FrameHeader, FrameType, Obu, ObuHeader, ObuType, ReferenceFrame, TileGroup,
    TileGroupAccumulator, TxMode,
};

#[test]
fn parses_the_320x240_ivf_container_packet_boundaries() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();

    assert_eq!(ivf.width, 320);
    assert_eq!(ivf.height, 240);
    assert_eq!(ivf.frame_rate_numerator, 30);
    assert_eq!(ivf.frame_rate_denominator, 1);
    assert_eq!(ivf.frames.len(), 10);
    assert_eq!(ivf.frames[0].timestamp, 0);
    assert_eq!(ivf.frames[0].data.len(), 2277);
    assert_eq!(ivf.frames[1].data.len(), 5387);
    assert_eq!(ivf.frames[9].timestamp, 9);
    assert_eq!(ivf.frames[9].data.len(), 975);
}

#[test]
fn parses_film_grain_parameters_from_a_libaom_stream() {
    let data = std::fs::read("tests/data/av1_film_grain_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let mut parser = Av1PacketParser::new();
    let mut coded = Vec::new();

    for packet in &ivf.frames {
        for event in parser.parse_packet(packet.data).unwrap() {
            if let Av1PacketEvent::Coded(frame) = event {
                coded.push(frame);
            }
        }
    }

    assert_eq!(coded.len(), 11);
    assert!(parser.sequence().unwrap().film_grain_params_present);
    assert_eq!(
        coded
            .iter()
            .map(|frame| frame.header.film_grain.apply_grain)
            .collect::<Vec<_>>(),
        [true, false, true, true, true, true, true, true, true, true, true]
    );

    let grain = &coded[0].header.film_grain;
    assert_eq!(grain.grain_seed, 0xb0af);
    assert_eq!(grain.num_y_points, 14);
    assert_eq!(grain.num_cb_points, 8);
    assert_eq!(grain.num_cr_points, 9);
    assert_eq!(grain.ar_coeff_lag, 2);
    assert!(!grain.chroma_scaling_from_luma);
}

#[test]
fn splits_av1_obus_without_losing_their_payload_boundaries() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();

    let first = split_obus(ivf.frames[0].data).unwrap();
    assert_eq!(
        first.iter().map(|obu| obu.obu_type).collect::<Vec<_>>(),
        [
            ObuType::TemporalDelimiter,
            ObuType::SequenceHeader,
            ObuType::Frame
        ]
    );
    assert_eq!(first[1].payload.len(), 11);
    assert_eq!(first[2].payload.len(), 2259);

    let packets = ivf
        .frames
        .iter()
        .map(|frame| split_obus(frame.data).unwrap().len())
        .collect::<Vec<_>>();
    assert_eq!(packets, [3, 5, 2, 2, 2, 3, 2, 2, 2, 2]);
}

#[test]
fn rejects_a_truncated_obu_instead_of_reading_past_the_packet() {
    let err = split_obus(&[0x32, 0x05, 0, 0]).unwrap_err();
    assert!(err.to_string().contains("exceeds packet"));
}

#[test]
fn parses_the_main_profile_8_bit_sequence_header() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let obus = split_obus(ivf.frames[0].data).unwrap();
    let sequence = parse_sequence_header(obus[1].payload).unwrap();

    assert_eq!(sequence.profile, 0);
    assert!(!sequence.still_picture);
    assert!(!sequence.reduced_still_picture_header);
    assert_eq!(sequence.operating_points.len(), 1);
    assert_eq!(sequence.operating_points[0].idc, 0);
    assert_eq!(sequence.operating_points[0].level_idx, 0);
    assert_eq!(sequence.frame_width_bits, 9);
    assert_eq!(sequence.frame_height_bits, 8);
    assert_eq!(sequence.max_frame_width, 320);
    assert_eq!(sequence.max_frame_height, 240);
    assert_eq!(sequence.bit_depth, 8);
    assert!(sequence.enable_filter_intra);
    assert!(sequence.enable_intra_edge_filter);
    assert!(sequence.enable_masked_compound);
    assert!(sequence.enable_warped_motion);
    assert!(sequence.enable_order_hint);
    assert!(sequence.enable_ref_frame_mvs);
    assert_eq!(sequence.order_hint_bits, 7);
    assert!(sequence.enable_cdef);
    assert!(!sequence.enable_restoration);
    assert!(!sequence.color_config.mono_chrome);
    assert!(sequence.color_config.subsampling_x);
    assert!(sequence.color_config.subsampling_y);
    assert!(!sequence.film_grain_params_present);
}

#[test]
fn rejects_a_truncated_sequence_header() {
    let err = parse_sequence_header(&[0x00]).unwrap_err();
    assert!(err.to_string().contains("truncated"));
}

#[test]
fn parses_the_first_key_frame_coding_and_filter_parameters() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let obus = split_obus(ivf.frames[0].data).unwrap();
    let sequence = parse_sequence_header(obus[1].payload).unwrap();
    let mut state = FrameDecodeState::new();

    let header = parse_frame_header(obus[2].payload, &sequence, &mut state).unwrap();

    assert_eq!(header.frame_type, FrameType::Key);
    assert!(header.show_frame);
    assert!(header.error_resilient_mode);
    assert!(!header.show_existing_frame);
    assert!(!header.disable_cdf_update);
    assert!(header.allow_screen_content_tools);
    assert!(!header.force_integer_mv);
    assert!(!header.frame_size_override_flag);
    assert_eq!(header.order_hint, 0);
    assert_eq!(header.primary_ref_frame, 7);
    assert_eq!(header.refresh_frame_flags, 0xff);
    assert_eq!(header.width, 320);
    assert_eq!(header.height, 240);
    assert_eq!(header.upscaled_width, 320);
    assert_eq!(header.render_width, 320);
    assert_eq!(header.render_height, 240);
    assert!(!header.allow_intrabc);
    assert!(!header.disable_frame_end_update_cdf);
    assert_eq!((header.tile_cols, header.tile_rows), (1, 1));
    assert_eq!(header.base_q_idx, 96);
    assert!(header.delta_q_present);
    assert_eq!(header.delta_q_res, 2);
    assert_eq!(header.loop_filter_level, [2, 2, 8, 12]);
    assert!(header.loop_filter_delta_enabled);
    assert!(!header.loop_filter_delta_update);
    assert_eq!(header.cdef_damping_minus_3, 1);
    assert_eq!(header.cdef_bits, 2);
    assert_eq!(header.tx_mode, TxMode::Select);
    assert!(!header.reduced_tx_set);
    assert_eq!(state.references.iter().filter(|r| r.valid).count(), 8);
}

#[test]
fn tracks_the_key_frame_in_every_reference_slot() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let obus = split_obus(ivf.frames[0].data).unwrap();
    let sequence = parse_sequence_header(obus[1].payload).unwrap();
    let mut state = FrameDecodeState::new();

    parse_frame_header(obus[2].payload, &sequence, &mut state).unwrap();

    for reference in &state.references {
        assert!(reference.valid);
        assert_eq!(reference.frame_type, FrameType::Key);
        assert_eq!(reference.frame_width, 320);
        assert_eq!(reference.frame_height, 240);
        assert_eq!(reference.bit_depth, 8);
        assert_eq!(reference.order_hint, 0);
    }
}

#[test]
fn relative_order_hint_distance_preserves_the_low_hint_bits() {
    assert_eq!(relative_distance(true, 7, 70, 2), -60);
    assert_eq!(relative_distance(true, 7, 2, 70), 60);
    assert_eq!(relative_distance(true, 7, 0, 1), -1);
    assert_eq!(relative_distance(true, 7, 1, 0), 1);
    assert_eq!(relative_distance(false, 0, 70, 2), 0);
}

#[test]
fn parses_every_frame_and_preserves_reference_slot_changes() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let mut sequence = None;
    let mut state = FrameDecodeState::new();
    let mut headers = Vec::new();

    for packet in &ivf.frames {
        for obu in split_obus(packet.data).unwrap() {
            match obu.obu_type {
                ObuType::SequenceHeader => {
                    sequence = Some(parse_sequence_header(obu.payload).unwrap());
                }
                ObuType::Frame | ObuType::FrameHeader => {
                    headers.push(
                        parse_frame_header(obu.payload, sequence.as_ref().unwrap(), &mut state)
                            .unwrap(),
                    );
                }
                _ => {}
            }
        }
    }

    assert_eq!(headers.len(), 14);
    assert_eq!(
        headers
            .iter()
            .filter(|header| !header.show_existing_frame)
            .map(|header| header.order_hint)
            .collect::<Vec<_>>(),
        [0, 8, 4, 2, 1, 3, 6, 5, 7, 8, 9]
    );
    assert_eq!(
        headers
            .iter()
            .filter(|header| header.show_existing_frame)
            .map(|header| header.frame_to_show_map_idx)
            .collect::<Vec<_>>(),
        [3, 2, 6]
    );
    assert_eq!(
        headers
            .iter()
            .filter(|header| !header.show_existing_frame)
            .map(|header| header.refresh_frame_flags)
            .collect::<Vec<_>>(),
        [0xff, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x10, 0, 0x08]
    );
}

#[test]
fn consumes_the_same_uncompressed_header_bits_as_c_mpp() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let mut sequence = None;
    let mut state = FrameDecodeState::new();
    let mut header_bits = Vec::new();

    for packet in &ivf.frames {
        for obu in split_obus(packet.data).unwrap() {
            match obu.obu_type {
                ObuType::SequenceHeader => {
                    sequence = Some(parse_sequence_header(obu.payload).unwrap());
                }
                ObuType::Frame | ObuType::FrameHeader => {
                    let header =
                        parse_frame_header(obu.payload, sequence.as_ref().unwrap(), &mut state)
                            .unwrap();
                    if !header.show_existing_frame {
                        let obu_header_bits = 8 * (obu.raw.len() - obu.payload.len());
                        header_bits.push(obu_header_bits + header.header_bits);
                    }
                }
                _ => {}
            }
        }
    }

    // Captured from the official MPP on RK3588 with av1d_debug=2. These are the
    // "film_grain out" bit counts for each non-show-existing frame header.
    assert_eq!(
        header_bits,
        [145, 170, 171, 159, 155, 155, 159, 155, 155, 146, 166]
    );
}

#[test]
fn locates_frame_obu_tile_data_with_the_same_offsets_as_c_mpp() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let mut sequence = None;
    let mut state = FrameDecodeState::new();
    let mut tag_sizes = Vec::new();
    let mut tile_lengths = Vec::new();

    for packet in &ivf.frames {
        for obu in split_obus(packet.data).unwrap() {
            match obu.obu_type {
                ObuType::SequenceHeader => {
                    sequence = Some(parse_sequence_header(obu.payload).unwrap());
                }
                ObuType::Frame => {
                    let frame =
                        parse_frame_obu(&obu, sequence.as_ref().unwrap(), &mut state).unwrap();
                    assert_eq!(frame.tile_group.tg_start, 0);
                    assert_eq!(frame.tile_group.tg_end, 0);
                    tag_sizes.push(frame.tag_size);
                    tile_lengths.push(frame.tile_data.len());
                }
                ObuType::FrameHeader => {
                    parse_frame_header(obu.payload, sequence.as_ref().unwrap(), &mut state)
                        .unwrap();
                }
                _ => {}
            }
        }
    }

    // C MPP jobs report 34 for the first packet, 24/22/20/20 for the four
    // OBUs in packet 1, and so on. Strip the two-byte temporal delimiter from
    // packet 1 (the following OBUs are separate hardware fragments).
    assert_eq!(tag_sizes, [19, 22, 22, 20, 20, 20, 20, 20, 20, 19, 21]);
    assert_eq!(
        tile_lengths,
        [2243, 2037, 1402, 961, 901, 748, 884, 829, 774, 5, 952]
    );
}

#[test]
fn parses_a_standalone_tile_group_from_the_reference_payload() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let obus = split_obus(ivf.frames[0].data).unwrap();
    let sequence = parse_sequence_header(obus[1].payload).unwrap();
    let mut state = FrameDecodeState::new();
    let frame = parse_frame_obu(&obus[2], &sequence, &mut state).unwrap();
    let tile_payload_offset = frame.tag_size - (obus[2].raw.len() - obus[2].payload.len());
    let tile_payload = &obus[2].payload[tile_payload_offset..];
    let mut header = obus[2].header;
    header.obu_type = ObuType::TileGroup;
    let tile_obu = rk_mpp_rs::codec::av1::parser::Obu {
        header,
        obu_type: ObuType::TileGroup,
        temporal_id: 0,
        spatial_id: 0,
        start: 0,
        payload: tile_payload,
        raw: tile_payload,
    };

    let tile_group = parse_tile_group_obu(&tile_obu, &frame.header).unwrap();

    assert_eq!(tile_group.tile_group.tg_start, 0);
    assert_eq!(tile_group.tile_group.tg_end, 0);
    assert_eq!(tile_group.tag_size, 0);
    assert_eq!(tile_group.tile_data, tile_payload);
    assert_eq!(tile_group.tile_data.len(), 2243);
}

fn insert_bits(data: &[u8], bit_position: usize, value: u32, width: u8) -> Vec<u8> {
    let mut bits = Vec::with_capacity(data.len() * 8 + width as usize);
    let put = |bits: &mut Vec<u8>, value: u32, width: u8| {
        for shift in (0..width).rev() {
            bits.push(((value >> shift) & 1) as u8);
        }
    };

    for bit in 0..bit_position {
        bits.push((data[bit / 8] >> (7 - bit % 8)) & 1);
    }
    put(&mut bits, value, width);
    for bit in bit_position..data.len() * 8 {
        bits.push((data[bit / 8] >> (7 - bit % 8)) & 1);
    }

    bits.chunks(8)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .fold(0u8, |byte, (index, bit)| byte | (bit << (7 - index)))
        })
        .collect()
}

#[test]
fn reads_the_frame_presentation_time_at_the_c_mpp_bit_position() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let obus = split_obus(ivf.frames[0].data).unwrap();
    let mut sequence = parse_sequence_header(obus[1].payload).unwrap();
    sequence.decoder_model_info_present = true;
    sequence
        .decoder_model_info
        .frame_presentation_time_length_minus_1 = 3;
    sequence.timing_info.equal_picture_interval = false;
    let payload = insert_bits(obus[2].payload, 4, 0b1010, 4);
    let payload = insert_bits(&payload, 19, 0, 1);
    let mut state = FrameDecodeState::new();

    let header = parse_frame_header(&payload, &sequence, &mut state).unwrap();

    assert_eq!(header.frame_presentation_time, 0b1010);
    assert!(!header.buffer_removal_time_present_flag);
    assert_eq!(header.header_bits, 126);
}

#[test]
fn reads_decoder_model_buffer_removal_times_like_c_mpp() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let obus = split_obus(ivf.frames[0].data).unwrap();
    let mut sequence = parse_sequence_header(obus[1].payload).unwrap();
    sequence.decoder_model_info_present = true;
    sequence
        .decoder_model_info
        .buffer_removal_time_length_minus_1 = 2;
    sequence.timing_info.equal_picture_interval = true;
    sequence.operating_points[0].decoder_model_present = true;
    sequence.operating_points[0].idc = 0;
    let insert = (1 << 3) | 0b101;
    let payload = insert_bits(obus[2].payload, 15, insert, 4);
    let mut state = FrameDecodeState::new();

    let header = parse_frame_header(&payload, &sequence, &mut state).unwrap();

    assert!(header.buffer_removal_time_present_flag);
    assert_eq!(header.buffer_removal_time[0], 0b101);
    assert_eq!(header.header_bits, 125);
}

#[test]
fn reads_buffer_removal_time_only_for_operating_points_in_the_frame_layers() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let obus = split_obus(ivf.frames[0].data).unwrap();
    let mut sequence = parse_sequence_header(obus[1].payload).unwrap();
    sequence.decoder_model_info_present = true;
    sequence
        .decoder_model_info
        .buffer_removal_time_length_minus_1 = 2;
    sequence.timing_info.equal_picture_interval = true;
    let mut selected = sequence.operating_points[0];
    selected.decoder_model_present = true;
    selected.idc = 0x101;
    let mut filtered = selected;
    filtered.idc = 0x201;
    sequence.operating_points = vec![filtered, selected];
    let insert = (1 << 3) | 0b010;
    let payload = insert_bits(obus[2].payload, 15, insert, 4);
    let mut state = FrameDecodeState::new();

    let header = parse_frame_header(&payload, &sequence, &mut state).unwrap();

    assert_eq!(header.buffer_removal_time[0], 0);
    assert_eq!(header.buffer_removal_time[1], 0b010);
    assert_eq!(header.header_bits, 125);
}

#[test]
fn show_existing_key_frame_reads_display_frame_id_and_restores_reference_state() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let obus = split_obus(ivf.frames[0].data).unwrap();
    let mut sequence = parse_sequence_header(obus[1].payload).unwrap();
    sequence.frame_id_numbers_present = true;
    sequence.delta_frame_id_length_minus_2 = 1;
    sequence.additional_frame_id_length_minus_1 = 1;
    sequence.timing_info.equal_picture_interval = true;
    let mut state = FrameDecodeState::new();
    state.references[0] = ReferenceFrame {
        valid: true,
        frame_id: 0b101,
        upscaled_width: 320,
        frame_width: 320,
        frame_height: 240,
        render_width: 320,
        render_height: 240,
        frame_type: FrameType::Key,
        bit_depth: 8,
        order_hint: 7,
        ..ReferenceFrame::default()
    };
    let payload = insert_bits(&[], 0, 1, 1);
    let payload = insert_bits(&payload, 1, 0, 3);
    let payload = insert_bits(&payload, 4, 0b11001, 5);

    let header = parse_frame_header(&payload, &sequence, &mut state).unwrap();

    assert_eq!(header.display_frame_id, 0b11001);
    assert_eq!(header.current_frame_id, 0b101);
    assert_eq!(header.refresh_frame_flags, 0xff);
    assert_eq!(state.order_hint, 7);
    assert_eq!(header.header_bits, 9);
}

fn frame_id_position(
    sequence: &rk_mpp_rs::codec::av1::parser::SequenceHeader,
    header: &rk_mpp_rs::codec::av1::parser::FrameHeader,
) -> usize {
    let mut position = 1 + 2 + 1;
    if header.show_frame
        && sequence.decoder_model_info_present
        && !sequence.timing_info.equal_picture_interval
    {
        position += usize::from(
            sequence
                .decoder_model_info
                .frame_presentation_time_length_minus_1
                + 1,
        );
    }
    if !header.show_frame {
        position += 1;
    }
    if header.frame_type != FrameType::Switch
        && !(header.frame_type == FrameType::Key && header.show_frame)
    {
        position += 1;
    }
    position += 1;
    if sequence.seq_force_screen_content_tools == 2 {
        position += 1;
    }
    if header.allow_screen_content_tools && sequence.seq_force_integer_mv == 2 {
        position += 1;
    }
    position
}

#[test]
fn inter_frame_reads_current_and_delta_frame_ids_at_the_c_mpp_positions() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let mut sequence = None;
    let mut state = FrameDecodeState::new();
    let mut found = None;

    'packets: for packet in &ivf.frames {
        for obu in split_obus(packet.data).unwrap() {
            match obu.obu_type {
                ObuType::SequenceHeader => {
                    sequence = Some(parse_sequence_header(obu.payload).unwrap());
                }
                ObuType::Frame | ObuType::FrameHeader => {
                    let before = state.clone();
                    let header =
                        parse_frame_header(obu.payload, sequence.as_ref().unwrap(), &mut state)
                            .unwrap();
                    if !header.show_existing_frame && header.frame_type == FrameType::Inter {
                        found = Some((
                            sequence.clone().unwrap(),
                            header,
                            before,
                            obu.payload.to_vec(),
                        ));
                        break 'packets;
                    }
                }
                _ => {}
            }
        }
    }

    let (mut sequence, original, mut state, payload) = found.unwrap();
    sequence.frame_id_numbers_present = true;
    sequence.delta_frame_id_length_minus_2 = 1;
    sequence.additional_frame_id_length_minus_1 = 1;
    assert!(!sequence.decoder_model_info_present);
    for reference in &mut state.references {
        reference.valid = true;
        reference.frame_id = 20;
    }

    let current_frame_id = 20;
    let id_len = 5;
    let delta_width = 3;
    let mut position = frame_id_position(&sequence, &original);
    let mut payload = insert_bits(&payload, position, current_frame_id, id_len);
    position += usize::from(id_len);

    if original.frame_type != FrameType::Switch && !sequence.reduced_still_picture_header {
        position += 1;
    }
    if sequence.enable_order_hint {
        position += usize::from(sequence.order_hint_bits);
    }
    let frame_is_intra = matches!(original.frame_type, FrameType::Key | FrameType::IntraOnly);
    if !(frame_is_intra || original.error_resilient_mode) {
        position += 3;
    }
    if original.frame_type != FrameType::Switch
        && !(original.frame_type == FrameType::Key && original.show_frame)
    {
        position += 8;
    }
    if sequence.enable_order_hint
        && (!frame_is_intra || original.refresh_frame_flags != 0xff)
        && original.error_resilient_mode
    {
        position += 8 * usize::from(sequence.order_hint_bits);
    }
    if sequence.enable_order_hint {
        position += 1;
        if original.frame_refs_short_signaling {
            position += 6;
        }
    }

    let delta_ids = [1, 2, 3, 4, 5, 6, 7];
    for delta_id in delta_ids {
        if !original.frame_refs_short_signaling {
            position += 3;
        }
        payload = insert_bits(&payload, position, delta_id, delta_width);
        position += usize::from(delta_width);
    }

    let header = parse_frame_header(&payload, &sequence, &mut state).unwrap();

    assert_eq!(header.current_frame_id, current_frame_id);
    assert_eq!(header.delta_frame_id_minus1, delta_ids);
}

#[test]
fn parses_each_tile_range_using_mpp_size_byte_rules() {
    let header = FrameHeader {
        tile_cols: 3,
        tile_rows: 1,
        tile_size_bytes_minus_1: 0,
        ..FrameHeader::default()
    };
    let group = TileGroup {
        tile_start_and_end_present_flag: true,
        tg_start: 0,
        tg_end: 2,
    };
    let payload = [2, 0xaa, 0xbb, 0xcc, 1, 0xdd, 0xee, 0xff, 0x11];

    let layout = parse_tile_group_layout(&payload, &header, group, 0).unwrap();

    assert_eq!(layout[0], Some(1..4));
    assert_eq!(layout[1], Some(5..7));
    assert_eq!(layout[2], Some(7..9));
}

#[test]
fn inserts_the_last_tile_group_at_its_absolute_packet_offset() {
    let header = FrameHeader {
        tile_cols: 2,
        tile_rows: 1,
        tile_size_bytes_minus_1: 0,
        ..FrameHeader::default()
    };
    let group = TileGroup {
        tile_start_and_end_present_flag: true,
        tg_start: 1,
        tg_end: 1,
    };
    let payload = [0xaa, 0xbb, 0xcc];

    let layout = parse_tile_group_layout(&payload, &header, group, 100).unwrap();

    assert_eq!(layout[0], None);
    assert_eq!(layout[1], Some(100..103));
}

#[test]
fn accumulates_tile_ranges_across_multiple_tile_groups() {
    let header = FrameHeader {
        tile_cols: 2,
        tile_rows: 2,
        tile_size_bytes_minus_1: 0,
        ..FrameHeader::default()
    };
    let first = TileGroup {
        tile_start_and_end_present_flag: true,
        tg_start: 0,
        tg_end: 1,
    };
    let second = TileGroup {
        tile_start_and_end_present_flag: true,
        tg_start: 2,
        tg_end: 3,
    };
    let mut accumulator = TileGroupAccumulator::new();

    accumulator
        .insert_group(
            &[2, 0xaa, 0xbb, 0xcc, 1, 0xdd, 0xee, 0xff, 0x11],
            &header,
            first,
            10,
        )
        .unwrap();
    accumulator
        .insert_group(&[0, 0x12, 0x34, 1, 0x56, 0x78, 0x9a], &header, second, 100)
        .unwrap();
    let layout = accumulator.finish(&header).unwrap();

    assert_eq!(layout[0], Some(11..14));
    assert_eq!(layout[1], Some(14..19));
    assert_eq!(layout[2], Some(101..102));
    assert_eq!(layout[3], Some(102..107));
}

#[test]
fn tile_accumulator_rejects_overlapping_or_incomplete_groups() {
    let header = FrameHeader {
        tile_cols: 2,
        tile_rows: 1,
        tile_size_bytes_minus_1: 0,
        ..FrameHeader::default()
    };
    let group = TileGroup {
        tile_start_and_end_present_flag: true,
        tg_start: 0,
        tg_end: 0,
    };
    let mut accumulator = TileGroupAccumulator::new();
    accumulator
        .insert_group(&[0, 0xaa], &header, group, 0)
        .unwrap();

    assert!(accumulator
        .insert_group(&[0, 0xbb], &header, group, 2)
        .is_err());
    assert!(accumulator.finish(&header).is_err());
}

fn test_obu(obu_type: ObuType, temporal_id: u8, spatial_id: u8) -> Obu<'static> {
    let header = ObuHeader {
        obu_type,
        extension_flag: temporal_id != 0 || spatial_id != 0,
        has_size_field: true,
        temporal_id,
        spatial_id,
    };
    Obu {
        header,
        obu_type,
        temporal_id,
        spatial_id,
        start: 0,
        payload: &[],
        raw: &[],
    }
}

#[test]
fn operating_point_filter_drops_obus_outside_the_selected_temporal_and_spatial_layer() {
    let operating_point_idc = (1 << 2) | (1 << (1 + 8));
    let inside = test_obu(ObuType::TileGroup, 2, 1);
    let outside_temporal = test_obu(ObuType::TileGroup, 1, 1);
    let outside_spatial = test_obu(ObuType::TileGroup, 2, 0);
    let no_extension = test_obu(ObuType::TileGroup, 0, 0);
    let sequence = test_obu(ObuType::SequenceHeader, 1, 0);

    assert!(obu_in_operating_point(&inside, operating_point_idc));
    assert!(!obu_in_operating_point(
        &outside_temporal,
        operating_point_idc
    ));
    assert!(!obu_in_operating_point(
        &outside_spatial,
        operating_point_idc
    ));
    assert!(obu_in_operating_point(&no_extension, operating_point_idc));
    assert!(obu_in_operating_point(&sequence, operating_point_idc));
    assert!(obu_in_operating_point(&outside_temporal, 0));
}

#[test]
fn multi_tile_stream_matches_c_mpp_header_bits_and_geometry() {
    let data = std::fs::read("tests/data/av1_tiles_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let mut sequence = None;
    let mut state = FrameDecodeState::new();
    let mut headers = Vec::new();

    for packet in &ivf.frames {
        for obu in split_obus(packet.data).unwrap() {
            match obu.obu_type {
                ObuType::SequenceHeader => {
                    sequence = Some(parse_sequence_header(obu.payload).unwrap());
                }
                ObuType::Frame => {
                    let frame =
                        parse_frame_obu(&obu, sequence.as_ref().unwrap(), &mut state).unwrap();
                    if !frame.header.show_existing_frame {
                        let obu_header_bits = 8 * (obu.raw.len() - obu.payload.len());
                        headers.push((frame.header, obu_header_bits));
                    }
                }
                ObuType::FrameHeader => {
                    let header =
                        parse_frame_header(obu.payload, sequence.as_ref().unwrap(), &mut state)
                            .unwrap();
                    if !header.show_existing_frame {
                        let obu_header_bits = 8 * (obu.raw.len() - obu.payload.len());
                        headers.push((header, obu_header_bits));
                    }
                }
                _ => {}
            }
        }
    }

    assert_eq!(headers.len(), 11);
    assert!(headers
        .iter()
        .all(|(header, _)| header.tile_cols == 2 && header.tile_rows == 2));
    let mut bits_by_order_hint = headers
        .iter()
        .map(|(header, obu_header_bits)| (header.order_hint, obu_header_bits + header.header_bits))
        .collect::<Vec<_>>();
    bits_by_order_hint.sort_unstable();
    assert_eq!(
        bits_by_order_hint,
        [
            (0, 127),
            (1, 161),
            (2, 165),
            (3, 161),
            (4, 177),
            (5, 161),
            (6, 165),
            (7, 161),
            (8, 152),
            (8, 176),
            (9, 172),
        ]
    );
}

#[test]
fn multi_tile_stream_matches_c_mpp_tile_byte_ranges() {
    let data = std::fs::read("tests/data/av1_tiles_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let mut sequence = None;
    let mut state = FrameDecodeState::new();
    let mut ranges = Vec::new();

    for packet in &ivf.frames {
        for obu in split_obus(packet.data).unwrap() {
            match obu.obu_type {
                ObuType::SequenceHeader => {
                    sequence = Some(parse_sequence_header(obu.payload).unwrap());
                }
                ObuType::Frame => {
                    let frame =
                        parse_frame_obu(&obu, sequence.as_ref().unwrap(), &mut state).unwrap();
                    let layout = parse_tile_group_layout(
                        frame.tile_data,
                        &frame.header,
                        frame.tile_group,
                        0,
                    )
                    .unwrap();
                    ranges.push((
                        frame.header.order_hint,
                        layout[..4]
                            .iter()
                            .map(|range| range.clone().unwrap())
                            .collect::<Vec<_>>(),
                    ));
                }
                _ => {}
            }
        }
    }

    // Captured from get_tiles_info() in the official MPP on RK3588. These are
    // offsets from the first tile byte; the last tile consumes the remainder.
    assert_eq!(
        ranges,
        [
            (0, vec![2..810, 812..1140, 1142..1237, 1237..2319]),
            (8, vec![2..715, 717..1035, 1037..1159, 1159..2117]),
            (4, vec![2..381, 383..538, 540..565, 565..1422]),
            (2, vec![2..153, 155..241, 243..256, 256..976]),
            (1, vec![2..137, 139..169, 171..182, 182..899]),
            (3, vec![2..85, 87..120, 122..128, 128..788]),
            (6, vec![2..160, 162..217, 219..264, 264..972]),
            (5, vec![2..127, 129..181, 183..208, 208..847]),
            (7, vec![2..91, 93..116, 118..155, 155..806]),
            (8, vec![1..6, 7..11, 12..17, 17..21]),
            (9, vec![2..132, 134..180, 182..199, 199..959]),
        ]
    );
}

#[test]
fn packet_assembler_separates_coded_frames_from_show_existing_frames() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let mut parser = Av1PacketParser::new();
    let mut coded = 0;
    let mut shown = 0;

    for packet in &ivf.frames {
        for event in parser.parse_packet(packet.data).unwrap() {
            match event {
                Av1PacketEvent::Coded(_) => coded += 1,
                Av1PacketEvent::ShowExisting(_) => shown += 1,
            }
        }
    }

    assert_eq!((coded, shown), (11, 3));
}

#[test]
fn packet_assembler_exposes_each_c_mpp_split_frame_length() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let mut parser = Av1PacketParser::new();
    let mut lengths = Vec::new();

    for packet in &ivf.frames {
        for event in parser.parse_packet(packet.data).unwrap() {
            if let Av1PacketEvent::Coded(frame) = event {
                lengths.push((frame.frame_start, frame.stream_len));
                if lengths.len() == 2 {
                    assert_eq!(lengths, [(0, 2_277), (0, 2_061)]);
                    return;
                }
            }
        }
    }
    panic!("fixture did not contain two coded frames");
}

#[test]
fn packet_assembler_attaches_the_cdf_selected_by_each_frame_header() {
    let data = std::fs::read("tests/data/av1_tiles_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let mut parser = Av1PacketParser::new();
    let mut coded = 0;

    for packet in &ivf.frames {
        for event in parser.parse_packet(packet.data).unwrap() {
            let Av1PacketEvent::Coded(frame) = event else {
                continue;
            };
            coded += 1;
            let resets = frame.header.error_resilient_mode
                || matches!(
                    frame.header.frame_type,
                    FrameType::Key | FrameType::IntraOnly
                )
                || frame.header.primary_ref_frame == 7;
            if resets {
                let expected = Av1Cdfs::default_for_q_index(u32::from(frame.header.base_q_idx));
                assert_eq!(
                    frame.cdf.regular.as_bytes(),
                    expected.as_bytes(),
                    "frame order_hint={} q={} primary={}",
                    frame.header.order_hint,
                    frame.header.base_q_idx,
                    frame.header.primary_ref_frame
                );
            }
        }
    }

    assert_eq!(coded, 11);
}

#[test]
fn packet_assembler_rejects_a_repeated_non_redundant_frame_header() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let mut parser = Av1PacketParser::new();
    parser.parse_packet(ivf.frames[0].data).unwrap();
    let frame_obu = split_obus(ivf.frames[1].data)
        .unwrap()
        .into_iter()
        .find(|obu| obu.obu_type == ObuType::Frame)
        .unwrap();
    let header = encode_obu(ObuType::FrameHeader, frame_obu.payload);
    let repeated = [header.as_slice(), header.as_slice()].concat();

    let error = parser.parse_packet(&repeated).unwrap_err();

    assert!(error.to_string().contains("repeated frame header"));
}

#[test]
fn packet_assembler_accepts_an_identical_redundant_header_and_its_tile_group() {
    let data = std::fs::read("tests/data/av1_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let sequence =
        parse_sequence_header(split_obus(ivf.frames[0].data).unwrap()[1].payload).unwrap();
    let mut state = FrameDecodeState::new();
    let mut parser = Av1PacketParser::new();
    parser.parse_packet(ivf.frames[0].data).unwrap();

    for obu in split_obus(ivf.frames[1].data).unwrap() {
        match obu.obu_type {
            ObuType::Frame => {
                let frame = parse_frame_obu(&obu, &sequence, &mut state).unwrap();
                let header_bytes = (frame.header.header_bits + 7) / 8;
                let tile_group_payload = &obu.payload[header_bytes..];
                let packet = [
                    encode_obu(ObuType::FrameHeader, obu.payload),
                    encode_obu(ObuType::RedundantFrameHeader, obu.payload),
                    encode_obu(ObuType::TileGroup, tile_group_payload),
                ]
                .concat();

                let events = parser.parse_packet(&packet).unwrap();

                assert_eq!(events.len(), 1);
                assert!(matches!(events[0], Av1PacketEvent::Coded(_)));
                return;
            }
            ObuType::FrameHeader => {
                let frame = parse_frame_header(obu.payload, &sequence, &mut state).unwrap();
                if !frame.show_existing_frame {
                    panic!("expected OBU_FRAME, found standalone header");
                }
            }
            _ => {}
        }
    }
    panic!("fixture did not contain a coded frame");
}

#[test]
fn packet_assembler_combines_tile_groups_from_separate_obus() {
    let data = std::fs::read("tests/data/av1_tiles_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let sequence =
        parse_sequence_header(split_obus(ivf.frames[0].data).unwrap()[1].payload).unwrap();
    let mut parser = Av1PacketParser::new();
    parser.parse_packet(ivf.frames[0].data).unwrap();
    let frame_obu = split_obus(ivf.frames[1].data)
        .unwrap()
        .into_iter()
        .find(|obu| obu.obu_type == ObuType::Frame)
        .unwrap();
    let mut state = parser.frame_state().clone();
    let frame = parse_frame_obu(&frame_obu, &sequence, &mut state).unwrap();
    let header_bytes = (frame.header.header_bits + 7) / 8;
    let tile_data = frame.tile_data;
    let split = 1035;
    let group_zero = [&[0x88][..], &tile_data[..split]].concat();
    let group_one = [&[0xd8][..], &tile_data[split..]].concat();
    let header_obu = encode_obu(ObuType::FrameHeader, frame_obu.payload);
    let group_zero_obu = encode_obu(ObuType::TileGroup, &group_zero);
    let group_one_obu = encode_obu(ObuType::TileGroup, &group_one);
    let packet = [
        header_obu.as_slice(),
        group_zero_obu.as_slice(),
        group_one_obu.as_slice(),
    ]
    .concat();

    let events = parser.parse_packet(&packet).unwrap();
    let Av1PacketEvent::Coded(frame) = &events[0] else {
        panic!("expected a coded frame");
    };
    let base_zero = header_obu.len() + group_zero_obu.len() - tile_data[..split].len();
    let base_one =
        header_obu.len() + group_zero_obu.len() + group_one_obu.len() - tile_data[split..].len();

    assert_eq!(events.len(), 1);
    assert_eq!(frame.tile_ranges[0], Some(base_zero + 2..base_zero + 715));
    assert_eq!(
        frame.tile_ranges[1],
        Some(base_zero + 715..base_zero + 1035)
    );
    assert_eq!(frame.tile_ranges[2], Some(base_one + 2..base_one + 124));
    assert_eq!(frame.tile_ranges[3], Some(base_one + 124..base_one + 1082));
    assert_eq!(frame.header.header_bits, 152);
    assert_eq!(header_bytes, 19);
}
