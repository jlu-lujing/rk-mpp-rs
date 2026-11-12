use rk_mpp_rs::hal::vdpp2::{Vdpp2Config, Vdpp2Job, Vdpp2Registers};

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
    let config = if args.get(5).map(String::as_str) == Some("hist") {
        let mut config = Vdpp2Config::nv12_to_yuv444(src_width, src_height, dst_width, dst_height);
        config.mode = rk_mpp_rs::hal::vdpp2::Vdpp2Mode::Histogram;
        config.histogram.enable = true;
        config
    } else if args.get(5).map(String::as_str) == Some("420") {
        Vdpp2Config::nv12_to_nv12(src_width, src_height, dst_width, dst_height)
    } else {
        Vdpp2Config::nv12_to_yuv444(src_width, src_height, dst_width, dst_height)
    };
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
