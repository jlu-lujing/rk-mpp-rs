//! Probe: does the pure-Rust MPP client reach the JPEG decoder?

use rk_mpp_rs::codec::h264::encoder as h264_syntax;
use rk_mpp_rs::codec::h265::encoder as h265_syntax;
use rk_mpp_rs::codec::jpeg::parser as jpeg;
use rk_mpp_rs::codec::jpeg::tables;
use rk_mpp_rs::hal::h264e;
use rk_mpp_rs::hal::h265e;
use rk_mpp_rs::hal::h265e_session;
use rk_mpp_rs::hal::jpegd as decode;
use rk_mpp_rs::hal::jpege;
use rk_mpp_rs::scale;
use rk_mpp_rs::service::buffer_pool::BufferPool;
use rk_mpp_rs::service::dma_heap;
use rk_mpp_rs::service::MppService;

use rk_mpp_rs::service::*;
use std::time::Instant;

fn us(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

fn encode_h264(
    path: &str,
    out_path: &str,
    width: usize,
    height: usize,
    qp_init: u8,
    quality_auto: bool,
    profile: &str,
    frames: usize,
    gop: u32,
    bps: Option<(h264_syntax::RcMode, i64, i64, i64)>,
    deblock: Option<(u32, i32, i32)>,
    rois: Vec<h264_syntax::RoiRegion>,
    gdr: Option<(h264_syntax::GdrMode, u32)>,
    split: Option<(h264_syntax::SplitMode, u32)>,
) -> Result<(usize, usize, usize, f64), String> {
    // The frames are read one at a time. slurping the file was fine while the
    // encoder only ever produced a single IDR, but with a GOP it charges the
    // whole clip to the encode: 12 frames of 1920x1088 is 37 MB read and held
    // live to be used 3 MB at a time, which was most of the gap between the
    // loop's own 58 ms and the command's 135 ms in the throughput measurement.
    let mut source =
        std::io::BufReader::new(std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?);
    let mut config = h264_syntax::EncoderConfig::new(width, height);
    // The SPS level is normally derived from the size, which is what MPP's own
    // `h264e_sps_init` does. This override exists for the level matrix that proves
    // the derivation against the reference at sizes where the bump would hide it.
    if let Ok(level) = std::env::var("RKMPP_LEVEL") {
        match level.parse::<u8>() {
            Ok(level) => config.level_idc = level,
            Err(_) => {
                eprintln!("RKMPP_LEVEL must be an SPS level idc, e.g. 40");
                std::process::exit(2);
            }
        }
    }
    config.qp_init = qp_init;
    config.quality_auto = quality_auto;
    config.gop = gop.max(1);
    // Same mapping `mpi_enc_test` uses: CABAC from Main upwards, transform-8x8
    // only at High.
    match profile {
        "baseline" | "" => {}
        "main" => {
            config.profile = h264_syntax::PROFILE_MAIN;
            config.entropy_coding = true;
        }
        "high" => {
            config.profile = h264_syntax::PROFILE_HIGH;
            config.entropy_coding = true;
            config.transform_8x8 = true;
        }
        other => {
            return Err(format!(
                "unknown profile {other}; expected baseline, main or high"
            ))
        }
    }
    match bps {
        Some((mode, target, min, max)) => {
            // `-bps`, like `mpi_enc_test`, is a request for rate control rather than
            // a label on a fixed-QP stream: the mode changes with it.
            config.rc_mode = mode;
            config.bps_target = target;
            config.bps_min = min;
            config.bps_max = max;
        }
        // Unset `-bps` is the harness's own derivation, bounds and all: the target
        // comes from the frame size and the bounds stay at `h264e_api_v2.c`'s
        // defaults. A fixed-QP encode ignores all three for quantisation but
        // prints them, so this cannot be skipped just because it is rate control.
        None => config.bps_target = h264_syntax::default_bps_target(width, height, config.fps_num),
    }
    if let Some((disable, alpha, beta)) = deblock {
        // The fields reach the slice header through the two writes in
        // `setup_vepu580_codec`; beta configures but is not written, as in C.
        config.deblock_disable = disable;
        config.deblock_offset_alpha = alpha;
        config.deblock_offset_beta = beta;
    }
    config.rois = rois;
    if let Some((mode, num)) = gdr {
        // `rc:refresh_en/refresh_mode/refresh_num`; the length the encoder
        // derives per the `h264e_proc_cfg` rule, not a caller figure.
        config.gdr = true;
        config.gdr_mode = mode;
        config.gdr_num = num;
    }
    if let Some((mode, arg)) = split {
        // `split:mode/split:arg` of `MppEncSliceSplit`; the output mode stays
        // at the reference's zero, one AU per picture with the slices inside.
        config.split_mode = mode;
        config.split_arg = arg;
    }
    let frame_bytes = h264e::input_size(&config)?;
    let mut encoder = h264e::Vepu580H264::open(config)?;
    let started = Instant::now();
    // Repeated pictures, which is what the C harness does with a one-frame raw
    // file too: every frame it wants and the file is short, it re-reads the same
    // bytes. `frames` > 1 exercises the P path against that reference.
    // The frame stride comes from the encoder geometry, not from the file's
    // length: dividing the input by the frame count silently mis-slices every
    // picture whenever the file holds more or fewer frames than asked for.
    let per_frame = frame_bytes;
    let wanted = frames.max(1);
    let mut frame = vec![0u8; per_frame];
    let mut read_frame = |buffer: &mut [u8]| -> Result<bool, String> {
        use std::io::Read;
        let mut filled = 0usize;
        while filled < buffer.len() {
            match source.read(&mut buffer[filled..]) {
                Ok(0) => return Ok(false),
                Ok(n) => filled += n,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(true)
    };
    let mut annex_b = Vec::new();
    let mut headers = 0usize;
    let mut payload = 0usize;
    for index in 0..wanted {
        if !read_frame(&mut frame)? {
            return Err(format!(
                "{path}: short at frame {}, need {} bytes each",
                index + 1,
                per_frame
            ));
        }
        let encoded = encoder.encode_frame(&frame)?;
        headers += encoded.headers.len();
        payload += encoded.payload.len();
        annex_b.extend_from_slice(&encoded.annex_b);
    }
    let elapsed = started.elapsed();
    std::fs::write(out_path, &annex_b).map_err(|e| format!("{out_path}: {e}"))?;
    Ok((annex_b.len(), headers, payload, us(elapsed) / 1000.0))
}

/// Write the H.265 VPS/SPS/PPS a fixed-QP IDR carries, and report the QP that the
/// derived register image would send. The input file is only checked to exist and
/// hold a picture; the parameter sets are pure software output and do not read it.
/// The IDR NAL itself still needs the on-board job template (see hal::h265e), so it
/// is deliberately not produced here yet.
fn encode_h265_headers(
    path: &str,
    out_path: &str,
    width: usize,
    height: usize,
    qp: u32,
) -> Result<(usize, u32), String> {
    let input = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let frame_bytes = width * height * 3 / 2;
    if input.len() < frame_bytes {
        return Err(format!(
            "{path}: {width}x{height} NV12 needs {frame_bytes} bytes, file has {}",
            input.len()
        ));
    }
    let config = h265_syntax::EncoderConfig::new(width, height);
    let headers = h265_syntax::build_headers(&config).map_err(|e| e.to_string())?;
    std::fs::write(out_path, &headers).map_err(|e| format!("{out_path}: {e}"))?;
    let job = h265e::IdrJob {
        width,
        height,
        qp,
        qp_min: qp,
        qp_max: qp,
        picture: h265e::Picture::Idr,
        long_term: false,
        rc: None,
        refresh: None,
        input_fd: -1,
        output_fd: -1,
        current_fd: -1,
        current_thumb_fd: -1,
        reference_fd: -1,
        reference_thumb_fd: -1,
    };
    let registers = h265e::build_registers(&job)?;
    let pic_qp = registers
        .get(&rk_mpp_rs::hal::h265e_regs::BASE_ENC_PIC_PIC_QP)
        .map_err(|e| e.to_string())?;
    Ok((headers.len(), pic_qp))
}

fn encode_jpeg(
    path: &str,
    out_path: &str,
    width: usize,
    height: usize,
    quality: jpege::Quality,
    input_format: jpege::InputFormat,
    chroma: jpege::ChromaFormat,
    rotation: jpege::Rotation,
    split_mcus: Option<u16>,
) -> Result<(usize, f64), String> {
    let input = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let config = jpege::EncoderConfig {
        quality,
        chroma,
        rotation,
        split_mcus,
        ..jpege::EncoderConfig::new_for_input(width, height, input_format)
    };
    config.validate()?;
    let (expected_width, expected_height) = config.output_size();
    let mut encoder = jpege::Vepu2Jpeg::open().map_err(|e| format!("open JPEG encoder: {e}"))?;
    let started = Instant::now();
    let encoded = encoder.encode(&input, config)?;
    let elapsed = started.elapsed();
    let info = jpeg::parse(&encoded).map_err(|e| format!("encoded stream is invalid: {e}"))?;
    if info.width != expected_width || info.height != expected_height {
        return Err(format!(
            "encoded dimensions are {}x{}, expected {expected_width}x{expected_height}",
            info.width, info.height
        ));
    }
    std::fs::write(out_path, &encoded).map_err(|e| format!("{out_path}: {e}"))?;
    Ok((encoded.len(), us(elapsed) / 1000.0))
}

fn bench_encode_nv12(
    path: &str,
    width: usize,
    height: usize,
    iters: usize,
    quality: jpege::Quality,
) -> Result<(f64, usize), String> {
    let input = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let config = jpege::EncoderConfig {
        quality,
        chroma: jpege::ChromaFormat::Yuv420,
        ..jpege::EncoderConfig::new(width, height)
    };
    let mut encoder = jpege::Vepu2Jpeg::open().map_err(|e| format!("open JPEG encoder: {e}"))?;
    for _ in 0..5 {
        encoder.encode_nv12(&input, config)?;
    }

    let started = Instant::now();
    let mut stream_len = 0;
    for _ in 0..iters {
        stream_len = encoder.encode_nv12(&input, config)?.len();
    }
    let per = us(started.elapsed()) / iters as f64 / 1000.0;
    Ok((per, stream_len))
}

fn decode_to_rgb(
    path: &str,
    out_path: &str,
    downscale: decode::JpegDownscale,
) -> Result<(usize, usize, usize), String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let info = jpeg::parse(&data).map_err(|e| e.to_string())?;
    let tables = tables::Tables::build(&info).map_err(|e| format!("{e:?}"))?;
    let rgb = std::env::var("RKMPP_NV12").is_err();
    let output_geometry = decode::geometry(&info, rgb, downscale).map_err(|e| e.to_string())?;

    let svc = MppService::open(VPU_CLIENT_JPEG_DEC).map_err(|e| e.to_string())?;

    // table buffer, stream buffer, output buffer
    let mut table_buf = dma_heap::DmaBuf::alloc("system", tables::RKD_TABLE_SIZE, false)
        .map_err(|e| e.to_string())?;
    table_buf.as_mut_slice()[..tables::RKD_TABLE_SIZE].copy_from_slice(tables.as_slice());

    let mut stream_buf =
        dma_heap::DmaBuf::alloc("system", data.len(), false).map_err(|e| e.to_string())?;
    stream_buf.as_mut_slice()[..data.len()].copy_from_slice(&data);

    let out_buf = dma_heap::DmaBuf::alloc("system", output_geometry.frame_size, false)
        .map_err(|e| e.to_string())?;

    let (regs, geom) = decode::build(&decode::Job {
        info: &info,
        table_fd: table_buf.fd(),
        stream_fd: stream_buf.fd(),
        stream_len: data.len(),
        output_fd: out_buf.fd(),
        rgb,
        downscale,
    })
    .map_err(|e| e.to_string())?;

    svc.submit_regs(&regs.words)
        .map_err(|e| format!("submit: {e}"))?;
    svc.poll_hw_finish().map_err(|e| format!("poll: {e}"))?;

    out_buf
        .sync_read_begin()
        .map_err(|e| format!("dma-buf read sync: {e}"))?;
    let n = geom.frame_size.min(out_buf.len());
    let result = std::fs::write(out_path, &out_buf.as_slice()[..n]).map_err(|e| e.to_string());
    out_buf
        .sync_read_end()
        .map_err(|e| format!("dma-buf read sync end: {e}"))?;
    result?;
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
    println!(
        "  scaler: from dma {:.3} ms, from Vec {:.3} ms",
        scale_dma, scale_vec
    );
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
    let out_buf =
        dma_heap::DmaBuf::alloc("system", aw * ah * 3, false).map_err(|e| e.to_string())?;

    let (regs, geom) = decode::build(&decode::Job {
        info: &info,
        table_fd: table_buf.fd(),
        stream_fd: stream_buf.fd(),
        stream_len: data.len(),
        output_fd: out_buf.fd(),
        rgb: true,
        downscale: decode::JpegDownscale::Disabled,
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
        out_buf
            .sync_read_begin()
            .map_err(|e| format!("dma-buf read sync: {e}"))?;
        let se = Instant::now();
        scaler.letterbox_strided(
            out_buf.as_slice(),
            geom.stride_bytes,
            geom.out_width,
            geom.out_height,
            &mut model_in,
        );
        out_buf
            .sync_read_end()
            .map_err(|e| format!("dma-buf read sync end: {e}"))?;
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

fn bench_decode(
    path: &str,
    iters: usize,
    downscale: decode::JpegDownscale,
) -> Result<(f64, usize, usize), String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let info = jpeg::parse(&data).map_err(|e| e.to_string())?;
    let tables = tables::Tables::build(&info).map_err(|e| format!("{e:?}"))?;
    let output_geometry = decode::geometry(&info, true, downscale).map_err(|e| e.to_string())?;
    let svc = MppService::open(VPU_CLIENT_JPEG_DEC).map_err(|e| e.to_string())?;

    let mut table_buf = dma_heap::DmaBuf::alloc("system", tables::RKD_TABLE_SIZE, false)
        .map_err(|e| e.to_string())?;
    table_buf.as_mut_slice()[..tables::RKD_TABLE_SIZE].copy_from_slice(tables.as_slice());
    let mut stream_buf =
        dma_heap::DmaBuf::alloc("system", data.len(), false).map_err(|e| e.to_string())?;
    stream_buf.as_mut_slice()[..data.len()].copy_from_slice(&data);

    let out_buf = dma_heap::DmaBuf::alloc("system", output_geometry.frame_size, false)
        .map_err(|e| e.to_string())?;

    let (regs, geom) = decode::build(&decode::Job {
        info: &info,
        table_fd: table_buf.fd(),
        stream_fd: stream_buf.fd(),
        stream_len: data.len(),
        output_fd: out_buf.fd(),
        rgb: true,
        downscale,
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

/// One synchronous full-size RGB JPEG decode, the same ioctl sequence the
/// `decode` subcommand runs, returning the frame instead of writing it.
/// The async demo runs this once as the oracle its pooled jobs must match.
fn decode_job_sync(
    data: &[u8],
    info: &jpeg::JpegInfo,
    downscale: decode::JpegDownscale,
) -> Result<Vec<u8>, String> {
    let tables = tables::Tables::build(info).map_err(|e| format!("{e:?}"))?;
    let output_geometry = decode::geometry(info, true, downscale).map_err(|e| e.to_string())?;
    let svc = MppService::open(VPU_CLIENT_JPEG_DEC).map_err(|e| e.to_string())?;
    let mut table_buf = dma_heap::DmaBuf::alloc("system", tables::RKD_TABLE_SIZE, false)
        .map_err(|e| e.to_string())?;
    table_buf.as_mut_slice()[..tables::RKD_TABLE_SIZE].copy_from_slice(tables.as_slice());
    let mut stream_buf =
        dma_heap::DmaBuf::alloc("system", data.len(), false).map_err(|e| e.to_string())?;
    stream_buf.as_mut_slice()[..data.len()].copy_from_slice(data);
    let out_buf = dma_heap::DmaBuf::alloc("system", output_geometry.frame_size, false)
        .map_err(|e| e.to_string())?;
    let (regs, geom) = decode::build(&decode::Job {
        info,
        table_fd: table_buf.fd(),
        stream_fd: stream_buf.fd(),
        stream_len: data.len(),
        output_fd: out_buf.fd(),
        rgb: true,
        downscale,
    })
    .map_err(|e| e.to_string())?;
    svc.submit_regs(&regs.words)
        .map_err(|e| format!("submit: {e}"))?;
    svc.poll_hw_finish().map_err(|e| format!("poll: {e}"))?;
    out_buf
        .sync_read_begin()
        .map_err(|e| format!("dma-buf read sync: {e}"))?;
    let n = geom.frame_size.min(out_buf.len());
    let frame = out_buf.as_slice()[..n].to_vec();
    out_buf
        .sync_read_end()
        .map_err(|e| format!("dma-buf read sync end: {e}"))?;
    Ok(frame)
}

/// The worker-side async job: the identical ioctl sequence, but every block
/// comes from the refcounted pool and returns to it before the job ends.
fn decode_job_pooled(
    data: &[u8],
    info: &jpeg::JpegInfo,
    downscale: decode::JpegDownscale,
    pool: &BufferPool<dma_heap::DmaBuf>,
) -> Result<Vec<u8>, String> {
    let tables = tables::Tables::build(info).map_err(|e| format!("{e:?}"))?;
    let output_geometry = decode::geometry(info, true, downscale).map_err(|e| e.to_string())?;
    let svc = MppService::open(VPU_CLIENT_JPEG_DEC).map_err(|e| e.to_string())?;
    let table_h = pool
        .acquire(tables::RKD_TABLE_SIZE)
        .map_err(|e| format!("pool table: {e}"))?;
    let stream_h = pool
        .acquire(data.len().max(1))
        .map_err(|e| format!("pool stream: {e}"))?;
    let out_h = pool
        .acquire(output_geometry.frame_size)
        .map_err(|e| format!("pool output: {e}"))?;
    pool.data_mut(table_h).unwrap()[..tables::RKD_TABLE_SIZE].copy_from_slice(tables.as_slice());
    pool.data_mut(stream_h).unwrap()[..data.len()].copy_from_slice(data);
    let (regs, geom) = decode::build(&decode::Job {
        info,
        table_fd: pool.fd(table_h).unwrap(),
        stream_fd: pool.fd(stream_h).unwrap(),
        stream_len: data.len(),
        output_fd: pool.fd(out_h).unwrap(),
        rgb: true,
        downscale,
    })
    .map_err(|e| e.to_string())?;
    svc.submit_regs(&regs.words)
        .map_err(|e| format!("submit: {e}"))?;
    svc.poll_hw_finish().map_err(|e| format!("poll: {e}"))?;
    let frame = {
        let view = pool.data(out_h).map_err(|e| format!("pool view: {e}"))?;
        // The job's own thread invalidates its mapping before the read, the
        // same DMA_BUF_SYNC contract the sync path obeys.
        view.buffer()
            .sync_read_begin()
            .map_err(|e| format!("dma-buf read sync: {e}"))?;
        let n = geom.frame_size.min(view.len());
        let frame = view[..n].to_vec();
        view.buffer()
            .sync_read_end()
            .map_err(|e| format!("dma-buf read sync end: {e}"))?;
        frame
    };
    let _ = pool.release(table_h);
    let _ = pool.release(stream_h);
    let _ = pool.release(out_h);
    Ok(frame)
}

/// Framework demo (M10): decode one JPEG `jobs` times through the async
/// task queue + refcounted buffer pool and require every output to be
/// byte-identical to the synchronous path. Prints a verdict per job plus a
/// summary line; returns Err on any disagreement so the caller exits 1.
fn async_demo(path: &str, jobs: usize, workers: usize, out_path: &str) -> Result<usize, String> {
    use rk_mpp_rs::service::task_queue::{PollTimeout, TaskOutcome, TaskQueue};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let info = Arc::new(jpeg::parse(&data).map_err(|e| e.to_string())?);
    let downscale = decode::JpegDownscale::Disabled;
    let reference = decode_job_sync(&data, &info, downscale)?;
    println!(
        "async-demo: sync baseline {}x{} RGB {} bytes",
        info.width,
        info.height,
        reference.len()
    );

    let pool = BufferPool::new(jobs + 4);
    let queue = TaskQueue::new(workers);
    let outputs: Arc<Mutex<Vec<(usize, Result<Vec<u8>, String>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let callbacks_done = Arc::new(AtomicUsize::new(0));
    let mut ids = Vec::new();
    let started = Instant::now();
    for job_idx in 0..jobs {
        let (data, info, pool, outputs, callbacks) = (
            data.clone(),
            Arc::clone(&info),
            pool.clone(),
            Arc::clone(&outputs),
            Arc::clone(&callbacks_done),
        );
        let id = queue
            .submit(
                move || {
                    let outcome = decode_job_pooled(&data, &info, downscale, &pool);
                    outputs.lock().unwrap().push((job_idx, outcome));
                    TaskOutcome::Done
                },
                move |_id, _outcome| {
                    callbacks.fetch_add(1, Ordering::SeqCst);
                },
            )
            .map_err(|e| format!("submit {job_idx}: {e:?}"))?;
        ids.push(id);
    }
    let mut failures = 0usize;
    for (slot, id) in ids.iter().enumerate() {
        match queue.wait(*id, PollTimeout::Ms(8000)) {
            Some(TaskOutcome::Done) => {}
            other => {
                println!("job {slot}: task outcome {other:?}");
                failures += 1;
            }
        }
    }
    let per_job_ms = us(started.elapsed()) / 1000.0 / jobs as f64;
    let collected = std::mem::take(&mut *outputs.lock().unwrap());
    if collected.len() != jobs {
        println!(
            "async-demo: only {} of {jobs} jobs reported",
            collected.len()
        );
        failures += jobs - collected.len();
    }
    let mut identical = 0usize;
    for (job_idx, outcome) in collected.iter() {
        match outcome {
            Ok(frame) if *frame == reference => {
                identical += 1;
                println!("job {job_idx}: identical to sync ({} bytes)", frame.len())
            }
            Ok(frame) => {
                println!(
                    "job {job_idx}: DIFFERS from sync ({} vs {} bytes)",
                    frame.len(),
                    reference.len()
                );
                failures += 1;
            }
            Err(e) => {
                println!("job {job_idx}: failed: {e}");
                failures += 1;
            }
        }
    }
    let stats = pool.stats();
    println!(
        "async-demo: {jobs} jobs, {workers} workers, {per_job_ms:.3} ms/job; \
         identical={identical} callbacks={}",
        callbacks_done.load(Ordering::SeqCst)
    );
    println!(
        "async-demo: pool slots={} cached={} in_use={} allocations={}",
        stats.slots, stats.cached, stats.in_use, stats.allocations
    );
    if !out_path.is_empty() {
        // Job 0's frame on disk so an external md5 can check the whole
        // async story against the `decode` subcommand's sync output.
        for (job_idx, outcome) in collected.iter() {
            if *job_idx == 0 {
                if let Ok(frame) = outcome {
                    std::fs::write(out_path, frame).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    if failures != 0 {
        return Err(format!("{failures} jobs disagreed with the sync path"));
    }
    Ok(reference.len())
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

/// One IEP2 I5O2 deinterlace: read three source frames (prev, current, next),
/// run the engine once, and write the two reconstructed frames it produces.
/// The dma-buf and `/dev/mpp_service` calls only exist on RK3588, so this
/// helper is reached only through the `iep-dei` subcommand and verified by
/// `scripts/board_iep_dei_vs_mpp.sh`, never by a host test.
fn run_iep_dei(
    data: &[u8],
    dst: &str,
    cfg: rk_mpp_rs::service::iep_session::DeinterlaceConfig,
) -> Result<usize, String> {
    use rk_mpp_rs::service::iep_session::IepDeinterlace;

    let mut dei = IepDeinterlace::open(cfg).map_err(|e| format!("open IEP client 28: {e}"))?;
    let frame = dei.src_frame_bytes();
    if data.len() < frame * 3 {
        return Err(format!(
            "input is {} bytes, I5O2 needs 3 frames of {frame}",
            data.len()
        ));
    }
    // The first iep2_test iteration maps prev -> src[2], current -> src[0],
    // next -> src[1]; three consecutive input frames reproduce that content.
    for (i, order) in [(2usize, 0usize), (0, 1), (1, 2)] {
        let off = order * frame;
        dei.src_frame_mut(i)
            .sync_write_begin()
            .map_err(|e| format!("src{i} write sync: {e}"))?;
        let n = frame.min(dei.src_frame_mut(i).len());
        dei.src_frame_mut(i).as_mut_slice()[..n].copy_from_slice(&data[off..off + n]);
        dei.flush_src(i).map_err(|e| format!("src{i} flush: {e}"))?;
    }

    dei.run().map_err(|e| format!("IEP deinterlace: {e}"))?;

    let out_len = dei.dst_frame_bytes();
    let mut out = Vec::with_capacity(out_len * 2);
    for i in 0..2 {
        let n = out_len.min(dei.dst_frame(i).len());
        out.extend_from_slice(&dei.dst_frame(i)[..n]);
    }
    std::fs::write(dst, &out).map_err(|e| format!("{dst}: {e}"))?;
    Ok(2)
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
    if args.len() > 1 && args[1] == "async-demo" {
        let path = args.get(2).map(|s| s.as_str()).unwrap_or("frame.jpg");
        let jobs: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(6);
        let workers: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(3);
        let out = args.get(5).map(|s| s.as_str()).unwrap_or("");
        match async_demo(path, jobs, workers, out) {
            Ok(n) => println!("async-demo: PASS ({jobs} jobs x {n} bytes all match)"),
            Err(e) => {
                println!("async-demo: FAIL {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() > 1 && args[1] == "bench-decode" {
        let path = args.get(2).map(|s| s.as_str()).unwrap_or("frame.jpg");
        let iters: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(200);
        let scale_env = std::env::var("RKMPP_SCALE").ok();
        let scale_arg = args.get(4).map(String::as_str).or(scale_env.as_deref());
        let factor = scale_arg.and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
        let Some(downscale) = decode::JpegDownscale::from_factor(factor) else {
            eprintln!("invalid JPEG downscale {factor}; expected 1, 2, 4, or 8");
            std::process::exit(2);
        };
        match bench_decode(path, iters, downscale) {
            Ok((ms, w, h)) => println!(
                "decode {} : {:.3} ms/frame ({:.0} fps) at {}x{}, 1/{} scale",
                path,
                ms,
                1000.0 / ms,
                w,
                h,
                downscale.factor()
            ),
            Err(e) => println!("decode failed: {e}"),
        }
        return;
    }
    if args.len() > 1 && args[1] == "decode" {
        let path = args.get(2).map(|s| s.as_str()).unwrap_or("frame.jpg");
        let out = args
            .get(3)
            .map(|s| s.as_str())
            .unwrap_or("/tmp/rust_dec.rgb");
        let scale_env = std::env::var("RKMPP_SCALE").ok();
        let scale_arg = args.get(4).map(String::as_str).or(scale_env.as_deref());
        let factor = scale_arg.and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
        let Some(downscale) = decode::JpegDownscale::from_factor(factor) else {
            eprintln!("invalid JPEG downscale {factor}; expected 1, 2, 4, or 8");
            std::process::exit(2);
        };
        match decode_to_rgb(path, out, downscale) {
            Ok((w, h, stride)) => {
                println!(
                    "decoded {path} -> {out} ({}x{}, stride {stride} bytes, 1/{} scale)",
                    w,
                    h,
                    downscale.factor()
                )
            }
            Err(e) => println!("decode failed: {e}"),
        }
        return;
    }
    if args.len() > 1 && args[1] == "h264-enc" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("");
        let width = args.get(4).and_then(|s| s.parse::<usize>().ok());
        let height = args.get(5).and_then(|s| s.parse::<usize>().ok());
        let (Some(width), Some(height)) = (width, height) else {
            eprintln!("usage: rk-mpp-rs h264-enc <in.nv12> <out.264> <width> <height> [qp|auto] [profile] [frames] [gop] [cbr:|vbr:|avbr:target:min:max] [dblk:disable:alpha:beta] [roi:x:y:w:h:intra:abs:qp]... [gdr:row:|gdr:col:]num [split:mb:|split:byte:]arg");
            std::process::exit(2);
        };
        // QP defaults to the value the recorded VEPU580 register image carries, so
        // an existing invocation keeps producing the same bytes. `auto` (or `-1`)
        // hands the first picture's QP to the rate control instead, the way a
        // caller who never set `rc:qp_init` leaves it in MPP.
        let (qp_init, quality_auto) = match args.get(6).map(String::as_str) {
            Some("auto") | Some("-1") => (26, true),
            _ => (
                args.get(6).and_then(|s| s.parse::<u8>().ok()).unwrap_or(26),
                false,
            ),
        };
        let profile = args.get(7).map(String::as_str).unwrap_or("baseline");
        let frames = args
            .get(8)
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1);
        // GOP 1 keeps every picture an IDR, which is what the encoder did before
        // it could encode anything else.
        let gop = args.get(9).and_then(|s| s.parse::<u32>().ok()).unwrap_or(1);
        // `target:min:max`, the triple `mpi_enc_test -bps` takes. It reaches the
        // uuid_rc_cfg SEI text today, which is still a reason a byte comparison has
        // to be able to give both sides the same numbers - and a reason "absent"
        // has to stay absent rather than arrive as zeroes, because what the
        // harness does when the option is missing is itself a rule under test.
        // Everything past the positional slots is a prefixed option, dispatched
        // by prefix rather than by position, so a `dblk:` never lands in the bit
        // rate's slot when no rate control was asked for. A bare `target:min:max`
        // in the slot the scripts written before VBR existed use is still CBR.
        let mut bps: Option<(h264_syntax::RcMode, i64, i64, i64)> = None;
        let mut deblock: Option<(u32, i32, i32)> = None;
        // The `RoiRegionCfg` list `mpi_enc_test` adds per frame, in the same
        // add order.
        let mut rois: Vec<h264_syntax::RoiRegion> = Vec::new();
        // GDR arrives as `gdr:row:<num>` / `gdr:col:<num>`, the `refresh_mode`
        // and `refresh_num` of `MppEncRcCfg`; slice split as `split:mb:<arg>`
        // (BY_CTU, macroblocks per slice) or `split:byte:<arg>` (BY_BYTE).
        let mut gdr: Option<(h264_syntax::GdrMode, u32)> = None;
        let mut split: Option<(h264_syntax::SplitMode, u32)> = None;
        for text in args.iter().skip(10) {
            if let Some(rest) = text.strip_prefix("dblk:") {
                // `dblk:disable:alpha:beta`, the three `MppEncH264Cfg` deblocking
                // fields. The reference range-checks none of them and VEPU580's
                // register fields are two and four bits wide, so the truncation
                // the bitfield assignment performs is reproduced, not policed.
                let parts: Vec<&str> = rest.split(':').collect();
                match parts.as_slice() {
                    [disable, alpha, beta] => {
                        match (
                            disable.parse::<u32>(),
                            alpha.parse::<i32>(),
                            beta.parse::<i32>(),
                        ) {
                            (Ok(disable), Ok(alpha), Ok(beta)) => {
                                deblock = Some((disable, alpha, beta))
                            }
                            _ => {
                                eprintln!("bad dblk; expected dblk:disable:alpha:beta");
                                std::process::exit(2);
                            }
                        }
                    }
                    _ => {
                        eprintln!("bad dblk; expected dblk:disable:alpha:beta");
                        std::process::exit(2);
                    }
                }
            } else if let Some(rest) = text.strip_prefix("roi:") {
                let parts: Vec<i64> = rest.split(':').filter_map(|s| s.parse().ok()).collect();
                match parts.as_slice() {
                    [x, y, w, h, intra, qp_abs, qp] => rois.push(h264_syntax::RoiRegion {
                        x: *x as i32,
                        y: *y as i32,
                        w: *w as i32,
                        h: *h as i32,
                        force_intra: *intra != 0,
                        qp_absolute: *qp_abs != 0,
                        qp_val: *qp as i32,
                    }),
                    _ => {
                        eprintln!("bad roi; expected roi:x:y:w:h:intra:abs:qp");
                        std::process::exit(2);
                    }
                }
            } else if let Some(rest) = text.strip_prefix("gdr:") {
                // `gdr:row:<num>` / `gdr:col:<num>`: the refresh axis and the
                // macroblock lines per picture, the reference's `refresh_mode`
                // and `refresh_num`. The GOP keeps its own slot - under GDR it
                // is the sweep interval, not an IDR interval.
                let parts: Vec<&str> = rest.split(':').collect();
                match parts.as_slice() {
                    [axis, num] => {
                        let mode = match *axis {
                            "row" => h264_syntax::GdrMode::Row,
                            "col" => h264_syntax::GdrMode::Col,
                            _ => {
                                eprintln!("bad gdr axis; expected gdr:row:num or gdr:col:num");
                                std::process::exit(2);
                            }
                        };
                        match num.parse::<u32>() {
                            Ok(num) => gdr = Some((mode, num)),
                            Err(_) => {
                                eprintln!("bad gdr refresh count {num}");
                                std::process::exit(2);
                            }
                        }
                    }
                    _ => {
                        eprintln!("bad gdr; expected gdr:row:num or gdr:col:num");
                        std::process::exit(2);
                    }
                }
            } else if let Some(rest) = text.strip_prefix("split:") {
                // `split:mb:<arg>` is `MPP_ENC_SPLIT_BY_CTU` - the name says
                // CTU, the unit is a macroblock - and `split:byte:<arg>` is
                // `MPP_ENC_SPLIT_BY_BYTE`. The output mode stays the reference
                // default of zero: every slice of the picture in one AU.
                let parts: Vec<&str> = rest.split(':').collect();
                match parts.as_slice() {
                    [kind, arg] => {
                        let mode = match *kind {
                            "mb" => h264_syntax::SplitMode::ByCtu,
                            "byte" => h264_syntax::SplitMode::ByByte,
                            _ => {
                                eprintln!(
                                    "bad split kind; expected split:mb:arg or split:byte:arg"
                                );
                                std::process::exit(2);
                            }
                        };
                        match arg.parse::<u32>() {
                            Ok(arg) => split = Some((mode, arg)),
                            Err(_) => {
                                eprintln!("bad split argument {arg}");
                                std::process::exit(2);
                            }
                        }
                    }
                    _ => {
                        eprintln!("bad split; expected split:mb:arg or split:byte:arg");
                        std::process::exit(2);
                    }
                }
            } else {
                let (mode, text) = match text.split_once(':') {
                    Some(("cbr", rest)) => (h264_syntax::RcMode::Cbr, rest),
                    Some(("vbr", rest)) => (h264_syntax::RcMode::Vbr, rest),
                    Some(("avbr", rest)) => (h264_syntax::RcMode::Avbr, rest),
                    _ => (h264_syntax::RcMode::Cbr, text.as_str()),
                };
                let parts: Vec<i64> = text.split(':').filter_map(|s| s.parse().ok()).collect();
                match parts.as_slice() {
                    [target, min, max] => bps = Some((mode, *target, *min, *max)),
                    _ => {
                        eprintln!(
                            "unknown option {text}; expected \
                             [cbr:|vbr:|avbr:]target:min:max, dblk:disable:alpha:beta, \
                             roi:x:y:w:h:intra:abs:qp, gdr:row:|col:num \
                             or split:mb:|byte:arg"
                        );
                        std::process::exit(2);
                    }
                }
            }
        }
        if src.is_empty() || dst.is_empty() {
            eprintln!("usage: rk-mpp-rs h264-enc <in.nv12> <out.264> <width> <height> [qp|auto] [profile] [frames] [gop] [cbr:|vbr:|avbr:target:min:max] [dblk:disable:alpha:beta] [roi:x:y:w:h:intra:abs:qp]... [gdr:row:|gdr:col:]num [split:mb:|split:byte:]arg");
            std::process::exit(2);
        }
        let qp_desc = if quality_auto {
            "auto".to_string()
        } else {
            qp_init.to_string()
        };
        match encode_h264(
            src,
            dst,
            width,
            height,
            qp_init,
            quality_auto,
            profile,
            frames,
            gop,
            bps,
            deblock,
            rois,
            gdr,
            split,
        ) {
            Ok((bytes, header_bytes, payload_bytes, ms)) => println!(
                "encoded {src} -> {dst}: {bytes} bytes ({header_bytes} header + {payload_bytes} payload), {width}x{height} {profile} qp {qp_desc} gop {gop}, {ms:.3} ms"
            ),
            Err(e) => {
                eprintln!("H.264 encode failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() > 1 && args[1] == "h265-enc" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("");
        let width = args.get(4).and_then(|s| s.parse::<usize>().ok());
        let height = args.get(5).and_then(|s| s.parse::<usize>().ok());
        let (Some(width), Some(height)) = (width, height) else {
            eprintln!("usage: rk-mpp-rs h265-enc <in.nv12> <out.265> <width> <height> [qp]");
            std::process::exit(2);
        };
        if src.is_empty() || dst.is_empty() {
            eprintln!("usage: rk-mpp-rs h265-enc <in.nv12> <out.265> <width> <height> [qp]");
            std::process::exit(2);
        }
        let qp = args
            .get(6)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(26);
        // M8 opens with the parameter sets, which the software emits and which are
        // the byte-comparable part before any on-board job template exists. The QP
        // never reaches them - `h265e_set_pps` pins pic_init_qp at intra_qp and the
        // hardware derives the slice QP against it, exactly as H.264's PPS ignores
        // qp_init - so the QP is reported through the derived register image below
        // rather than through the header bytes.
        match encode_h265_headers(src, dst, width, height, qp) {
            Ok((header_bytes, pic_qp)) => println!(
                "h265-enc {src} -> {dst}: {header_bytes} header bytes (VPS+SPS+PPS), {width}x{height} fixed qp {pic_qp} (IDR payload pending the board job template)"
            ),
            Err(e) => {
                eprintln!("H.265 encode failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() > 1 && args[1] == "h265-regs" {
        let width = args.get(2).and_then(|s| s.parse::<usize>().ok());
        let height = args.get(3).and_then(|s| s.parse::<usize>().ok());
        let (Some(width), Some(height)) = (width, height) else {
            eprintln!("usage: rk-mpp-rs h265-regs <width> <height> [qp]");
            std::process::exit(2);
        };
        let qp = args
            .get(4)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(26);
        let long_term = args.iter().skip(5).any(|a| a == "lt");
        let picture = match args.get(5).map(|s| s.as_str()) {
            Some(mode) if mode.starts_with('p') => {
                let poc = mode[1..].parse::<u32>().unwrap_or(0);
                h265e::Picture::P { poc_lsb: poc }
            }
            _ => h265e::Picture::Idr,
        };
        let rc = args.get(6).map(|s| s.as_str()).and_then(|spec| {
            let f: Vec<i32> = spec
                .strip_prefix("rc:")?
                .split(':')
                .filter_map(|v| v.parse().ok())
                .collect();
            if f.len() != 4 {
                return None;
            }
            Some(h265e::RateControl {
                bit_target: f[0],
                qp_target: f[1] as u32,
                qp_min: f[2] as u32,
                qp_max: f[3] as u32,
                qp_delta_row: 2,
                qp_delta_row_i: 2,
            })
        });
        let job = h265e::IdrJob {
            width,
            height,
            qp,
            qp_min: qp,
            qp_max: qp,
            picture,
            long_term,
            rc,
            refresh: None,
            input_fd: -1,
            output_fd: -1,
            current_fd: -1,
            current_thumb_fd: -1,
            reference_fd: -1,
            reference_thumb_fd: -1,
        };
        match h265e::build_registers(&job) {
            Ok(registers) => {
                // One header line per section - `name first_word count` - then the
                // words, eight per line. `tools/h265e_trace_diff.py` reads this.
                for (name, first, words) in registers.captured() {
                    println!("{name} {first} {}", words.len());
                    for row in words.chunks(8) {
                        let hex: Vec<String> = row.iter().map(|w| format!("{w:08x}")).collect();
                        println!("{}", hex.join(" "));
                    }
                }
            }
            Err(e) => {
                eprintln!("H.265 register image failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() > 1 && args[1] == "h265-idr" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("");
        let width = args.get(4).and_then(|s| s.parse::<usize>().ok());
        let height = args.get(5).and_then(|s| s.parse::<usize>().ok());
        let usage = "usage: rk-mpp-rs h265-idr <in.nv12> <out.265> <width> <height> [qp]";
        let (Some(width), Some(height)) = (width, height) else {
            eprintln!("{usage}");
            std::process::exit(2);
        };
        if src.is_empty() || dst.is_empty() {
            eprintln!("{usage}");
            std::process::exit(2);
        }
        let qp = args
            .get(6)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(26);
        let input = std::fs::read(src).map_err(|e| format!("{src}: {e}"));
        let input = match input {
            Ok(input) => input,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };
        let result = h265e_session::Vepu580H265::open(width, height, qp)
            .and_then(|mut encoder| encoder.encode_idr(&input));
        match result.and_then(|stream| {
            std::fs::write(dst, &stream).map_err(|e| format!("{dst}: {e}"))?;
            Ok(stream.len())
        }) {
            Ok(bytes) => {
                println!("h265-idr {src} -> {dst}: {bytes} bytes, {width}x{height} fixed qp {qp}")
            }
            Err(e) => {
                eprintln!("H.265 IDR encode failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() > 1 && args[1] == "h265-gop" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("");
        let width = args.get(4).and_then(|s| s.parse::<usize>().ok());
        let height = args.get(5).and_then(|s| s.parse::<usize>().ok());
        let usage = "usage: rk-mpp-rs h265-gop <in.nv12> <out.265> <width> <height> [frames] [qp]";
        let (Some(width), Some(height)) = (width, height) else {
            eprintln!("{usage}");
            std::process::exit(2);
        };
        if src.is_empty() || dst.is_empty() {
            eprintln!("{usage}");
            std::process::exit(2);
        }
        let frames = args
            .get(6)
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(2);
        let qp = args
            .get(7)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(26);
        let input = match std::fs::read(src) {
            Ok(input) => input,
            Err(e) => {
                eprintln!("{src}: {e}");
                std::process::exit(1);
            }
        };
        let frame_size = width * height * 3 / 2;
        if input.len() < frame_size * frames {
            eprintln!(
                "{src}: {frames} frames of {width}x{height} need {} bytes, file has {}",
                frame_size * frames,
                input.len()
            );
            std::process::exit(1);
        }
        let refresh = args.get(8).map(|s| s.as_str());
        let opened = if refresh.is_some() {
            h265e_session::Vepu580H265::open_gop_short_term(width, height, qp)
        } else {
            h265e_session::Vepu580H265::open_gop(width, height, qp)
        };
        let result = opened.and_then(|mut encoder| {
            if let Some(spec) = refresh {
                let (mode, num) = spec.split_once(':').unwrap_or(("row", "1"));
                let mode = if mode == "col" {
                    h265e::RefreshMode::Col
                } else {
                    h265e::RefreshMode::Row
                };
                encoder.enable_refresh(mode, num.parse().unwrap_or(1))?;
            }
            let mut out = Vec::new();
            for index in 0..frames {
                let frame = &input[index * frame_size..(index + 1) * frame_size];
                out.extend_from_slice(&encoder.encode_frame(frame)?);
            }
            Ok(out)
        });
        match result.and_then(|stream| {
            std::fs::write(dst, &stream).map_err(|e| format!("{dst}: {e}"))?;
            Ok(stream.len())
        }) {
            Ok(bytes) => println!(
                "h265-gop {src} -> {dst}: {bytes} bytes, {frames} frames {width}x{height} fixed qp {qp}"
            ),
            Err(e) => {
                eprintln!("H.265 GOP encode failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() > 1 && args[1] == "h265-gop-rc" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("");
        let width = args.get(4).and_then(|s| s.parse::<usize>().ok());
        let height = args.get(5).and_then(|s| s.parse::<usize>().ok());
        let usage = "usage: rk-mpp-rs h265-gop-rc <in.nv12> <out.265> <w> <h> <qp:bits,...> [min_qp] [max_qp]";
        let (Some(width), Some(height)) = (width, height) else {
            eprintln!("{usage}");
            std::process::exit(2);
        };
        let spec = args.get(6).map(|s| s.as_str()).unwrap_or("");
        let steps: Vec<(u32, i32)> = spec
            .split(',')
            .filter_map(|item| {
                let (q, b) = item.split_once(':')?;
                Some((q.parse().ok()?, b.parse().ok()?))
            })
            .collect();
        if src.is_empty() || dst.is_empty() || steps.is_empty() {
            eprintln!("{usage}");
            std::process::exit(2);
        }
        let min_qp = args
            .get(7)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(10);
        let max_qp = args
            .get(8)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(51);
        let input = match std::fs::read(src) {
            Ok(input) => input,
            Err(e) => {
                eprintln!("{src}: {e}");
                std::process::exit(1);
            }
        };
        let frame_size = width * height * 3 / 2;
        if input.len() < frame_size * steps.len() {
            eprintln!(
                "{src}: {} frames of {width}x{height} need {} bytes, file has {}",
                steps.len(),
                frame_size * steps.len(),
                input.len()
            );
            std::process::exit(1);
        }
        let bps: Option<(i64, i64, i64)> = args.get(9).and_then(|spec| {
            let f: Vec<i64> = spec.split(':').filter_map(|v| v.parse().ok()).collect();
            if f.len() == 3 {
                Some((f[0], f[1], f[2]))
            } else {
                None
            }
        });
        let result =
            h265e_session::Vepu580H265::open_gop(width, height, max_qp).and_then(|mut encoder| {
                if let Some((target, min, max)) = bps {
                    encoder.set_rate_control_sei(h265e_session::RcSei {
                        target,
                        min,
                        max,
                        init_quality: args
                            .get(10)
                            .and_then(|s| s.parse::<i32>().ok())
                            .unwrap_or(steps[0].0 as i32),
                        qp_min: min_qp,
                        qp_max: max_qp,
                    });
                }
                let mut out = Vec::new();
                for (index, (qp, bits)) in steps.iter().enumerate() {
                    let frame = &input[index * frame_size..(index + 1) * frame_size];
                    out.extend_from_slice(
                        &encoder.encode_frame_with_rc(frame, *bits, *qp, min_qp, max_qp)?,
                    );
                }
                Ok(out)
            });
        match result.and_then(|stream| {
            std::fs::write(dst, &stream).map_err(|e| format!("{dst}: {e}"))?;
            Ok(stream.len())
        }) {
            Ok(bytes) => println!(
                "h265-gop-rc {src} -> {dst}: {bytes} bytes, {} decided pictures",
                steps.len()
            ),
            Err(e) => {
                eprintln!("H.265 RC GOP encode failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() > 1 && args[1] == "h265-rate" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("");
        let width = args.get(4).and_then(|s| s.parse::<usize>().ok());
        let height = args.get(5).and_then(|s| s.parse::<usize>().ok());
        let frames = args.get(6).and_then(|s| s.parse::<usize>().ok());
        let bps = args.get(7).and_then(|spec| {
            let f: Vec<i32> = spec.split(':').filter_map(|v| v.parse().ok()).collect();
            if f.len() == 3 {
                Some((f[0], f[1], f[2]))
            } else {
                None
            }
        });
        let mode_name = args.get(11).map(|s| s.as_str()).unwrap_or("cbr");
        let mode = match mode_name {
            "vbr" => h265e_session::RcMode::Vbr,
            "avbr" => h265e_session::RcMode::Avbr,
            _ => h265e_session::RcMode::Cbr,
        };
        let usage = "usage: rk-mpp-rs h265-rate <in.nv12> <out.265> <w> <h> <frames> <bps:min:max> [init_qp] [qp_min] [qp_max] [cbr|vbr|avbr]";
        let (Some(width), Some(height), Some(frames), Some((target, min, max))) =
            (width, height, frames, bps)
        else {
            eprintln!("{usage}");
            std::process::exit(2);
        };
        if src.is_empty() || dst.is_empty() || frames == 0 {
            eprintln!("{usage}");
            std::process::exit(2);
        }
        let init_qp = args
            .get(8)
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(26);
        let qp_min = args
            .get(9)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(10);
        let qp_max = args
            .get(10)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(51);
        let input = match std::fs::read(src) {
            Ok(input) => input,
            Err(e) => {
                eprintln!("{src}: {e}");
                std::process::exit(1);
            }
        };
        let frame_size = width * height * 3 / 2;
        if input.len() < frame_size * frames {
            eprintln!("{src}: {frames} frames need {} bytes", frame_size * frames);
            std::process::exit(1);
        }
        let result = h265e_session::Vepu580H265::open_rate(
            width, height, mode, target, min, max, init_qp, qp_min, qp_max,
        )
        .and_then(|mut encoder| {
            let mut out = Vec::new();
            for index in 0..frames {
                out.extend_from_slice(
                    &encoder.encode_frame(&input[index * frame_size..(index + 1) * frame_size])?,
                );
            }
            Ok(out)
        });
        match result.and_then(|stream| {
            std::fs::write(dst, &stream).map_err(|e| format!("{dst}: {e}"))?;
            Ok(stream.len())
        }) {
            Ok(bytes) => println!("h265-cbr {src} -> {dst}: {bytes} bytes, {frames} pictures"),
            Err(e) => {
                eprintln!("H.265 rate-control encode failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() > 1 && args[1] == "jpeg-enc" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("");
        let width = args.get(4).and_then(|s| s.parse::<usize>().ok());
        let height = args.get(5).and_then(|s| s.parse::<usize>().ok());
        if src.is_empty() || dst.is_empty() {
            eprintln!(
                "usage: rk-mpp-rs jpeg-enc <in.yuv> <out.jpg> <width> <height> [quant 0..10|qf<1..100>] [420|422|gray|444] [nv12|yuyv|uyvy|rgb565|bgr565|rgb555|bgr555|rgb444|bgr444|rgb101010|bgr101010|argb8888|abgr8888|bgra8888|rgba8888] [rst<mcus>] [rot90|rot270]"
            );
            std::process::exit(2);
        }
        let (Some(width), Some(height)) = (width, height) else {
            eprintln!("invalid JPEG encoder dimensions");
            std::process::exit(2);
        };
        // The sixth field takes the quantization-table index or a qf<1..100>
        // quality factor, the mode mpi_enc_test drives through jpeg:q_factor.
        let quant_arg = args.get(6).map(String::as_str).unwrap_or("10");
        let (quality, quality_label) = if let Some(factor) = quant_arg.strip_prefix("qf") {
            let factor = factor.parse::<u8>().unwrap_or_else(|_| {
                eprintln!("invalid JPEG quality factor {quant_arg}");
                std::process::exit(2);
            });
            (jpege::Quality::QFactor(factor), format!("qfactor {factor}"))
        } else {
            let quant = quant_arg.parse::<u8>().unwrap_or(10);
            (jpege::Quality::Quant(quant), format!("quant {quant}"))
        };
        let chroma = match args.get(7).map(String::as_str).unwrap_or("420") {
            "gray" => jpege::ChromaFormat::Gray,
            "420" => jpege::ChromaFormat::Yuv420,
            "422" => jpege::ChromaFormat::Yuv422,
            "444" => jpege::ChromaFormat::Yuv444,
            value => {
                eprintln!("invalid JPEG chroma format {value}; expected gray, 420, 422, or 444");
                std::process::exit(2);
            }
        };
        let input_format = match args.get(8).map(String::as_str) {
            Some("nv12") => jpege::InputFormat::Nv12,
            Some("yuyv") => jpege::InputFormat::Yuyv,
            Some("uyvy") => jpege::InputFormat::Uyvy,
            Some("rgb565") => jpege::InputFormat::Rgb565,
            Some("bgr565") => jpege::InputFormat::Bgr565,
            Some("rgb555") => jpege::InputFormat::Rgb555,
            Some("bgr555") => jpege::InputFormat::Bgr555,
            Some("rgb444") => jpege::InputFormat::Rgb444,
            Some("bgr444") => jpege::InputFormat::Bgr444,
            Some("rgb101010") => jpege::InputFormat::Rgb101010,
            Some("bgr101010") => jpege::InputFormat::Bgr101010,
            Some("argb8888") => jpege::InputFormat::Argb8888,
            Some("abgr8888") => jpege::InputFormat::Abgr8888,
            Some("bgra8888") => jpege::InputFormat::Bgra8888,
            Some("rgba8888") => jpege::InputFormat::Rgba8888,
            Some(value) => {
                eprintln!("invalid JPEG input format {value}; see the usage line");
                std::process::exit(2);
            }
            None if chroma == jpege::ChromaFormat::Yuv422 => jpege::InputFormat::Yuyv,
            None => jpege::InputFormat::Nv12,
        };
        // Trailing order-independent options: restart interval and rotation.
        let mut rotation = jpege::Rotation::None;
        let mut split_mcus: Option<u16> = None;
        for extra in args.get(9..).unwrap_or(&[]) {
            if let Some(degrees) = extra.strip_prefix("rot") {
                rotation = match degrees {
                    "90" => jpege::Rotation::Degree90,
                    "270" => jpege::Rotation::Degree270,
                    _ => {
                        eprintln!("invalid JPEG rotation {extra}; VEPU2 supports rot90 or rot270");
                        std::process::exit(2);
                    }
                };
            } else if let Some(interval) = extra.strip_prefix("rst") {
                split_mcus = Some(interval.parse().unwrap_or_else(|_| {
                    eprintln!("invalid JPEG restart interval {extra}");
                    std::process::exit(2);
                }));
            } else {
                eprintln!("unknown JPEG encoder option {extra}");
                std::process::exit(2);
            }
        }
        match encode_jpeg(
            src, dst, width, height, quality, input_format, chroma, rotation, split_mcus,
        ) {
            Ok((bytes, ms)) => println!(
                "encoded {src} -> {dst}: {bytes} bytes, {width}x{height}, {quality_label}, {ms:.3} ms"
            ),
            Err(e) => {
                eprintln!("JPEG encode failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() > 1 && args[1] == "bench-jpeg-enc" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let width = args.get(3).and_then(|s| s.parse::<usize>().ok());
        let height = args.get(4).and_then(|s| s.parse::<usize>().ok());
        let iters = args
            .get(5)
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(200);
        if src.is_empty() || width.is_none() || height.is_none() {
            eprintln!(
                "usage: rk-mpp-rs bench-jpeg-enc <in.nv12> <width> <height> [iters] [quant 0..10]"
            );
            std::process::exit(2);
        }
        let quant = args.get(6).and_then(|s| s.parse::<u8>().ok()).unwrap_or(10);
        match bench_encode_nv12(
            src,
            width.unwrap(),
            height.unwrap(),
            iters,
            jpege::Quality::Quant(quant),
        ) {
            Ok((ms, bytes)) => println!(
                "JPEG encode benchmark {src}: {ms:.3} ms/frame ({:.0} fps), {bytes} bytes, quant {quant}",
                1000.0 / ms
            ),
            Err(e) => {
                eprintln!("JPEG encode benchmark failed: {e}");
                std::process::exit(1);
            }
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
                        println!(
                            "  comp {k}: id={} h={} v={} tq={} td={} ta={}",
                            c.id, c.h, c.v, c.tq, c.td, c.ta
                        );
                    }
                }
                Err(e) => println!("{path}: {e}"),
            },
            Err(e) => println!("{path}: {e}"),
        }
        return;
    }
    if args.len() > 1 && args[1] == "hevcdec" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("/tmp/hevc.yuv");
        if src.is_empty() {
            eprintln!("usage: rk-mpp-rs hevcdec <in.265> <out.yuv>");
            std::process::exit(1);
        }
        let data = match std::fs::read(src) {
            Ok(data) => data,
            Err(e) => {
                eprintln!("{src}: {e}");
                std::process::exit(1);
            }
        };
        let aus = rk_mpp_rs::codec::hevc::decoder::split_access_units(&data);
        println!("{} access units", aus.len());
        if aus.is_empty() {
            eprintln!("{src}: no H.265 access units found");
            std::process::exit(1);
        }
        let mut dec = match rk_mpp_rs::codec::hevc::Decoder::new() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("open rkvdec failed: {e}");
                std::process::exit(1);
            }
        };
        use std::io::Write;
        let mut pictures = Vec::with_capacity(aus.len());
        let trace = std::env::var_os("RFI_HEVC_TRACE").is_some();
        let t = Instant::now();
        let mut failed = false;
        for (i, au) in aus.iter().enumerate() {
            match dec.decode_access_unit(au) {
                Ok(Some(pic)) => {
                    if trace {
                        println!(
                            "  au {i}: poc={} epoch={} err={:#x} output={} status={:08x}/{:08x}/{:08x}",
                            pic.poc,
                            pic.epoch,
                            pic.err_info,
                            rk_mpp_rs::codec::hevc::picture_is_output(&pic),
                            dec.status().0,
                            dec.status().1,
                            dec.status().2
                        );
                    }
                    if i < 3 && !pic.yuv.is_empty() {
                        let y = &pic.yuv[..pic.width * pic.height];
                        let sum: u64 = y.iter().map(|v| *v as u64).sum();
                        println!(
                            "  frame {i}: poc={} {}x{} y_mean={:.1} status={:08x}/{:08x}/{:08x}",
                            pic.poc,
                            pic.width,
                            pic.height,
                            sum as f64 / y.len() as f64,
                            dec.status().0,
                            dec.status().1,
                            dec.status().2
                        );
                    }
                    pictures.push(pic);
                }
                Ok(None) => {}
                Err(e) => {
                    eprintln!("frame {i}: decode error: {e}");
                    failed = true;
                    break;
                }
            }
        }
        let decode_ms = t.elapsed().as_secs_f64() * 1000.0;
        rk_mpp_rs::codec::hevc::sort_pictures_for_display(&mut pictures);
        let mut out = std::io::BufWriter::new(std::fs::File::create(dst).expect("create out"));
        let decoded = pictures.len();
        let n = pictures
            .iter()
            .filter(|pic| rk_mpp_rs::codec::hevc::picture_is_output(pic))
            .count();
        for pic in pictures
            .iter()
            .filter(|pic| rk_mpp_rs::codec::hevc::picture_is_output(pic))
        {
            out.write_all(&pic.yuv).expect("write frame");
        }
        out.flush().ok();
        if trace {
            let pocs: Vec<i32> = pictures
                .iter()
                .filter(|pic| rk_mpp_rs::codec::hevc::picture_is_output(pic))
                .map(|pic| pic.poc)
                .collect();
            println!("output pocs: {pocs:?}");
        }
        println!(
            "decoded {decoded} frames ({n} output) in {decode_ms:.1} ms ({:.1} fps), POC-sorted output",
            n as f64 / (decode_ms / 1000.0)
        );
        if failed {
            std::process::exit(1);
        }
        return;
    }
    if args.len() > 1 && args[1] == "av1dec" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("/tmp/av1.yuv");
        if src.is_empty() {
            eprintln!("usage: rk-mpp-rs av1dec <in.ivf> <out.yuv>");
            std::process::exit(1);
        }
        let data = match std::fs::read(src) {
            Ok(data) => data,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let ivf = match rk_mpp_rs::codec::av1::parser::parse_ivf(&data) {
            Ok(ivf) => ivf,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let mut decoder = match rk_mpp_rs::codec::av1::decoder::Decoder::new() {
            Ok(decoder) => decoder,
            Err(error) => {
                eprintln!("open av1d failed: {error}");
                std::process::exit(1);
            }
        };
        use std::io::Write;
        let mut output = std::io::BufWriter::new(match std::fs::File::create(dst) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("{dst}: {error}");
                std::process::exit(1);
            }
        });
        let started = Instant::now();
        let mut decoded = 0usize;
        let mut shown = 0usize;
        let mut failed = false;
        for (index, frame) in ivf.frames.iter().enumerate() {
            let pictures = match decoder.decode_packet(frame.data) {
                Ok(pictures) => pictures,
                Err(error) => {
                    eprintln!("frame {index}: {error}");
                    failed = true;
                    break;
                }
            };
            decoded += 1;
            for picture in pictures {
                if picture.err_info == 0 {
                    output.write_all(&picture.yuv).expect("write AV1 frame");
                }
                shown += 1;
                if std::env::var_os("RFI_AV1_TRACE").is_some() {
                    println!(
                        "  frame {index}: poc={} {}x{} err={:#x} status={:08x}/{:08x}/{:08x}",
                        picture.poc,
                        picture.width,
                        picture.height,
                        picture.err_info,
                        decoder.status().0,
                        decoder.status().1,
                        decoder.status().2,
                    );
                }
            }
        }
        output.flush().ok();
        let elapsed = started.elapsed().as_secs_f64();
        println!(
            "decoded {decoded} packets ({shown} output) in {:.1} ms ({:.1} fps)",
            elapsed * 1000.0,
            shown as f64 / elapsed.max(f64::EPSILON)
        );
        if failed {
            std::process::exit(1);
        }
        return;
    }
    if args.len() > 1 && args[1] == "vp9dec" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("/tmp/vp9.yuv");
        if src.is_empty() {
            eprintln!("usage: rk-mpp-rs vp9dec <in.ivf> <out.yuv>");
            std::process::exit(1);
        }
        let data = match std::fs::read(src) {
            Ok(data) => data,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let mut decoder = match rk_mpp_rs::codec::vp9::decoder::Decoder::new() {
            Ok(decoder) => decoder,
            Err(error) => {
                eprintln!("open rkvdec failed: {error}");
                std::process::exit(1);
            }
        };
        use std::io::Write;
        let mut output = std::io::BufWriter::new(match std::fs::File::create(dst) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("{dst}: {error}");
                std::process::exit(1);
            }
        });
        let started = Instant::now();
        let outcome = decoder.decode_stream(&data);
        let mut shown = 0usize;
        for picture in &outcome.pictures {
            if picture.err_info == 0 {
                output.write_all(&picture.yuv).expect("write VP9 frame");
            }
            shown += 1;
            if std::env::var_os("RFI_VP9_TRACE").is_some() {
                println!(
                    "  picture {shown}: poc={} {}x{} err={:#x} status={:08x}/{:08x}/{:08x}",
                    picture.poc,
                    picture.width,
                    picture.height,
                    picture.err_info,
                    decoder.status().0,
                    decoder.status().1,
                    decoder.status().2,
                );
            }
        }
        for drop in &outcome.dropped {
            eprintln!("packet {} dropped: {}", drop.packet, drop.reason);
        }
        output.flush().ok();
        let elapsed = started.elapsed().as_secs_f64();
        println!(
            "decoded {} packets ({} output, {} packets dropped{}) in {:.1} ms ({:.1} fps)",
            shown + outcome.dropped.len(),
            shown,
            outcome.dropped.len(),
            if outcome.stopped_early {
                ", stopped early"
            } else {
                ""
            },
            elapsed * 1000.0,
            shown as f64 / elapsed.max(f64::EPSILON)
        );
        if outcome.stopped_early {
            std::process::exit(1);
        }
        return;
    }
    if args.len() > 1 && args[1] == "h264dec" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("/tmp/out.yuv");
        if src.is_empty() {
            eprintln!("usage: rk-mpp-rs h264dec <in.264> <out.yuv>");
            std::process::exit(1);
        }
        let data = match std::fs::read(src) {
            Ok(data) => data,
            Err(e) => {
                eprintln!("{src}: {e}");
                std::process::exit(1);
            }
        };
        let aus = rk_mpp_rs::codec::h264::decoder::split_access_units(&data);
        println!("{} access units", aus.len());
        if aus.is_empty() {
            eprintln!("{src}: no H.264 access units found");
            std::process::exit(1);
        }
        let mut dec = match rk_mpp_rs::codec::h264::Decoder::new() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("open rkvdec failed: {e}");
                std::process::exit(1);
            }
        };
        use std::io::Write;
        let mut pictures = Vec::with_capacity(aus.len());
        let mut failed = false;
        let t = Instant::now();
        for (i, au) in aus.iter().enumerate() {
            let t0 = Instant::now();
            match dec.decode_access_unit(au) {
                _ if false => {}
                Ok(Some(pic)) => {
                    if i < 3 {
                        println!(
                            "    frame {i} submit+poll: {:.2} ms",
                            t0.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                    if std::env::var("RFI_DUMP_UV").is_ok() {
                        std::fs::write("/tmp/raw_uv.bin", &pic.raw_uv).ok();
                    }
                    if i < 3 {
                        let y = &pic.yuv[..pic.width * pic.height];
                        let sum: u64 = y.iter().map(|v| *v as u64).sum();
                        let maxv = y.iter().copied().max().unwrap_or(0);
                        println!(
                            "  frame {i}: poc={} {}x{} y_mean={:.1} y_max={} status={:08x}/{:08x}/{:08x}",
                            pic.poc,
                            pic.width,
                            pic.height,
                            sum as f64 / y.len() as f64,
                            maxv,
                            dec.status().0,
                            dec.status().1,
                            dec.status().2
                        );
                    }
                    pictures.push(pic);
                }
                Ok(None) => {}
                Err(e) => {
                    eprintln!("frame {i}: decode error: {e}");
                    failed = true;
                    break;
                }
            }
        }
        let decode_ms = t.elapsed().as_secs_f64() * 1000.0;
        rk_mpp_rs::codec::h264::sort_pictures_by_poc(&mut pictures);
        let mut out = std::io::BufWriter::new(std::fs::File::create(dst).expect("create out"));
        let decoded = pictures.len();
        let n = pictures
            .iter()
            .filter(|pic| rk_mpp_rs::codec::h264::decoder::picture_is_output(pic))
            .count();
        for pic in pictures
            .iter()
            .filter(|pic| rk_mpp_rs::codec::h264::decoder::picture_is_output(pic))
        {
            out.write_all(&pic.yuv).expect("write frame");
        }
        out.flush().ok();
        println!(
            "decoded {decoded} frames ({n} output) in {decode_ms:.1} ms ({:.1} fps), POC-sorted output",
            n as f64 / (decode_ms / 1000.0)
        );
        if failed {
            std::process::exit(1);
        }
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
        let block = rk_mpp_rs::codec::h264::hw::build_sps_pps(
            &sps,
            &pps,
            &[],
            &rk_mpp_rs::codec::h264::SliceHeader::default(),
        );
        let hex: Vec<String> = block.iter().map(|b| format!("{b:02x}")).collect();
        println!("{}", hex.join(" "));
        return;
    }
    if args.len() > 1 && args[1] == "hevc" {
        let path = args.get(2).map(|s| s.as_str()).unwrap_or("");
        if path.is_empty() {
            println!("usage: rk-mpp-rs hevc <annexb.265>");
            return;
        }
        let data = std::fs::read(path).expect("read stream");
        use rk_mpp_rs::codec::hevc;
        let nals = hevc::split_annex_b(&data);
        let mut vcl = 0usize;
        let mut sps_v: Option<hevc::Sps> = None;
        let mut pps_v: Option<hevc::Pps> = None;
        let mut slices = 0usize;
        for n in &nals {
            match n.unit_type {
                hevc::parser::NAL_VPS => {
                    let v = hevc::parse_vps(&n.rbsp).expect("vps");
                    println!(
                        "VPS id={} sub_layers={} profile={} level={}",
                        v.vps_id,
                        v.max_sub_layers,
                        v.profile.general_profile_idc,
                        v.profile.general_level_idc
                    );
                }
                hevc::parser::NAL_SPS => {
                    let s = hevc::parse_sps(&n.rbsp).expect("sps");
                    sps_v = Some(s.clone());
                    println!(
                        "SPS id={} vps={} {}x{} chroma={} depth={} poc_lsb={} st_rps={} sao={} tmvp={} ctb={}/{} tb={}/{} dpb={}",
                        s.sps_id,
                        s.vps_id,
                        s.width,
                        s.height,
                        s.chroma_format_idc,
                        s.bit_depth_luma,
                        s.log2_max_pic_order_cnt_lsb,
                        s.num_short_term_ref_pic_sets,
                        s.sao_enabled,
                        s.temporal_mvp_enabled,
                        s.log2_min_cb,
                        s.log2_diff_max_min_cb,
                        s.log2_min_tb,
                        s.log2_diff_max_min_tb,
                        s.max_dec_pic_buffering
                    );
                }
                hevc::parser::NAL_PPS => {
                    let p = hevc::parse_pps(&n.rbsp).expect("pps");
                    pps_v = Some(p.clone());
                    println!(
                        "PPS id={} sps={} init_qp={} dependent_slices={} weighted_pred={} tiles={} wpp={} lists_mod={}",
                        p.pps_id,
                        p.sps_id,
                        p.init_qp,
                        p.dependent_slice_segments,
                        p.weighted_pred,
                        p.tiles_enabled,
                        p.entropy_coding_sync,
                        p.lists_modification_present
                    );
                }
                t if t <= 31 => {
                    vcl += 1;
                    if let (Some(sps), Some(pps)) = (&sps_v, &pps_v) {
                        match hevc::parse_slice_header(&n.rbsp, sps, pps, t) {
                            Ok(sh) => {
                                slices += 1;
                                if slices <= 4 {
                                    println!(
                                        "  slice first={} dependent={} address={} type={} poc_lsb={} sao={}/{} tmvp={} rps s0={:?} s1={:?}",
                                        sh.first_slice,
                                        sh.dependent,
                                        sh.segment_address,
                                        sh.slice_type,
                                        sh.poc_lsb,
                                        sh.sao_luma as u8,
                                        sh.sao_chroma as u8,
                                        sh.temporal_mvp as u8,
                                        sh.rps.s0,
                                        sh.rps.s1
                                    );
                                }
                            }
                            Err(e) => {
                                if slices < 2 {
                                    println!("  slice parse: {e}");
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        println!(
            "NAL units: {} (vcl {vcl}), stream {} bytes",
            nals.len(),
            data.len()
        );
        return;
    }
    if args.len() > 1 && args[1] == "hevcpps" {
        let path = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let data = std::fs::read(path).expect("read");
        use rk_mpp_rs::codec::hevc;
        let nals = hevc::split_annex_b(&data);
        let mut sps_opt = None;
        let mut pps_opt = None;
        for n in &nals {
            if n.unit_type == 33 && sps_opt.is_none() {
                if let Ok(v) = hevc::parse_sps(&n.rbsp) {
                    sps_opt = Some(v);
                }
            }
            if n.unit_type == 34 && pps_opt.is_none() {
                if let Ok(v) = hevc::parse_pps(&n.rbsp) {
                    pps_opt = Some(v);
                }
            }
        }
        let (sps, pps) = match (sps_opt, pps_opt) {
            (Some(a), Some(b)) => (a, b),
            _ => {
                println!("could not parse parameter sets");
                return;
            }
        };
        let pkt = hevc::hw::build_pps_packet(&sps, &pps);
        let hex: Vec<String> = pkt.iter().map(|b| format!("{b:02x}")).collect();
        println!("{}", hex.join(" "));
        return;
    }
    if args.len() > 1 && args[1] == "hevcdpb" {
        let path = args.get(2).map(|s| s.as_str()).unwrap_or("");
        if path.is_empty() {
            println!("usage: rk-mpp-rs hevcdpb <annexb.265>");
            return;
        }
        let data = std::fs::read(path).expect("read stream");
        use rk_mpp_rs::codec::hevc;
        let nals = hevc::split_annex_b(&data);
        let mut sps: Option<hevc::Sps> = None;
        let mut pps: Option<hevc::Pps> = None;
        let mut dpb = hevc::Dpb::new();
        let mut slot = 0u8;
        for n in &nals {
            match n.unit_type {
                hevc::parser::NAL_SPS => {
                    let parsed = hevc::parse_sps(&n.rbsp).expect("sps");
                    println!(
                        "sps st_sets={} lt_present={} scaling_list={} lt_sps={:?}",
                        parsed.short_term_ref_pic_sets.len(),
                        parsed.long_term_ref_pics_present,
                        parsed.scaling_list_enabled,
                        parsed.long_term_refs_sps,
                    );
                    sps = Some(parsed);
                }
                hevc::parser::NAL_PPS => {
                    let parsed = hevc::parse_pps(&n.rbsp).expect("pps");
                    println!("pps scaling_list_data={}", parsed.scaling_list.is_some());
                    pps = Some(parsed);
                }
                t if t <= 31 => {
                    let (sps_ref, pps_ref) = match (&sps, &pps) {
                        (Some(a), Some(b)) => (a, b),
                        _ => continue,
                    };
                    let sh = match hevc::parse_slice_header(&n.rbsp, sps_ref, pps_ref, t) {
                        Ok(sh) => sh,
                        Err(_) => continue,
                    };
                    if !sh.first_slice {
                        continue;
                    }
                    let irap_no_rasl = t == 19 || t == 20 || (21..=23).contains(&t);
                    let poc = dpb.compute_poc(&sh, sps_ref, irap_no_rasl);
                    let l0 = dpb.build_ref_list(
                        &sh.rps,
                        &sh.long_term_rps,
                        poc,
                        sps_ref.log2_max_pic_order_cnt_lsb,
                    );
                    println!(
                        "type={} poc={} sao={}{} st={:?} lt={:?} refs={:?} dpb={:?}",
                        sh.slice_type,
                        poc,
                        sh.sao_luma as u8,
                        sh.sao_chroma as u8,
                        sh.rps,
                        sh.long_term_rps.refs,
                        l0,
                        dpb.frames().iter().map(|f| f.poc).collect::<Vec<_>>()
                    );
                    if t < 16 || (16..=23).contains(&t) {
                        dpb.refresh_references(
                            &sh.rps,
                            &sh.long_term_rps,
                            poc,
                            sps_ref.log2_max_pic_order_cnt_lsb,
                        );
                        dpb.push(hevc::FrameStore {
                            poc,
                            slot,
                            buf: slot as usize,
                            referenced: false,
                            error: false,
                            long_term: false,
                        });
                        slot = slot.wrapping_add(1) % 16;
                    }
                }
                _ => {}
            }
        }
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
                    let sps = match sps_map
                        .get(pps.sps_id as usize % 32)
                        .and_then(|s| s.as_ref())
                    {
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
                        &n.rbsp,
                        sps,
                        pps,
                        n.unit_type,
                        n.ref_idc,
                    ) {
                        Ok(sh) => sh,
                        Err(e) => {
                            println!(
                                "slice err: {e} (nal type={} ref_idc={} len={} first8={:02x?})",
                                n.unit_type,
                                n.ref_idc,
                                n.rbsp.len(),
                                &n.rbsp[..n.rbsp.len().min(8)]
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
                    let (l0, l1) = d.build_ref_lists(&sh, poc, sps.log2_max_frame_num);
                    let p_list = d.build_p_ref_list(&sh, sps.log2_max_frame_num);
                    let table: Vec<(u8, i32)> = d
                        .reference_table()
                        .iter()
                        .map(|frame| (frame.slot, frame.poc))
                        .collect();
                    println!(
                        "fn={:<2} poc={:<4} type={} dpb={} table={:?} mmco={:?} mods={:?}/{:?} p={:?} refs_l0={:?} l1={:?}",
                        sh.frame_num,
                        poc,
                        sh.slice_type,
                        d.frames().len(),
                        table,
                        sh.mmco,
                        sh.mod_l0,
                        sh.mod_l1,
                        p_list,
                        l0,
                        l1
                    );
                    if n.ref_idc != 0 {
                        d.prepare_reference_insertion(sh.adaptive_ref_pic_buffering);
                        let slot = d.free_slot(16).unwrap_or(0);
                        d.push(FrameStore {
                            frame_num: sh.frame_num,
                            poc,
                            top_poc: poc,
                            bottom_poc: poc,
                            long_term: false,
                            long_term_frame_idx: 0,
                            slot,
                            buf: slot as usize,
                            referenced: true,
                            colmv_valid: true,
                            field_coded: sh.field_pic,
                            top_present: !sh.field_pic || !sh.bottom_field,
                            bottom_present: !sh.field_pic || sh.bottom_field,
                            top_used: !sh.field_pic || !sh.bottom_field,
                            bottom_used: !sh.field_pic || sh.bottom_field,
                            error: 0,
                        });
                        if !sh.mmco.is_empty() {
                            d.apply_mmco(&sh.mmco, sh.frame_num, 0, 1u32 << sps.log2_max_frame_num);
                        }
                    }
                    d.update_poc_state(&sh, sps, poc);
                    out_order.push(poc);
                }
                _ => {}
            }
        }
        println!("output order by POC: {:?}", out_order);
        return;
    }
    if args.len() > 1 && args[1] == "m2vdec" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("/tmp/m2v.yuv");
        if src.is_empty() {
            eprintln!("usage: rk-mpp-rs m2vdec <in.m2v> <out.yuv>");
            std::process::exit(1);
        }
        let data = match std::fs::read(src) {
            Ok(data) => data,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let mut decoder = match rk_mpp_rs::codec::m2v::decoder::Decoder::new() {
            Ok(decoder) => decoder,
            Err(error) => {
                eprintln!("open vdpu2 failed: {error}");
                std::process::exit(1);
            }
        };
        use std::io::Write;
        let mut output = std::io::BufWriter::new(match std::fs::File::create(dst) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("{dst}: {error}");
                std::process::exit(1);
            }
        });
        let started = Instant::now();
        let pictures = match decoder.decode(&data) {
            Ok(pictures) => pictures,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
        let mut failed = false;
        let mut shown = 0usize;
        for picture in &pictures {
            if picture.err_info != 0 {
                failed = true;
                eprintln!(
                    "picture tr={} failed with err={:#x} irq={:#x}",
                    picture.temporal_reference, picture.err_info, decoder.last_irq
                );
            }
            output.write_all(&picture.yuv).expect("write MPEG-2 frame");
            shown += 1;
            if std::env::var_os("RFI_M2V_TRACE").is_some() {
                println!(
                    "  picture {shown}: tr={} {}x{} err={:#x} irq={:#010x}",
                    picture.temporal_reference,
                    picture.width,
                    picture.height,
                    picture.err_info,
                    decoder.last_irq,
                );
            }
        }
        output.flush().ok();
        println!(
            "decoded {shown} frames ({shown} output) in {decode_ms:.1} ms ({:.1} fps)",
            shown as f64 / (decode_ms / 1000.0).max(f64::EPSILON)
        );
        if failed {
            std::process::exit(1);
        }
        return;
    }
    if args.len() > 1 && args[1] == "mp4dec" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("/tmp/mp4.yuv");
        if src.is_empty() {
            eprintln!("usage: rk-mpp-rs mp4dec <in.m4v> <out.yuv>");
            std::process::exit(1);
        }
        let data = match std::fs::read(src) {
            Ok(data) => data,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let mut decoder = match rk_mpp_rs::codec::mpg4::decoder::Decoder::new() {
            Ok(decoder) => decoder,
            Err(error) => {
                eprintln!("open vdpu2 failed: {error}");
                std::process::exit(1);
            }
        };
        use std::io::Write;
        let mut output = std::io::BufWriter::new(match std::fs::File::create(dst) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("{dst}: {error}");
                std::process::exit(1);
            }
        });
        let started = Instant::now();
        let pictures = match decoder.decode(&data) {
            Ok(pictures) => pictures,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
        let mut failed = false;
        let mut shown = 0usize;
        for picture in &pictures {
            if picture.err_info != 0 {
                failed = true;
                eprintln!(
                    "picture {} failed with err={:#x} irq={:#x}",
                    shown + 1,
                    picture.err_info,
                    decoder.last_irq
                );
            }
            output.write_all(&picture.yuv).expect("write MPEG-4 frame");
            shown += 1;
            if std::env::var_os("RFI_MP4_TRACE").is_some() {
                println!(
                    "  picture {shown}: {}x{} err={:#x} irq={:#010x}",
                    picture.width, picture.height, picture.err_info, decoder.last_irq,
                );
            }
        }
        output.flush().ok();
        println!(
            "decoded {shown} frames ({shown} output) in {decode_ms:.1} ms ({:.1} fps)",
            shown as f64 / (decode_ms / 1000.0).max(f64::EPSILON)
        );
        if failed {
            std::process::exit(1);
        }
        return;
    }
    if args.len() > 1 && args[1] == "h263dec" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("/tmp/h263.yuv");
        if src.is_empty() {
            eprintln!("usage: rk-mpp-rs h263dec <in.263> <out.yuv>");
            std::process::exit(1);
        }
        let data = match std::fs::read(src) {
            Ok(data) => data,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let mut decoder = match rk_mpp_rs::codec::h263::decoder::Decoder::new() {
            Ok(decoder) => decoder,
            Err(error) => {
                eprintln!("open vdpu2 failed: {error}");
                std::process::exit(1);
            }
        };
        use std::io::Write;
        let mut output = std::io::BufWriter::new(match std::fs::File::create(dst) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("{dst}: {error}");
                std::process::exit(1);
            }
        });
        let started = Instant::now();
        let pictures = match decoder.decode(&data) {
            Ok(pictures) => pictures,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
        let mut failed = false;
        let mut shown = 0usize;
        for picture in &pictures {
            if picture.err_info != 0 {
                failed = true;
                eprintln!(
                    "picture tr={} failed with err={:#x} irq={:#x}",
                    picture.temporal_reference, picture.err_info, decoder.last_irq
                );
            }
            output.write_all(&picture.yuv).expect("write H.263 frame");
            shown += 1;
            if std::env::var_os("RFI_H263_TRACE").is_some() {
                println!(
                    "  picture {shown}: tr={} {} {}x{} err={:#x} irq={:#010x}",
                    picture.temporal_reference,
                    if picture.coding_type == rk_mpp_rs::codec::h263::parser::VOP_I {
                        "I"
                    } else {
                        "P"
                    },
                    picture.width,
                    picture.height,
                    picture.err_info,
                    decoder.last_irq,
                );
            }
        }
        output.flush().ok();
        println!(
            "decoded {shown} frames ({shown} output) in {decode_ms:.1} ms ({:.1} fps)",
            shown as f64 / (decode_ms / 1000.0).max(f64::EPSILON)
        );
        if failed {
            std::process::exit(1);
        }
        return;
    }
    if args.len() > 1 && args[1] == "vp8dec" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("/tmp/vp8.yuv");
        if src.is_empty() {
            eprintln!("usage: rk-mpp-rs vp8dec <in.ivf> <out.yuv>");
            std::process::exit(1);
        }
        let data = match std::fs::read(src) {
            Ok(data) => data,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let mut decoder = match rk_mpp_rs::codec::vp8::decoder::Decoder::new() {
            Ok(decoder) => decoder,
            Err(error) => {
                eprintln!("open vdpu2 failed: {error}");
                std::process::exit(1);
            }
        };
        use std::io::Write;
        let mut output = std::io::BufWriter::new(match std::fs::File::create(dst) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("{dst}: {error}");
                std::process::exit(1);
            }
        });
        let started = Instant::now();
        let outcome = decoder.decode_stream(&data);
        let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
        let mut failed = false;
        let mut shown = 0usize;
        for picture in &outcome.pictures {
            if picture.err_info != 0 {
                failed = true;
                eprintln!(
                    "picture {} failed with err={:#x} irq={:#x}",
                    shown + 1,
                    picture.err_info,
                    decoder.last_irq
                );
            }
            output.write_all(&picture.yuv).expect("write VP8 frame");
            shown += 1;
            if std::env::var_os("RFI_VP8_TRACE").is_some() {
                println!(
                    "  picture {shown}: {}x{} err={:#x} irq={:#010x}",
                    picture.width, picture.height, picture.err_info, decoder.last_irq,
                );
            }
        }
        for drop in &outcome.dropped {
            eprintln!("packet {} dropped: {}", drop.packet, drop.reason);
        }
        output.flush().ok();
        println!(
            "decoded {shown} frames ({} output, {} packets dropped{}) in {decode_ms:.1} ms ({:.1} fps)",
            shown,
            outcome.dropped.len(),
            if outcome.stopped_early { ", stopped early" } else { "" },
            shown as f64 / (decode_ms / 1000.0).max(f64::EPSILON)
        );
        if outcome.stopped_early {
            std::process::exit(1);
        }
        if failed {
            // The reference harness prints failed pictures and still exits 0;
            // the board table of board_vp_corrupt_vs_mpp.sh matched on that.
            eprintln!("some pictures carried hardware error flags");
        }
        return;
    }
    if args.len() > 1 && args[1] == "vp8" {
        // VP8 frame-header dump: no hardware needed, same role as `m2v`.
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        if src.is_empty() {
            eprintln!("usage: rk-mpp-rs vp8 <in.ivf>");
            std::process::exit(1);
        }
        let data = match std::fs::read(src) {
            Ok(data) => data,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let frames = match rk_mpp_rs::codec::vp8::parser::parse_ivf(&data) {
            Ok(frames) => frames,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let mut parser = rk_mpp_rs::codec::vp8::parser::Parser::new();
        for (index, frame) in frames.iter().enumerate() {
            match parser.parse_picture(frame.data) {
                Ok(Some(picture)) => println!(
                    "frame {:>3}: {} {}x{} show={} parts={} qindex={} filt {} level={} sharp={} seg={} bool={}:{} start={}+{} len={} first={}",
                    index + 1,
                    if picture.key_frame { "I" } else { "P" },
                    picture.width,
                    picture.height,
                    picture.show_frame,
                    1 << picture.log2_dct_partitions,
                    picture.base_qindex,
                    picture.loop_filter_type,
                    picture.loop_filter_level,
                    picture.sharpness,
                    picture.segmentation_enabled,
                    picture.bool_value,
                    picture.bool_range,
                    picture.stream_start_offset,
                    picture.stream_start_bit,
                    picture.stream_end_pos,
                    picture.offset_to_dct_parts,
                ),
                Ok(None) => println!("frame {:>3}: dropped, no key frame yet", index + 1),
                Err(error) => {
                    eprintln!("frame {:>3}: {error}", index + 1);
                    std::process::exit(1);
                }
            }
        }
        return;
    }
    if args.len() > 1 && args[1] == "m2v" {
        // MPEG-2 header dump: no hardware needed, same role as `h264`/`hevc`.
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        if src.is_empty() {
            eprintln!("usage: rk-mpp-rs m2v <in.m2v>");
            std::process::exit(1);
        }
        let data = match std::fs::read(src) {
            Ok(data) => data,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        use rk_mpp_rs::codec::m2v::parser::Unit;
        if std::env::var("RKMPP_DEBUG").is_ok() {
            for (code, payload) in rk_mpp_rs::codec::m2v::parser::split_start_codes(&data) {
                if let Err(error) = rk_mpp_rs::codec::m2v::parser::parse_unit(code, payload) {
                    eprintln!("code {code:#04x} len {}: {error}", payload.len());
                }
            }
        }
        match rk_mpp_rs::codec::m2v::parser::parse_units(&data) {
            Ok(units) => {
                let mut pictures = 0usize;
                let mut slices = 0usize;
                for unit in &units {
                    match unit {
                        Unit::SequenceHeader(s) => println!(
                            "sequence {}x{} aspect={} rate={}",
                            s.horizontal_size_value,
                            s.vertical_size_value,
                            s.aspect_ratio_information,
                            s.frame_rate_code
                        ),
                        Unit::SequenceExtension(e) => println!(
                            "  seq_ext profile_level={:#04x} progressive={} chroma={}",
                            e.profile_and_level_indication, e.progressive_sequence, e.chroma_format
                        ),
                        Unit::SequenceDisplayExtension(e) => println!(
                            "  seq_disp signal_type={} colour={}",
                            e.video_signal_type, e.colour_description
                        ),
                        Unit::QuantMatrixExtension(e) => println!(
                            "  quant_ext intra={} non_intra={} chroma_intra={} chroma_non_intra={}",
                            e.load_intra_quant_matrix,
                            e.load_non_intra_quant_matrix,
                            e.load_intra_chroma_quant_matrix,
                            e.load_non_intra_chroma_quant_matrix
                        ),
                        Unit::Picture(h) => {
                            pictures += 1;
                            println!(
                                "picture {} tr={} vbv={}",
                                h.coding_letter(),
                                h.temporal_reference,
                                h.vbv_delay
                            );
                        }
                        Unit::PictureCodingExtension(e) => println!(
                            "  pic_ext fcode={:?} structure={} progressive_frame={}",
                            e.f_code, e.picture_structure, e.progressive_frame
                        ),
                        Unit::Slice { .. } => slices += 1,
                        Unit::UserData(_) => println!("user_data"),
                        Unit::Other { code, .. } => println!("other code={code:#04x}"),
                    }
                }
                println!(
                    "{} units, {pictures} pictures, {slices} slices",
                    units.len()
                );
            }
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        }
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
                        pps.pps_id,
                        pps.sps_id,
                        pps.entropy_coding,
                        pps.num_ref_idx_l0,
                        pps.deblocking_filter_control,
                        pps.transform_8x8,
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
                        h.slice_type,
                        h.first_mb,
                        h.frame_num,
                        h.idr_pic_id.is_some(),
                        h.poc_lsb
                    ),
                    Err(e) => println!("slice parse: {e}"),
                }
            }
        }
        println!(
            "NAL units: {} (vcl {vcl}), stream {} bytes",
            nals.len(),
            data.len()
        );
        return;
    }
    if args.len() > 1 && args[1] == "probe-frame" {
        // Write the synthetic NV12 frame the encoder tests encode, so that
        // `mpi_enc_test` can be pointed at byte-identical input.
        let dst = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let width = args.get(3).and_then(|s| s.parse::<usize>().ok());
        let height = args.get(4).and_then(|s| s.parse::<usize>().ok());
        let (Some(width), Some(height)) = (width, height) else {
            eprintln!("usage: rk-mpp-rs probe-frame <out.nv12> <width> <height>");
            std::process::exit(2);
        };
        if dst.is_empty() {
            eprintln!("usage: rk-mpp-rs probe-frame <out.nv12> <width> <height>");
            std::process::exit(2);
        }
        match std::fs::write(dst, rk_mpp_rs::hal::h264e::probe_nv12(width, height)) {
            Ok(()) => println!("wrote {width}x{height} probe frame to {dst}"),
            Err(error) => {
                eprintln!("{dst}: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() > 1 && args[1] == "bench-scale" {
        let path = args
            .get(2)
            .map(|s| s.as_str())
            .unwrap_or("/tmp/src1080.rgb");
        let sw: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1920);
        let sh: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1080);
        let iters: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(200);
        bench_scale(path, sw, sh, iters);
        return;
    }
    if args.len() > 1 && args[1] == "iep-probe" {
        // Client 28 = IEP2 (deinterlace). The client binds per the capability
        // survey, but no job has ever been driven from this crate, so this is
        // the first thing to run on-board: does it bind, does it answer.
        let r = iep_session::probe();
        println!("IEP client 28:");
        println!("  kernel present (probe_hw_support bit 28): {}", r.present);
        println!(
            "  session init (INIT_CLIENT_TYPE):         {}",
            if r.binds { "binds" } else { "FAIL" }
        );
        if !r.binds {
            if let Some(e) = r.open_errno {
                println!("    errno: {e} ({})", std::io::Error::from_raw_os_error(e));
            }
        }
        println!(
            "  SET_REG_ADDR_OFFSET:                     {}",
            if r.reg_offset { "yes" } else { "no" }
        );
        println!(
            "  POLL_HW_FINISH:                          {}",
            if r.poll { "yes" } else { "no" }
        );
        println!(
            "  hw_id: {}",
            match r.hw_id {
                Some(id) => id.to_string(),
                None => "-".to_string(),
            }
        );
        return;
    }
    if args.len() > 1 && args[1] == "iep-dei" {
        // One I5O2 deinterlace: three source frames in, two progressive frames
        // out. `iep-dei <in.yuv> <out.yuv> <width> <height> [TFF|BFF]`.
        use rk_mpp_rs::hal::iep::{Iep2DilMode, Iep2FieldOrder, Iep2Fmt, Iep2OutMode};
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("");
        let width: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
        let height: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
        let field_order = match args.get(6).map(|s| s.as_str()) {
            Some("BFF") => Iep2FieldOrder::Bff,
            _ => Iep2FieldOrder::Tff,
        };
        if src.is_empty() || dst.is_empty() || width == 0 || height == 0 {
            eprintln!("usage: rk-mpp-rs iep-dei <in.yuv> <out.yuv> <width> <height> [TFF|BFF]");
            std::process::exit(2);
        }
        let cfg = iep_session::DeinterlaceConfig {
            width,
            height,
            hor_stride: width,
            src_fmt: Iep2Fmt::Yuv420,
            dst_fmt: Iep2Fmt::Yuv420,
            dil_mode: Iep2DilMode::I5O2,
            out_mode: Iep2OutMode::Line,
            field_order,
            ..iep_session::DeinterlaceConfig::default()
        };
        let data = match std::fs::read(src) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{src}: {e}");
                std::process::exit(1);
            }
        };
        match run_iep_dei(&data, dst, cfg) {
            Ok(frames) => println!("iep-dei: wrote {frames} deinterlaced frames to {dst}"),
            Err(e) => {
                eprintln!("iep-dei: {e}");
                std::process::exit(1);
            }
        }
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
                    // Two different questions, and only the second one decides
                    // whether a block can be driven from userspace: does the
                    // kernel know the hardware exists, and did it bind a driver
                    // session for it. MPP prints its own version of the second
                    // answer as "client N driver is not ready!".
                    println!("  hw support mask: 0x{mask:08x}");
                    println!("    block                                   kernel  session  reg-offset  poll");
                    for (c, name) in Client::all() {
                        let bit = *c as u32;
                        let has = (mask >> bit) & 1 == 1;
                        let (bind, flags) = match MppService::open(bit) {
                            Ok(probe_svc) => (
                                "binds".to_string(),
                                format!(
                                    "{:>7}  {:>4}",
                                    if probe_svc.cmd_supported(MPP_CMD_SET_REG_ADDR_OFFSET) {
                                        "yes"
                                    } else {
                                        "no"
                                    },
                                    if probe_svc.cmd_supported(MPP_CMD_POLL_HW_FINISH) {
                                        "yes"
                                    } else {
                                        "no"
                                    }
                                ),
                            ),
                            Err(error) => (
                                format!("ENO{}", error.raw_os_error().unwrap_or(0)),
                                String::new(),
                            ),
                        };
                        println!(
                            "    {:<40} {:>6}  {:<9}{}",
                            format!("[{:>3}] {name}", bit),
                            if has { "present" } else { "-" },
                            bind,
                            flags
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
