use super::vdpp2::SHARP_WORDS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vdpp2SharpConfig {
    pub enabled: bool,
    pub coloradj_bypass_enabled: bool,

    pub lti_h_enabled: bool,
    pub lti_h_radius: i32,
    pub lti_h_slope: i32,
    pub lti_h_threshold: i32,
    pub lti_h_gain: i32,
    pub lti_h_noise_threshold_pos: i32,
    pub lti_h_noise_threshold_neg: i32,

    pub lti_v_enabled: bool,
    pub lti_v_radius: i32,
    pub lti_v_slope: i32,
    pub lti_v_threshold: i32,
    pub lti_v_gain: i32,
    pub lti_v_noise_threshold_pos: i32,
    pub lti_v_noise_threshold_neg: i32,

    pub cti_h_enabled: bool,
    pub cti_h_radius: i32,
    pub cti_h_slope: i32,
    pub cti_h_threshold: i32,
    pub cti_h_gain: i32,
    pub cti_h_noise_threshold_pos: i32,
    pub cti_h_noise_threshold_neg: i32,

    pub peaking_enabled: bool,
    pub peaking_gain: i32,
    pub peaking_coring_enabled: bool,
    pub peaking_coring_zero: [i32; 8],
    pub peaking_coring_threshold: [i32; 8],
    pub peaking_coring_ratio: [i32; 8],
    pub peaking_gain_enabled: bool,
    pub peaking_gain_pos: [i32; 8],
    pub peaking_gain_neg: [i32; 8],
    pub peaking_limit_enabled: bool,
    pub peaking_limit_pos0: [i32; 8],
    pub peaking_limit_pos1: [i32; 8],
    pub peaking_limit_neg0: [i32; 8],
    pub peaking_limit_neg1: [i32; 8],
    pub peaking_limit_ratio: [i32; 8],
    pub peaking_limit_bound_pos: [i32; 8],
    pub peaking_limit_bound_neg: [i32; 8],

    pub peaking_edge_enabled: bool,
    pub peaking_edge_non_direction_threshold: i32,
    pub peaking_edge_direction_compare_ratio: i32,
    pub peaking_edge_non_direction_weight_offset: i32,
    pub peaking_edge_non_direction_weight_ratio: i32,
    pub peaking_edge_direction_count_threshold: i32,
    pub peaking_edge_direction_count_average: i32,
    pub peaking_edge_direction_count_offset: i32,
    pub peaking_edge_diagonal_direction_threshold: i32,
    pub peaking_edge_diagonal_adjust_gain: [i32; 8],

    pub peaking_estc_enabled: bool,
    pub peaking_estc_delta_offset_h: i32,
    pub peaking_estc_alpha_over_h: i32,
    pub peaking_estc_alpha_under_h: i32,
    pub peaking_estc_alpha_over_unlimited_h: i32,
    pub peaking_estc_alpha_under_unlimited_h: i32,
    pub peaking_estc_delta_offset_v: i32,
    pub peaking_estc_alpha_over_v: i32,
    pub peaking_estc_alpha_under_v: i32,
    pub peaking_estc_alpha_over_unlimited_v: i32,
    pub peaking_estc_alpha_under_unlimited_v: i32,
    pub peaking_estc_delta_offset_d0: i32,
    pub peaking_estc_alpha_over_d0: i32,
    pub peaking_estc_alpha_under_d0: i32,
    pub peaking_estc_alpha_over_unlimited_d0: i32,
    pub peaking_estc_alpha_under_unlimited_d0: i32,
    pub peaking_estc_delta_offset_d1: i32,
    pub peaking_estc_alpha_over_d1: i32,
    pub peaking_estc_alpha_under_d1: i32,
    pub peaking_estc_alpha_over_unlimited_d1: i32,
    pub peaking_estc_alpha_under_unlimited_d1: i32,
    pub peaking_estc_delta_offset_non: i32,
    pub peaking_estc_alpha_over_non: i32,
    pub peaking_estc_alpha_under_non: i32,
    pub peaking_estc_alpha_over_unlimited_non: i32,
    pub peaking_estc_alpha_under_unlimited_non: i32,
    pub peaking_diagonal_enhance_coefficient: i32,

    pub peaking_filter_h0: [i32; 6],
    pub peaking_filter_h1: [i32; 6],
    pub peaking_filter_h2: [i32; 6],
    pub peaking_filter_h3: [i32; 6],
    pub peaking_filter_v0: [i32; 3],
    pub peaking_filter_v1: [i32; 3],
    pub peaking_filter_v2: [i32; 3],
    pub peaking_filter_usm: [i32; 3],

    pub shoot_control_enabled: bool,
    pub shoot_filter_radius: i32,
    pub shoot_delta_offset: i32,
    pub shoot_alpha_over: i32,
    pub shoot_alpha_under: i32,
    pub shoot_alpha_over_unlimited: i32,
    pub shoot_alpha_under_unlimited: i32,

    pub global_gain_enabled: bool,
    pub global_gain_luma_mode: i32,
    pub global_gain_luma_gradient: [i32; 6],
    pub global_gain_luma_value: [i32; 6],
    pub global_gain_adaptive_gradient: [i32; 6],
    pub global_gain_adaptive_value: [i32; 6],
    pub global_gain_variance_gradient: [i32; 6],
    pub global_gain_variance_value: [i32; 6],

    pub color_control_enabled: bool,
    pub color_p0_scaling: i32,
    pub color_p0_point_u: i32,
    pub color_p0_point_v: i32,
    pub color_p0_table: [i32; 16],
    pub color_p1_scaling: i32,
    pub color_p1_point_u: i32,
    pub color_p1_point_v: i32,
    pub color_p1_table: [i32; 16],
    pub color_p2_scaling: i32,
    pub color_p2_point_u: i32,
    pub color_p2_point_v: i32,
    pub color_p2_table: [i32; 16],
    pub color_p3_scaling: i32,
    pub color_p3_point_u: i32,
    pub color_p3_point_v: i32,
    pub color_p3_table: [i32; 16],

    pub texture_adjust_enabled: bool,
    pub texture_adjust_y_mode: i32,
    pub texture_adjust_mode: i32,
    pub texture_adjust_gradient: [i32; 6],
    pub texture_adjust_value: [i32; 6],
}

impl Default for Vdpp2SharpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            coloradj_bypass_enabled: true,
            lti_h_enabled: false,
            lti_h_radius: 1,
            lti_h_slope: 100,
            lti_h_threshold: 21,
            lti_h_gain: 8,
            lti_h_noise_threshold_pos: 1023,
            lti_h_noise_threshold_neg: 1023,
            lti_v_enabled: false,
            lti_v_radius: 1,
            lti_v_slope: 100,
            lti_v_threshold: 21,
            lti_v_gain: 8,
            lti_v_noise_threshold_pos: 1023,
            lti_v_noise_threshold_neg: 1023,
            cti_h_enabled: false,
            cti_h_radius: 1,
            cti_h_slope: 100,
            cti_h_threshold: 21,
            cti_h_gain: 8,
            cti_h_noise_threshold_pos: 1023,
            cti_h_noise_threshold_neg: 1023,
            peaking_enabled: true,
            peaking_gain: 196,
            peaking_coring_enabled: true,
            peaking_coring_zero: [5, 5, 8, 5, 8, 5, 5, 0],
            peaking_coring_threshold: [40, 40, 40, 24, 26, 30, 26, 0],
            peaking_coring_ratio: [1479, 1188, 1024, 1422, 1024, 1024, 1024, 0],
            peaking_gain_enabled: true,
            peaking_gain_pos: [128, 256, 512, 256, 512, 256, 256, 0],
            peaking_gain_neg: [128, 256, 512, 256, 512, 256, 256, 0],
            peaking_limit_enabled: true,
            peaking_limit_pos0: [64; 8],
            peaking_limit_pos1: [120; 8],
            peaking_limit_neg0: [64; 8],
            peaking_limit_neg1: [120; 8],
            peaking_limit_ratio: [128; 8],
            peaking_limit_bound_pos: [81, 131, 63, 81, 63, 63, 63, 0],
            peaking_limit_bound_neg: [81, 131, 63, 81, 63, 63, 63, 0],
            peaking_edge_enabled: true,
            peaking_edge_non_direction_threshold: 16,
            peaking_edge_direction_compare_ratio: 4,
            peaking_edge_non_direction_weight_offset: 64,
            peaking_edge_non_direction_weight_ratio: 16,
            peaking_edge_direction_count_threshold: 2,
            peaking_edge_direction_count_average: 3,
            peaking_edge_direction_count_offset: 2,
            peaking_edge_diagonal_direction_threshold: 16,
            peaking_edge_diagonal_adjust_gain: [6, 7, 8, 9, 10, 11, 12, 13],
            peaking_estc_enabled: true,
            peaking_estc_delta_offset_h: 4,
            peaking_estc_alpha_over_h: 8,
            peaking_estc_alpha_under_h: 16,
            peaking_estc_alpha_over_unlimited_h: 64,
            peaking_estc_alpha_under_unlimited_h: 112,
            peaking_estc_delta_offset_v: 4,
            peaking_estc_alpha_over_v: 8,
            peaking_estc_alpha_under_v: 16,
            peaking_estc_alpha_over_unlimited_v: 64,
            peaking_estc_alpha_under_unlimited_v: 112,
            peaking_estc_delta_offset_d0: 4,
            peaking_estc_alpha_over_d0: 16,
            peaking_estc_alpha_under_d0: 16,
            peaking_estc_alpha_over_unlimited_d0: 96,
            peaking_estc_alpha_under_unlimited_d0: 96,
            peaking_estc_delta_offset_d1: 4,
            peaking_estc_alpha_over_d1: 16,
            peaking_estc_alpha_under_d1: 16,
            peaking_estc_alpha_over_unlimited_d1: 96,
            peaking_estc_alpha_under_unlimited_d1: 96,
            peaking_estc_delta_offset_non: 4,
            peaking_estc_alpha_over_non: 8,
            peaking_estc_alpha_under_non: 8,
            peaking_estc_alpha_over_unlimited_non: 112,
            peaking_estc_alpha_under_unlimited_non: 112,
            peaking_diagonal_enhance_coefficient: 6,
            peaking_filter_h0: [4, 16, 24, 0, 0, 0],
            peaking_filter_h1: [-16, 0, 32, 0, 0, 0],
            peaking_filter_h2: [0, -16, 32, 0, 0, 0],
            peaking_filter_h3: [0; 6],
            peaking_filter_v0: [1, 4, 6],
            peaking_filter_v1: [-4, 0, 8],
            peaking_filter_v2: [0, -4, 8],
            peaking_filter_usm: [1, 4, 6],
            shoot_control_enabled: true,
            shoot_filter_radius: 1,
            shoot_delta_offset: 16,
            shoot_alpha_over: 8,
            shoot_alpha_under: 8,
            shoot_alpha_over_unlimited: 112,
            shoot_alpha_under_unlimited: 112,
            global_gain_enabled: false,
            global_gain_luma_mode: 0,
            global_gain_luma_gradient: [0, 200, 300, 860, 960, 1023],
            global_gain_luma_value: [64; 6],
            global_gain_adaptive_gradient: [0, 4, 60, 180, 300, 1023],
            global_gain_adaptive_value: [64; 6],
            global_gain_variance_gradient: [0, 39, 102, 209, 500, 1023],
            global_gain_variance_value: [64; 6],
            color_control_enabled: false,
            color_p0_scaling: 1,
            color_p0_point_u: 115,
            color_p0_point_v: 155,
            color_p0_table: [0, 0, 0, 1, 2, 3, 4, 6, 8, 10, 11, 12, 13, 14, 15, 15],
            color_p1_scaling: 1,
            color_p1_point_u: 90,
            color_p1_point_v: 120,
            color_p1_table: [
                31, 31, 30, 29, 28, 27, 25, 23, 21, 19, 18, 17, 16, 16, 15, 15,
            ],
            color_p2_scaling: 1,
            color_p2_point_u: 128,
            color_p2_point_v: 128,
            color_p2_table: [16; 16],
            color_p3_scaling: 1,
            color_p3_point_u: 128,
            color_p3_point_v: 128,
            color_p3_table: [16; 16],
            texture_adjust_enabled: false,
            texture_adjust_y_mode: 3,
            texture_adjust_mode: 0,
            texture_adjust_gradient: [0, 128, 256, 400, 600, 1023],
            texture_adjust_value: [40, 60, 80, 100, 127, 127],
        }
    }
}

struct PeakingProfile {
    index_negative: [i32; 4],
    index_positive: [i32; 4],
    value_negative: [i32; 4],
    value_positive: [i32; 4],
    ratio_negative01: i32,
    ratio_negative12: i32,
    ratio_negative23: i32,
    ratio_positive01: i32,
    ratio_positive12: i32,
    ratio_positive23: i32,
}

pub(super) fn build_sharp(config: &Vdpp2SharpConfig) -> [u32; SHARP_WORDS] {
    let mut words = [0u32; SHARP_WORDS];
    let mut lti_h_gain = config.lti_h_gain;
    let mut lti_v_gain = config.lti_v_gain;
    let lti_enabled = config.lti_h_enabled || config.lti_v_enabled;
    if lti_enabled && !config.lti_h_enabled {
        lti_h_gain = 0;
    }
    if lti_enabled && !config.lti_v_enabled {
        lti_v_gain = 0;
    }

    words[0] = field(config.enabled as u32, 0, 1)
        | field(lti_enabled as u32, 1, 1)
        | field(config.cti_h_enabled as u32, 2, 1)
        | field(config.peaking_enabled as u32, 3, 1)
        | field(
            (config.peaking_coring_enabled
                || config.peaking_gain_enabled
                || config.peaking_limit_enabled) as u32,
            4,
            1,
        )
        | field(config.peaking_edge_enabled as u32, 6, 1)
        | field(config.shoot_control_enabled as u32, 7, 1)
        | field(config.global_gain_enabled as u32, 8, 1)
        | field(config.color_control_enabled as u32, 9, 1)
        | field(config.texture_adjust_enabled as u32, 10, 1)
        | field(config.coloradj_bypass_enabled as u32, 11, 1);

    words[2] = field(config.peaking_filter_v0[0] as u32, 0, 4)
        | field(config.peaking_filter_v0[1] as u32, 4, 4)
        | field(config.peaking_filter_v0[2] as u32, 8, 4)
        | field(config.peaking_filter_v1[0] as u32, 12, 4)
        | field(config.peaking_filter_v1[1] as u32, 16, 4)
        | field(config.peaking_filter_v1[2] as u32, 20, 4);
    words[3] = field(config.peaking_filter_v2[0] as u32, 0, 4)
        | field(config.peaking_filter_v2[1] as u32, 4, 4)
        | field(config.peaking_filter_v2[2] as u32, 8, 4)
        | field(config.peaking_filter_usm[0] as u32, 12, 4)
        | field(config.peaking_filter_usm[1] as u32, 16, 4)
        | field(config.peaking_filter_usm[2] as u32, 20, 4)
        | field(config.peaking_diagonal_enhance_coefficient as u32, 24, 3);
    pack_filter(&mut words, 4, &config.peaking_filter_h0);
    pack_filter(&mut words, 6, &config.peaking_filter_h1);
    pack_filter(&mut words, 8, &config.peaking_filter_h2);

    for profile in 0..7 {
        let start = [12, 23, 34, 45, 56, 67, 78][profile];
        pack_peaking_profile(&mut words, start, peaking_profile(config, profile));
    }

    words[100] = field(config.peaking_gain as u32, 0, 10)
        | field(config.peaking_edge_non_direction_threshold as u32, 12, 7)
        | field(config.peaking_edge_direction_compare_ratio as u32, 20, 4)
        | field(config.peaking_edge_non_direction_weight_ratio as u32, 24, 5);
    words[101] = field(config.peaking_edge_non_direction_weight_offset as u32, 0, 8)
        | field(config.peaking_edge_direction_count_threshold as u32, 8, 4)
        | field(config.peaking_edge_direction_count_average as u32, 12, 3)
        | field(config.peaking_edge_direction_count_offset as u32, 16, 4)
        | field(
            config.peaking_edge_diagonal_direction_threshold as u32,
            20,
            7,
        );
    for (index, gain) in config.peaking_edge_diagonal_adjust_gain.iter().enumerate() {
        words[102] |= field(*gain as u32, index as u32 * 4, 4);
    }

    words[103] = pack_alpha_group(
        config.peaking_estc_alpha_over_non,
        config.peaking_estc_alpha_under_non,
        config.peaking_estc_alpha_over_unlimited_non,
        config.peaking_estc_alpha_under_unlimited_non,
    );
    words[104] = pack_alpha_group(
        config.peaking_estc_alpha_over_v,
        config.peaking_estc_alpha_under_v,
        config.peaking_estc_alpha_over_unlimited_v,
        config.peaking_estc_alpha_under_unlimited_v,
    );
    words[105] = pack_alpha_group(
        config.peaking_estc_alpha_over_h,
        config.peaking_estc_alpha_under_h,
        config.peaking_estc_alpha_over_unlimited_h,
        config.peaking_estc_alpha_under_unlimited_h,
    );
    words[106] = pack_alpha_group(
        config.peaking_estc_alpha_over_d0,
        config.peaking_estc_alpha_under_d0,
        config.peaking_estc_alpha_over_unlimited_d0,
        config.peaking_estc_alpha_under_unlimited_d0,
    );
    words[107] = pack_alpha_group(
        config.peaking_estc_alpha_over_d1,
        config.peaking_estc_alpha_under_d1,
        config.peaking_estc_alpha_over_unlimited_d1,
        config.peaking_estc_alpha_under_unlimited_d1,
    );
    words[108] = field(config.peaking_estc_delta_offset_non as u32, 0, 8)
        | field(config.peaking_estc_delta_offset_v as u32, 8, 8)
        | field(config.peaking_estc_delta_offset_h as u32, 16, 8);
    words[109] = field(config.peaking_estc_delta_offset_d0 as u32, 0, 8)
        | field(config.peaking_estc_delta_offset_d1 as u32, 8, 8);

    words[110] = field(config.shoot_filter_radius as u32, 0, 1)
        | field(config.shoot_delta_offset as u32, 4, 8)
        | field(config.shoot_alpha_over as u32, 12, 7)
        | field(config.shoot_alpha_under as u32, 20, 7);
    words[111] = field(config.shoot_alpha_over_unlimited as u32, 0, 7)
        | field(config.shoot_alpha_under_unlimited as u32, 8, 7);

    pack_global_gain(
        &mut words,
        &config.global_gain_adaptive_gradient,
        &config.global_gain_adaptive_value,
    );
    pack_global_variance(
        &mut words,
        &config.global_gain_variance_gradient,
        &config.global_gain_variance_value,
    );
    pack_global_luma(
        &mut words,
        config.global_gain_luma_mode,
        &config.global_gain_luma_gradient,
        &config.global_gain_luma_value,
    );

    pack_color_profile(
        &mut words,
        140,
        config.color_p0_point_u,
        config.color_p0_point_v,
        config.color_p0_scaling,
        &config.color_p0_table,
    );
    pack_color_profile(
        &mut words,
        144,
        config.color_p1_point_u,
        config.color_p1_point_v,
        config.color_p1_scaling,
        &config.color_p1_table,
    );
    pack_color_profile(
        &mut words,
        148,
        config.color_p2_point_u,
        config.color_p2_point_v,
        config.color_p2_scaling,
        &config.color_p2_table,
    );
    pack_color_profile(
        &mut words,
        152,
        config.color_p3_point_u,
        config.color_p3_point_v,
        config.color_p3_scaling,
        &config.color_p3_table,
    );

    let texture_slopes = gain_slopes(
        &config.texture_adjust_gradient,
        &config.texture_adjust_value,
    );
    words[156] = field(config.texture_adjust_mode as u32, 0, 1)
        | field(config.texture_adjust_y_mode as u32, 1, 2)
        | field((config.texture_adjust_gradient[1] >> 2) as u32, 4, 8)
        | field((config.texture_adjust_gradient[2] >> 2) as u32, 16, 8);
    words[157] = field((config.texture_adjust_gradient[3] >> 2) as u32, 0, 8)
        | field((config.texture_adjust_gradient[4] >> 2) as u32, 12, 8)
        | field(config.texture_adjust_value[0] as u32, 24, 7);
    words[158] = field(config.texture_adjust_value[1] as u32, 0, 7)
        | field(config.texture_adjust_value[2] as u32, 8, 7)
        | field(config.texture_adjust_value[3] as u32, 16, 7)
        | field(config.texture_adjust_value[4] as u32, 24, 7);
    words[159] = field(texture_slopes[0] as u32, 0, 11) | field(texture_slopes[1] as u32, 12, 11);
    words[160] = field(texture_slopes[2] as u32, 0, 11) | field(texture_slopes[3] as u32, 12, 11);
    words[161] = field(texture_slopes[4] as u32, 0, 11);

    words[162] = field(config.lti_h_radius as u32, 0, 1)
        | field(config.lti_h_slope as u32, 4, 9)
        | field(config.lti_h_threshold as u32, 16, 9);
    words[163] = field(config.lti_h_noise_threshold_neg as u32, 0, 10)
        | field(config.lti_h_noise_threshold_pos as u32, 12, 10)
        | field(lti_h_gain as u32, 24, 5);
    words[164] = field(config.lti_v_radius as u32, 0, 1)
        | field(config.lti_v_slope as u32, 4, 9)
        | field(config.lti_v_threshold as u32, 16, 9);
    words[165] = field(config.lti_v_noise_threshold_neg as u32, 0, 10)
        | field(config.lti_v_noise_threshold_pos as u32, 12, 10)
        | field(lti_v_gain as u32, 24, 5);
    words[166] = field(config.cti_h_radius as u32, 0, 1)
        | field(config.cti_h_slope as u32, 4, 9)
        | field(config.cti_h_threshold as u32, 16, 9);
    words[167] = field(config.cti_h_noise_threshold_neg as u32, 0, 10)
        | field(config.cti_h_noise_threshold_pos as u32, 12, 10)
        | field(config.cti_h_gain as u32, 24, 5);
    words
}

fn peaking_profile(config: &Vdpp2SharpConfig, index: usize) -> PeakingProfile {
    let (coring_ratio, coring_zero, coring_threshold) = if config.peaking_coring_enabled {
        (
            config.peaking_coring_ratio[index],
            config.peaking_coring_zero[index] >> 2,
            config.peaking_coring_threshold[index] >> 2,
        )
    } else {
        (1024, 0, 0)
    };
    let (gain_positive, gain_negative) = if config.peaking_gain_enabled {
        (
            config.peaking_gain_pos[index],
            config.peaking_gain_neg[index],
        )
    } else {
        (1024, 1024)
    };
    let (limit_p0, limit_p1, limit_n0, limit_n1, limit_ratio) = if config.peaking_limit_enabled {
        (
            config.peaking_limit_pos0[index] >> 2,
            config.peaking_limit_pos1[index] >> 2,
            config.peaking_limit_neg0[index] >> 2,
            config.peaking_limit_neg1[index] >> 2,
            config.peaking_limit_ratio[index],
        )
    } else {
        (63, 63, 63, 63, 1024)
    };

    let ratio_positive01 = (coring_ratio * gain_positive + 512) >> 10;
    let ratio_negative01 = (coring_ratio * gain_negative + 512) >> 10;
    let mut value_positive = [0; 4];
    let mut value_negative = [0; 4];
    value_positive[1] = (ratio_positive01 * (coring_threshold - coring_zero) + 512) >> 10;
    value_negative[1] = -((ratio_negative01 * (coring_threshold - coring_zero) + 512) >> 10);
    let add_positive = (gain_positive * (limit_p0 - coring_threshold) + 512) >> 10;
    value_positive[2] = value_positive[1] + add_positive;
    let add_negative = (gain_negative * (limit_n0 - coring_threshold) + 512) >> 10;
    value_negative[2] = value_negative[1] - add_negative;

    let ratio_positive23 = (limit_ratio * gain_positive + 512) >> 10;
    let ratio_negative23 = (limit_ratio * gain_negative + 512) >> 10;
    let add_positive = (ratio_positive23 * (limit_p1 - limit_p0) + 512) >> 10;
    value_positive[3] = value_positive[2] + add_positive;
    let add_negative = (ratio_negative23 * (limit_n1 - limit_n0) + 512) >> 10;
    value_negative[3] = value_negative[2] - add_negative;

    let index_positive = [
        coring_zero.clamp(-256, 255),
        coring_threshold.clamp(-256, 255),
        limit_p0.clamp(-256, 255),
        limit_p1.clamp(-256, 255),
    ];
    let index_negative = [
        (-coring_zero).clamp(-256, 255),
        (-coring_threshold).clamp(-256, 255),
        (-limit_n0).clamp(-256, 255),
        (-limit_n1).clamp(-256, 255),
    ];
    for value in &mut value_positive {
        *value = (*value).clamp(-256, 255);
    }
    for value in &mut value_negative {
        *value = (*value).clamp(-256, 255);
    }
    PeakingProfile {
        index_negative,
        index_positive,
        value_negative,
        value_positive,
        ratio_negative01: ratio_negative01.clamp(0, 4095),
        ratio_negative12: gain_negative.clamp(0, 4095),
        ratio_negative23: ratio_negative23.clamp(0, 4095),
        ratio_positive01: ratio_positive01.clamp(0, 4095),
        ratio_positive12: gain_positive.clamp(0, 4095),
        ratio_positive23: ratio_positive23.clamp(0, 4095),
    }
}

fn pack_peaking_profile(words: &mut [u32], start: usize, profile: PeakingProfile) {
    let index_width = if start == 78 { 12 } else { 9 };
    words[start] = field(profile.index_negative[0] as u32, 0, index_width)
        | field(profile.index_negative[1] as u32, 16, index_width);
    words[start + 1] = field(profile.index_negative[2] as u32, 0, 9)
        | field(profile.index_negative[3] as u32, 16, 9);
    words[start + 2] = field(profile.index_positive[0] as u32, 0, 9)
        | field(profile.index_positive[1] as u32, 16, 9);
    words[start + 3] = field(profile.index_positive[2] as u32, 0, 9)
        | field(profile.index_positive[3] as u32, 16, 9);
    words[start + 4] = field(profile.value_negative[1] as u32, 0, 9)
        | field(profile.value_negative[2] as u32, 16, 9);
    words[start + 5] = field(profile.value_negative[3] as u32, 0, 9)
        | field(profile.value_positive[1] as u32, 16, 9);
    words[start + 6] = field(profile.value_positive[2] as u32, 0, 9)
        | field(profile.value_positive[3] as u32, 16, 9);
    words[start + 7] = field(profile.ratio_negative01 as u32, 0, 12)
        | field(profile.ratio_negative12 as u32, 16, 12);
    words[start + 8] = field(profile.ratio_negative23 as u32, 0, 12)
        | field(profile.ratio_positive01 as u32, 16, 12);
    words[start + 9] = field(profile.ratio_positive12 as u32, 0, 12)
        | field(profile.ratio_positive23 as u32, 12, 12);
}

fn pack_filter(words: &mut [u32], index: usize, coefficients: &[i32; 6]) {
    words[index] = field(coefficients[0] as u32, 0, 6)
        | field(coefficients[1] as u32, 8, 6)
        | field(coefficients[2] as u32, 16, 6);
}

fn pack_alpha_group(over: i32, under: i32, over_unlimited: i32, under_unlimited: i32) -> u32 {
    field(over as u32, 0, 7)
        | field(under as u32, 8, 7)
        | field(over_unlimited as u32, 16, 7)
        | field(under_unlimited as u32, 24, 7)
}

fn pack_global_gain(words: &mut [u32], gradient: &[i32; 6], gain: &[i32; 6]) {
    let index0 = gradient[1] >> 2;
    let index1 = gradient[2] >> 2;
    let index2 = gradient[3] >> 2;
    let index3 = gradient[4] >> 2;
    words[112] =
        field(index0 as u32, 0, 8) | field(index1 as u32, 10, 8) | field(index2 as u32, 20, 8);
    words[113] =
        field(index3 as u32, 0, 8) | field(gain[0] as u32, 12, 7) | field(gain[1] as u32, 20, 7);
    words[114] =
        field(gain[2] as u32, 0, 7) | field(gain[3] as u32, 8, 7) | field(gain[4] as u32, 16, 7);
    let slopes = gain_slopes(gradient, gain);
    words[115] = field(slopes[0] as u32, 0, 11) | field(slopes[1] as u32, 12, 11);
    words[128] = field(slopes[2] as u32, 0, 11) | field(slopes[3] as u32, 12, 11);
    words[129] = field(slopes[4] as u32, 0, 11);
}

fn pack_global_variance(words: &mut [u32], gradient: &[i32; 6], gain: &[i32; 6]) {
    let index0 = gradient[1] >> 2;
    let index1 = gradient[2] >> 2;
    let index2 = gradient[3] >> 2;
    let index3 = gradient[4] >> 2;
    words[129] |= field(index0 as u32, 11, 8) | field(index1 as u32, 21, 8);
    words[130] =
        field(index2 as u32, 0, 8) | field(index3 as u32, 12, 8) | field(gain[0] as u32, 24, 7);
    words[131] = field(gain[1] as u32, 0, 7)
        | field(gain[2] as u32, 8, 7)
        | field(gain[3] as u32, 16, 7)
        | field(gain[4] as u32, 24, 7);
    let slopes = gain_slopes(gradient, gain);
    words[132] = field(slopes[0] as u32, 0, 11) | field(slopes[1] as u32, 12, 11);
    words[133] = field(slopes[2] as u32, 0, 11) | field(slopes[3] as u32, 12, 11);
    words[134] = field(slopes[4] as u32, 0, 11);
}

fn pack_global_luma(words: &mut [u32], mode: i32, gradient: &[i32; 6], gain: &[i32; 6]) {
    let index0 = gradient[1] >> 2;
    let index1 = gradient[2] >> 2;
    let index2 = gradient[3] >> 2;
    let index3 = gradient[4] >> 2;
    words[134] |= field(mode as u32, 16, 2) | field(index0 as u32, 20, 8);
    words[135] =
        field(index1 as u32, 0, 8) | field(index2 as u32, 10, 8) | field(index3 as u32, 20, 8);
    words[136] = field(gain[0] as u32, 0, 7)
        | field(gain[1] as u32, 8, 7)
        | field(gain[2] as u32, 16, 7)
        | field(gain[3] as u32, 24, 7);
    let slopes = gain_slopes(gradient, gain);
    words[137] = field(gain[4] as u32, 0, 7)
        | field(slopes[0] as u32, 8, 11)
        | field(slopes[1] as u32, 20, 11);
    words[138] = field(slopes[2] as u32, 0, 11) | field(slopes[3] as u32, 12, 11);
    words[139] = field(slopes[4] as u32, 0, 11);
}

fn gain_slopes(gradient: &[i32; 6], gain: &[i32; 6]) -> [i32; 5] {
    let index = [
        0,
        gradient[1] >> 2,
        gradient[2] >> 2,
        gradient[3] >> 2,
        gradient[4] >> 2,
        255,
    ];
    let mut slopes = [0; 5];
    for (slope_index, output) in slopes.iter_mut().enumerate() {
        let delta = (index[slope_index + 1] - index[slope_index]) as f64;
        let gain_delta = (gain[slope_index + 1] - gain[slope_index]) as f64;
        *output = round_away_from_zero(128.0 * gain_delta / delta.max(1.0)).clamp(-1024, 1023);
    }
    slopes
}

fn round_away_from_zero(value: f64) -> i32 {
    if value > 0.0 {
        (value + 0.5) as i32
    } else {
        (value - 0.5) as i32
    }
}

fn pack_color_profile(
    words: &mut [u32],
    start: usize,
    point_u: i32,
    point_v: i32,
    scaling: i32,
    table: &[i32; 16],
) {
    words[start] =
        field(point_u as u32, 0, 8) | field(point_v as u32, 12, 8) | field(scaling as u32, 24, 3);
    for group in 0..3 {
        let mut word = 0;
        for item in 0..if group == 2 { 4 } else { 6 } {
            word |= field(table[group * 6 + item] as u32, item as u32 * 5, 5);
        }
        words[start + group + 1] = word;
    }
}

const fn field(value: u32, shift: u32, width: u32) -> u32 {
    (value & ((1u32 << width) - 1)) << shift
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hal::vdpp2_defaults::DEFAULT_SHARP;

    #[test]
    fn default_sharp_matches_the_vendor_register_trace() {
        assert_eq!(build_sharp(&Vdpp2SharpConfig::default()), DEFAULT_SHARP);
    }
}
