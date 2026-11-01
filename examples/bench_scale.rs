//! RGB letterbox row-throughput bench for the scale.rs row kernels.
//!
//! One line per case: `case <index> <name> <ms-per-frame>`. Frames are
//! synthesised; the destination is sized once and reused, so what is timed
//! is the scaler itself (the caller's job is to compare against the
//! `neon_disabled` twin: the same binary reports both, interleaved, so the
//! board script can take medians of each without a rebuild).

use std::hint::black_box;
use std::time::Instant;

use rk_mpp_rs::scale::Scaler;

struct Case {
    name: &'static str,
    sw: usize,
    sh: usize,
    dw: usize,
    dh: usize,
}

const CASES: &[Case] = &[
    Case {
        name: "1920x1080->640x640",
        sw: 1920,
        sh: 1080,
        dw: 640,
        dh: 640,
    },
    Case {
        name: "1280x720->640x640",
        sw: 1280,
        sh: 720,
        dw: 640,
        dh: 640,
    },
    Case {
        name: "640x480->640x640",
        sw: 640,
        sh: 480,
        dw: 640,
        dh: 640,
    },
    Case {
        name: "1930x1080->640x640",
        sw: 1930,
        sh: 1080,
        dw: 640,
        dh: 640,
    },
];

fn main() {
    let iters: usize = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let mut seed = 0x1234_5678_u32;
    let mut random = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 24) as u8
    };
    for case in CASES {
        let src: Vec<u8> = (0..case.sw * case.sh * 3).map(|_| random()).collect();
        let mut dst = vec![0u8; case.dw * case.dh * 3];
        let mut vector = Scaler::new(case.dw, case.dh);
        let mut scalar = Scaler::new(case.dw, case.dh);
        scalar.set_neon_disabled(true);
        // Interleave the two paths frame by frame, like the board decode
        // bench interleaves the two binaries: clock and cache state drift.
        let mut vector_ms = Vec::with_capacity(iters);
        let mut scalar_ms = Vec::with_capacity(iters);
        for _ in 0..iters {
            let start = Instant::now();
            vector.letterbox(black_box(&src), case.sw, case.sh, black_box(&mut dst));
            vector_ms.push(start.elapsed().as_secs_f64() * 1000.0);
            let start = Instant::now();
            scalar.letterbox(black_box(&src), case.sw, case.sh, black_box(&mut dst));
            scalar_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let median = |v: &mut Vec<f64>| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[v.len() / 2]
        };
        println!(
            "case {} vector_ms {:.4} scalar_ms {:.4}",
            case.name,
            median(&mut vector_ms),
            median(&mut scalar_ms)
        );
    }
}
