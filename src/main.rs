//! Probe: does the pure-Rust MPP client reach the JPEG decoder?

use rk_mpp_rs::codec::jpeg::parser as jpeg;
use rk_mpp_rs::codec::jpeg::tables;
use rk_mpp_rs::hal::jpegd as decode;
use rk_mpp_rs::scale;
use rk_mpp_rs::service::dma_heap;
use rk_mpp_rs::service::MppService;

use rk_mpp_rs::service::*;
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

fn bench_dma() {
    let n = 640 * 640 * 3;
    let mut buf = dma_heap::DmaBuf::alloc("system", n, false).unwrap();
    for (i, b) in buf.as_mut_slice().iter_mut().enumerate() {
        *b = (i & 0xff) as u8;
    }
    let mut dst = vec![0u8; n];
    let iters = 300;
    let t = Instant::now();
    for _ in 0..iters {
        dst.copy_from_slice(&buf.as_slice()[..n]);
    }
    let plain = us(t.elapsed()) / iters as f64 / 1000.0;
    let t = Instant::now();
    for _ in 0..iters {
        let _ = buf.sync_read_begin();
        dst.copy_from_slice(&buf.as_slice()[..n]);
        let _ = buf.sync_read_end();
    }
    let synced = us(t.elapsed()) / iters as f64 / 1000.0;
    let t = Instant::now();
    for _ in 0..iters {
        let _ = buf.sync_read_begin();
        let _ = buf.sync_read_end();
    }
    let sync_only = us(t.elapsed()) / iters as f64 / 1000.0;
    let mut sc = scale::Scaler::new(640, 640);
    let mut dst2 = vec![0u8; n];
    sc.letterbox(buf.as_slice(), 640, 640, &mut dst2);
    let t = Instant::now();
    for _ in 0..iters {
        sc.letterbox(buf.as_slice(), 640, 640, &mut dst2);
    }
    let scale_dma = us(t.elapsed()) / iters as f64 / 1000.0;
    let mut src_vec = vec![0u8; n];
    src_vec.copy_from_slice(&buf.as_slice()[..n]);
    let t = Instant::now();
    for _ in 0..iters {
        sc.letterbox(&src_vec, 640, 640, &mut dst2);
    }
    let scale_vec = us(t.elapsed()) / iters as f64 / 1000.0;
    println!("  scaler: from dma {:.3} ms, from Vec {:.3} ms", scale_dma, scale_vec);
    println!(
        "dma-heap system, {:.2} MB: copy {:.3} ms, copy+sync {:.3} ms, sync only {:.3} ms",
        n as f64 / 1e6,
        plain,
        synced,
        sync_only
    );
}

fn pipeline(path: &str, iters: usize, out_path: &str) -> Result<(f64, f64, f64), String> {
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

    let mut scaler = scale::Scaler::new(640, 640);
    let mut model_in = vec![0u8; 640 * 640 * 3];

    // warmup
    for _ in 0..5 {
        svc.submit_regs(&regs.words).map_err(|e| e.to_string())?;
        svc.poll_hw_finish().map_err(|e| e.to_string())?;
        scaler.letterbox_strided(
            out_buf.as_slice(),
            geom.stride_bytes,
            geom.out_width,
            geom.out_height,
            &mut model_in,
        );
    }

    let mut t_dec = 0.0;
    let mut t_sca = 0.0;
    let mut t_sync = 0.0;
    for _ in 0..iters {
        let a = Instant::now();
        svc.submit_regs(&regs.words).map_err(|e| e.to_string())?;
        svc.poll_hw_finish().map_err(|e| e.to_string())?;
        let b = Instant::now();
        // The decoder wrote through the IOMMU; invalidate before CPU reads.
        let sb = Instant::now();
        let _ = out_buf.sync_read_begin();
        let se = Instant::now();
        scaler.letterbox_strided(
            out_buf.as_slice(),
            geom.stride_bytes,
            geom.out_width,
            geom.out_height,
            &mut model_in,
        );
        let _ = out_buf.sync_read_end();
        let c = Instant::now();
        t_dec += us(b - a) / 1000.0;
        t_sca += us(c - b) / 1000.0;
        t_sync += us(se - sb) / 1000.0;
    }
    if !out_path.is_empty() {
        std::fs::write(out_path, &model_in).map_err(|e| e.to_string())?;
    }
    let n = iters as f64;
    if std::env::var("RKMPP_DEBUG").is_ok() {
        eprintln!("  of which sync_begin {:.3} ms", t_sync / n);
    }
    Ok((t_dec / n, t_sca / n, (t_dec + t_sca) / n))
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
    if args.len() > 1 && args[1] == "bench-dma" {
        bench_dma();
        return;
    }
    if args.len() > 1 && args[1] == "pipeline" {
        let path = args.get(2).map(|s| s.as_str()).unwrap_or("frame.jpg");
        let iters: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(200);
        let out = args.get(4).map(|s| s.as_str()).unwrap_or("");
        match pipeline(path, iters, out) {
            Ok((dec, sca, tot)) => println!(
                "pipeline {} : decode {:.3} ms + scale {:.3} ms = {:.3} ms ({:.0} fps)",
                path,
                dec,
                sca,
                tot,
                1000.0 / tot
            ),
            Err(e) => println!("pipeline failed: {e}"),
        }
        return;
    }
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
    if args.len() > 1 && args[1] == "h264dec" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("/tmp/out.yuv");
        if src.is_empty() {
            println!("usage: rk-mpp-rs h264dec <in.264> <out.yuv>");
            return;
        }
        let data = std::fs::read(src).expect("read stream");
        let aus = rk_mpp_rs::codec::h264::decoder::split_access_units(&data);
        println!("{} access units", aus.len());
        let mut dec = match rk_mpp_rs::codec::h264::Decoder::new() {
            Ok(d) => d,
            Err(e) => {
                println!("open rkvdec failed: {e}");
                return;
            }
        };
        let mut out = std::io::BufWriter::new(std::fs::File::create(dst).expect("create out"));
        use std::io::Write;
        let mut n = 0usize;
        let t = Instant::now();
        for (i, au) in aus.iter().enumerate() {
            let t0 = Instant::now();
            match dec.decode_access_unit(au) {
                _ if false => {}
                Ok(Some(pic)) => {
                    n += 1;
                    if i < 3 {
                        println!("    frame {i} submit+poll: {:.2} ms", t0.elapsed().as_secs_f64() * 1000.0);
                    }
                    out.write_all(&pic.yuv).expect("write frame");
                    if std::env::var("RFI_DUMP_UV").is_ok() {
                        std::fs::write("/tmp/raw_uv.bin", &pic.raw_uv).ok();
                    }
                    if i < 3 {
                        let y = &pic.yuv[..pic.width * pic.height];
                        let sum: u64 = y.iter().map(|v| *v as u64).sum();
                        let maxv = y.iter().copied().max().unwrap_or(0);
                        println!(
                            "  frame {i}: {}x{} y_mean={:.1} y_max={} status={:08x}/{:08x}/{:08x}",
                            pic.width,
                            pic.height,
                            sum as f64 / y.len() as f64,
                            maxv,
                            dec.status().0,
                            dec.status().1,
                            dec.status().2
                        );
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    println!("frame {i}: decode error: {e}");
                    break;
                }
            }
        }
        out.flush().ok();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        println!("decoded {n} frames in {ms:.1} ms ({:.1} fps)", n as f64 / (ms / 1000.0));
        return;
    }
    if args.len() > 1 && args[1] == "spspps" {
        let path = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let data = std::fs::read(path).expect("read");
        let nals = rk_mpp_rs::codec::h264::split_annex_b(&data);
        let sps = nals
            .iter()
            .find(|n| n.unit_type == 7)
            .map(|n| rk_mpp_rs::codec::h264::parse_sps(&n.rbsp).unwrap())
            .unwrap();
        let pps = nals
            .iter()
            .find(|n| n.unit_type == 8)
            .map(|n| rk_mpp_rs::codec::h264::parse_pps(&n.rbsp).unwrap())
            .unwrap();
        let block = rk_mpp_rs::codec::h264::hw::build_sps_pps(&sps, &pps, &[]);
        let hex: Vec<String> = block.iter().map(|b| format!("{b:02x}")).collect();
        println!("{}", hex.join(" "));
        return;
    }
    if args.len() > 1 && args[1] == "h264dpb" {
        let path = args.get(2).map(|s| s.as_str()).unwrap_or("");
        if path.is_empty() {
            println!("usage: rk-mpp-rs h264dpb <annexb.264>");
            return;
        }
        let data = std::fs::read(path).expect("read stream");
        let nals = rk_mpp_rs::codec::h264::split_annex_b(&data);
        use rk_mpp_rs::codec::h264::{Dpb, FrameStore};
        let mut sps_map: [Option<rk_mpp_rs::codec::h264::Sps>; 32] = Default::default();
        let mut pps_map: Vec<Option<rk_mpp_rs::codec::h264::Pps>> =
            (0..256).map(|_| None).collect();
        let mut dpb: Option<Dpb> = None;
        let mut out_order = Vec::new();
        for n in &nals {
            match n.unit_type {
                7 => {
                    let sp = rk_mpp_rs::codec::h264::parse_sps(&n.rbsp).expect("sps");
                    let idx = sp.sps_id as usize % 32;
                    sps_map[idx] = Some(sp);
                }
                8 => {
                    let pp = rk_mpp_rs::codec::h264::parse_pps(&n.rbsp).expect("pps");
                    let idx = pp.pps_id as usize;
                    pps_map[idx] = Some(pp);
                }
                1..=5 => {
                    let pid = match rk_mpp_rs::codec::h264::parser::slice_pps_id(&n.rbsp) {
                        Ok(v) => v as usize,
                        Err(_) => continue,
                    };
                    let pps = match pps_map.get(pid).and_then(|p| p.as_ref()) {
                        Some(p) => p,
                        None => continue,
                    };
                    let sps = match sps_map.get(pps.sps_id as usize % 32).and_then(|s| s.as_ref()) {
                        Some(s) => s,
                        None => continue,
                    };
                    if dpb.is_none() {
                        dpb = Some(Dpb::new(sps.max_num_ref_frames, 0));
                    }
                    let d = dpb.as_mut().unwrap();
                    let idr = n.unit_type == 5;
                    if idr {
                        d.clear();
                    }
                    let sh = match rk_mpp_rs::codec::h264::parse_slice_header(
                        &n.rbsp, sps, pps, n.unit_type, n.ref_idc,
                    ) {
                        Ok(sh) => sh,
                        Err(e) => {
                            println!(
                                "slice err: {e} (nal type={} ref_idc={} len={} first8={:02x?})",
                                n.unit_type, n.ref_idc, n.rbsp.len(), &n.rbsp[..n.rbsp.len().min(8)]
                            );
                            continue;
                        }
                    };
                    if sh.first_mb != 0 {
                        continue;
                    }
                    let fno = d.frame_num_offset(&sh, sps, idr, false);
                    let poc = d.compute_poc(&sh, sps, idr, fno);
                    d.set_frame_num_offset(fno);
                    let (l0, l1) = d.build_ref_lists(&sh, poc);
                    println!(
                        "fn={:<2} poc={:<4} type={} dpb={} mmco={} refs_l0={:?} l1={:?}",
                        sh.frame_num, poc, sh.slice_type, d.frames().len(), sh.mmco.len(), l0, l1
                    );
                    if n.ref_idc != 0 {
                        if d.frames().len() >= d.max_size {
                            d.sliding_window();
                        }
                        let slot = d.free_slot(16).unwrap_or(0);
                        d.push(FrameStore {
                            frame_num: sh.frame_num,
                            poc,
                            long_term: false,
                            long_term_frame_idx: 0,
                            slot,
                            buf: slot as usize,
                            referenced: true,
                        });
                        if !sh.mmco.is_empty() {
                            d.apply_mmco(&sh.mmco, sh.frame_num, 0);
                        }
                        d.update_poc_state(&sh, sps, poc);
                    }
                    out_order.push(poc);
                }
                _ => {}
            }
        }
        println!("output order by POC: {:?}", out_order);
        return;
    }
    if args.len() > 1 && args[1] == "h264" {
        let path = args.get(2).map(|s| s.as_str()).unwrap_or("");
        if path.is_empty() {
            println!("usage: rk-mpp-rs h264 <annexb.264>");
            return;
        }
        let data = std::fs::read(path).expect("read stream");
        let nals = rk_mpp_rs::codec::h264::split_annex_b(&data);
        let mut sps_v = Vec::new();
        let mut pps_v = Vec::new();
        let mut vcl = 0usize;
        for n in &nals {
            match n.unit_type {
                7 => {
                    let sps = rk_mpp_rs::codec::h264::parse_sps(&n.rbsp).expect("sps");
                    println!(
                        "SPS id={} profile={} level={} {}x{} refs={} poc_type={}",
                        sps.sps_id,
                        sps.profile_idc,
                        sps.level_idc,
                        sps.width,
                        sps.height,
                        sps.max_num_ref_frames,
                        sps.poc_type
                    );
                    sps_v.push(sps);
                }
                8 => {
                    let pps = rk_mpp_rs::codec::h264::parse_pps(&n.rbsp).expect("pps");
                    println!(
                        "PPS id={} sps={} entropy={} refs_l0={} deblock={} t8x8={} scqp={}",
                        pps.pps_id, pps.sps_id, pps.entropy_coding, pps.num_ref_idx_l0,
                        pps.deblocking_filter_control, pps.transform_8x8,
                        pps.second_chroma_qp_offset
                    );
                    pps_v.push(pps);
                }
                1..=5 => vcl += 1,
                _ => {}
            }
        }
        if let (Some(sps), Some(pps)) = (sps_v.first(), pps_v.first()) {
            for n in nals.iter().filter(|n| n.is_vcl()).take(3) {
                match rk_mpp_rs::codec::h264::parse_slice_header(
                    &n.rbsp,
                    sps,
                    pps,
                    n.unit_type,
                    n.ref_idc,
                ) {
                    Ok(h) => println!(
                        "slice type={} first_mb={} frame_num={} idr={} poc={:?}",
                        h.slice_type, h.first_mb, h.frame_num, h.idr_pic_id.is_some(), h.poc_lsb
                    ),
                    Err(e) => println!("slice parse: {e}"),
                }
            }
        }
        println!("NAL units: {} (vcl {vcl}), stream {} bytes", nals.len(), data.len());
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
                    for (c, name) in Client::all() {
                        let bit = *c as u32;
                        let has = (mask >> bit) & 1 == 1;
                        println!(
                            "    [{:>5}] {:<40} {}",
                            bit,
                            name,
                            if has { "present" } else { "-" }
                        );
                    }
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
