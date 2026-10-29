//! Probe: does the pure-Rust MPP client reach the JPEG decoder?

mod dma_heap;
mod decode;
mod jpeg;
mod jpeg_regs;
mod mpp;
mod scale;
mod tables;

use mpp::*;
use std::time::Instant;

fn us(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

fn decode_to_rgb(path: &str, out_path: &str) -> Result<(usize, usize, usize), String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let info = jpeg::parse(&data).map_err(|e| e.to_string())?;
    let tables = tables::Tables::build(&info).map_err(|e| format!("{e:?}"))?;

    let svc = MppService::open(VPU_CLIENT_JPEG_DEC).map_err(|e| e.to_string())?;

    // table buffer, stream buffer, output buffer
    let mut table_buf = dma_heap::DmaBuf::alloc("system", tables::RKD_TABLE_SIZE, false)
        .map_err(|e| e.to_string())?;
    table_buf.as_mut_slice()[..tables::RKD_TABLE_SIZE].copy_from_slice(tables.as_slice());

    let mut stream_buf = dma_heap::DmaBuf::alloc("system", data.len(), false)
        .map_err(|e| e.to_string())?;
    stream_buf.as_mut_slice()[..data.len()].copy_from_slice(&data);

    // Worst case RGB888: align(width,16) aligned height, 3 bytes per pixel.
    let aw = (info.width + 15) / 16 * 16;
    let ah = (info.height + 15) / 8 * 8;
    let mut out_buf = dma_heap::DmaBuf::alloc("system", aw * ah * 3, false)
        .map_err(|e| e.to_string())?;

    let (regs, geom) = decode::build(&decode::Job {
        info: &info,
        table_fd: table_buf.fd(),
        stream_fd: stream_buf.fd(),
        stream_len: data.len(),
        output_fd: out_buf.fd(),
        rgb: std::env::var("RKMPP_NV12").is_err(),
    })
    .map_err(|e| e.to_string())?;

    svc.submit_regs(&regs.words).map_err(|e| format!("submit: {e}"))?;
    svc.poll_hw_finish().map_err(|e| format!("poll: {e}"))?;

    let n = geom.frame_size.min(out_buf.len());
    std::fs::write(out_path, &out_buf.as_slice()[..n]).map_err(|e| e.to_string())?;
    Ok((geom.out_width, geom.out_height, geom.stride_bytes))
}

fn bench_decode(path: &str, iters: usize) -> Result<(f64, usize, usize), String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let info = jpeg::parse(&data).map_err(|e| e.to_string())?;
    let tables = tables::Tables::build(&info).map_err(|e| format!("{e:?}"))?;
    let svc = MppService::open(VPU_CLIENT_JPEG_DEC).map_err(|e| e.to_string())?;

    let mut table_buf = dma_heap::DmaBuf::alloc("system", tables::RKD_TABLE_SIZE, false)
        .map_err(|e| e.to_string())?;
    table_buf.as_mut_slice()[..tables::RKD_TABLE_SIZE].copy_from_slice(tables.as_slice());
    let mut stream_buf =
        dma_heap::DmaBuf::alloc("system", data.len(), false).map_err(|e| e.to_string())?;
    stream_buf.as_mut_slice()[..data.len()].copy_from_slice(&data);

    let aw = (info.width + 15) / 16 * 16;
    let ah = (info.height + 15) / 8 * 8;
    let out_buf = dma_heap::DmaBuf::alloc("system", aw * ah * 3, false)
        .map_err(|e| e.to_string())?;

    let (regs, geom) = decode::build(&decode::Job {
        info: &info,
        table_fd: table_buf.fd(),
        stream_fd: stream_buf.fd(),
        stream_len: data.len(),
        output_fd: out_buf.fd(),
        rgb: true,
    })
    .map_err(|e| e.to_string())?;

    // warmup
    for _ in 0..5 {
        svc.submit_regs(&regs.words).map_err(|e| e.to_string())?;
        svc.poll_hw_finish().map_err(|e| e.to_string())?;
    }
    let t = Instant::now();
    for _ in 0..iters {
        svc.submit_regs(&regs.words).map_err(|e| e.to_string())?;
        svc.poll_hw_finish().map_err(|e| e.to_string())?;
    }
    let ms = us(t.elapsed()) / iters as f64 / 1000.0;
    Ok((ms, geom.out_width, geom.out_height))
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
    if args.len() > 1 && args[1] == "bench-decode" {
        let path = args.get(2).map(|s| s.as_str()).unwrap_or("frame.jpg");
        let iters: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(200);
        match bench_decode(path, iters) {
            Ok((ms, w, h)) => println!(
                "decode {} : {:.3} ms/frame ({:.0} fps) at {}x{}",
                path,
                ms,
                1000.0 / ms,
                w,
                h
            ),
            Err(e) => println!("decode failed: {e}"),
        }
        return;
    }
    if args.len() > 1 && args[1] == "decode" {
        let path = args.get(2).map(|s| s.as_str()).unwrap_or("frame.jpg");
        let out = args.get(3).map(|s| s.as_str()).unwrap_or("/tmp/rust_dec.rgb");
        match decode_to_rgb(path, out) {
            Ok((w, h, stride)) => println!("decoded {path} -> {out} ({w}x{h}, stride {stride} bytes)"),
            Err(e) => println!("decode failed: {e}"),
        }
        return;
    }
    if args.len() > 1 && args[1] == "jpeg-info" {
        let path = args.get(2).map(|s| s.as_str()).unwrap_or("frame.jpg");
        match std::fs::read(path) {
            Ok(d) => match jpeg::parse(&d) {
                Ok(i) => {
                    println!(
                        "{}: {}x{} comps={} sampling={:?} progressive={} restart={} entropy={}..{}",
                        path,
                        i.width,
                        i.height,
                        i.components.len(),
                        i.luma_sampling(),
                        i.progressive,
                        i.restart_interval,
                        i.entropy_start,
                        i.entropy_end
                    );
                    for (k, c) in i.components.iter().enumerate() {
                        println!("  comp {k}: id={} h={} v={} tq={} td={} ta={}", c.id, c.h, c.v, c.tq, c.td, c.ta);
                    }
                }
                Err(e) => println!("{path}: {e}"),
            },
            Err(e) => println!("{path}: {e}"),
        }
        return;
    }
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
