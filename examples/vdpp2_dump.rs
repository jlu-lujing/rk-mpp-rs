use rk_mpp_rs::hal::vdpp2::{
    Vdpp2Config, Vdpp2DmsrConfig, Vdpp2EsConfig, Vdpp2Job, Vdpp2Registers,
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
