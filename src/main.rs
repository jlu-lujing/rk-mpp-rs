//! Probe: does the pure-Rust MPP client reach the JPEG decoder?

mod dma_heap;
mod mpp;
mod scale;

use mpp::*;
use std::time::Instant;

fn us(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

fn bench_scale(path: &str, sw: usize, sh: usize, iters: usize) {
    let src = std::fs::read(path).expect("read source");
    assert_eq!(src.len(), sw * sh * 3, "source size mismatch");
    let mut dst = vec![0u8; 640 * 640 * 3];
    let mut s = scale::Scaler::new(640, 640);

    s.letterbox(&src, sw, sh, &mut dst);
    let t = Instant::now();
    for _ in 0..iters {
        s.letterbox(&src, sw, sh, &mut dst);
    }
    let per = us(t.elapsed()) / iters as f64;
    println!(
        "scale {}x{} -> 640x640 letterbox: {:.3} ms/iter ({:.0} fps)",
        sw,
        sh,
        per / 1000.0,
        1e6 / per
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "bench-scale" {
        let path = args.get(2).map(|s| s.as_str()).unwrap_or("/tmp/src1080.rgb");
        let sw: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1920);
        let sh: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1080);
        let iters: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(200);
        bench_scale(path, sw, sh, iters);
        return;
    }
    println!("rk-mpp-rs probe");

    let t = Instant::now();
    let opened = MppService::open(VPU_CLIENT_JPEG_DEC);
    let t_open = us(t.elapsed());
    let t = Instant::now();
    match &opened {
        Ok(_) => {}
        Err(_) => {}
    }
    let _ = t.elapsed();

    match opened {
        Ok(svc) => {
            println!("  open + INIT_CLIENT_TYPE: {t_open:.1} us");
            println!("  session: client_type={}", svc.client_type());
            let t = Instant::now();
            let probe = svc.probe_hw_support();
            let t_probe = us(t.elapsed());
            let t2 = Instant::now();
            let hid = svc.hw_id();
            let t_id = us(t2.elapsed());
            println!("  probe_hw_support: {t_probe:.1} us   hw_id: {t_id:.1} us");
            match probe {
                Ok(mask) => {
                    println!("  hw support mask: 0x{mask:08x}");
                    println!(
                        "  jpeg dec present: {}",
                        (mask >> VPU_CLIENT_JPEG_DEC) & 1 == 1
                    );
                    println!(
                        "  rkvdec present:  {}",
                        (mask >> VPU_CLIENT_RKVDEC) & 1 == 1
                    );
                }
                Err(e) => println!("  probe_hw_support failed: {e}"),
            }
            match hid {
                Ok(id) => println!("  hw id: {id}"),
                Err(e) => println!("  hw_id failed: {e}"),
            }
        }
        Err(e) => println!("  open {MPP_SERVICE_PATH} failed: {e}"),
    }

    for heap in ["system", "system-uncached", "cma"] {
        match dma_heap::DmaBuf::alloc(heap, 1 << 20, false) {
            Ok(buf) => {
                // steady-state cost of a 1 MiB alloc+mmap+free cycle
                let n = 200;
                let t = Instant::now();
                for _ in 0..n {
                    let b = dma_heap::DmaBuf::alloc(heap, 1 << 20, false).unwrap();
                    drop(b);
                }
                let per = us(t.elapsed()) / n as f64;
                println!(
                    "  dma-heap {heap}: {} bytes fd={}  alloc+mmap+free {:.1} us/cycle",
                    buf.len(),
                    buf.fd(),
                    per
                );
            }
            Err(e) => println!("  dma-heap {heap}: {e}"),
        }
    }
}
