//! Compare direct strided-NV12 scaling with the legacy
//! NV12 -> full RGB intermediate -> RGB scale path.

use std::hint::black_box;
use std::time::Instant;

use rk_mpp_rs::scale::Scaler;

struct Case {
    name: &'static str,
    sw: usize,
    sh: usize,
}

const CASES: &[Case] = &[
    Case {
        name: "1920x1080->640x640",
        sw: 1920,
        sh: 1080,
    },
    Case {
        name: "1280x720->640x640",
        sw: 1280,
        sh: 720,
    },
    Case {
        name: "640x480->640x640",
        sw: 640,
        sh: 480,
    },
    Case {
        name: "1930x1080->640x640",
        sw: 1930,
        sh: 1080,
    },
];

fn clamp(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

fn nv12_to_rgb(
    y_plane: &[u8],
    uv_plane: &[u8],
    stride: usize,
    width: usize,
    height: usize,
) -> Vec<u8> {
    let mut rgb = vec![0u8; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let y_value = y_plane[y * stride + x] as i32;
            let uv = (y / 2) * stride + (x / 2) * 2;
            let u = uv_plane[uv] as i32;
            let v = uv_plane[uv + 1] as i32;
            let c = (y_value - 16).max(0);
            let d = u - 128;
            let e = v - 128;
            let pixel = (y * width + x) * 3;
            rgb[pixel] = clamp((298 * c + 409 * e + 128) >> 8);
            rgb[pixel + 1] = clamp((298 * c - 100 * d - 208 * e + 128) >> 8);
            rgb[pixel + 2] = clamp((298 * c + 516 * d + 128) >> 8);
        }
    }
    rgb
}

fn main() {
    let iters: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(50);
    let mut state = 0x1234_5678_u32;
    let mut random = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 24) as u8
    };
    for case in CASES {
        let y_plane: Vec<u8> = (0..case.sw * case.sh).map(|_| random()).collect();
        let uv_plane: Vec<u8> = (0..case.sw * case.sh / 2).map(|_| random()).collect();
        let mut direct_dst = vec![0u8; 640 * 640 * 3];
        let mut copy_dst = vec![0u8; 640 * 640 * 3];
        let mut direct = Scaler::new(640, 640);
        let mut copy_scale = Scaler::new(640, 640);
        let mut direct_samples = Vec::with_capacity(iters);
        let mut copy_samples = Vec::with_capacity(iters);
        for _ in 0..iters {
            let start = Instant::now();
            direct.letterbox_nv12_strided(
                black_box(&y_plane),
                black_box(&uv_plane),
                case.sw,
                case.sw,
                case.sh,
                black_box(&mut direct_dst),
            );
            direct_samples.push(start.elapsed().as_secs_f64() * 1000.0);

            let start = Instant::now();
            let rgb = nv12_to_rgb(&y_plane, &uv_plane, case.sw, case.sw, case.sh);
            copy_scale.letterbox(black_box(&rgb), case.sw, case.sh, black_box(&mut copy_dst));
            copy_samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let median = |values: &mut Vec<f64>| {
            values.sort_by(|left, right| left.partial_cmp(right).unwrap());
            values[values.len() / 2]
        };
        println!(
            "case {} direct_ms {:.4} copy_scale_ms {:.4}",
            case.name,
            median(&mut direct_samples),
            median(&mut copy_samples)
        );
    }
}
