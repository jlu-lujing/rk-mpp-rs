use rk_mpp_rs::hal::vdpp2::{
    Vdpp2Config, Vdpp2DmsrConfig, Vdpp2EsConfig, Vdpp2Job, Vdpp2Registers, Vdpp2SharpConfig,
};

fn custom_dmsr() -> Vdpp2DmsrConfig {
    Vdpp2DmsrConfig {
        enabled: true,
        strength_primary: 17,
        strength_secondary: 9,
        dumping: 11,
        primary_weight_odd_1: 7,
        primary_weight_even_1: 13,
        primary_weight_odd_2: 21,
        primary_weight_even_2: 29,
        secondary_weight: 11,
        block_flat_threshold: 123,
        contrast_confidence_x0: 100,
        contrast_confidence_x1: 900,
        contrast_confidence_y0: 7,
        contrast_confidence_y1: 60_000,
        diff_core_threshold0: 3,
        diff_core_threshold1: 23,
        diff_core_weight0: 9,
        diff_core_weight1: 17,
        diff_core_weight2: 25,
        edge_low_thresholds: [1, 2, 3, 4, 5, 6, 7],
        edge_high_thresholds: [3, 13, 23, 33, 43, 53, 63],
    }
}

fn custom_es() -> Vdpp2EsConfig {
    Vdpp2EsConfig {
        enabled: true,
        angle_delta: 20,
        angle_delta_extra: 3,
        gradient_no_direction_threshold: 41,
        gradient_flat_threshold: 99,
        weight_gain: 200,
        weight_decay: 77,
        low_confidence_threshold: 111,
        low_confidence_ratio: 22,
        confidence_count_threshold: 7,
        weight_local_threshold: 100,
        diff_gain0: 0x2345,
        diff_gain1: 0x3456,
        diff_limit: 0x4567,
        diff_to_confidence_x: [0, 500, 1000, 1500, 2000, 3000, 4000, 5000, 60_000],
        diff_to_confidence_y: [0, 20, 50, 90, 140, 180, 220, 240, 255],
        endpoint_check_enabled: true,
        memory_gating_enabled: false,
    }
}

fn custom_sharp() -> Vdpp2SharpConfig {
    let mut config = Vdpp2SharpConfig::default();
    config.coloradj_bypass_enabled = false;
    config.lti_h_enabled = true;
    config.lti_h_slope = 123;
    config.lti_h_threshold = 37;
    config.lti_h_gain = 17;
    config.lti_h_noise_threshold_pos = 900;
    config.lti_h_noise_threshold_neg = 800;
    config.lti_v_enabled = true;
    config.lti_v_radius = 0;
    config.lti_v_slope = 111;
    config.lti_v_threshold = 41;
    config.lti_v_gain = 19;
    config.lti_v_noise_threshold_pos = 901;
    config.lti_v_noise_threshold_neg = 799;
    config.cti_h_enabled = true;
    config.cti_h_slope = 99;
    config.cti_h_threshold = 31;
    config.cti_h_gain = 11;
    config.cti_h_noise_threshold_pos = 777;
    config.cti_h_noise_threshold_neg = 666;
    config.peaking_gain = 333;
    for index in 0..7 {
        config.peaking_coring_zero[index] = index as i32 + 2;
        config.peaking_coring_threshold[index] = 20 + index as i32;
        config.peaking_coring_ratio[index] = 900 + index as i32 * 10;
        config.peaking_gain_pos[index] = 300 + index as i32 * 20;
        config.peaking_gain_neg[index] = 280 + index as i32 * 20;
        config.peaking_limit_pos0[index] = 60 + index as i32;
        config.peaking_limit_pos1[index] = 116 + index as i32;
        config.peaking_limit_neg0[index] = 58 + index as i32;
        config.peaking_limit_neg1[index] = 114 + index as i32;
        config.peaking_limit_ratio[index] = 96 + index as i32;
    }
    config.peaking_filter_h0[..3].copy_from_slice(&[5, 15, 25]);
    config.peaking_filter_h1[..3].copy_from_slice(&[-8, 1, 31]);
    config.peaking_filter_v0.copy_from_slice(&[0, 5, 7]);
    config.peaking_edge_non_direction_threshold = 21;
    config.peaking_edge_direction_compare_ratio = 5;
    config.peaking_edge_non_direction_weight_offset = 73;
    config.peaking_edge_non_direction_weight_ratio = 19;
    config.peaking_edge_direction_count_threshold = 4;
    config.peaking_edge_direction_count_average = 5;
    config.peaking_edge_direction_count_offset = 3;
    config.peaking_edge_diagonal_direction_threshold = 17;
    config.peaking_edge_diagonal_adjust_gain = [5, 6, 7, 8, 9, 10, 11, 12];
    config.peaking_estc_delta_offset_h = 5;
    config.peaking_estc_alpha_over_h = 9;
    config.peaking_estc_alpha_under_h = 17;
    config.peaking_estc_alpha_over_unlimited_h = 65;
    config.peaking_estc_alpha_under_unlimited_h = 111;
    config.shoot_delta_offset = 17;
    config.shoot_alpha_over = 9;
    config.shoot_alpha_under = 10;
    config.shoot_alpha_over_unlimited = 109;
    config.shoot_alpha_under_unlimited = 108;
    config.global_gain_enabled = true;
    config.global_gain_luma_mode = 1;
    config.global_gain_luma_gradient = [0, 100, 400, 700, 900, 1023];
    config.global_gain_luma_value = [20, 40, 60, 80, 100, 120];
    config.global_gain_adaptive_gradient = [0, 20, 80, 200, 400, 1023];
    config.global_gain_adaptive_value = [30, 40, 50, 60, 70, 80];
    config.global_gain_variance_gradient = [0, 50, 120, 220, 520, 1023];
    config.global_gain_variance_value = [25, 35, 45, 55, 65, 75];
    config.color_control_enabled = true;
    config.color_p0_scaling = 2;
    config.color_p0_point_u = 110;
    config.color_p0_point_v = 150;
    config.color_p1_scaling = 3;
    config.color_p1_point_u = 80;
    config.color_p1_point_v = 130;
    config.color_p2_scaling = 4;
    config.color_p2_point_u = 120;
    config.color_p2_point_v = 140;
    config.color_p3_scaling = 5;
    config.color_p3_point_u = 130;
    config.color_p3_point_v = 150;
    for index in 0..16 {
        config.color_p0_table[index] = index as i32;
        config.color_p1_table[index] = 31 - index as i32;
        config.color_p2_table[index] = 8 + index as i32 / 2;
        config.color_p3_table[index] = 24 - index as i32 / 2;
    }
    config.texture_adjust_enabled = true;
    config.texture_adjust_y_mode = 2;
    config.texture_adjust_mode = 1;
    config.texture_adjust_gradient = [0, 100, 300, 500, 700, 1023];
    config.texture_adjust_value = [30, 45, 60, 75, 90, 110];
    config
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src_width: usize = args
        .get(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(720);
    let src_height: usize = args
        .get(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(480);
    let dst_width: usize = args
        .get(3)
        .and_then(|value| value.parse().ok())
        .unwrap_or(1920);
    let dst_height: usize = args
        .get(4)
        .and_then(|value| value.parse().ok())
        .unwrap_or(1080);
    let mut config = if args.get(5).map(String::as_str) == Some("hist") {
        let mut config = Vdpp2Config::nv12_to_yuv444(src_width, src_height, dst_width, dst_height);
        config.mode = rk_mpp_rs::hal::vdpp2::Vdpp2Mode::Histogram;
        config.histogram.enable = true;
        config
    } else if args.get(5).map(String::as_str) == Some("420") {
        Vdpp2Config::nv12_to_nv12(src_width, src_height, dst_width, dst_height)
    } else {
        Vdpp2Config::nv12_to_yuv444(src_width, src_height, dst_width, dst_height)
    };
    if args.get(6).map(String::as_str) == Some("scale-only") {
        config.enhancement = false;
    }
    if args.get(5).map(String::as_str) == Some("custom-enh") {
        config.enhancement = true;
        config.dmsr = custom_dmsr();
        config.es = custom_es();
    }
    if args.get(5).map(String::as_str) == Some("custom-sharp") {
        config.enhancement = true;
        config.sharp = custom_sharp();
    }
    let registers = Vdpp2Registers::build(
        &config,
        &Vdpp2Job {
            src_fd: 7,
            src_uv_offset: (config.src_width_vir * config.src_height_vir) as u32,
            dst_fd: 8,
            dst_uv_offset: (config.dst_width_vir * config.dst_height_vir) as u32,
            hist_fd: 9,
        },
    )
    .unwrap();
    for (name, words) in [
        ("yrgb_hor_coe", registers.yrgb_hor_coe.as_slice()),
        ("yrgb_ver_coe", registers.yrgb_ver_coe.as_slice()),
        ("cbcr_hor_coe", registers.cbcr_hor_coe.as_slice()),
        ("cbcr_ver_coe", registers.cbcr_ver_coe.as_slice()),
        ("zme_common", registers.zme_common.as_slice()),
        ("dmsr", registers.dmsr.as_slice()),
        ("hist", registers.hist.as_slice()),
        ("es", registers.es.as_slice()),
        ("sharp", registers.sharp.as_slice()),
        ("common", registers.common.as_slice()),
    ] {
        print!("{name}:");
        for word in words {
            print!(" {word:08x}");
        }
        println!();
    }
}
