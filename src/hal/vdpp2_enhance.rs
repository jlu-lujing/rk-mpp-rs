use std::f64::consts::PI;

use super::vdpp2::{DMSR_WORDS, ES_WORDS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vdpp2DmsrConfig {
    pub enabled: bool,
    pub strength_primary: u32,
    pub strength_secondary: u32,
    pub dumping: u32,
    pub primary_weight_even_1: u32,
    pub primary_weight_even_2: u32,
    pub primary_weight_odd_1: u32,
    pub primary_weight_odd_2: u32,
    pub secondary_weight: u32,
    pub block_flat_threshold: u32,
    pub contrast_confidence_x0: u32,
    pub contrast_confidence_x1: u32,
    pub contrast_confidence_y0: u32,
    pub contrast_confidence_y1: u32,
    pub diff_core_threshold0: u32,
    pub diff_core_threshold1: u32,
    pub diff_core_weight0: u32,
    pub diff_core_weight1: u32,
    pub diff_core_weight2: u32,
    pub edge_low_thresholds: [u32; 7],
    pub edge_high_thresholds: [u32; 7],
}

impl Default for Vdpp2DmsrConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strength_primary: 10,
            strength_secondary: 4,
            dumping: 6,
            primary_weight_even_1: 12,
            primary_weight_even_2: 12,
            primary_weight_odd_1: 8,
            primary_weight_odd_2: 16,
            secondary_weight: 5,
            block_flat_threshold: 20,
            contrast_confidence_x0: 1680,
            contrast_confidence_x1: 6720,
            contrast_confidence_y0: 0,
            contrast_confidence_y1: 65535,
            diff_core_threshold0: 1,
            diff_core_threshold1: 5,
            diff_core_weight0: 16,
            diff_core_weight1: 16,
            diff_core_weight2: 16,
            edge_low_thresholds: [30, 10, 0, 0, 0, 0, 0],
            edge_high_thresholds: [60, 40, 20, 10, 10, 10, 10],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vdpp2EsConfig {
    pub enabled: bool,
    pub angle_delta: u32,
    pub angle_delta_extra: u32,
    pub gradient_no_direction_threshold: u32,
    pub gradient_flat_threshold: u32,
    pub weight_gain: u32,
    pub weight_decay: u32,
    pub low_confidence_threshold: u32,
    pub low_confidence_ratio: u32,
    pub confidence_count_threshold: u32,
    pub weight_local_threshold: u32,
    pub diff_gain0: u32,
    pub diff_gain1: u32,
    pub diff_limit: u32,
    pub diff_to_confidence_x: [u32; 9],
    pub diff_to_confidence_y: [u32; 9],
    pub endpoint_check_enabled: bool,
    pub memory_gating_enabled: bool,
}

impl Default for Vdpp2EsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            angle_delta: 17,
            angle_delta_extra: 5,
            gradient_no_direction_threshold: 37,
            gradient_flat_threshold: 75,
            weight_gain: 128,
            weight_decay: 128,
            low_confidence_threshold: 96,
            low_confidence_ratio: 32,
            confidence_count_threshold: 4,
            weight_local_threshold: 64,
            diff_gain0: 4096,
            diff_gain1: 7168,
            diff_limit: 65_280,
            diff_to_confidence_x: [0, 1024, 2048, 3072, 4096, 6144, 8192, 12_288, 65_535],
            diff_to_confidence_y: [0, 84, 141, 179, 204, 233, 246, 253, 255],
            endpoint_check_enabled: true,
            memory_gating_enabled: false,
        }
    }
}

pub(super) fn build_dmsr(config: &Vdpp2DmsrConfig) -> [u32; DMSR_WORDS] {
    let mut words = [0u32; DMSR_WORDS];
    for index in 0..7 {
        words[index] = field(config.edge_low_thresholds[index], 0, 16)
            | field(config.edge_high_thresholds[index], 16, 16);
    }

    let edge_k: [u16; 7] = std::array::from_fn(|index| {
        let threshold = config.edge_high_thresholds[index]
            .wrapping_sub(config.edge_low_thresholds[index]) as u16;
        65_535 / u16::from(threshold.max(1))
    });
    words[7] = field(u32::from(edge_k[0]), 0, 16) | field(u32::from(edge_k[1]), 16, 16);
    words[8] = field(u32::from(edge_k[2]), 0, 16) | field(u32::from(edge_k[3]), 16, 16);
    words[9] = field(u32::from(edge_k[4]), 0, 16) | field(u32::from(edge_k[5]), 16, 16);

    let x_delta = config
        .contrast_confidence_x1
        .wrapping_sub(config.contrast_confidence_x0);
    let y_delta = config
        .contrast_confidence_y1
        .wrapping_sub(config.contrast_confidence_y0);
    let contrast_to_confidence = y_delta
        .wrapping_mul(256)
        .checked_div(x_delta.max(1))
        .unwrap_or(0)
        .min(65_535);
    words[10] = field(u32::from(edge_k[6]), 0, 16) | field(contrast_to_confidence, 16, 16);

    words[11] =
        field(config.contrast_confidence_x0, 0, 16) | field(config.contrast_confidence_x1, 16, 16);
    words[12] =
        field(config.contrast_confidence_y0, 0, 16) | field(config.contrast_confidence_y1, 16, 16);
    words[13] = field(config.block_flat_threshold, 0, 16);
    words[14] = field(config.diff_core_threshold0, 0, 8) | field(config.diff_core_threshold1, 8, 8);
    words[15] = field(config.diff_core_weight0, 0, 6)
        | field(config.diff_core_weight1, 8, 6)
        | field(config.diff_core_weight2, 16, 6);

    let diff_core_y0 = config
        .diff_core_threshold0
        .wrapping_mul(config.diff_core_weight0) as u16;
    let diff_core_y1 = config
        .diff_core_threshold1
        .wrapping_sub(config.diff_core_threshold0)
        .wrapping_mul(config.diff_core_weight1)
        .wrapping_add(u32::from(diff_core_y0)) as u16;
    words[16] = field(u32::from(diff_core_y0), 0, 14) | field(u32::from(diff_core_y1), 16, 14);
    words[17] = field(config.primary_weight_odd_1, 0, 6)
        | field(config.primary_weight_even_1, 8, 6)
        | field(config.primary_weight_odd_2, 16, 6)
        | field(config.primary_weight_even_2, 24, 6);
    words[18] =
        field(config.secondary_weight, 0, 6) | field(config.secondary_weight.wrapping_mul(2), 8, 6);
    words[19] = field(config.strength_primary, 0, 5)
        | field(config.strength_secondary, 8, 5)
        | field(config.dumping, 16, 4);
    words
}

pub(super) fn build_es(config: &Vdpp2EsConfig) -> [u32; ES_WORDS] {
    let local_mean_threshold = if config.weight_gain == 0 {
        0
    } else {
        (256.0f32 * config.weight_local_threshold as f32 / config.weight_gain as f32)
            .clamp(0.0, 255.0) as u32
    };
    let angle_low = 23u32
        .wrapping_sub(config.angle_delta)
        .wrapping_sub(config.angle_delta_extra);
    let angle_high = 23u32.wrapping_add(config.angle_delta);
    let tan_low = tangent_threshold(angle_low);
    let tan_high = tangent_threshold(angle_high);

    let mut words = [0u32; ES_WORDS];
    words[0] = u32::from(config.enabled);
    words[1] = field(config.gradient_flat_threshold, 0, 8)
        | field(config.gradient_no_direction_threshold, 8, 8);
    words[2] = field(tan_high, 0, 9) | field(tan_low, 16, 9);
    words[3] = field(u32::from(config.endpoint_check_enabled), 0, 1)
        | field(u32::from(config.memory_gating_enabled), 8, 1);
    words[4] = field(config.diff_gain0, 0, 16) | field(config.diff_limit, 16, 16);
    words[5] = field(config.diff_to_confidence_x[0], 0, 16) | field(config.diff_gain1, 16, 16);
    words[6] = field(config.diff_to_confidence_x[2], 0, 16)
        | field(config.diff_to_confidence_x[1], 16, 16);
    words[7] = field(config.diff_to_confidence_x[4], 0, 16)
        | field(config.diff_to_confidence_x[3], 16, 16);
    words[8] = field(config.diff_to_confidence_x[6], 0, 16)
        | field(config.diff_to_confidence_x[5], 16, 16);
    words[9] = field(config.diff_to_confidence_x[8], 0, 16)
        | field(config.diff_to_confidence_x[7], 16, 16);
    for (index, output) in words[10..12].iter_mut().enumerate() {
        let base = index * 4;
        *output = field(config.diff_to_confidence_y[base], 0, 8)
            | field(config.diff_to_confidence_y[base + 1], 8, 8)
            | field(config.diff_to_confidence_y[base + 2], 16, 8)
            | field(config.diff_to_confidence_y[base + 3], 24, 8);
    }
    words[12] = field(config.diff_to_confidence_y[8], 0, 8);

    let slopes: [u32; 8] = std::array::from_fn(|index| {
        let x0 = f64::from(config.diff_to_confidence_x[index]);
        let x1 = f64::from(config.diff_to_confidence_x[index + 1]);
        let y0 = f64::from(config.diff_to_confidence_y[index]);
        let y1 = f64::from(config.diff_to_confidence_y[index + 1]);
        let slope = (256.0 * (y1 - y0) / (x1 - x0)).floor();
        if slope.is_finite() {
            slope.clamp(0.0, 255.0) as u32
        } else {
            255
        }
    });
    words[13] = field(slopes[0], 0, 8)
        | field(slopes[1], 8, 8)
        | field(slopes[2], 16, 8)
        | field(slopes[3], 24, 8);
    words[14] = field(slopes[4], 0, 8)
        | field(slopes[5], 8, 8)
        | field(slopes[6], 16, 8)
        | field(slopes[7], 24, 8);
    words[15] = field(config.weight_decay, 0, 8) | field(config.weight_gain, 8, 8);
    words[16] = field(local_mean_threshold, 0, 8)
        | field(config.confidence_count_threshold, 8, 4)
        | field(config.low_confidence_ratio, 16, 8)
        | field(config.low_confidence_threshold, 24, 8);
    words
}

fn tangent_threshold(angle: u32) -> u32 {
    ((f64::from(angle) * PI / 180.0).tan() * 512.0).clamp(0.0, 511.0) as u32
}

const fn field(value: u32, shift: u32, width: u32) -> u32 {
    (value & ((1u32 << width) - 1)) << shift
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hal::vdpp2_defaults::{DEFAULT_DMSR, DEFAULT_ES};

    #[test]
    fn default_dmsr_matches_the_vendor_register_trace() {
        assert_eq!(build_dmsr(&Vdpp2DmsrConfig::default()), DEFAULT_DMSR);
    }

    #[test]
    fn default_es_matches_the_vendor_register_trace() {
        assert_eq!(build_es(&Vdpp2EsConfig::default()), DEFAULT_ES);
    }

    #[test]
    fn custom_dmsr_and_es_fields_are_packed() {
        let mut dmsr = Vdpp2DmsrConfig::default();
        dmsr.edge_low_thresholds[0] = 2;
        dmsr.edge_high_thresholds[0] = 6;
        dmsr.strength_primary = 31;
        dmsr.strength_secondary = 30;
        dmsr.dumping = 15;
        let dmsr_words = build_dmsr(&dmsr);
        assert_eq!(dmsr_words[0], 0x0006_0002);
        assert_eq!(dmsr_words[7] & 0xffff, 0x3fff);
        assert_eq!(dmsr_words[19], 0x000f_1e1f);

        let mut es = Vdpp2EsConfig::default();
        es.enabled = true;
        es.gradient_no_direction_threshold = 0x55;
        es.gradient_flat_threshold = 0xaa;
        es.diff_gain0 = 0x1234;
        es.diff_limit = 0x5678;
        let es_words = build_es(&es);
        assert_eq!(es_words[0], 1);
        assert_eq!(es_words[1], 0x0000_55aa);
        assert_eq!(es_words[4], 0x5678_1234);
    }
}
