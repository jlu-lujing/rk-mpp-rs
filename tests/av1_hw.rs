use rk_mpp_rs::codec::av1::cdf::{Av1Cdf, Av1Cdfs, Av1MvCdfs};
use rk_mpp_rs::codec::av1::parser::FrameType;
use rk_mpp_rs::codec::av1::picparams::{Av1PicParams, PicReference};
use rk_mpp_rs::hal::av1d::{
    build_global_model, build_probability_buffer, build_tile_info, filter_layout, frame_layout,
    Av1DmaBuffers, Av1RegisterState, Av1Registers, FilterLayout, FrameLayout, RegOffset,
    AV1_REGISTER_COUNT,
};

#[test]
fn av1_frame_layout_matches_the_c_mpp_buffer_formula() {
    assert_eq!(
        frame_layout(320, 240, 10),
        FrameLayout {
            width: 400,
            height: 240,
            luma_size: 96_000,
            chroma_size: 48_000,
            direction_motion_size: 23_040,
            tile_output_size: 167_552,
        }
    );
}

#[test]
fn av1_filter_layout_uses_the_c_mpp_tile_column_multiplication() {
    assert_eq!(
        filter_layout(240, 1),
        FilterLayout {
            db_data_col: 3_840,
            db_ctrl_col: 2_048,
            cdef_col: 3_584,
            superres_col: 17_280,
            lr_col: 9_600,
            offsets: [0, 7_680, 11_776, 18_944, 53_504],
            allocation_size: 73_728,
        }
    );
}

#[test]
fn av1_global_model_uses_the_hardware_matrix_and_shear_order() {
    let mut params = Av1PicParams::default();
    params.frame_refs[0] = PicReference {
        wmmat: [10, 20, 30, 40, 50, 60],
        alpha: 0x1234,
        beta: 0x2345,
        gamma: 0x3456,
        delta: 0x4567,
        ..PicReference::default()
    };
    params.frame_refs[6] = PicReference {
        wmmat: [-10, -20, -30, -40, -50, -60],
        alpha: (-1i16) as u16,
        beta: (-2i16) as u16,
        gamma: (-3i16) as u16,
        delta: (-4i16) as u16,
        ..PicReference::default()
    };

    let model = build_global_model(&params);
    assert_eq!(model.len(), 2_048);
    let words: Vec<i32> = model[..28]
        .chunks_exact(4)
        .map(|word| i32::from_le_bytes(word.try_into().unwrap()))
        .collect();
    assert_eq!(words[..6], [10, 20, 40, 30, 50, 60]);
    let shears: Vec<i16> = model[24..32]
        .chunks_exact(2)
        .map(|word| i16::from_le_bytes(word.try_into().unwrap()))
        .collect();
    assert_eq!(shears, [0x1234, 0x2345, 0x3456, 0x4567]);

    let last = 6 * 32;
    let words: Vec<i32> = model[last..last + 24]
        .chunks_exact(4)
        .map(|word| i32::from_le_bytes(word.try_into().unwrap()))
        .collect();
    assert_eq!(words, [-10, -20, -40, -30, -50, -60]);
    let shears: Vec<i16> = model[last + 24..last + 32]
        .chunks_exact(2)
        .map(|word| i16::from_le_bytes(word.try_into().unwrap()))
        .collect();
    assert_eq!(shears, [-1, -2, -3, -4]);
}

#[test]
fn av1_tile_info_is_transposed_but_offsets_keep_the_av1_tile_id() {
    let mut params = Av1PicParams::default();
    params.tiles.cols = 2;
    params.tiles.rows = 2;
    params.tiles.widths[..2].copy_from_slice(&[2, 3]);
    params.tiles.heights[..2].copy_from_slice(&[4, 5]);
    params.tiles.tile_offset_start[..4].copy_from_slice(&[100, 500, 300, 700]);
    params.tiles.tile_offset_end[..4].copy_from_slice(&[200, 600, 400, 800]);

    let info = build_tile_info(&params, true);
    assert_eq!(info.len(), 2_048);
    let records: Vec<[u32; 4]> = info[..64]
        .chunks_exact(16)
        .map(|record| {
            std::array::from_fn(|field| {
                u32::from_le_bytes(record[field * 4..field * 4 + 4].try_into().unwrap())
            })
        })
        .collect();
    assert_eq!(
        records,
        [
            [2, 4, 100, 200],
            [2, 5, 300, 400],
            [3, 4, 500, 600],
            [3, 5, 700, 800],
        ]
    );
    assert!(info[64..].iter().all(|byte| *byte == 0));
}

#[test]
fn av1_probability_buffer_replaces_only_the_mv_region_for_intra_frames() {
    let cdf = Av1Cdf::new(
        Av1Cdfs::default_for_q_index(0),
        Av1MvCdfs::default_intrabc(),
    );
    let intra = build_probability_buffer(&cdf, true);
    let inter = build_probability_buffer(&cdf, false);

    assert_eq!(intra.len(), 12_256);
    assert_eq!(inter.len(), 12_256);
    assert_eq!(&intra[..2_192], &inter[..2_192]);
    assert_eq!(&intra[2_330..], &inter[2_330..]);
    assert_eq!(
        &intra[2_192..2_330],
        Av1MvCdfs::default_intrabc().as_bytes()
    );
}

fn key_params() -> Av1PicParams {
    let mut params = Av1PicParams::default();
    params.width = 320;
    params.height = 240;
    params.max_width = 320;
    params.max_height = 240;
    params.upscaled_width = 320;
    params.bitdepth = 10;
    params.superres_denom = 8;
    params.frame_tag_size = 29;
    params.tiles.cols = 3;
    params.tiles.rows = 2;
    params.tiles.widths[..3].copy_from_slice(&[2, 2, 1]);
    params.tiles.heights[..2].copy_from_slice(&[1, 1]);
    params.tile_cols_log2 = 1;
    params.format.frame_type = FrameType::Key as u8;
    params.format.show_frame = 1;
    params.quantization.base_qindex = 96;
    params.coded_lossless = 0;
    params.skip_ref0 = 1;
    params.skip_ref1 = 1;
    params
}

#[test]
fn av1_register_file_has_the_512_word_c_layout_and_address_offsets() {
    let params = key_params();
    let buffers = Av1DmaBuffers {
        current_output: 500,
        stream: 501,
        stream_len: 1_701,
        global_model: 502,
        tile_info: 503,
        probability: 504,
        probability_out: 505,
        filter: 506,
        tile: 507,
        display_output: 508,
        display_hor_stride: 384,
        display_ver_stride: 256,
        ..Av1DmaBuffers::default()
    };
    let mut state = Av1RegisterState::default();
    let regs = Av1Registers::build(&params, &buffers, &mut state).unwrap();

    assert_eq!(regs.words().len(), AV1_REGISTER_COUNT);
    assert_eq!(regs.field(1, 0, 1), 1);
    assert_eq!(regs.field(1, 5, 1), 0);
    assert_eq!(regs.field(3, 27, 5), 17);
    assert_eq!(regs.field(4, 6, 13), 30);
    assert_eq!(regs.field(4, 19, 13), 40);
    assert_eq!(regs.field(8, 16, 1), 1);
    assert_eq!(regs.field(8, 8, 8), 96);
    // C's `sw_scale_denom_minus9` field is only three bits. PicParams carries
    // the no-superres sentinel 8, which the C bitfield assignment truncates.
    assert_eq!(regs.field(9, 14, 3), 0);
    assert_eq!(regs.field(10, 0, 1), 1);
    assert_eq!(regs.field(10, 1, 1), 1);
    assert_eq!(regs.field(10, 10, 7), 2);
    assert_eq!(regs.field(10, 17, 7), 3);
    assert_eq!(regs.field(12, 10, 3), 6);
    assert_eq!(regs.field(12, 13, 3), 3);
    assert_eq!(regs.field(51, 0, 14), 14);
    assert_eq!(regs.field(51, 14, 14), 14);
    assert_eq!(regs.words()[65], 500);
    assert_eq!(regs.words()[99], 500);
    assert_eq!(regs.words()[133], 500);
    assert_eq!(regs.words()[169], 501);
    assert_eq!(regs.words()[167], 503);
    assert_eq!(regs.words()[171], 505);
    assert_eq!(regs.words()[173], 504);
    assert_eq!(regs.words()[85], 506);
    assert_eq!(regs.words()[89], 506);
    assert_eq!(regs.words()[91], 506);
    assert_eq!(regs.words()[179], 506);
    assert_eq!(regs.words()[183], 506);
    assert_eq!(regs.field(258, 0, 32), 1_792);
    assert_eq!(regs.field(6, 0, 32), 1_792);
    assert_eq!(regs.field(320, 0, 1), 1);
    assert_eq!(regs.field(322, 27, 5), 0);
    assert_eq!(regs.field(322, 18, 5), 3);
    assert_eq!(regs.field(331, 0, 16), 120);
    assert_eq!(regs.field(331, 16, 16), 160);
    assert_eq!(regs.field(332, 0, 16), 240);
    assert_eq!(regs.field(332, 16, 16), 320);
    assert_eq!(regs.field(329, 0, 16), 384);
    assert_eq!(regs.field(329, 16, 16), 384);
    assert_eq!(regs.words()[326], 508);
    assert_eq!(regs.words()[328], 508);

    let expected_offsets = [
        RegOffset {
            reg_idx: 85,
            offset: 11_776,
        },
        RegOffset {
            reg_idx: 89,
            offset: 18_944,
        },
        RegOffset {
            reg_idx: 91,
            offset: 53_504,
        },
        RegOffset {
            reg_idx: 99,
            offset: 96_000,
        },
        RegOffset {
            reg_idx: 133,
            offset: 144_064,
        },
        RegOffset {
            reg_idx: 169,
            offset: 16,
        },
        RegOffset {
            reg_idx: 183,
            offset: 7_680,
        },
        RegOffset {
            reg_idx: 328,
            offset: 98_304,
        },
    ];
    assert_eq!(regs.address_offsets(), expected_offsets);
}

#[test]
fn av1_inter_reference_registers_use_texture_slots_and_mpp_scaling() {
    let mut params = key_params();
    params.format.frame_type = FrameType::Inter as u8;
    params.order_hint = 8;
    params.order_hint_bits = 5;
    params.primary_ref_frame = 0;
    params.coding = (1 << 16) | (1 << 26) | (1 << 31) | (1 << 38);
    params.frame_refs[0] = PicReference {
        width: 320,
        height: 240,
        order_hint: 4,
        alt_frame_offset: 99,
        index: 1,
        wmtype: 1,
        ..PicReference::default()
    };
    params.frame_refs[1] = PicReference {
        width: 160,
        height: 120,
        order_hint: 9,
        index: 2,
        wmtype: 2,
        ..PicReference::default()
    };
    for reference in &mut params.frame_refs[2..7] {
        reference.index = -1;
    }

    let mut buffers = Av1DmaBuffers {
        current_output: 500,
        stream: 501,
        stream_len: 1_000,
        global_model: 502,
        tile_info: 503,
        probability: 504,
        probability_out: 505,
        filter: 506,
        tile: 507,
        display_output: 508,
        display_hor_stride: 384,
        display_ver_stride: 256,
        ..Av1DmaBuffers::default()
    };
    buffers.references[1] = Some(601);
    buffers.references[2] = Some(602);
    let mut state = Av1RegisterState {
        previous_output_slot: 1,
        ..Av1RegisterState::default()
    };
    let regs = Av1Registers::build(&params, &buffers, &mut state).unwrap();

    assert_eq!(regs.field(4, 0, 4), 2);
    assert_eq!(regs.field(33, 0, 16), 240);
    assert_eq!(regs.field(33, 16, 16), 320);
    assert_eq!(regs.field(34, 0, 16), 120);
    assert_eq!(regs.field(34, 16, 16), 160);
    assert_eq!(regs.field(36, 16, 16), 16_384);
    assert_eq!(regs.field(36, 0, 16), 16_384);
    assert_eq!(regs.field(37, 16, 16), 8_192);
    assert_eq!(regs.field(37, 0, 16), 8_192);
    assert_eq!(regs.words()[67], 601);
    assert_eq!(regs.words()[69], 602);
    assert_eq!(regs.words()[101], 601);
    assert_eq!(regs.words()[103], 602);
    assert_eq!(regs.words()[135], 601);
    assert_eq!(regs.words()[137], 602);
    assert_eq!(regs.field(59, 28, 1), 0);
    assert_eq!(regs.field(59, 29, 1), 1);
    assert_eq!(regs.field(11, 4, 2), 2);
    assert_eq!(regs.field(11, 3, 1), 1);
    // `set_superres` owns bit 17; bit 18 is warped motion.
    assert_eq!(regs.field(5, 17, 1), 0);
    assert_eq!(regs.field(5, 18, 1), 1);
    assert_eq!(regs.field(184, 27, 2), 1);
    assert_eq!(regs.field(185, 27, 2), 2);
    assert_eq!(regs.field(184, 9, 9), 4);
    assert_eq!(regs.field(184, 0, 9), 508);
    assert_eq!(state.previous_output_slot, 1);
    assert!(regs.address_offsets().contains(&RegOffset {
        reg_idx: 101,
        offset: 96_000,
    }));
    assert!(regs.address_offsets().contains(&RegOffset {
        reg_idx: 135,
        offset: 144_064,
    }));
}

#[test]
fn av1_motion_fields_select_last2_without_a_positive_offset_like_c_mpp() {
    let mut params = key_params();
    params.format.frame_type = FrameType::Inter as u8;
    params.order_hint = 6;
    params.order_hint_bits = 5;
    params.coding = 1 << 38;
    for reference in &mut params.frame_refs {
        reference.index = -1;
    }
    params.frame_refs[0] = PicReference {
        width: 320,
        height: 240,
        order_hint: 4,
        alt_frame_offset: 255,
        index: 2,
        ..PicReference::default()
    };
    params.frame_refs[1] = PicReference {
        width: 320,
        height: 240,
        order_hint: 3,
        index: 5,
        ..PicReference::default()
    };
    params.frame_refs[4] = PicReference {
        width: 320,
        height: 240,
        order_hint: 1,
        index: 4,
        ..PicReference::default()
    };
    params.frame_refs[5] = PicReference {
        width: 320,
        height: 240,
        order_hint: 0,
        index: 0,
        is_intra_frame: 1,
        ..PicReference::default()
    };
    params.frame_refs[6] = PicReference {
        width: 320,
        height: 240,
        order_hint: 8,
        index: 1,
        ..PicReference::default()
    };

    let buffers = Av1DmaBuffers::default();
    let mut state = Av1RegisterState::default();
    let regs = Av1Registers::build(&params, &buffers, &mut state).unwrap();

    assert_eq!(regs.field(9, 5, 3), 0, "first motion field is LAST");
    assert_eq!(regs.field(9, 8, 3), 6, "second motion field is ALTREF");
    assert_eq!(
        regs.field(9, 11, 3),
        1,
        "LAST2 is selected even with a negative order-hint distance"
    );
    assert_eq!(regs.field(11, 1, 1), 1, "third temporal field is enabled");
}

#[test]
fn av1_register_builder_programs_the_c_film_grain_fields_and_address() {
    let mut params = key_params();
    params.film_grain.apply_grain = 1;
    params.film_grain.scaling_shift_minus8 = 4;
    params.film_grain.chroma_scaling_from_luma = 0;
    params.film_grain.num_y_points = 2;
    params.film_grain.num_cb_points = 1;
    params.film_grain.num_cr_points = 0;
    params.film_grain.overlap_flag = 1;
    params.film_grain.clip_to_restricted_range = 1;
    params.film_grain.grain_seed = 0xabcd;
    params.film_grain.cb_mult = 120;
    params.film_grain.cb_luma_mult = 130;
    params.film_grain.cb_offset = 100;
    params.film_grain.cr_mult = 140;
    params.film_grain.cr_luma_mult = 110;
    params.film_grain.cr_offset = 300;
    let buffers = Av1DmaBuffers {
        film_grain: 909,
        ..Av1DmaBuffers::default()
    };
    let mut state = Av1RegisterState::default();

    let regs = Av1Registers::build(&params, &buffers, &mut state).unwrap();

    assert_eq!(regs.field(7, 7, 1), 1);
    assert_eq!(regs.field(7, 8, 1), 1);
    assert_eq!(regs.field(7, 9, 1), 1);
    assert_eq!(regs.field(7, 10, 1), 0);
    assert_eq!(regs.field(7, 11, 1), 1);
    assert_eq!(regs.field(7, 12, 1), 1);
    assert_eq!(regs.field(7, 13, 1), 0);
    assert_eq!(regs.field(7, 14, 16), 0xabcd);
    assert_eq!(regs.field(8, 0, 4), 12);
    assert_eq!(regs.field(28, 0, 9), 356);
    assert_eq!(regs.field(28, 9, 8), 2);
    assert_eq!(regs.field(28, 17, 8), 248);
    assert_eq!(regs.field(29, 0, 9), 44);
    assert_eq!(regs.field(29, 9, 8), 238);
    assert_eq!(regs.field(29, 17, 8), 12);
    assert_eq!(regs.words()[94], 0);
    assert_eq!(regs.words()[95], 909);

    params.film_grain.chroma_scaling_from_luma = 1;
    let regs = Av1Registers::build(&params, &buffers, &mut state).unwrap();
    assert_eq!(regs.field(7, 13, 1), 1);
    for register in [28, 29] {
        assert_eq!(regs.field(register, 0, 9), 0);
        assert_eq!(regs.field(register, 9, 8), 64);
        assert_eq!(regs.field(register, 17, 8), 0);
    }
}

#[test]
fn av1_register_builder_leaves_the_film_grain_address_unset_when_disabled() {
    let params = key_params();
    let buffers = Av1DmaBuffers {
        film_grain: 909,
        ..Av1DmaBuffers::default()
    };
    let mut state = Av1RegisterState::default();

    let regs = Av1Registers::build(&params, &buffers, &mut state).unwrap();

    assert_eq!(regs.field(7, 7, 1), 0);
    assert_eq!(regs.words()[94], 0);
    assert_eq!(regs.words()[95], 0);
}
