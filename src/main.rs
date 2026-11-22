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
use rk_mpp_rs::hal::jpege720;
use rk_mpp_rs::hal::vepu510_h264;
use rk_mpp_rs::hal::vepu510_h265;
use rk_mpp_rs::scale;
use rk_mpp_rs::service::buffer_pool::BufferPool;
use rk_mpp_rs::service::dma_heap;
use rk_mpp_rs::service::soc;
use rk_mpp_rs::service::MppService;

use rk_mpp_rs::service::*;
use std::collections::VecDeque;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::sync::mpsc;
use std::time::Instant;

fn us(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

enum FrameSource {
    Mapped(MappedFile),
    Stream(std::io::BufReader<std::fs::File>),
}

impl FrameSource {
    fn open(path: &str) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|error| format!("{path}: {error}"))?;
        let regular = file
            .metadata()
            .map(|metadata| metadata.is_file() && metadata.len() > 0)
            .unwrap_or(false);
        if regular {
            match MappedFile::new(file) {
                Ok(mapped) => return Ok(Self::Mapped(mapped)),
                Err(_) => {
                    let file =
                        std::fs::File::open(path).map_err(|error| format!("{path}: {error}"))?;
                    return Ok(Self::Stream(std::io::BufReader::new(file)));
                }
            }
        }
        Ok(Self::Stream(std::io::BufReader::new(file)))
    }
}

impl Read for FrameSource {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Mapped(mapped) => mapped.read(buffer),
            Self::Stream(stream) => stream.read(buffer),
        }
    }
}

struct MappedFile {
    _file: std::fs::File,
    ptr: *const u8,
    len: usize,
    position: usize,
}

impl MappedFile {
    fn new(file: std::fs::File) -> std::io::Result<Self> {
        let len = usize::try_from(file.metadata()?.len()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "input file is too large to map",
            )
        })?;
        // SAFETY: the descriptor stays open in `_file`, the mapping is
        // read-only, and `len` is the exact regular-file length queried above.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_PRIVATE,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            _file: file,
            ptr: ptr.cast(),
            len,
            position: 0,
        })
    }
}

impl Read for MappedFile {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = buffer.len().min(self.len - self.position);
        if count == 0 {
            return Ok(0);
        }
        // SAFETY: `position + count <= len`, the mapping is live until Drop,
        // and `buffer` is a separate writable allocation.
        unsafe {
            std::ptr::copy_nonoverlapping(self.ptr.add(self.position), buffer.as_mut_ptr(), count);
        }
        self.position += count;
        Ok(count)
    }
}

impl Drop for MappedFile {
    fn drop(&mut self) {
        // SAFETY: `new` mapped exactly `len` bytes and this value owns that
        // mapping until it is unmapped here once.
        unsafe {
            libc::munmap(self.ptr.cast_mut().cast(), self.len);
        }
    }
}

struct DmaInputFrame {
    buffer: dma_heap::DmaBuf,
    index: usize,
}

enum DmaInputEvent {
    Frame(DmaInputFrame),
    Error(String),
}

struct DmaInputPipeline {
    frames: mpsc::Receiver<DmaInputEvent>,
    recycle: mpsc::SyncSender<dma_heap::DmaBuf>,
    stop: mpsc::SyncSender<()>,
    worker: Option<std::thread::JoinHandle<()>>,
}

enum Av1CliDecoder {
    Vdpu34x(rk_mpp_rs::codec::av1::decoder::Decoder),
    Vdpu383(rk_mpp_rs::codec::av1::decoder383::Decoder),
}

impl Av1CliDecoder {
    fn new() -> Result<Self, String> {
        let profile = soc::SocProfile::detect().map_err(|error| error.to_string())?;
        match profile.video_decode {
            soc::VideoDecodeHal::Vdpu383 => Ok(Self::Vdpu383(
                rk_mpp_rs::codec::av1::decoder383::Decoder::new(),
            )),
            _ => rk_mpp_rs::codec::av1::decoder::Decoder::new()
                .map(Self::Vdpu34x)
                .map_err(|error| error.to_string()),
        }
    }

    fn decode_packet(
        &mut self,
        packet: &[u8],
    ) -> Result<Vec<rk_mpp_rs::codec::av1::decoder::Picture>, String> {
        match self {
            Self::Vdpu34x(decoder) => decoder.decode_packet(packet),
            Self::Vdpu383(decoder) => decoder.decode_packet(packet),
        }
    }

    fn status(&self) -> (u32, u32, u32) {
        match self {
            Self::Vdpu34x(decoder) => decoder.status(),
            Self::Vdpu383(decoder) => decoder.status(),
        }
    }

    fn recycle_output(&mut self, buffer: Vec<u8>) {
        match self {
            Self::Vdpu34x(decoder) => decoder.recycle_output(buffer),
            Self::Vdpu383(decoder) => decoder.recycle_output(buffer),
        }
    }

    fn set_output_dmabuf(&mut self, enabled: bool) {
        match self {
            Self::Vdpu34x(decoder) => decoder.set_output_dmabuf(enabled),
            Self::Vdpu383(decoder) => decoder.set_output_dmabuf(enabled),
        }
    }
}

impl DmaInputPipeline {
    fn start(
        path: &str,
        frame_bytes: usize,
        wanted: usize,
        surface_count: usize,
    ) -> Result<Self, String> {
        if surface_count < 2 {
            return Err("encoder input pipeline needs at least two surfaces".into());
        }
        let (frame_tx, frames) = mpsc::sync_channel(1);
        let (recycle, recycle_rx) = mpsc::sync_channel(surface_count);
        let (stop, stop_rx) = mpsc::sync_channel(1);
        for _ in 0..surface_count {
            let buffer = dma_heap::DmaBuf::alloc("system-uncached", frame_bytes, false)
                .map_err(|error| format!("allocate encoder input surface: {error}"))?;
            recycle
                .send(buffer)
                .map_err(|_| "encoder input pipeline stopped during setup".to_string())?;
        }
        let path = path.to_string();
        let reader_cpu = select_input_reader_cpu();
        let worker = std::thread::Builder::new()
            .name("rk-mpp-h264-input".to_string())
            .spawn(move || {
                if let Some(cpu) = reader_cpu {
                    pin_current_thread(cpu);
                }
                let result = (|| -> Result<(), String> {
                    let mut source = FrameSource::open(&path)?;
                    for index in 0..wanted {
                        let mut buffer = recycle_rx
                            .recv()
                            .map_err(|_| "encoder input pipeline was closed".to_string())?;
                        fill_dma_input(&mut buffer, frame_bytes, &mut source)?;
                        frame_tx
                            .send(DmaInputEvent::Frame(DmaInputFrame { buffer, index }))
                            .map_err(|_| "encoder stopped reading input".to_string())?;
                    }
                    let _ = stop_rx.recv();
                    Ok(())
                })();
                if let Err(error) = result {
                    let _ = frame_tx.send(DmaInputEvent::Error(error));
                }
            })
            .map_err(|error| format!("start H.264 input pipeline: {error}"))?;
        Ok(Self {
            frames,
            recycle,
            stop,
            worker: Some(worker),
        })
    }

    fn next(&self, expected: usize) -> Result<dma_heap::DmaBuf, String> {
        match self
            .frames
            .recv()
            .map_err(|_| "encoder input pipeline stopped unexpectedly".to_string())?
        {
            DmaInputEvent::Frame(frame) => {
                if frame.index != expected {
                    return Err(format!(
                        "encoder input pipeline frame {} arrived while waiting for {}",
                        frame.index, expected
                    ));
                }
                Ok(frame.buffer)
            }
            DmaInputEvent::Error(error) => Err(error),
        }
    }

    fn recycle(&self, buffer: dma_heap::DmaBuf) -> Result<(), String> {
        self.recycle
            .send(buffer)
            .map_err(|_| "encoder input pipeline stopped before recycle".to_string())
    }

    fn finish(mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn fill_dma_input(
    buffer: &mut dma_heap::DmaBuf,
    expected: usize,
    source: &mut impl Read,
) -> Result<(), String> {
    if expected > buffer.len() {
        return Err(format!(
            "encoder input surface is {} bytes, needs {expected}",
            buffer.len()
        ));
    }
    buffer
        .sync_write_begin()
        .map_err(|error| format!("encoder input sync begin: {error}"))?;
    let read_result = (|| {
        let mut filled = 0usize;
        let target = &mut buffer.as_mut_slice()[..expected];
        while filled < expected {
            match source.read(&mut target[filled..]) {
                Ok(0) => return Err(format!("short at byte {filled}, need {expected} bytes")),
                Ok(count) => filled += count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(())
    })();
    buffer
        .sync_write_end()
        .map_err(|error| format!("encoder input flush: {error}"))?;
    read_result
}

#[cfg(target_os = "linux")]
fn select_input_reader_cpu() -> Option<usize> {
    if let Some(cpu) = std::env::var("RKMPP_INPUT_CPU")
        .ok()
        .and_then(|cpu| cpu.parse::<usize>().ok())
    {
        return Some(cpu);
    }
    // SAFETY: both cpu_set_t buffers are initialized before use, their size is
    // passed to the kernel, and CPU ids are checked against CPU_SETSIZE.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) != 0 {
            return None;
        }
        let current = libc::sched_getcpu();
        (0..libc::CPU_SETSIZE as usize)
            .rev()
            .find(|cpu| libc::CPU_ISSET(*cpu, &set) && *cpu as i32 != current)
    }
}

#[cfg(not(target_os = "linux"))]
fn select_input_reader_cpu() -> Option<usize> {
    None
}

#[cfg(target_os = "linux")]
fn pin_current_thread(cpu: usize) {
    if cpu >= libc::CPU_SETSIZE as usize {
        return;
    }
    // SAFETY: `set` is zeroed, CPU_SET stays within its fixed capacity, and
    // the exact cpu_set_t size is passed to sched_setaffinity.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(cpu, &mut set);
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
    }
}

#[cfg(not(target_os = "linux"))]
fn pin_current_thread(_cpu: usize) {}

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
    rc_qp_bounds: Option<(u32, u32)>,
    hier_qp: Option<([i32; 4], [i32; 4])>,
    smart_p_vi_len: Option<u32>,
    max_reencode_times: Option<u32>,
    super_frame: Option<(u8, u32, u32, u8)>,
    drop_frame: Option<(u8, u32, u32)>,
    debreath_strength: Option<u8>,
    deblock: Option<(u32, i32, i32)>,
    cabac_init_idc: Option<u8>,
    constraint_set: Option<u32>,
    base_layer_pid: Option<u8>,
    entropy_coding: Option<bool>,
    transform_8x8: Option<bool>,
    rois: Vec<h264_syntax::RoiRegion>,
    gdr: Option<(h264_syntax::GdrMode, u32)>,
    split: Option<(h264_syntax::SplitMode, u32)>,
    split_out: u32,
) -> Result<(usize, usize, usize, usize, f64), String> {
    // Never slurp the clip into a Vec. A regular file is mapped and its pages
    // are copied one picture at a time; when process affinity covers multiple
    // CPUs, the next surface is filled while the encoder drives the current
    // one. Pipes and other non-regular inputs retain the streaming fallback.
    let pipeline_compatible = bps.is_none()
        && gdr.is_none()
        && debreath_strength.is_none()
        && hier_qp.is_none()
        && smart_p_vi_len.is_none()
        && max_reencode_times.is_none()
        && super_frame.is_none()
        && drop_frame.is_none()
        && split_out & h264_syntax::MPP_ENC_SPLIT_OUT_LOWDELAY == 0;
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
    if let Some((min, max)) = rc_qp_bounds {
        config.rc_qp_min = min;
        config.rc_qp_max = max;
    }
    if let Some((delta, frames)) = hier_qp {
        config.hier_qp_enabled = true;
        config.hier_qp_delta = delta;
        config.hier_frame_num = frames;
    }
    if let Some(vi_len) = smart_p_vi_len {
        config.smart_vi_len = vi_len;
        // The smart reference schedule keeps the IDR long-term plus one working
        // short-term picture, which is the two-reference DPB the C SPS advertises.
        config.num_ref_frames = 2;
    }
    if let Some(times) = max_reencode_times {
        config.max_reencode_times = times;
    }
    if let Some((mode, i_thd, p_thd, priority)) = super_frame {
        config.super_mode = mode;
        config.super_i_thd = i_thd;
        config.super_p_thd = p_thd;
        config.rc_priority = priority;
    }
    if let Some((mode, threshold, gap)) = drop_frame {
        config.drop_mode = mode;
        config.drop_threshold = threshold;
        config.drop_gap = gap;
    }
    if let Some(strength) = debreath_strength {
        config.debreath_en = true;
        config.debreath_strength = strength;
    }
    if config.split_out & h264_syntax::MPP_ENC_SPLIT_OUT_LOWDELAY != 0 {
        config.max_reencode_times = 0;
    }
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
    if let Some(entropy_coding) = entropy_coding {
        config.entropy_coding = entropy_coding;
    }
    if let Some(transform_8x8) = transform_8x8 {
        config.transform_8x8 = transform_8x8;
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
    if let Some(cabac_init_idc) = cabac_init_idc {
        config.cabac_init_idc = cabac_init_idc;
    }
    if let Some(constraint_set) = constraint_set {
        config.constraint_set_force = ((constraint_set >> 16) & 0x3f) as u8;
        config.constraint_set_value = (constraint_set & 0x3f) as u8;
    }
    if let Some(base_layer_pid) = base_layer_pid {
        config.base_layer_pid = base_layer_pid;
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
    config.split_out = split_out;
    if std::env::var_os("H264_QBIAS_EN").is_some()
        || std::env::var_os("H264_QBIAS_I").is_some()
        || std::env::var_os("H264_QBIAS_P").is_some()
    {
        let read = |name: &str| -> Result<u32, String> {
            match std::env::var(name) {
                Ok(value) => value
                    .parse()
                    .map_err(|_| format!("{name} must be an unsigned integer, got {value}")),
                Err(_) => Ok(0),
            }
        };
        config.qbias = Some((
            read("H264_QBIAS_EN")? != 0,
            read("H264_QBIAS_I")?,
            read("H264_QBIAS_P")?,
        ));
    }
    let frame_bytes = h264e::input_size(&config)?;
    let mut encoder = h264e::Vepu580H264::open(config)?;
    if let Ok(value) = std::env::var("H264_SCENE_MODE") {
        let scene_mode = value
            .parse::<u8>()
            .ok()
            .filter(|value| *value <= 2)
            .ok_or_else(|| format!("H264_SCENE_MODE must be 0, 1 or 2, got {value}"))?;
        encoder.set_scene_mode(scene_mode == 1);
    }
    let started = Instant::now();
    // Repeated pictures, which is what the C harness does with a one-frame raw
    // file too: every frame it wants and the file is short, it re-reads the same
    // bytes. `frames` > 1 exercises the P path against that reference.
    // The frame stride comes from the encoder geometry, not from the file's
    // length: dividing the input by the frame count silently mis-slices every
    // picture whenever the file holds more or fewer frames than asked for.
    let per_frame = frame_bytes;
    let wanted = frames.max(1);
    let input_parallelism = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1);
    let pipeline_requested = std::env::var("RKMPP_ENCODER_PIPELINE").as_deref() == Ok("1");
    let pipeline_depth = std::env::var("RKMPP_ENCODER_PIPELINE_DEPTH")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 2);
    let hardware_pipeline = pipeline_compatible && pipeline_requested && input_parallelism > 1;
    if hardware_pipeline {
        encoder.set_pipeline_depth(pipeline_depth);
        encoder.set_pipeline(true);
    }
    let pipeline = if input_parallelism > 1 {
        Some(DmaInputPipeline::start(
            path,
            per_frame,
            wanted,
            if hardware_pipeline {
                pipeline_depth + 1
            } else {
                2
            },
        )?)
    } else {
        None
    };
    let mut source = if pipeline.is_none() {
        Some(FrameSource::open(path)?)
    } else {
        None
    };
    let mut output = if out_path == "/dev/null" {
        None
    } else {
        Some(std::io::BufWriter::with_capacity(
            1024 * 1024,
            std::fs::File::create(out_path).map_err(|error| format!("{out_path}: {error}"))?,
        ))
    };
    let mut total_bytes = 0usize;
    let mut headers = 0usize;
    let mut payload = 0usize;
    let mut packets = 0usize;
    let trace_packets = std::env::var_os("RK_MPP_TRACE_PACKETS").is_some();
    let mut in_flight = VecDeque::new();
    let mut emit = |encoded: h264e::EncodedFrame| -> Result<(), String> {
        headers += encoded.headers.len();
        payload += encoded.payload.len();
        packets += encoded.packets.len();
        let output_len = if encoded.annex_b.is_empty() {
            encoded.packets.iter().map(Vec::len).sum()
        } else {
            encoded.annex_b.len()
        };
        total_bytes += output_len;
        if trace_packets {
            let sizes = encoded
                .packets
                .iter()
                .map(|packet| packet.len().to_string())
                .collect::<Vec<_>>()
                .join(",");
            eprintln!(
                "h264 frame {}: {} packet(s): {sizes}",
                encoded.frame_num,
                encoded.packets.len()
            );
        }
        if let Some(output) = output.as_mut() {
            use std::io::Write;
            if encoded.annex_b.is_empty() {
                for packet in &encoded.packets {
                    output
                        .write_all(packet)
                        .map_err(|error| format!("{out_path}: {error}"))?;
                }
            } else {
                output
                    .write_all(&encoded.annex_b)
                    .map_err(|error| format!("{out_path}: {error}"))?;
            }
        }
        Ok(())
    };
    for index in 0..wanted {
        let completed = if let Some(pipeline) = pipeline.as_ref() {
            let input = pipeline.next(index)?;
            let input_fd = input.fd();
            if hardware_pipeline {
                in_flight.push_back(input);
                encoder.encode_frame_fd_pipeline(input_fd)?
            } else {
                let encoded = encoder.encode_frame_fd(input_fd)?;
                pipeline.recycle(input)?;
                vec![encoded]
            }
        } else {
            let source = source.as_mut().expect("sequential input source");
            if !encoder.load_input_frame(per_frame, source)? {
                return Err(format!(
                    "{path}: short at frame {}, need {} bytes each",
                    index + 1,
                    per_frame
                ));
            }
            vec![encoder.encode_prepared_frame()?]
        };
        for encoded in completed {
            if hardware_pipeline {
                let input = in_flight
                    .pop_front()
                    .ok_or_else(|| "H.264 completion has no in-flight input".to_string())?;
                pipeline
                    .as_ref()
                    .expect("hardware pipeline owns an input pipeline")
                    .recycle(input)?;
            }
            emit(encoded)?;
        }
    }
    if hardware_pipeline {
        for encoded in encoder.flush_pipeline()? {
            let input = in_flight
                .pop_front()
                .ok_or_else(|| "H.264 flush has no in-flight input".to_string())?;
            pipeline
                .as_ref()
                .expect("hardware pipeline owns an input pipeline")
                .recycle(input)?;
            emit(encoded)?;
        }
    }
    drop(emit);
    if let Some(output) = output.as_mut() {
        use std::io::Write;
        output
            .flush()
            .map_err(|error| format!("{out_path}: {error}"))?;
    }
    let elapsed = started.elapsed();
    if let Some(pipeline) = pipeline {
        pipeline.finish();
    }
    Ok((total_bytes, headers, payload, packets, us(elapsed) / 1000.0))
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
        temporal_mvp: false,
        force_cu_qp_delta: false,
        scene_motion_flag: 2,
        qbias: None,
        vmaf_opt: false,
        rc: None,
        refresh: None,
        split: h265_syntax::SliceSplit::default(),
        syntax: h265_syntax::SyntaxOptions::default(),
        tile: None,
        roi_fd: -1,
        roi_qp_fd: -1,
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

fn parse_h265_split_option(
    text: &str,
    split: &mut h265_syntax::SliceSplit,
) -> Result<bool, String> {
    if let Some(rest) = text.strip_prefix("split:") {
        let Some((kind, arg)) = rest.split_once(':') else {
            return Err("expected split:ctu:<arg> or split:byte:<arg>".into());
        };
        split.mode = match kind {
            "ctu" => h265_syntax::SplitMode::ByCtu,
            "byte" => h265_syntax::SplitMode::ByByte,
            _ => return Err("expected split:ctu:<arg> or split:byte:<arg>".into()),
        };
        split.arg = arg
            .parse()
            .map_err(|_| format!("invalid H.265 slice split argument {arg}"))?;
        return Ok(true);
    }
    if text == "splitout:lowdelay" {
        split.out = h265_syntax::MPP_ENC_SPLIT_OUT_LOWDELAY;
        return Ok(true);
    }
    if let Some(value) = text.strip_prefix("splitout:") {
        split.out = value
            .parse()
            .map_err(|_| format!("invalid H.265 splitout value {value}"))?;
        return Ok(true);
    }
    Ok(false)
}

fn parse_h265_syntax_option(
    text: &str,
    options: &mut h265_syntax::SyntaxOptions,
) -> Result<bool, String> {
    if text == "tile:auto" {
        options.auto_tile = true;
        return Ok(true);
    }
    if text == "tile:off" {
        options.auto_tile = false;
        return Ok(true);
    }
    if let Some(rest) = text.strip_prefix("dblk:") {
        let parts: Vec<i32> = rest.split(':').filter_map(|v| v.parse().ok()).collect();
        let [disable, beta, tc] = parts.as_slice() else {
            return Err("expected dblk:disable:beta:tc".into());
        };
        options.deblock_disable = *disable != 0;
        options.deblock_beta_offset_div2 = *beta;
        options.deblock_tc_offset_div2 = *tc;
        return Ok(true);
    }
    if let Some(rest) = text.strip_prefix("sao:") {
        let parts: Vec<u32> = rest.split(':').filter_map(|v| v.parse().ok()).collect();
        let [luma_disable, chroma_disable] = parts.as_slice() else {
            return Err("expected sao:luma_disable:chroma_disable".into());
        };
        options.sao_luma_disable = *luma_disable != 0;
        options.sao_chroma_disable = *chroma_disable != 0;
        return Ok(true);
    }
    if let Some(rest) = text.strip_prefix("lpf:") {
        let parts: Vec<u32> = rest.split(':').filter_map(|v| v.parse().ok()).collect();
        let [across_slices, disable_tiles] = parts.as_slice() else {
            return Err("expected lpf:across_slices:disable_tiles".into());
        };
        options.loop_filter_across_slices = *across_slices != 0;
        options.loop_filter_across_tiles = *disable_tiles == 0;
        return Ok(true);
    }
    Ok(false)
}

fn parse_h265_roi_option(
    text: &str,
    regions: &mut Vec<rk_mpp_rs::codec::h265::roi::RoiRegion>,
) -> Result<bool, String> {
    let Some(rest) = text.strip_prefix("roi:") else {
        return Ok(false);
    };
    let parts: Vec<i64> = rest
        .split(':')
        .filter_map(|value| value.parse().ok())
        .collect();
    let [x, y, w, h, intra, absolute, qp] = parts.as_slice() else {
        return Err("expected roi:x:y:w:h:intra:abs:qp".into());
    };
    if *x < 0 || *y < 0 || *w <= 0 || *h <= 0 || *w > u16::MAX as i64 || *h > u16::MAX as i64 {
        return Err("H.265 ROI geometry is outside the unsigned region fields".into());
    }
    let qp = i16::try_from(*qp).map_err(|_| format!("H.265 ROI QP {qp} is outside i16"))?;
    regions.push(rk_mpp_rs::codec::h265::roi::RoiRegion {
        x: *x as u16,
        y: *y as u16,
        w: *w as u16,
        h: *h as u16,
        force_intra: *intra != 0,
        qp_absolute: *absolute != 0,
        qp_val: qp,
    });
    Ok(true)
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
    low_delay: bool,
) -> Result<(usize, usize, f64), String> {
    let input = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let config = jpege::EncoderConfig {
        quality,
        chroma,
        rotation,
        split_mcus,
        low_delay,
        ..jpege::EncoderConfig::new_for_input(width, height, input_format)
    };
    config.validate()?;
    let (expected_width, expected_height) = config.output_size();
    let mut encoder =
        jpege720::JpegEncoder::open().map_err(|e| format!("open JPEG encoder: {e}"))?;
    let started = Instant::now();
    let encoded = encoder.encode_packets(&input, config)?;
    let elapsed = started.elapsed();
    if std::env::var_os("RK_MPP_TRACE_PACKETS").is_some() {
        let sizes = encoded
            .packets
            .iter()
            .map(|packet| packet.len().to_string())
            .collect::<Vec<_>>()
            .join(",");
        eprintln!("jpeg frame 0: {} packet(s): {sizes}", encoded.packets.len());
    }
    let info =
        jpeg::parse(&encoded.bytes).map_err(|e| format!("encoded stream is invalid: {e}"))?;
    if info.width != expected_width || info.height != expected_height {
        return Err(format!(
            "encoded dimensions are {}x{}, expected {expected_width}x{expected_height}",
            info.width, info.height
        ));
    }
    std::fs::write(out_path, &encoded.bytes).map_err(|e| format!("{out_path}: {e}"))?;
    Ok((
        encoded.bytes.len(),
        encoded.packets.len(),
        us(elapsed) / 1000.0,
    ))
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
    let mut encoder =
        jpege720::JpegEncoder::open().map_err(|e| format!("open JPEG encoder: {e}"))?;
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

    let mut svc = MppService::open(VPU_CLIENT_JPEG_DEC).map_err(|e| e.to_string())?;

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

    svc.submit_regs_recovering(&regs.words)
        .map_err(|e| format!("submit: {e}"))?;
    svc.poll_hw_finish_recovering()
        .map_err(|e| format!("poll: {e}"))?;

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
    let mut svc = MppService::open(VPU_CLIENT_JPEG_DEC).map_err(|e| e.to_string())?;

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
        svc.submit_regs_recovering(&regs.words)
            .map_err(|e| e.to_string())?;
        svc.poll_hw_finish_recovering().map_err(|e| e.to_string())?;
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
        svc.submit_regs_recovering(&regs.words)
            .map_err(|e| e.to_string())?;
        svc.poll_hw_finish_recovering().map_err(|e| e.to_string())?;
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
    let mut svc = MppService::open(VPU_CLIENT_JPEG_DEC).map_err(|e| e.to_string())?;

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
        svc.submit_regs_recovering(&regs.words)
            .map_err(|e| e.to_string())?;
        svc.poll_hw_finish_recovering().map_err(|e| e.to_string())?;
    }
    let t = Instant::now();
    for _ in 0..iters {
        svc.submit_regs_recovering(&regs.words)
            .map_err(|e| e.to_string())?;
        svc.poll_hw_finish_recovering().map_err(|e| e.to_string())?;
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
    let mut svc = MppService::open(VPU_CLIENT_JPEG_DEC).map_err(|e| e.to_string())?;
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
    svc.submit_regs_recovering(&regs.words)
        .map_err(|e| format!("submit: {e}"))?;
    svc.poll_hw_finish_recovering()
        .map_err(|e| format!("poll: {e}"))?;
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
/// comes from the refcounted pool through an RAII lease. Normal completion and
/// every early `?` return both release the leases.
fn decode_job_pooled(
    data: &[u8],
    info: &jpeg::JpegInfo,
    downscale: decode::JpegDownscale,
    pool: &BufferPool<dma_heap::DmaBuf>,
) -> Result<Vec<u8>, String> {
    let tables = tables::Tables::build(info).map_err(|e| format!("{e:?}"))?;
    let output_geometry = decode::geometry(info, true, downscale).map_err(|e| e.to_string())?;
    let table = pool
        .acquire_lease(tables::RKD_TABLE_SIZE)
        .map_err(|e| format!("pool table: {e}"))?;
    let stream = pool
        .acquire_lease(data.len().max(1))
        .map_err(|e| format!("pool stream: {e}"))?;
    let output = pool
        .acquire_lease(output_geometry.frame_size)
        .map_err(|e| format!("pool output: {e}"))?;
    table.data_mut().unwrap()[..tables::RKD_TABLE_SIZE].copy_from_slice(tables.as_slice());
    stream.data_mut().unwrap()[..data.len()].copy_from_slice(data);
    let (regs, geom) = decode::build(&decode::Job {
        info,
        table_fd: table.fd().unwrap(),
        stream_fd: stream.fd().unwrap(),
        stream_len: data.len(),
        output_fd: output.fd().unwrap(),
        rgb: true,
        downscale,
    })
    .map_err(|e| e.to_string())?;
    // Declare the service after every pool lease: on both success and an early
    // `?`, Rust drops locals in reverse order, so the kernel client closes
    // before any shared dma-buf returns to the pool.
    let mut svc = MppService::open(VPU_CLIENT_JPEG_DEC).map_err(|e| e.to_string())?;
    svc.submit_regs_recovering(&regs.words)
        .map_err(|e| format!("submit: {e}"))?;
    svc.poll_hw_finish_recovering()
        .map_err(|e| format!("poll: {e}"))?;
    let frame = {
        let view = output.data().map_err(|e| format!("pool view: {e}"))?;
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
    // The RK3588 driver keeps the service's DMA imports alive until its client fd
    // closes. Releasing pooled dma-buf leases first lets another worker reuse
    // the same fds while the old service still owns kernel-side import state;
    // that path has previously ended in `mpp_dma_find_buffer_fd()` spinning on
    // CPU6. Close this service before any lease can return to the pool.
    drop(svc);
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
    let queue_metrics = queue.metrics();
    println!(
        "async-demo: {jobs} jobs, {workers} workers, {per_job_ms:.3} ms/job; \
         identical={identical} callbacks={}",
        callbacks_done.load(Ordering::SeqCst)
    );
    println!(
        "async-demo: pool slots={} cached={} in_use={} allocations={}",
        stats.slots, stats.cached, stats.in_use, stats.allocations
    );
    println!(
        "async-demo: queue submitted={} done={} failed={} cancelled={} \
         avg_wait_ms={:.3} avg_run_ms={:.3} max_queued={} max_running={}",
        queue_metrics.submitted,
        queue_metrics.done,
        queue_metrics.failed,
        queue_metrics.cancelled,
        queue_metrics.total_queue_wait.as_secs_f64() * 1000.0 / jobs as f64,
        queue_metrics.total_run_time.as_secs_f64() * 1000.0 / jobs as f64,
        queue_metrics.max_queued,
        queue_metrics.max_running
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
) -> Result<(usize, usize), String> {
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

    let output = dei.run().map_err(|e| format!("IEP deinterlace: {e}"))?;
    if std::env::var_os("IEP_DEBUG").is_some() {
        eprintln!("iep output[0..8]: {:?}", &output[..8]);
        eprintln!(
            "iep output nonzero: {}",
            output.iter().filter(|w| **w != 0).count()
        );
    }

    let out_len = dei.dst_frame_bytes();
    let mut out = Vec::with_capacity(out_len * 2);
    for i in 0..2 {
        out.extend_from_slice(&dei.dst_frame_compact(i)?);
    }
    std::fs::write(dst, &out).map_err(|e| format!("{dst}: {e}"))?;
    let (top, bottom) = cfg.dil_mode.output_slots();
    Ok((2, usize::from(top) + usize::from(bottom)))
}

/// Run a stream of frames through the PD/DECT path the way
/// `tools/iep2_dect_probe.c` does: three-frame window, one `run()` per picture,
/// both raw destination slots written in the reference's order.
fn run_iep_dect(
    data: &[u8],
    dst: &str,
    cfg: rk_mpp_rs::service::iep_session::DeinterlaceConfig,
    frames: Option<usize>,
) -> Result<Vec<(u32, i32, u32, i32, u32)>, String> {
    use rk_mpp_rs::service::iep_session::IepDeinterlace;

    let mut dei = IepDeinterlace::open(cfg).map_err(|e| format!("open IEP client 28: {e}"))?;
    let frame = dei.src_frame_bytes();
    let available = data.len() / frame;
    let total = frames.unwrap_or(available).min(available);
    if total < 3 {
        return Err(format!("need at least 3 frames, input has {available}"));
    }

    let out_len = dei.dst_frame_bytes();
    let mut out = Vec::with_capacity(out_len * 2 * (total - 2));
    let mut report = Vec::new();
    let mut out_order = usize::from(cfg.field_order == rk_mpp_rs::hal::iep::Iep2FieldOrder::Bff);

    for k in 1..total - 1 {
        // Slot 0 is current, 1 next, 2 previous - the content `iep2_test` maps
        // into its own three rotating buffers.
        for (slot, order) in [(0usize, k), (1, k + 1), (2, k - 1)] {
            let off = order * frame;
            dei.src_frame_mut(slot)
                .sync_write_begin()
                .map_err(|e| format!("src{slot} write sync: {e}"))?;
            let n = frame.min(dei.src_frame_mut(slot).len());
            dei.src_frame_mut(slot).as_mut_slice()[..n].copy_from_slice(&data[off..off + n]);
            dei.flush_src(slot)
                .map_err(|e| format!("src{slot} flush: {e}"))?;
        }

        dei.run().map_err(|e| format!("IEP PD/DECT: {e}"))?;
        let (frm_mode, ratio) = dei.frame_mode();
        let order = dei.field_order();
        report.push((
            dei.pd_mode(),
            dei.pulldown().pdtype,
            order as u32,
            frm_mode,
            ratio,
        ));

        for slot in [out_order, 1 - out_order] {
            out.extend_from_slice(&dei.dst_frame_compact(slot)?);
        }
        out_order = usize::from(order == rk_mpp_rs::hal::iep::Iep2FieldOrder::Bff);
    }

    std::fs::write(dst, &out).map_err(|e| format!("{dst}: {e}"))?;
    Ok(report)
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
    if args.len() > 1 && args[1] == "h264e510" {
        if args.len() != 4 {
            eprintln!("usage: rk-mpp-rs h264e510 <in.320x240.nv12> <out.264>");
            std::process::exit(2);
        }
        let source = match std::fs::read(&args[2]) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("read {}: {error}", args[2]);
                std::process::exit(1);
            }
        };
        let mut encoder = match vepu510_h264::Vepu510H264::open() {
            Ok(encoder) => encoder,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        let stream = match encoder.encode_fixed_qp_idr_320x240(&source) {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        if let Err(error) = std::fs::write(&args[3], &stream) {
            eprintln!("write {}: {error}", args[3]);
            std::process::exit(1);
        }
        println!("h264e510: {} bytes", stream.len());
        return;
    }

    if args.len() > 1 && args[1] == "h265e510" {
        if args.len() != 4 {
            eprintln!("usage: rk-mpp-rs h265e510 <in.320x240.nv12> <out.265>");
            std::process::exit(2);
        }
        let source = match std::fs::read(&args[2]) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("read {}: {error}", args[2]);
                std::process::exit(1);
            }
        };
        let mut encoder = match vepu510_h265::Vepu510H265::open() {
            Ok(encoder) => encoder,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        let stream = match encoder.encode_fixed_qp_idr_320x240(&source) {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        if let Err(error) = std::fs::write(&args[3], &stream) {
            eprintln!("write {}: {error}", args[3]);
            std::process::exit(1);
        }
        println!("h265e510: {} bytes", stream.len());
        return;
    }

    if args.len() > 1 && args[1] == "h265e510-10bit" {
        if args.len() != 4 {
            eprintln!("usage: rk-mpp-rs h265e510-10bit <unused.nv12> <unused.265>");
            std::process::exit(2);
        }
        match vepu510_h265::Vepu510H265::open_bit_depth(10) {
            Ok(_) => std::process::exit(1),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
    }

    if args.len() > 1 && args[1] == "h265e510-tiles" {
        if args.len() != 5 {
            eprintln!("usage: rk-mpp-rs h265e510-tiles <unused.nv12> <unused.265> <columns>");
            std::process::exit(2);
        }
        let columns = match args[4].parse::<u32>() {
            Ok(columns) => columns,
            Err(error) => {
                eprintln!("invalid tile count {}: {error}", args[4]);
                std::process::exit(2);
            }
        };
        match vepu510_h265::Vepu510H265::open_tile_columns(columns) {
            Ok(_) => std::process::exit(1),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
    }

    if args.len() > 1 && args[1] == "h265e510-gop2" {
        if args.len() != 4 {
            eprintln!("usage: rk-mpp-rs h265e510-gop2 <in.2x320x240.nv12> <out.265>");
            std::process::exit(2);
        }
        let source = match std::fs::read(&args[2]) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("read {}: {error}", args[2]);
                std::process::exit(1);
            }
        };
        let mut encoder = match vepu510_h265::Vepu510H265::open() {
            Ok(encoder) => encoder,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        let stream = match encoder.encode_fixed_qp_gop2_320x240(&source) {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        if let Err(error) = std::fs::write(&args[3], &stream) {
            eprintln!("write {}: {error}", args[3]);
            std::process::exit(1);
        }
        println!("h265e510-gop2: {} bytes", stream.len());
        return;
    }

    if args.len() > 1 && args[1] == "h265e510-roi2" {
        if args.len() != 4 {
            eprintln!("usage: rk-mpp-rs h265e510-roi2 <in.2x320x240.nv12> <out.265>");
            std::process::exit(2);
        }
        let source = match std::fs::read(&args[2]) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("read {}: {error}", args[2]);
                std::process::exit(1);
            }
        };
        let mut encoder = match vepu510_h265::Vepu510H265::open() {
            Ok(encoder) => encoder,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        let stream = match encoder.encode_fixed_qp_roi_320x240(
            &source,
            2,
            &[h264_syntax::RoiRegion {
                x: 0,
                y: 0,
                w: 160,
                h: 120,
                force_intra: true,
                qp_absolute: true,
                qp_val: 30,
            }],
        ) {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        if let Err(error) = std::fs::write(&args[3], &stream) {
            eprintln!("write {}: {error}", args[3]);
            std::process::exit(1);
        }
        println!("h265e510-roi2: {} bytes", stream.len());
        return;
    }

    if args.len() > 1 && args[1] == "h265e510-gop6" {
        if args.len() != 4 {
            eprintln!("usage: rk-mpp-rs h265e510-gop6 <in.6x320x240.nv12> <out.265>");
            std::process::exit(2);
        }
        let source = match std::fs::read(&args[2]) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("read {}: {error}", args[2]);
                std::process::exit(1);
            }
        };
        let mut encoder = match vepu510_h265::Vepu510H265::open() {
            Ok(encoder) => encoder,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        let stream = match encoder.encode_fixed_qp_gop6_320x240(&source) {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        if let Err(error) = std::fs::write(&args[3], &stream) {
            eprintln!("write {}: {error}", args[3]);
            std::process::exit(1);
        }
        println!("h265e510-gop6: {} bytes", stream.len());
        return;
    }

    if args.len() > 1 && args[1] == "h265e510-control" {
        if args.len() != 6 {
            eprintln!(
                "usage: rk-mpp-rs h265e510-control <in.Nx320x240.nv12> <out.265> \
                 <force:frame[,frame...]|-> <sei:on|off>"
            );
            std::process::exit(2);
        }
        let source = match std::fs::read(&args[2]) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("read {}: {error}", args[2]);
                std::process::exit(1);
            }
        };
        let frame_size = 320 * 240 * 3 / 2;
        let frames = source.len() / frame_size;
        if frames == 0 || source.len() != frames * frame_size {
            eprintln!(
                "h265e510-control input is {} bytes, expected whole 320x240 NV12 pictures",
                source.len()
            );
            std::process::exit(2);
        }
        let forced: Vec<usize> = if args[4] == "-" {
            Vec::new()
        } else {
            let mut forced = Vec::new();
            for value in args[4].split(',') {
                match value.parse::<usize>() {
                    Ok(picture) if picture < frames => forced.push(picture),
                    _ => {
                        eprintln!("invalid forced-IDR picture {value:?}");
                        std::process::exit(2);
                    }
                }
            }
            forced
        };
        let sei_enabled = match args[5].as_str() {
            "on" => true,
            "off" => false,
            other => {
                eprintln!("invalid SEI mode {other:?}; expected on or off");
                std::process::exit(2);
            }
        };
        let mut encoder = match vepu510_h265::Vepu510H265::open() {
            Ok(encoder) => encoder,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        encoder.set_headers_each_idr(true);
        encoder.set_sei_enabled(sei_enabled);
        let mut stream = Vec::new();
        for picture in 0..frames {
            if forced.contains(&picture) {
                encoder.force_idr();
            }
            let input = &source[picture * frame_size..(picture + 1) * frame_size];
            match encoder.encode_fixed_qp_next_320x240(input) {
                Ok((bytes, _, _, _)) => stream.extend_from_slice(&bytes),
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
        }
        if let Err(error) = std::fs::write(&args[3], &stream) {
            eprintln!("write {}: {error}", args[3]);
            std::process::exit(1);
        }
        println!("h265e510-control: {} bytes", stream.len());
        return;
    }

    if args.len() > 1 && args[1] == "h265e510-cbr6" {
        if args.len() != 4 {
            eprintln!("usage: rk-mpp-rs h265e510-cbr6 <in.6x320x240.nv12> <out.265>");
            std::process::exit(2);
        }
        let source = match std::fs::read(&args[2]) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("read {}: {error}", args[2]);
                std::process::exit(1);
            }
        };
        let mut encoder = match vepu510_h265::Vepu510H265::open() {
            Ok(encoder) => encoder,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        let stream = match encoder.encode_cbr_gop6_320x240(&source) {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        if let Err(error) = std::fs::write(&args[3], &stream) {
            eprintln!("write {}: {error}", args[3]);
            std::process::exit(1);
        }
        println!("h265e510-cbr6: {} bytes", stream.len());
        return;
    }

    if args.len() > 1 && args[1] == "h265e510-vbr6" {
        if args.len() != 4 {
            eprintln!("usage: rk-mpp-rs h265e510-vbr6 <in.6x320x240.nv12> <out.265>");
            std::process::exit(2);
        }
        let source = match std::fs::read(&args[2]) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("read {}: {error}", args[2]);
                std::process::exit(1);
            }
        };
        let mut encoder = match vepu510_h265::Vepu510H265::open() {
            Ok(encoder) => encoder,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        let stream = match encoder.encode_vbr_gop6_320x240(&source) {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        if let Err(error) = std::fs::write(&args[3], &stream) {
            eprintln!("write {}: {error}", args[3]);
            std::process::exit(1);
        }
        println!("h265e510-vbr6: {} bytes", stream.len());
        return;
    }

    if args.len() > 1 && args[1] == "h265e510-avbr6" {
        if args.len() != 4 {
            eprintln!("usage: rk-mpp-rs h265e510-avbr6 <in.6x320x240.nv12> <out.265>");
            std::process::exit(2);
        }
        let source = match std::fs::read(&args[2]) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("read {}: {error}", args[2]);
                std::process::exit(1);
            }
        };
        let mut encoder = match vepu510_h265::Vepu510H265::open() {
            Ok(encoder) => encoder,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        let stream = match encoder.encode_avbr_gop6_320x240(&source) {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        if let Err(error) = std::fs::write(&args[3], &stream) {
            eprintln!("write {}: {error}", args[3]);
            std::process::exit(1);
        }
        println!("h265e510-avbr6: {} bytes", stream.len());
        return;
    }

    if args.len() > 1 && args[1] == "h265e510-split2" {
        let lowdelay = args.len() == 6 && args[5] == "lowdelay";
        if args.len() != 5 && !lowdelay {
            eprintln!(
                "usage: rk-mpp-rs h265e510-split2 <in.2x320x240.nv12> <out.265> <ctu:N|byte:N> [lowdelay]"
            );
            std::process::exit(2);
        }
        let (split_mode, split_arg) = if let Some(value) = args[4].strip_prefix("ctu:") {
            (h265_syntax::SplitMode::ByCtu, value.parse::<u32>())
        } else if let Some(value) = args[4].strip_prefix("byte:") {
            (h265_syntax::SplitMode::ByByte, value.parse::<u32>())
        } else {
            eprintln!("expected split argument ctu:N or byte:N");
            std::process::exit(2);
        };
        let split_arg = match split_arg {
            Ok(value) => value,
            Err(error) => {
                eprintln!("invalid split argument {}: {error}", args[4]);
                std::process::exit(2);
            }
        };
        let source = match std::fs::read(&args[2]) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("read {}: {error}", args[2]);
                std::process::exit(1);
            }
        };
        let mut encoder = match vepu510_h265::Vepu510H265::open() {
            Ok(encoder) => encoder,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        let split_out = if lowdelay {
            h265_syntax::MPP_ENC_SPLIT_OUT_LOWDELAY
        } else {
            0
        };
        let (stream, packet_lengths) = match encoder
            .encode_fixed_qp_split_320x240(&source, 2, split_mode, split_arg, split_out)
        {
            Ok(encoded) => encoded,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        if let Err(error) = std::fs::write(&args[3], &stream) {
            eprintln!("write {}: {error}", args[3]);
            std::process::exit(1);
        }
        println!("h265e510-split2: {} bytes", stream.len());
        if lowdelay {
            let lengths = packet_lengths
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(" ");
            println!("h265e510-split2 packet lengths: {lengths}");
        }
        return;
    }

    if args.len() > 1 && args[1] == "h265e510-gdr" {
        if args.len() != 5 {
            eprintln!(
                "usage: rk-mpp-rs h265e510-gdr <in.Nx320x240.nv12> <out.265> <row:num:gop:frames|col:num:gop:frames>"
            );
            std::process::exit(2);
        }
        let parts: Vec<&str> = args[4].split(':').collect();
        let [axis, num, gop, frames] = parts.as_slice() else {
            eprintln!("expected gdr row:num:gop:frames or col:num:gop:frames");
            std::process::exit(2);
        };
        let mode = match *axis {
            "row" => h265e::RefreshMode::Row,
            "col" => h265e::RefreshMode::Col,
            _ => {
                eprintln!("GDR axis must be row or col");
                std::process::exit(2);
            }
        };
        let parse = |name: &str, value: &str| match value.parse::<u32>() {
            Ok(value) => value,
            Err(error) => {
                eprintln!("invalid GDR {name} {value}: {error}");
                std::process::exit(2);
            }
        };
        let refresh_num = parse("refresh count", num);
        let gop = parse("GOP", gop);
        let frames = parse("frame count", frames) as usize;
        let source = match std::fs::read(&args[2]) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("read {}: {error}", args[2]);
                std::process::exit(1);
            }
        };
        let mut encoder = match vepu510_h265::Vepu510H265::open() {
            Ok(encoder) => encoder,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        let stream =
            match encoder.encode_fixed_qp_gdr_320x240(&source, frames, gop, mode, refresh_num) {
                Ok(stream) => stream,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            };
        if let Err(error) = std::fs::write(&args[3], &stream) {
            eprintln!("write {}: {error}", args[3]);
            std::process::exit(1);
        }
        println!("h265e510-gdr: {} bytes", stream.len());
        return;
    }

    if args.len() > 1 && args[1] == "h264e510-split2" {
        let lowdelay = args.len() == 6 && args[5] == "lowdelay";
        if args.len() != 5 && !lowdelay {
            eprintln!(
                "usage: rk-mpp-rs h264e510-split2 <in.2x320x240.nv12> <out.264> <mb:N|byte:N> [lowdelay]"
            );
            std::process::exit(2);
        }
        let (split_mode, split_arg) = if let Some(value) = args[4].strip_prefix("mb:") {
            (h264_syntax::SplitMode::ByCtu, value.parse::<u32>())
        } else if let Some(value) = args[4].strip_prefix("byte:") {
            (h264_syntax::SplitMode::ByByte, value.parse::<u32>())
        } else {
            eprintln!("expected split argument mb:N or byte:N");
            std::process::exit(2);
        };
        let split_arg = match split_arg {
            Ok(value) => value,
            Err(error) => {
                eprintln!("invalid split argument {}: {error}", args[4]);
                std::process::exit(2);
            }
        };
        let source = match std::fs::read(&args[2]) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("read {}: {error}", args[2]);
                std::process::exit(1);
            }
        };
        let mut encoder = match vepu510_h264::Vepu510H264::open() {
            Ok(encoder) => encoder,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        let (stream, packet_lengths) = if lowdelay {
            match encoder
                .encode_fixed_qp_gop_split_lowdelay_320x240(&source, 2, split_mode, split_arg)
            {
                Ok(encoded) => encoded,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
        } else {
            match encoder.encode_fixed_qp_gop_split_320x240(&source, 2, split_mode, split_arg) {
                Ok(stream) => (stream, Vec::new()),
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
        };
        if let Err(error) = std::fs::write(&args[3], &stream) {
            eprintln!("write {}: {error}", args[3]);
            std::process::exit(1);
        }
        println!("h264e510-split2: {} bytes", stream.len());
        if lowdelay {
            let lengths = packet_lengths
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(" ");
            println!("h264e510-split2 packet lengths: {lengths}");
        }
        return;
    }

    if args.len() > 1 && args[1] == "h264e510-gdr" {
        if args.len() != 5 {
            eprintln!(
                "usage: rk-mpp-rs h264e510-gdr <in.Nx320x240.nv12> <out.264> <row:num:gop:frames|col:num:gop:frames>"
            );
            std::process::exit(2);
        }
        let parts: Vec<&str> = args[4].split(':').collect();
        let [axis, num, gop, frames] = parts.as_slice() else {
            eprintln!("expected gdr row:num:gop:frames or col:num:gop:frames");
            std::process::exit(2);
        };
        let mode = match *axis {
            "row" => h264_syntax::GdrMode::Row,
            "col" => h264_syntax::GdrMode::Col,
            _ => {
                eprintln!("GDR axis must be row or col");
                std::process::exit(2);
            }
        };
        let parse = |name: &str, value: &str| match value.parse::<u32>() {
            Ok(value) => value,
            Err(error) => {
                eprintln!("invalid GDR {name} {value}: {error}");
                std::process::exit(2);
            }
        };
        let gdr_num = parse("refresh count", num);
        let gop = parse("GOP", gop);
        let frames = parse("frame count", frames) as usize;
        let source = match std::fs::read(&args[2]) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("read {}: {error}", args[2]);
                std::process::exit(1);
            }
        };
        let mut encoder = match vepu510_h264::Vepu510H264::open() {
            Ok(encoder) => encoder,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        let (stream, packet_lengths) =
            match encoder.encode_fixed_qp_gdr_320x240(&source, frames, mode, gdr_num, gop) {
                Ok(encoded) => encoded,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            };
        if let Err(error) = std::fs::write(&args[3], &stream) {
            eprintln!("write {}: {error}", args[3]);
            std::process::exit(1);
        }
        println!("h264e510-gdr: {} bytes", stream.len());
        let lengths = packet_lengths
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        println!("h264e510-gdr packet lengths: {lengths}");
        return;
    }

    if args.len() > 1
        && matches!(
            args[1].as_str(),
            "h264e510-gop2"
                | "h264e510-roi2"
                | "h264e510-gop6"
                | "h264e510-cbr6"
                | "h264e510-vbr6"
                | "h264e510-avbr6"
        )
    {
        if args.len() != 4 {
            eprintln!("usage: rk-mpp-rs {} <in.Nx320x240.nv12> <out.264>", args[1]);
            std::process::exit(2);
        }
        let frames = if matches!(args[1].as_str(), "h264e510-gop2" | "h264e510-roi2") {
            2
        } else {
            6
        };
        let source = match std::fs::read(&args[2]) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("read {}: {error}", args[2]);
                std::process::exit(1);
            }
        };
        let mut encoder = match vepu510_h264::Vepu510H264::open() {
            Ok(encoder) => encoder,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        let result = match args[1].as_str() {
            "h264e510-cbr6" => {
                encoder.encode_rate_gop_320x240(&source, frames, h264_syntax::RcMode::Cbr)
            }
            "h264e510-vbr6" => {
                encoder.encode_rate_gop_320x240(&source, frames, h264_syntax::RcMode::Vbr)
            }
            "h264e510-avbr6" => {
                encoder.encode_rate_gop_320x240(&source, frames, h264_syntax::RcMode::Avbr)
            }
            "h264e510-roi2" => encoder.encode_fixed_qp_gop_roi_320x240(
                &source,
                frames,
                &[h264_syntax::RoiRegion {
                    x: 0,
                    y: 0,
                    w: 160,
                    h: 120,
                    force_intra: true,
                    qp_absolute: true,
                    qp_val: 30,
                }],
            ),
            _ => encoder.encode_fixed_qp_gop_320x240(&source, frames),
        };
        let stream = match result {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        if let Err(error) = std::fs::write(&args[3], &stream) {
            eprintln!("write {}: {error}", args[3]);
            std::process::exit(1);
        }
        println!("{}: {} bytes", args[1], stream.len());
        return;
    }

    if args.len() > 1 && args[1] == "h264-enc" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("");
        let width = args.get(4).and_then(|s| s.parse::<usize>().ok());
        let height = args.get(5).and_then(|s| s.parse::<usize>().ok());
        let (Some(width), Some(height)) = (width, height) else {
            eprintln!("usage: rk-mpp-rs h264-enc <in.nv12> <out.264> <width> <height> [qp|auto] [profile] [frames] [gop] [cbr:|vbr:|avbr:target:min:max] [qprc:min:max] [hierq:d0:d1:d2:d3:n0:n1:n2:n3] [smartp:vi_len] [reenc:n] [super:mode:i_thd:p_thd:priority] [drop:mode:threshold:gap] [debreath:strength] [dblk:disable:alpha:beta] [cabac:0|1] [trans8x8:0|1] [cabac_idc:0..2] [constraint_set:0xforcevalue] [base_layer_pid:0..56] [roi:x:y:w:h:intra:abs:qp]... [gdr:row:|gdr:col:]num [split:mb:|split:byte:]arg [splitout:lowdelay]");
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
        let mut rc_qp_bounds: Option<(u32, u32)> = None;
        let mut hier_qp: Option<([i32; 4], [i32; 4])> = None;
        let mut smart_p_vi_len: Option<u32> = None;
        let mut max_reencode_times: Option<u32> = None;
        let mut super_frame: Option<(u8, u32, u32, u8)> = None;
        let mut drop_frame: Option<(u8, u32, u32)> = None;
        let mut debreath_strength: Option<u8> = None;
        let mut deblock: Option<(u32, i32, i32)> = None;
        let mut cabac_init_idc: Option<u8> = None;
        let mut constraint_set: Option<u32> = None;
        let mut base_layer_pid: Option<u8> = None;
        let mut entropy_coding: Option<bool> = None;
        let mut transform_8x8: Option<bool> = None;
        // The `RoiRegionCfg` list `mpi_enc_test` adds per frame, in the same
        // add order.
        let mut rois: Vec<h264_syntax::RoiRegion> = Vec::new();
        // GDR arrives as `gdr:row:<num>` / `gdr:col:<num>`, the `refresh_mode`
        // and `refresh_num` of `MppEncRcCfg`; slice split as `split:mb:<arg>`
        // (BY_CTU, macroblocks per slice) or `split:byte:<arg>` (BY_BYTE).
        let mut gdr: Option<(h264_syntax::GdrMode, u32)> = None;
        let mut split: Option<(h264_syntax::SplitMode, u32)> = None;
        let mut split_out = 0u32;
        for text in args.iter().skip(10) {
            if let Some(value) = text.strip_prefix("cabac_idc:") {
                match value.parse::<u8>() {
                    Ok(value @ 0..=2) => cabac_init_idc = Some(value),
                    _ => {
                        eprintln!("bad cabac_idc; expected cabac_idc:0..=2");
                        std::process::exit(2);
                    }
                }
            } else if let Some(value) = text.strip_prefix("constraint_set:") {
                let digits = value
                    .strip_prefix("0x")
                    .or_else(|| value.strip_prefix("0X"))
                    .unwrap_or(value);
                match u32::from_str_radix(digits, 16) {
                    Ok(value @ ..=0x003f_003f) => constraint_set = Some(value),
                    _ => {
                        eprintln!("bad constraint_set; expected constraint_set:0x000000..0x3f003f");
                        std::process::exit(2);
                    }
                }
            } else if let Some(value) = text.strip_prefix("base_layer_pid:") {
                match value.parse::<u8>() {
                    Ok(value @ ..=56) => base_layer_pid = Some(value),
                    _ => {
                        eprintln!("bad base_layer_pid; expected base_layer_pid:0..=56");
                        std::process::exit(2);
                    }
                }
            } else if let Some(value) = text.strip_prefix("cabac:") {
                match value {
                    "0" => entropy_coding = Some(false),
                    "1" => entropy_coding = Some(true),
                    _ => {
                        eprintln!("bad cabac; expected cabac:0|1");
                        std::process::exit(2);
                    }
                }
            } else if let Some(value) = text.strip_prefix("trans8x8:") {
                match value {
                    "0" => transform_8x8 = Some(false),
                    "1" => transform_8x8 = Some(true),
                    _ => {
                        eprintln!("bad trans8x8; expected trans8x8:0|1");
                        std::process::exit(2);
                    }
                }
            } else if let Some(rest) = text.strip_prefix("dblk:") {
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
            } else if let Some(rest) = text.strip_prefix("qprc:") {
                let parts: Vec<u32> = rest.split(':').filter_map(|s| s.parse().ok()).collect();
                match parts.as_slice() {
                    [min, max] => rc_qp_bounds = Some((*min, *max)),
                    _ => {
                        eprintln!("bad qprc; expected qprc:min:max");
                        std::process::exit(2);
                    }
                }
            } else if let Some(rest) = text.strip_prefix("hierq:") {
                let parts: Result<Vec<i32>, _> = rest.split(':').map(str::parse::<i32>).collect();
                match parts.as_deref() {
                    Ok([d0, d1, d2, d3, n0, n1, n2, n3]) => {
                        hier_qp = Some(([*d0, *d1, *d2, *d3], [*n0, *n1, *n2, *n3]))
                    }
                    _ => {
                        eprintln!("bad hierq; expected hierq:d0:d1:d2:d3:n0:n1:n2:n3");
                        std::process::exit(2);
                    }
                }
            } else if let Some(value) = text.strip_prefix("smartp:") {
                match value.parse::<u32>() {
                    Ok(vi_len) if vi_len > 0 => smart_p_vi_len = Some(vi_len),
                    _ => {
                        eprintln!("bad smartp; expected smartp:vi_len");
                        std::process::exit(2);
                    }
                }
            } else if let Some(value) = text.strip_prefix("reenc:") {
                match value.parse::<u32>() {
                    Ok(times) => max_reencode_times = Some(times),
                    Err(_) => {
                        eprintln!("bad reenc; expected reenc:n");
                        std::process::exit(2);
                    }
                }
            } else if let Some(rest) = text.strip_prefix("super:") {
                let parts: Result<Vec<u32>, _> = rest.split(':').map(str::parse::<u32>).collect();
                match parts.as_deref() {
                    Ok([mode, i_thd, p_thd, priority]) => {
                        super_frame = Some((
                            u8::try_from(*mode).unwrap_or(u8::MAX),
                            *i_thd,
                            *p_thd,
                            u8::try_from(*priority).unwrap_or(u8::MAX),
                        ))
                    }
                    _ => {
                        eprintln!("bad super; expected super:mode:i_thd:p_thd:priority");
                        std::process::exit(2);
                    }
                }
            } else if let Some(rest) = text.strip_prefix("drop:") {
                let parts: Result<Vec<u32>, _> = rest.split(':').map(str::parse::<u32>).collect();
                match parts.as_deref() {
                    Ok([mode, threshold, gap]) => {
                        drop_frame =
                            Some((u8::try_from(*mode).unwrap_or(u8::MAX), *threshold, *gap))
                    }
                    _ => {
                        eprintln!("bad drop; expected drop:mode:threshold:gap");
                        std::process::exit(2);
                    }
                }
            } else if let Some(value) = text.strip_prefix("debreath:") {
                match value.parse::<u8>() {
                    Ok(strength) if strength <= 35 => debreath_strength = Some(strength),
                    _ => {
                        eprintln!("bad debreath; expected debreath:strength in 0..=35");
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
            } else if text == "splitout:lowdelay" {
                split_out = h264_syntax::MPP_ENC_SPLIT_OUT_LOWDELAY;
            } else if let Some(value) = text.strip_prefix("splitout:") {
                match value.parse::<u32>() {
                    Ok(value) => split_out = value,
                    Err(_) => {
                        eprintln!("bad splitout value {value}");
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
                             [cbr:|vbr:|avbr:]target:min:max, \
                             hierq:d0:d1:d2:d3:n0:n1:n2:n3, smartp:vi_len, reenc:n, \
                             super:mode:i_thd:p_thd:priority, drop:mode:threshold:gap, \
                             dblk:disable:alpha:beta, cabac:0|1, trans8x8:0|1, \
                             roi:x:y:w:h:intra:abs:qp, gdr:row:|col:num \
                             or split:mb:|byte:arg"
                        );
                        std::process::exit(2);
                    }
                }
            }
        }
        if src.is_empty() || dst.is_empty() {
            eprintln!("usage: rk-mpp-rs h264-enc <in.nv12> <out.264> <width> <height> [qp|auto] [profile] [frames] [gop] [cbr:|vbr:|avbr:target:min:max] [qprc:min:max] [hierq:d0:d1:d2:d3:n0:n1:n2:n3] [smartp:vi_len] [reenc:n] [super:mode:i_thd:p_thd:priority] [drop:mode:threshold:gap] [debreath:strength] [dblk:disable:alpha:beta] [cabac:0|1] [trans8x8:0|1] [cabac_idc:0..2] [constraint_set:0xforcevalue] [base_layer_pid:0..56] [roi:x:y:w:h:intra:abs:qp]... [gdr:row:|gdr:col:]num [split:mb:|split:byte:]arg [splitout:lowdelay]");
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
            rc_qp_bounds,
            hier_qp,
            smart_p_vi_len,
            max_reencode_times,
            super_frame,
            drop_frame,
            debreath_strength,
            deblock,
            cabac_init_idc,
            constraint_set,
            base_layer_pid,
            entropy_coding,
            transform_8x8,
            rois,
            gdr,
            split,
            split_out,
        ) {
            Ok((bytes, header_bytes, payload_bytes, packets, ms)) => println!(
                "encoded {src} -> {dst}: {bytes} bytes ({header_bytes} header + {payload_bytes} payload), {packets} packet(s), {width}x{height} {profile} qp {qp_desc} gop {gop}, {ms:.3} ms"
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
        let mut syntax = h265_syntax::SyntaxOptions::default();
        for text in args.iter().skip(5) {
            match parse_h265_syntax_option(text, &mut syntax) {
                Ok(true) => {}
                Ok(false) => {}
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(2);
                }
            }
        }
        // One source of truth for the column rule and the uniform split: the same
        // derivation the PPS writer and the session use.
        let mut tile_config = h265_syntax::EncoderConfig::new(width, height);
        tile_config.apply_syntax_options(syntax);
        let columns = tile_config.tile_columns_minus1() as usize + 1;
        let tile_index = std::env::var("H265_TILE_INDEX")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0)
            .min(columns - 1);
        let (tile_start, tile_width) =
            h265e_session::uniform_tile_widths(width, columns)[tile_index];
        let tile = (columns > 1).then_some(h265e::TileJob {
            index: tile_index as u32,
            start_x: tile_start,
            width_mb: tile_width,
            height_mb: ((height + 63) / 64) as u32,
            columns: columns as u32,
        });
        let job = h265e::IdrJob {
            width,
            height,
            qp,
            qp_min: qp,
            qp_max: qp,
            picture,
            long_term,
            temporal_mvp: false,
            force_cu_qp_delta: false,
            scene_motion_flag: 2,
            qbias: None,
            vmaf_opt: false,
            rc,
            refresh: None,
            split: h265_syntax::SliceSplit::default(),
            syntax,
            tile,
            roi_fd: -1,
            roi_qp_fd: -1,
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
        let usage = "usage: rk-mpp-rs h265-idr <in.nv12> <out.265> <width> <height> [qp] [roi:x:y:w:h:intra:abs:qp]... [split:ctu:|split:byte:arg] [splitout:lowdelay] [dblk:disable:beta:tc] [sao:luma_disable:chroma_disable] [lpf:across_slices:disable_tiles]";
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
        let mut split = h265_syntax::SliceSplit::default();
        let mut syntax = h265_syntax::SyntaxOptions::default();
        let mut rois = Vec::new();
        for text in args.iter().skip(7) {
            match parse_h265_syntax_option(text, &mut syntax) {
                Ok(true) => continue,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(2);
                }
                Ok(false) => {}
            }
            match parse_h265_split_option(text, &mut split) {
                Ok(true) => {}
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(2);
                }
                Ok(false) => match parse_h265_roi_option(text, &mut rois) {
                    Ok(true) => {}
                    Ok(false) => {
                        eprintln!("unknown H.265 option {text}");
                        std::process::exit(2);
                    }
                    Err(error) => {
                        eprintln!("{error}");
                        std::process::exit(2);
                    }
                },
            }
        }
        let input = std::fs::read(src).map_err(|e| format!("{src}: {e}"));
        let input = match input {
            Ok(input) => input,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };
        let result = h265e_session::Vepu580H265::open(width, height, qp).and_then(|mut encoder| {
            encoder.set_syntax_options(syntax);
            encoder.set_slice_split(split)?;
            encoder.set_roi_regions(&rois)?;
            encoder.encode_idr_packets(&input)
        });
        match result.and_then(|encoded| {
            std::fs::write(dst, &encoded.annex_b).map_err(|e| format!("{dst}: {e}"))?;
            if std::env::var_os("RK_MPP_TRACE_PACKETS").is_some() {
                let sizes = encoded
                    .packets
                    .iter()
                    .map(|packet| packet.len().to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                eprintln!("h265 frame 0: {} packet(s): {sizes}", encoded.packets.len());
            }
            Ok((encoded.annex_b.len(), encoded.packets.len()))
        }) {
            Ok((bytes, packets)) => {
                println!("h265-idr {src} -> {dst}: {bytes} bytes, {packets} packet(s), {width}x{height} fixed qp {qp}")
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
        let usage = "usage: rk-mpp-rs h265-gop <in.nv12> <out.265> <width> <height> [frames] [qp] [row:|col:num] [shortterm] [roi:x:y:w:h:intra:abs:qp]... [split:ctu:|split:byte:arg] [splitout:lowdelay] [dblk:disable:beta:tc] [sao:luma_disable:chroma_disable] [lpf:across_slices:disable_tiles]";
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
        // Stream one NV12 frame at a time. Reading the whole file made every
        // concurrent session pull its own copy of the input through the page
        // cache; perf put most of the system time there.
        let frame_size = width * height * 3 / 2;
        let needed = frame_size * frames;
        match std::fs::metadata(src) {
            Ok(meta) if meta.len() >= needed as u64 => {}
            Ok(meta) => {
                eprintln!(
                    "{src}: {frames} frames of {width}x{height} need {needed} bytes, file has {}",
                    meta.len()
                );
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("{src}: {e}");
                std::process::exit(1);
            }
        }
        let mut split = h265_syntax::SliceSplit::default();
        let mut syntax = h265_syntax::SyntaxOptions::default();
        let mut refresh = None;
        let mut short_term = false;
        let mut rois = Vec::new();
        for text in args.iter().skip(8) {
            match parse_h265_syntax_option(text, &mut syntax) {
                Ok(true) => continue,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(2);
                }
                Ok(false) => {}
            }
            match parse_h265_split_option(text, &mut split) {
                Ok(true) => continue,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(2);
                }
                Ok(false) if text.starts_with("row:") || text.starts_with("col:") => {
                    refresh = Some(text.as_str());
                }
                Ok(false) if text == "shortterm" => short_term = true,
                Ok(false) => match parse_h265_roi_option(text, &mut rois) {
                    Ok(true) => {}
                    Ok(false) => {
                        eprintln!("unknown H.265 option {text}");
                        std::process::exit(2);
                    }
                    Err(error) => {
                        eprintln!("{error}");
                        std::process::exit(2);
                    }
                },
            }
        }
        let pipeline_compatible = refresh.is_none()
            && split.out & h265_syntax::MPP_ENC_SPLIT_OUT_LOWDELAY == 0
            && !syntax.auto_tile;
        let opened = if refresh.is_some() || short_term {
            h265e_session::Vepu580H265::open_gop_short_term(width, height, qp)
        } else {
            h265e_session::Vepu580H265::open_gop(width, height, qp)
        };
        let result = opened.and_then(|mut encoder| {
            encoder.set_syntax_options(syntax);
            if let Ok(value) = std::env::var("H265_SCENE_MODE") {
                let scene_mode = value
                    .parse::<u8>()
                    .ok()
                    .filter(|value| *value <= 2)
                    .ok_or_else(|| format!("H265_SCENE_MODE must be 0, 1 or 2, got {value}"))?;
                encoder.set_scene_mode(scene_mode == 1);
            }
            if std::env::var_os("H265_QBIAS_EN").is_some()
                || std::env::var_os("H265_QBIAS_I").is_some()
                || std::env::var_os("H265_QBIAS_P").is_some()
            {
                let read = |name: &str| -> Result<u32, String> {
                    match std::env::var(name) {
                        Ok(value) => value.parse().map_err(|_| {
                            format!("{name} must be an unsigned integer, got {value}")
                        }),
                        Err(_) => Ok(0),
                    }
                };
                encoder.set_qbias(Some((
                    read("H265_QBIAS_EN")? != 0,
                    read("H265_QBIAS_I")?,
                    read("H265_QBIAS_P")?,
                )));
            }
            if let Ok(value) = std::env::var("H265_VMAF_OPT") {
                let enabled = match value.as_str() {
                    "0" => false,
                    "1" => true,
                    _ => return Err(format!("H265_VMAF_OPT must be 0 or 1, got {value}")),
                };
                encoder.set_vmaf_opt(enabled);
            }
            if let Ok(gop) = std::env::var("H265_GOP") {
                let gop = gop
                    .parse::<u32>()
                    .map_err(|_| format!("invalid H265_GOP {gop}"))?;
                encoder.set_gop(gop)?;
            }
            if let Some(spec) = refresh {
                let (mode, num) = spec.split_once(':').unwrap_or(("row", "1"));
                let mode = if mode == "col" {
                    h265e::RefreshMode::Col
                } else {
                    h265e::RefreshMode::Row
                };
                encoder.enable_refresh(mode, num.parse().unwrap_or(1))?;
            }
            encoder.set_slice_split(split)?;
            encoder.set_roi_regions(&rois)?;
            let mut output = if dst == "/dev/null" {
                None
            } else {
                Some(std::io::BufWriter::with_capacity(
                    1024 * 1024,
                    std::fs::File::create(dst).map_err(|error| format!("{dst}: {error}"))?,
                ))
            };
            let input_parallelism = std::thread::available_parallelism()
                .map(|parallelism| parallelism.get())
                .unwrap_or(1);
            let pipeline_requested = std::env::var("RKMPP_ENCODER_PIPELINE").as_deref() != Ok("0");
            let pipeline_depth = std::env::var("RKMPP_ENCODER_PIPELINE_DEPTH")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(2)
                .clamp(1, 2);
            let hardware_pipeline =
                pipeline_compatible && pipeline_requested && input_parallelism > 1;
            if hardware_pipeline {
                encoder.set_pipeline_depth(pipeline_depth);
                encoder.set_pipeline(true);
            }
            let pipeline = if input_parallelism > 1 {
                Some(DmaInputPipeline::start(
                    src,
                    frame_size,
                    frames,
                    if hardware_pipeline {
                        pipeline_depth + 1
                    } else {
                        2
                    },
                )?)
            } else {
                None
            };
            let mut file = if pipeline.is_none() {
                Some(std::fs::File::open(src).map_err(|error| format!("{src}: {error}"))?)
            } else {
                None
            };
            let mut frame = if pipeline.is_none() {
                vec![0u8; frame_size]
            } else {
                Vec::new()
            };
            let mut total_bytes = 0usize;
            let mut packets = 0;
            let trace_packets = std::env::var_os("RK_MPP_TRACE_PACKETS").is_some();
            let mut in_flight = VecDeque::new();
            let mut emit = |encoded: h265e_session::EncodedFrame| -> Result<(), String> {
                packets += encoded.packets.len();
                if trace_packets {
                    let sizes = encoded
                        .packets
                        .iter()
                        .map(|packet| packet.len().to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    eprintln!(
                        "h265 frame {}: {} packet(s): {sizes}",
                        encoded.frame_num,
                        encoded.packets.len()
                    );
                }
                let output_len = if encoded.annex_b.is_empty() {
                    encoded.packets.iter().map(Vec::len).sum()
                } else {
                    encoded.annex_b.len()
                };
                total_bytes += output_len;
                if let Some(out) = output.as_mut() {
                    use std::io::Write as _;
                    if encoded.annex_b.is_empty() {
                        for packet in &encoded.packets {
                            out.write_all(packet)
                                .map_err(|error| format!("{dst}: {error}"))?;
                        }
                    } else {
                        out.write_all(&encoded.annex_b)
                            .map_err(|error| format!("{dst}: {error}"))?;
                    }
                }
                Ok(())
            };
            for index in 0..frames {
                let completed = if let Some(pipeline) = pipeline.as_ref() {
                    let input = pipeline.next(index)?;
                    let input_fd = input.fd();
                    if hardware_pipeline {
                        in_flight.push_back(input);
                        encoder.encode_frame_fd_pipeline(input_fd)?
                    } else {
                        let encoded = encoder.encode_frame_packets_fd(input_fd)?;
                        pipeline.recycle(input)?;
                        vec![encoded]
                    }
                } else {
                    use std::io::Read as _;
                    file.as_mut()
                        .expect("sequential H.265 input source")
                        .read_exact(&mut frame)
                        .map_err(|error| format!("{src}: frame {index}: {error}"))?;
                    vec![encoder.encode_frame_packets(&frame)?]
                };
                for encoded in completed {
                    if hardware_pipeline {
                        let input = in_flight
                            .pop_front()
                            .ok_or_else(|| "H.265 completion has no in-flight input".to_string())?;
                        pipeline
                            .as_ref()
                            .expect("hardware pipeline owns an input pipeline")
                            .recycle(input)?;
                    }
                    emit(encoded)?;
                }
            }
            if hardware_pipeline {
                for encoded in encoder.flush_pipeline()? {
                    let input = in_flight
                        .pop_front()
                        .ok_or_else(|| "H.265 flush has no in-flight input".to_string())?;
                    pipeline
                        .as_ref()
                        .expect("hardware pipeline owns an input pipeline")
                        .recycle(input)?;
                    emit(encoded)?;
                }
            }
            drop(emit);
            if let Some(pipeline) = pipeline {
                pipeline.finish();
            }
            if let Some(out) = output.as_mut() {
                use std::io::Write as _;
                out.flush().map_err(|error| format!("{dst}: {error}"))?;
            }
            Ok((total_bytes, packets))
        });
        match result {
            Ok((bytes, packets)) => println!(
                "h265-gop {src} -> {dst}: {bytes} bytes, {packets} packet(s), {frames} frames {width}x{height} fixed qp {qp}"
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
        let usage = "usage: rk-mpp-rs h265-gop-rc <in.nv12> <out.265> <w> <h> <qp:bits,...> [min_qp] [max_qp] [bps:min:max] [init_qp] [dblk:disable:beta:tc] [sao:luma_disable:chroma_disable] [lpf:across_slices:disable_tiles]";
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
        // Stream frames instead of slurping the clip: concurrent sessions
        // otherwise each pull a private copy of the input through the page
        // cache, and perf attributed most of the system time to those reads.
        let frame_size = width * height * 3 / 2;
        let needed = frame_size * steps.len();
        match std::fs::metadata(src) {
            Ok(meta) if meta.len() >= needed as u64 => {}
            Ok(meta) => {
                eprintln!(
                    "{src}: {} frames of {width}x{height} need {needed} bytes, file has {}",
                    steps.len(),
                    meta.len()
                );
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("{src}: {e}");
                std::process::exit(1);
            }
        }
        let bps: Option<(i64, i64, i64)> = args.get(9).and_then(|spec| {
            let f: Vec<i64> = spec.split(':').filter_map(|v| v.parse().ok()).collect();
            if f.len() == 3 {
                Some((f[0], f[1], f[2]))
            } else {
                None
            }
        });
        let mut syntax = h265_syntax::SyntaxOptions::default();
        for text in args.iter().skip(11) {
            match parse_h265_syntax_option(text, &mut syntax) {
                Ok(true) => {}
                Ok(false) => {
                    eprintln!("unknown H.265 option {text}");
                    std::process::exit(2);
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(2);
                }
            }
        }
        let result =
            h265e_session::Vepu580H265::open_gop(width, height, max_qp).and_then(|mut encoder| {
                encoder.set_syntax_options(syntax);
                if let Some((target, min, max)) = bps {
                    encoder.set_rate_control_sei(h265e_session::RcSei::from_harness(
                        target,
                        min,
                        max,
                        args.get(10)
                            .and_then(|s| s.parse::<i32>().ok())
                            .unwrap_or(steps[0].0 as i32),
                        min_qp,
                        max_qp,
                    ));
                }
                let input_parallelism = std::thread::available_parallelism()
                    .map(|parallelism| parallelism.get())
                    .unwrap_or(1);
                let pipeline = if input_parallelism > 1 {
                    Some(DmaInputPipeline::start(src, frame_size, steps.len(), 2)?)
                } else {
                    None
                };
                let mut file = if pipeline.is_none() {
                    Some(std::fs::File::open(src).map_err(|error| format!("{src}: {error}"))?)
                } else {
                    None
                };
                let mut frame = if pipeline.is_none() {
                    vec![0u8; frame_size]
                } else {
                    Vec::new()
                };
                let mut output = if dst == "/dev/null" {
                    None
                } else {
                    Some(std::io::BufWriter::with_capacity(
                        1024 * 1024,
                        std::fs::File::create(dst).map_err(|error| format!("{dst}: {error}"))?,
                    ))
                };
                let mut total_bytes = 0usize;
                for (index, (qp, bits)) in steps.iter().enumerate() {
                    let encoded = if let Some(pipeline) = pipeline.as_ref() {
                        let input = pipeline.next(index)?;
                        let encoded = encoder.encode_frame_with_rc_packets_fd(
                            input.fd(),
                            *bits,
                            *qp,
                            min_qp,
                            max_qp,
                        )?;
                        pipeline.recycle(input)?;
                        encoded
                    } else {
                        use std::io::Read as _;
                        file.as_mut()
                            .expect("sequential H.265 RC input source")
                            .read_exact(&mut frame)
                            .map_err(|error| format!("{src}: frame {index}: {error}"))?;
                        encoder.encode_frame_with_rc_packets(&frame, *bits, *qp, min_qp, max_qp)?
                    };
                    total_bytes += encoded.annex_b.len();
                    if let Some(output) = output.as_mut() {
                        use std::io::Write as _;
                        output
                            .write_all(&encoded.annex_b)
                            .map_err(|error| format!("{dst}: {error}"))?;
                    }
                }
                if let Some(pipeline) = pipeline {
                    pipeline.finish();
                }
                if let Some(output) = output.as_mut() {
                    use std::io::Write as _;
                    output.flush().map_err(|error| format!("{dst}: {error}"))?;
                }
                Ok(total_bytes)
            });
        match result {
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
        let usage = "usage: rk-mpp-rs h265-rate <in.nv12> <out.265> <w> <h> <frames> <bps:min:max> [init_qp] [qp_min] [qp_max] [cbr|vbr|avbr] [split:ctu:|split:byte:arg] [splitout:lowdelay] [dblk:disable:beta:tc] [sao:luma_disable:chroma_disable] [lpf:across_slices:disable_tiles]";
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
        let mut split = h265_syntax::SliceSplit::default();
        let mut syntax = h265_syntax::SyntaxOptions::default();
        for text in args.iter().skip(12) {
            match parse_h265_syntax_option(text, &mut syntax) {
                Ok(true) => continue,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(2);
                }
                Ok(false) => {}
            }
            match parse_h265_split_option(text, &mut split) {
                Ok(true) => {}
                Ok(false) => {
                    eprintln!("unknown H.265 option {text}");
                    std::process::exit(2);
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(2);
                }
            }
        }
        // Stream frames; see the h265-gop path for why the whole-file read
        // hurts concurrent sessions.
        let frame_size = width * height * 3 / 2;
        let needed = frame_size * frames;
        match std::fs::metadata(src) {
            Ok(meta) if meta.len() >= needed as u64 => {}
            Ok(meta) => {
                eprintln!(
                    "{src}: {frames} frames of {width}x{height} need {needed} bytes, file has {}",
                    meta.len()
                );
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("{src}: {e}");
                std::process::exit(1);
            }
        }
        let result = h265e_session::Vepu580H265::open_rate(
            width, height, mode, target, min, max, init_qp, qp_min, qp_max,
        )
        .and_then(|mut encoder| {
            encoder.set_syntax_options(syntax);
            encoder.set_slice_split(split)?;
            let input_parallelism = std::thread::available_parallelism()
                .map(|parallelism| parallelism.get())
                .unwrap_or(1);
            let pipeline = if input_parallelism > 1 {
                Some(DmaInputPipeline::start(src, frame_size, frames, 2)?)
            } else {
                None
            };
            let mut file = if pipeline.is_none() {
                Some(std::fs::File::open(src).map_err(|error| format!("{src}: {error}"))?)
            } else {
                None
            };
            let mut frame = if pipeline.is_none() {
                vec![0u8; frame_size]
            } else {
                Vec::new()
            };
            let mut output = if dst == "/dev/null" {
                None
            } else {
                Some(std::io::BufWriter::with_capacity(
                    1024 * 1024,
                    std::fs::File::create(dst).map_err(|error| format!("{dst}: {error}"))?,
                ))
            };
            let mut total_bytes = 0usize;
            let mut packets = 0;
            let trace_packets = std::env::var_os("RK_MPP_TRACE_PACKETS").is_some();
            for index in 0..frames {
                let encoded = if let Some(pipeline) = pipeline.as_ref() {
                    let input = pipeline.next(index)?;
                    let encoded = encoder.encode_frame_packets_fd(input.fd())?;
                    pipeline.recycle(input)?;
                    encoded
                } else {
                    use std::io::Read as _;
                    file.as_mut()
                        .expect("sequential H.265 rate input source")
                        .read_exact(&mut frame)
                        .map_err(|error| format!("{src}: frame {index}: {error}"))?;
                    encoder.encode_frame_packets(&frame)?
                };
                packets += encoded.packets.len();
                if trace_packets {
                    let sizes = encoded
                        .packets
                        .iter()
                        .map(|packet| packet.len().to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    eprintln!(
                        "h265 frame {index}: {} packet(s): {sizes}",
                        encoded.packets.len()
                    );
                }
                total_bytes += encoded.annex_b.len();
                if let Some(output) = output.as_mut() {
                    use std::io::Write as _;
                    output
                        .write_all(&encoded.annex_b)
                        .map_err(|error| format!("{dst}: {error}"))?;
                }
            }
            if let Some(pipeline) = pipeline {
                pipeline.finish();
            }
            if let Some(output) = output.as_mut() {
                use std::io::Write as _;
                output.flush().map_err(|error| format!("{dst}: {error}"))?;
            }
            Ok((total_bytes, packets))
        });
        match result {
            Ok((bytes, packets)) => println!(
                "h265-cbr {src} -> {dst}: {bytes} bytes, {packets} packet(s), {frames} pictures"
            ),
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
                "usage: rk-mpp-rs jpeg-enc <in.yuv> <out.jpg> <width> <height> [quant 0..10|qf<1..100>] [420|422|gray|444] [nv12|nv21|i420|nv16|nv61|yuv422p|yuyv|yvyu|uyvy|vyuy|nv24|yuv444p|yuv400|rgb565|bgr565|rgb555|bgr555|rgb444|bgr444|rgb101010|bgr101010|argb8888|abgr8888|bgra8888|rgba8888] [rst<mcus>] [rot90|rot270] [splitout:lowdelay]"
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
            Some("nv21") => jpege::InputFormat::Nv21,
            Some("i420") => jpege::InputFormat::Yuv420p,
            Some("nv16") => jpege::InputFormat::Nv16,
            Some("nv61") => jpege::InputFormat::Nv61,
            Some("yuv422p") => jpege::InputFormat::Yuv422p,
            Some("yuyv") => jpege::InputFormat::Yuyv,
            Some("yvyu") => jpege::InputFormat::Yvyu,
            Some("uyvy") => jpege::InputFormat::Uyvy,
            Some("vyuy") => jpege::InputFormat::Vyuy,
            Some("nv24") => jpege::InputFormat::Nv24,
            Some("yuv444p") => jpege::InputFormat::Yuv444p,
            Some("yuv400") => jpege::InputFormat::Yuv400,
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
        let mut low_delay = false;
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
            } else if extra == "splitout:lowdelay" || extra == "lowdelay" {
                low_delay = true;
            } else {
                eprintln!("unknown JPEG encoder option {extra}");
                std::process::exit(2);
            }
        }
        match encode_jpeg(
            src,
            dst,
            width,
            height,
            quality,
            input_format,
            chroma,
            rotation,
            split_mcus,
            low_delay,
        ) {
            Ok((bytes, packets, ms)) => println!(
                "encoded {src} -> {dst}: {bytes} bytes, {packets} packet(s), {width}x{height}, {quality_label}, {ms:.3} ms"
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
        let dmabuf_output = std::env::var_os("RKMPP_DECODE_DMABUF").is_some();
        if dmabuf_output && dst != "/dev/null" {
            eprintln!("RKMPP_DECODE_DMABUF requires an output of /dev/null");
            std::process::exit(2);
        }
        let data = match std::fs::read(src) {
            Ok(data) => data,
            Err(e) => {
                eprintln!("{src}: {e}");
                std::process::exit(1);
            }
        };
        let aus = rk_mpp_rs::codec::hevc::decoder::split_parsed_access_units(&data);
        println!("{} access units", aus.len());
        if aus.is_empty() {
            eprintln!("{src}: no H.265 access units found");
            std::process::exit(1);
        }
        let trace = std::env::var_os("RFI_HEVC_TRACE").is_some();
        let requested_depth = match std::env::var("RKMPP_HEVC_PIPELINE") {
            Ok(value) => value.parse::<usize>().unwrap_or(3),
            Err(_) => 3,
        };
        use std::io::Write;
        let mut out = std::io::BufWriter::new(std::fs::File::create(dst).expect("create out"));
        // A queued-job failure is discovered after later pictures were
        // submitted. Re-run through the blocking path and recreate the output
        // so error semantics match the historical collect-and-sort CLI.
        let run = |depth: usize,
                   out: &mut std::io::BufWriter<std::fs::File>|
         -> Result<
            (usize, usize, f64, bool, bool, (u32, u32, u32), Vec<i32>),
            String,
        > {
            let mut dec = rk_mpp_rs::codec::hevc::Decoder::new().map_err(|e| e.to_string())?;
            if depth != 0 && !dec.set_pipeline_depth(true, depth) {
                eprintln!("H.265 pipelining unavailable on this SoC; using blocking decode");
            }
            dec.set_output_dmabuf(dmabuf_output);
            let mut reorder = rk_mpp_rs::codec::hevc::DisplayReorder::new(0);
            let mut decoded = 0usize;
            let mut shown = 0usize;
            let mut failed = false;
            let mut write_error = None;
            let mut decode_time = std::time::Duration::ZERO;
            let mut pocs = Vec::with_capacity(aus.len());
            let recycle = std::cell::RefCell::new(Vec::new());
            let mut emit =
                |picture: rk_mpp_rs::codec::hevc::Picture,
                 out: &mut std::io::BufWriter<std::fs::File>| {
                    let mut reusable = None;
                    let output = rk_mpp_rs::codec::hevc::picture_is_output(&picture);
                    if !dmabuf_output && write_error.is_none() {
                        if let Err(error) = out.write_all(&picture.yuv) {
                            write_error = Some(error);
                        } else {
                            reusable = Some(picture.yuv);
                        }
                    }
                    if write_error.is_none() {
                        shown += 1;
                        pocs.push(picture.poc);
                        if trace {
                            println!(
                                "  picture {shown}: poc={} epoch={} err={:#x} output={}",
                                picture.poc, picture.epoch, picture.err_info, output
                            );
                        }
                    }
                    if let Some(buffer) = reusable {
                        recycle.borrow_mut().push(buffer);
                    }
                };
            for (i, au) in aus.iter().enumerate() {
                let started = Instant::now();
                let result = dec.decode_parsed_access_unit(au);
                decode_time += started.elapsed();
                match result {
                    Ok(Some(picture)) => {
                        decoded += 1;
                        if i < 3
                            && !picture.yuv.is_empty()
                            && std::env::var_os("RKMPP_TRACE").is_some()
                        {
                            let y = &picture.yuv[..picture.width * picture.height];
                            let sum: u64 = y.iter().map(|value| *value as u64).sum();
                            println!(
                                "  frame {i}: poc={} {}x{} y_mean={:.1} status={:08x}/{:08x}/{:08x}",
                                picture.poc,
                                picture.width,
                                picture.height,
                                sum as f64 / y.len() as f64,
                                dec.status().0,
                                dec.status().1,
                                dec.status().2
                            );
                        }
                        reorder.set_max_reorder(dec.max_num_reorder_pics());
                        reorder.push(picture, |picture| emit(picture, out));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        eprintln!("frame {i}: decode error: {error}");
                        failed = true;
                        break;
                    }
                }
                for buffer in recycle.borrow_mut().drain(..) {
                    dec.recycle_output(buffer);
                }
            }
            let started = Instant::now();
            let flushed = dec.flush();
            decode_time += started.elapsed();
            match flushed {
                Ok(pictures) => {
                    for picture in pictures {
                        decoded += 1;
                        reorder.set_max_reorder(dec.max_num_reorder_pics());
                        reorder.push(picture, |picture| emit(picture, out));
                    }
                }
                Err(error) => {
                    eprintln!("pipeline flush: {error}");
                    failed = true;
                }
            }
            reorder.flush(|picture| emit(picture, out));
            for buffer in recycle.borrow_mut().drain(..) {
                dec.recycle_output(buffer);
            }
            if let Some(error) = write_error {
                eprintln!("write output: {error}");
                failed = true;
            }
            out.flush().ok();
            let poisoned = dec.pipeline_poisoned();
            let status = dec.status();
            Ok((
                decoded,
                shown,
                decode_time.as_secs_f64() * 1000.0,
                failed,
                poisoned,
                status,
                pocs,
            ))
        };
        let (mut decoded, mut shown, mut decode_ms, mut failed, poisoned, mut status, mut pocs) =
            match run(requested_depth, &mut out) {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("H.265 decode failed: {error}");
                    std::process::exit(1);
                }
            };
        if poisoned && requested_depth != 0 {
            eprintln!("H.265 queued job failed; re-decoding through the blocking path");
            out.flush().ok();
            out = std::io::BufWriter::new(std::fs::File::create(dst).expect("truncate output"));
            (decoded, shown, decode_ms, failed, _, status, pocs) = match run(0, &mut out) {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("H.265 blocking decode failed: {error}");
                    std::process::exit(1);
                }
            };
        }
        if trace {
            println!("output pocs: {pocs:?}");
        }
        let output_mode = if dmabuf_output {
            "POC-streamed DMA-BUF output"
        } else {
            "POC-streamed output"
        };
        println!(
            "decoded {decoded} frames ({shown} output) in {decode_ms:.1} ms ({:.1} fps), {output_mode}, status={:08x}/{:08x}/{:08x}",
            shown as f64 / (decode_ms / 1000.0),
            status.0,
            status.1,
            status.2
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
        let dmabuf_output = std::env::var_os("RKMPP_DECODE_DMABUF").is_some();
        if dmabuf_output && dst != "/dev/null" {
            eprintln!("RKMPP_DECODE_DMABUF requires an output of /dev/null");
            std::process::exit(2);
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
        let mut decoder = match Av1CliDecoder::new() {
            Ok(decoder) => decoder,
            Err(error) => {
                eprintln!("open AV1 decoder failed: {error}");
                std::process::exit(1);
            }
        };
        if dmabuf_output {
            decoder.set_output_dmabuf(true);
        }
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
                if picture.err_info == 0 {
                    decoder.recycle_output(picture.yuv);
                }
            }
        }
        output.flush().ok();
        let elapsed = started.elapsed().as_secs_f64();
        println!(
            "decoded {decoded} packets ({shown} output) in {:.1} ms ({:.1} fps), {}",
            elapsed * 1000.0,
            shown as f64 / elapsed.max(f64::EPSILON),
            if dmabuf_output {
                "DMA-BUF output"
            } else {
                "planar output"
            }
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
        let dmabuf_output = std::env::var_os("RKMPP_DECODE_DMABUF").is_some();
        if dmabuf_output && dst != "/dev/null" {
            eprintln!("RKMPP_DECODE_DMABUF requires an output of /dev/null");
            std::process::exit(2);
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
        if dmabuf_output {
            decoder.set_output_dmabuf(true);
        }
        use std::io::Write;
        let mut output = std::io::BufWriter::new(match std::fs::File::create(dst) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("{dst}: {error}");
                std::process::exit(1);
            }
        });
        let started = Instant::now();
        // Same stream policy as `decode_stream`, but written out one picture
        // at a time. Collecting the whole stream keeps ~1.4 MB per 720p
        // picture alive, so the allocator faults in and clears fresh pages for
        // every frame (page-clear/fault/dcache traffic showed up as ~20 % of
        // the VP9 profile).
        let mut stopped_early = false;
        let mut dropped: Vec<(usize, String)> = Vec::new();
        let ivf = match rk_mpp_rs::codec::vp9::parser::parse_ivf(&data) {
            Ok(ivf) => ivf,
            Err(error) => {
                eprintln!("packet {} dropped: container: {error}", usize::MAX);
                output.flush().ok();
                eprintln!("decoded 0 packets (0 output, 1 packets dropped, stopped early)");
                std::process::exit(1);
            }
        };
        let mut shown = 0usize;
        for (index, frame) in ivf.frames.iter().enumerate() {
            match decoder.decode_packet_checked_pub(frame.data) {
                Ok(pictures) => {
                    for picture in pictures {
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
                        if picture.err_info == 0 {
                            decoder.recycle_output(picture.yuv);
                        }
                    }
                }
                Err(rk_mpp_rs::codec::vp9::decoder::PacketError::Syntax(error))
                    if error.recoverable() =>
                {
                    dropped.push((index, error.to_string()));
                }
                Err(error) => {
                    dropped.push((index, error.to_string()));
                    stopped_early = true;
                    break;
                }
            }
        }
        for (packet, reason) in &dropped {
            eprintln!("packet {packet} dropped: {reason}");
        }
        output.flush().ok();
        let elapsed = started.elapsed().as_secs_f64();
        println!(
            "decoded {} packets ({} output, {} packets dropped{}) in {:.1} ms ({:.1} fps), {}",
            shown + dropped.len(),
            shown,
            dropped.len(),
            if stopped_early { ", stopped early" } else { "" },
            elapsed * 1000.0,
            shown as f64 / elapsed.max(f64::EPSILON),
            if dmabuf_output {
                "DMA-BUF output"
            } else {
                "planar output"
            }
        );
        if stopped_early {
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
        let dmabuf_output = std::env::var_os("RKMPP_DECODE_DMABUF").is_some();
        if dmabuf_output && dst != "/dev/null" {
            eprintln!("RKMPP_DECODE_DMABUF requires an output of /dev/null");
            std::process::exit(2);
        }
        let nv12_output = std::env::var_os("RKMPP_DECODE_NV12").is_some();
        let input_file = match std::fs::File::open(src) {
            Ok(file) => file,
            Err(e) => {
                eprintln!("{src}: {e}");
                std::process::exit(1);
            }
        };
        let mut input = std::io::BufReader::with_capacity(4 * 1024 * 1024, input_file);
        let mut scanner = rk_mpp_rs::codec::h264::decoder::AccessUnitScanner::default();
        let mut scan_buffer = Vec::new();
        let mut scan_chunk = vec![0u8; 4 * 1024 * 1024];
        let mut split_time = std::time::Duration::ZERO;
        let mut access_units = 0usize;
        let mut dec = match rk_mpp_rs::codec::h264::Decoder::new() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("open rkvdec failed: {e}");
                std::process::exit(1);
            }
        };
        if dmabuf_output {
            dec.set_output_dmabuf(true);
        } else if nv12_output {
            dec.set_output_nv12(true);
        }
        // Keep three jobs in the submission ring. The vendor runtime keeps
        // two to four queued and a single blocking job leaves the second
        // rkvdec core idle. Depth 3 reaches the same poll time as 4 without
        // exhausting the surface pool on streams with 16 DPB references;
        // `RKMPP_H264_PIPELINE=0` restores the blocking path.
        let depth = match std::env::var("RKMPP_H264_PIPELINE") {
            Ok(value) => value.parse::<usize>().unwrap_or(3),
            Err(_) => 3,
        };
        if depth != 0 && !dec.set_pipeline_depth(true, depth) {
            eprintln!("H.264 pipelining unavailable on this SoC; using blocking decode");
        }
        use std::io::{Read, Write};
        let mut out = std::io::BufWriter::new(std::fs::File::create(dst).expect("create out"));
        let mut reorder = rk_mpp_rs::codec::h264::DisplayReorder::new(dec.max_num_reorder_frames());
        let mut decode_time = std::time::Duration::ZERO;
        let mut decoded = 0usize;
        let mut written = 0usize;
        let mut write_error = None;
        let mut failed = false;
        let trace = std::env::var_os("RKMPP_TRACE").is_some();
        let capture_raw_uv = std::env::var("RFI_DUMP_UV").is_ok();
        let mut process_au = |i: usize, au: &[u8]| -> bool {
            let t0 = Instant::now();
            let decoded_picture = dec.decode_access_unit(au);
            decode_time += t0.elapsed();
            reorder.set_max_reorder(dec.max_num_reorder_frames());
            match decoded_picture {
                Ok(Some(pic)) => {
                    decoded += 1;
                    if i < 3 && trace {
                        println!(
                            "    frame {i} submit+poll: {:.2} ms",
                            t0.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                    if capture_raw_uv {
                        std::fs::write("/tmp/raw_uv.bin", &pic.raw_uv).ok();
                    }
                    if i < 3 && pic.yuv.len() >= pic.width * pic.height {
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
                    } else if i < 3 {
                        println!(
                            "  frame {i}: poc={} {}x{} err={:#x} yuv_bytes={} status={:08x}/{:08x}/{:08x}",
                            pic.poc,
                            pic.width,
                            pic.height,
                            pic.err_info,
                            pic.yuv.len(),
                            dec.status().0,
                            dec.status().1,
                            dec.status().2
                        );
                    }
                    reorder.push(pic, |pic| {
                        if write_error.is_none() {
                            let rk_mpp_rs::codec::h264::decoder::Picture { yuv, nv12, .. } = pic;
                            let bytes = nv12.as_deref().unwrap_or(&yuv);
                            if let Err(e) = out.write_all(bytes) {
                                write_error = Some(e);
                            } else {
                                written += 1;
                                if let Some(buffer) = nv12 {
                                    dec.recycle_output(buffer);
                                } else {
                                    dec.recycle_output(yuv);
                                }
                            }
                        }
                    });
                    true
                }
                Ok(None) => true,
                Err(e) => {
                    eprintln!("frame {i}: decode error: {e}");
                    false
                }
            }
        };
        loop {
            let read = match input.read(&mut scan_chunk) {
                Ok(read) => read,
                Err(error) => {
                    eprintln!("{src}: {error}");
                    failed = true;
                    break;
                }
            };
            if read == 0 {
                break;
            }
            scan_buffer.extend_from_slice(&scan_chunk[..read]);
            let started = Instant::now();
            scanner.push(&scan_buffer);
            let units = scanner.take(&mut scan_buffer, false);
            split_time += started.elapsed();
            for au in units {
                access_units += 1;
                if !process_au(access_units - 1, &au) {
                    failed = true;
                    break;
                }
            }
            if failed {
                break;
            }
        }
        if !failed {
            let started = Instant::now();
            scanner.push(&scan_buffer);
            let units = scanner.take(&mut scan_buffer, true);
            split_time += started.elapsed();
            for au in units {
                access_units += 1;
                if !process_au(access_units - 1, &au) {
                    failed = true;
                    break;
                }
            }
        }
        if trace {
            println!(
                "access-unit split: {:.2} ms",
                split_time.as_secs_f64() * 1000.0
            );
        }
        println!("{access_units} access units");
        if access_units == 0 {
            eprintln!("{src}: no H.264 access units found");
            failed = true;
        }
        match dec.flush() {
            Ok(pictures) => {
                for pic in pictures {
                    decoded += 1;
                    reorder.push(pic, |pic| {
                        if write_error.is_none() {
                            let rk_mpp_rs::codec::h264::decoder::Picture { yuv, nv12, .. } = pic;
                            let bytes = nv12.as_deref().unwrap_or(&yuv);
                            if let Err(e) = out.write_all(bytes) {
                                write_error = Some(e);
                            } else {
                                written += 1;
                                if let Some(buffer) = nv12 {
                                    dec.recycle_output(buffer);
                                } else {
                                    dec.recycle_output(yuv);
                                }
                            }
                        }
                    });
                }
            }
            Err(e) => {
                eprintln!("pipeline flush: {e}");
                failed = true;
            }
        }
        reorder.flush(|pic| {
            if write_error.is_none() {
                let rk_mpp_rs::codec::h264::decoder::Picture { yuv, nv12, .. } = pic;
                let bytes = nv12.as_deref().unwrap_or(&yuv);
                if let Err(e) = out.write_all(bytes) {
                    write_error = Some(e);
                } else {
                    written += 1;
                    if let Some(buffer) = nv12 {
                        dec.recycle_output(buffer);
                    } else {
                        dec.recycle_output(yuv);
                    }
                }
            }
        });
        out.flush().ok();
        if let Some(e) = write_error {
            eprintln!("write output: {e}");
            failed = true;
        }
        let decode_ms = decode_time.as_secs_f64() * 1000.0;
        println!(
            "decoded {decoded} frames ({written} output) in {decode_ms:.1} ms ({:.1} fps), POC-sorted output",
            written as f64 / (decode_ms / 1000.0)
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
                            frame_num_offset: fno,
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
    if args.len() > 1 && args[1] == "avsdec" {
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("/tmp/avs.yuv");
        if src.is_empty() {
            eprintln!("usage: rk-mpp-rs avsdec <in.avs> <out.yuv> [max-frames] [i420|nv12|vproc]");
            std::process::exit(1);
        }
        let max_frames = match args.get(4) {
            Some(value) => match value.parse::<usize>() {
                Ok(value) if value > 0 => value,
                _ => {
                    eprintln!("max-frames must be a positive integer, got {value}");
                    std::process::exit(2);
                }
            },
            None => 1,
        };
        let data = match std::fs::read(src) {
            Ok(data) => data,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let mut decoder = match rk_mpp_rs::codec::avs::decoder::Decoder::new() {
            Ok(decoder) => decoder,
            Err(error) => {
                eprintln!("open AVS+ client failed: {error}");
                std::process::exit(1);
            }
        };
        let output_format = match args.get(5).map(String::as_str).unwrap_or("i420") {
            "i420" => "i420",
            "nv12" => "nv12",
            "vproc" => "vproc",
            value => {
                eprintln!("output format must be i420, nv12 or vproc, got {value}");
                std::process::exit(2);
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
        let mut failed = false;
        let result = if output_format == "vproc" {
            let mut frame_buffer = Vec::new();
            decoder.decode_with_vproc(&data, max_frames, |bytes, err_info| {
                if err_info != 0 {
                    failed = true;
                    eprintln!("vproc picture failed with err={err_info:#x}");
                    return Ok(());
                }
                frame_buffer.clear();
                frame_buffer.extend_from_slice(bytes);
                output
                    .write_all(&frame_buffer)
                    .map_err(|error| error.to_string())
            })
        } else if output_format == "nv12" {
            let mut frame_buffer = Vec::new();
            decoder.decode_with_nv12(&data, max_frames, |bytes, err_info| {
                if err_info != 0 {
                    failed = true;
                    eprintln!("picture failed with err={err_info:#x}");
                    return Ok(());
                }
                frame_buffer.clear();
                frame_buffer.extend_from_slice(bytes);
                output
                    .write_all(&frame_buffer)
                    .map_err(|error| error.to_string())
            })
        } else {
            decoder.decode_with(&data, max_frames, |picture| {
                if picture.err_info != 0 {
                    failed = true;
                    eprintln!(
                        "picture at distance {} failed with err={:#x}",
                        picture.picture_distance, picture.err_info
                    );
                    return Ok(());
                }
                output
                    .write_all(&picture.yuv)
                    .map_err(|error| error.to_string())
            })
        };
        let shown = match result {
            Ok(shown) => shown,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
        output.flush().ok();
        println!(
            "decoded {shown} AVS frames in {decode_ms:.1} ms ({:.1} fps), irq={:#010x}",
            shown as f64 / (decode_ms / 1000.0).max(f64::EPSILON),
            decoder.last_irq
        );
        if failed {
            std::process::exit(1);
        }
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
        let mut failed = false;
        let mut shown = 0usize;
        let result = decoder.decode_with(&data, |picture| {
            if picture.err_info != 0 {
                failed = true;
                eprintln!(
                    "picture {} failed with err={:#x}",
                    shown + 1,
                    picture.err_info
                );
            }
            output
                .write_all(&picture.yuv)
                .map_err(|error| error.to_string())?;
            shown += 1;
            Ok(())
        });
        if let Err(error) = result {
            eprintln!("{src}: {error}");
            std::process::exit(1);
        }
        let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
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
        let mut failed = false;
        let mut shown = 0usize;
        let trace = std::env::var_os("RFI_MP4_TRACE").is_some();
        let result = decoder.decode_with(&data, |picture| {
            if picture.err_info != 0 {
                failed = true;
                eprintln!(
                    "picture {} failed with err={:#x}",
                    shown + 1,
                    picture.err_info
                );
            }
            output
                .write_all(&picture.yuv)
                .map_err(|error| error.to_string())?;
            shown += 1;
            if trace {
                println!(
                    "  picture {shown}: {}x{} err={:#x}",
                    picture.width, picture.height, picture.err_info,
                );
            }
            Ok(())
        });
        if let Err(error) = result {
            eprintln!("{src}: {error}");
            std::process::exit(1);
        }
        let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
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
        let mut failed = false;
        let mut shown = 0usize;
        let result = decoder.decode_with(&data, |picture| {
            if picture.err_info != 0 {
                failed = true;
                eprintln!(
                    "picture {} failed with err={:#x}",
                    shown + 1,
                    picture.err_info
                );
            }
            output
                .write_all(&picture.yuv)
                .map_err(|error| error.to_string())?;
            shown += 1;
            Ok(())
        });
        if let Err(error) = result {
            eprintln!("{src}: {error}");
            std::process::exit(1);
        }
        let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
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
        let mut failed = false;
        let mut shown = 0usize;
        let trace = std::env::var_os("RFI_VP8_TRACE").is_some();
        let summary = decoder.decode_stream_with(&data, |picture| {
            if picture.err_info != 0 {
                failed = true;
                eprintln!(
                    "picture {} failed with err={:#x}",
                    shown + 1,
                    picture.err_info
                );
            }
            output.write_all(&picture.yuv).expect("write VP8 frame");
            shown += 1;
            if trace {
                println!(
                    "  picture {shown}: {}x{} err={:#x}",
                    picture.width, picture.height, picture.err_info,
                );
            }
        });
        let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
        for drop in &summary.dropped {
            eprintln!("packet {} dropped: {}", drop.packet, drop.reason);
        }
        output.flush().ok();
        println!(
            "decoded {shown} frames ({} output, {} packets dropped{}) in {decode_ms:.1} ms ({:.1} fps)",
            shown,
            summary.dropped.len(),
            if summary.stopped_early { ", stopped early" } else { "" },
            shown as f64 / (decode_ms / 1000.0).max(f64::EPSILON)
        );
        if summary.stopped_early {
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
    if args.len() > 1 && args[1] == "vdpp-probe" {
        let r = rk_mpp_rs::service::vdpp2_session::probe();
        println!("VDPP client 29:");
        println!("  kernel present (probe_hw_support bit 29): {}", r.present);
        println!(
            "  session init (INIT_CLIENT_TYPE):         {}",
            if r.binds { "binds" } else { "FAIL" }
        );
        if !r.binds {
            if let Some(error) = r.open_errno {
                println!(
                    "    errno: {error} ({})",
                    std::io::Error::from_raw_os_error(error)
                );
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
            r.hw_id.map_or_else(|| "-".to_string(), |id| id.to_string())
        );
        return;
    }
    if args.len() > 1 && args[1] == "vdpp-scale" {
        use rk_mpp_rs::hal::vdpp2::Vdpp2Config;
        use rk_mpp_rs::service::vdpp2_session::Vdpp2Processor;

        let src = args.get(2).map(String::as_str).unwrap_or("");
        let dst = args.get(3).map(String::as_str).unwrap_or("");
        let src_width: usize = args
            .get(4)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let src_height: usize = args
            .get(5)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let dst_width: usize = args
            .get(6)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let dst_height: usize = args
            .get(7)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let output_420 = args.get(8).map(String::as_str) == Some("420");
        let split_output = args.get(9).map(String::as_str) == Some("split");
        let scale_only = args.get(9).map(String::as_str) == Some("scale-only")
            || args.get(10).map(String::as_str) == Some("scale-only");
        if src.is_empty()
            || dst.is_empty()
            || src_width == 0
            || src_height == 0
            || dst_width == 0
            || dst_height == 0
        {
            eprintln!(
                "usage: rk-mpp-rs vdpp-scale <in.nv12> <out.yuv> <src_w> <src_h> <dst_w> <dst_h> [420|444] [split] [scale-only]"
            );
            std::process::exit(2);
        }
        let mut config = if output_420 {
            Vdpp2Config::nv12_to_nv12(src_width, src_height, dst_width, dst_height)
        } else {
            Vdpp2Config::nv12_to_yuv444(src_width, src_height, dst_width, dst_height)
        };
        if scale_only {
            config.enhancement = false;
        }
        config.yuv_out_diff = split_output;
        let input = match std::fs::read(src) {
            Ok(input) => input,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let expected = src_width * src_height * 3 / 2;
        if input.len() < expected {
            eprintln!("{src}: expected at least {expected} bytes");
            std::process::exit(1);
        }
        let mut processor = match Vdpp2Processor::open(config) {
            Ok(processor) => processor,
            Err(error) => {
                eprintln!("vdpp-scale: {error}");
                std::process::exit(1);
            }
        };
        let source = processor.src_frame_mut();
        if let Err(error) = source.sync_write_begin() {
            eprintln!("vdpp-scale source sync: {error}");
            std::process::exit(1);
        }
        let source_bytes = source.as_mut_slice();
        source_bytes.fill(0);
        for row in 0..src_height {
            let input_start = row * src_width;
            let output_start = row * config.src_width_vir;
            source_bytes[output_start..output_start + src_width]
                .copy_from_slice(&input[input_start..input_start + src_width]);
        }
        let chroma_height = src_height.div_ceil(2);
        let luma_size = src_width * src_height;
        let output_uv = config.src_width_vir * config.src_height_vir;
        for row in 0..chroma_height {
            let input_start = luma_size + row * src_width;
            let output_start = output_uv + row * config.src_width_vir;
            source_bytes[output_start..output_start + src_width]
                .copy_from_slice(&input[input_start..input_start + src_width]);
        }
        if let Err(error) = processor.run() {
            eprintln!("vdpp-scale: {error}");
            std::process::exit(1);
        }
        let output = match processor.dst_frame_compact() {
            Ok(output) => output,
            Err(error) => {
                eprintln!("vdpp-scale output: {error}");
                std::process::exit(1);
            }
        };
        if let Err(error) = std::fs::write(dst, output) {
            eprintln!("{dst}: {error}");
            std::process::exit(1);
        }
        return;
    }
    if args.len() > 1 && args[1] == "vdpp-hist" {
        use rk_mpp_rs::hal::vdpp2::{Vdpp2Config, Vdpp2Mode};
        use rk_mpp_rs::service::vdpp2_session::Vdpp2Processor;

        let src = args.get(2).map(String::as_str).unwrap_or("");
        let output_prefix = args.get(3).map(String::as_str).unwrap_or("");
        let width: usize = args
            .get(4)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let height: usize = args
            .get(5)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        if src.is_empty() || output_prefix.is_empty() || width == 0 || height == 0 {
            eprintln!("usage: rk-mpp-rs vdpp-hist <in.nv12> <out-prefix> <width> <height>");
            std::process::exit(2);
        }
        let mut config = Vdpp2Config::nv12_to_yuv444(width, height, width, height);
        config.mode = Vdpp2Mode::Histogram;
        config.histogram.enable = true;
        config.enhancement = false;
        let input = match std::fs::read(src) {
            Ok(input) => input,
            Err(error) => {
                eprintln!("{src}: {error}");
                std::process::exit(1);
            }
        };
        let expected = width * height * 3 / 2;
        if input.len() < expected {
            eprintln!("{src}: expected at least {expected} bytes");
            std::process::exit(1);
        }
        let mut processor = match Vdpp2Processor::open(config) {
            Ok(processor) => processor,
            Err(error) => {
                eprintln!("vdpp-hist: {error}");
                std::process::exit(1);
            }
        };
        let source = processor.src_frame_mut();
        if let Err(error) = source.sync_write_begin() {
            eprintln!("vdpp-hist source sync: {error}");
            std::process::exit(1);
        }
        let source_bytes = source.as_mut_slice();
        source_bytes.fill(0);
        for row in 0..height {
            let input_start = row * width;
            let output_start = row * config.src_width_vir;
            source_bytes[output_start..output_start + width]
                .copy_from_slice(&input[input_start..input_start + width]);
        }
        let luma_size = width * height;
        let output_uv = config.src_width_vir * config.src_height_vir;
        for row in 0..height.div_ceil(2) {
            let input_start = luma_size + row * width;
            let output_start = output_uv + row * config.src_width_vir;
            source_bytes[output_start..output_start + width]
                .copy_from_slice(&input[input_start..input_start + width]);
        }
        if let Err(error) = processor.run() {
            eprintln!("vdpp-hist: {error}");
            std::process::exit(1);
        }
        let histogram = match processor.parsed_histogram() {
            Ok(histogram) => histogram,
            Err(error) => {
                eprintln!("vdpp-hist parse: {error}");
                std::process::exit(1);
            }
        };
        let packed = format!("{output_prefix}.packed");
        let local = format!("{output_prefix}.local.bin");
        let global = format!("{output_prefix}.global.bin");
        if let Err(error) = std::fs::write(&packed, processor.histogram()) {
            eprintln!("{packed}: {error}");
            std::process::exit(1);
        }
        let local_bytes: Vec<u8> = histogram
            .local
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let global_bytes: Vec<u8> = histogram
            .global
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        if let Err(error) = std::fs::write(&local, local_bytes) {
            eprintln!("{local}: {error}");
            std::process::exit(1);
        }
        if let Err(error) = std::fs::write(&global, global_bytes) {
            eprintln!("{global}: {error}");
            std::process::exit(1);
        }
        match histogram.mean_luma_u10() {
            Some(mean) => println!("mean_luma_u10={mean}"),
            None => println!("mean_luma_u10=-1"),
        }
        return;
    }
    if args.len() > 1 && args[1] == "iep-dect" {
        // The PD/DECT two-pass stream: `iep2_test` never selects this mode, so
        // this subcommand is the Rust half of `tools/iep2_dect_probe.c`.
        use rk_mpp_rs::hal::iep::{Iep2DilMode, Iep2FieldOrder, Iep2Fmt, Iep2OutMode, Iep2YuvSwap};
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("");
        let width: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
        let height: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
        let frames: Option<usize> = args.get(6).and_then(|s| s.parse().ok());
        let mut field_order = Iep2FieldOrder::Tff;
        let mut src_fmt = Iep2Fmt::Yuv420;
        let mut src_swap = Iep2YuvSwap::P;
        let mut dst_swap = Iep2YuvSwap::SpUv;
        for text in args.iter().skip(6) {
            match text.as_str() {
                "TFF" => field_order = Iep2FieldOrder::Tff,
                "BFF" => field_order = Iep2FieldOrder::Bff,
                "420p" => {
                    src_fmt = Iep2Fmt::Yuv420;
                    src_swap = Iep2YuvSwap::P;
                }
                "420sp" => {
                    src_fmt = Iep2Fmt::Yuv420;
                    src_swap = Iep2YuvSwap::SpUv;
                }
                "dst420p" => dst_swap = Iep2YuvSwap::P,
                "dst420sp" => dst_swap = Iep2YuvSwap::SpUv,
                "dst420spvu" => dst_swap = Iep2YuvSwap::SpVu,
                _ => {}
            }
        }
        if src.is_empty() || dst.is_empty() || width == 0 || height == 0 {
            eprintln!(
                "usage: rk-mpp-rs iep-dect <in.yuv> <out.yuv> <width> <height> [frames] \
                 [TFF|BFF] [420p|420sp] [dst420p|dst420sp|dst420spvu]"
            );
            std::process::exit(2);
        }
        let cfg = iep_session::DeinterlaceConfig {
            width,
            height,
            hor_stride: width,
            ver_stride: height,
            src_fmt,
            dst_fmt: Iep2Fmt::Yuv420,
            src_swap,
            dst_swap,
            dil_mode: Iep2DilMode::Dect,
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
        match run_iep_dect(&data, dst, cfg, frames) {
            Ok(report) => {
                for (index, (pd_flag, pd_types, order, frm_mode, ratio)) in
                    report.iter().enumerate()
                {
                    println!(
                        "frame {} pd_flag {} pd_types {} dil_order {} frm_mode {} ratio {}",
                        index, pd_flag, pd_types, order, frm_mode, ratio
                    );
                }
                println!("iep-dect: wrote {} pictures to {dst}", report.len());
            }
            Err(e) => {
                eprintln!("iep-dect failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() > 1 && args[1] == "iep-dei" {
        // One IEP2 deinterlace. The raw output always contains both hardware
        // destination slots so single-output modes can verify the unused slot is
        // zero; the active slot count is reported separately.
        use rk_mpp_rs::hal::iep::{Iep2DilMode, Iep2FieldOrder, Iep2Fmt, Iep2OutMode, Iep2YuvSwap};
        let src = args.get(2).map(|s| s.as_str()).unwrap_or("");
        let dst = args.get(3).map(|s| s.as_str()).unwrap_or("");
        let width: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
        let height: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
        let mut field_order = Iep2FieldOrder::Tff;
        let mut dil_mode = Iep2DilMode::I5O2;
        let mut format = Iep2Fmt::Yuv420;
        let mut swap = Iep2YuvSwap::SpUv;
        for text in args.iter().skip(6) {
            match text.as_str() {
                "TFF" => field_order = Iep2FieldOrder::Tff,
                "BFF" => field_order = Iep2FieldOrder::Bff,
                "I5O2" => dil_mode = Iep2DilMode::I5O2,
                "I5O1T" => dil_mode = Iep2DilMode::I5O1T,
                "I5O1B" => dil_mode = Iep2DilMode::I5O1B,
                "I2O2" => dil_mode = Iep2DilMode::I2O2,
                "I1O1T" => dil_mode = Iep2DilMode::I1O1T,
                "I1O1B" => dil_mode = Iep2DilMode::I1O1B,
                "BYPASS" => dil_mode = Iep2DilMode::Bypass,
                "DISABLE" => dil_mode = Iep2DilMode::Disable,
                "420sp" => {
                    format = Iep2Fmt::Yuv420;
                    swap = Iep2YuvSwap::SpUv;
                }
                "420spvu" => {
                    format = Iep2Fmt::Yuv420;
                    swap = Iep2YuvSwap::SpVu;
                }
                "420p" => {
                    format = Iep2Fmt::Yuv420;
                    swap = Iep2YuvSwap::P;
                }
                "422sp" => {
                    format = Iep2Fmt::Yuv422;
                    swap = Iep2YuvSwap::SpUv;
                }
                "422spvu" => {
                    format = Iep2Fmt::Yuv422;
                    swap = Iep2YuvSwap::SpVu;
                }
                "422p" => {
                    format = Iep2Fmt::Yuv422;
                    swap = Iep2YuvSwap::P;
                }
                other => {
                    eprintln!("unknown IEP mode {other}");
                    std::process::exit(2);
                }
            }
        }
        if src.is_empty() || dst.is_empty() || width == 0 || height == 0 {
            eprintln!("usage: rk-mpp-rs iep-dei <in.yuv> <out.yuv> <width> <height> [TFF|BFF] [I5O2|I5O1T|I5O1B|I2O2|I1O1T|I1O1B] [420sp|420spvu|420p|422sp|422spvu|422p]");
            std::process::exit(2);
        }
        let cfg = iep_session::DeinterlaceConfig {
            width,
            height,
            hor_stride: width,
            ver_stride: height,
            src_fmt: format,
            dst_fmt: format,
            src_swap: swap,
            dst_swap: swap,
            dil_mode,
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
            Ok((slots, active)) => {
                println!("iep-dei: wrote {slots} raw slots ({active} active) to {dst}")
            }
            Err(e) => {
                eprintln!("iep-dei: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    println!("rk-mpp-rs probe");
    match soc::SocProfile::detect() {
        Ok(profile) => {
            println!(
                "  soc: {:?}   model: {}",
                profile.id,
                if profile.model.is_empty() {
                    "unknown"
                } else {
                    &profile.model
                }
            );
            println!(
                "  compatible: {}",
                profile.compatible_tokens().collect::<Vec<_>>().join(", ")
            );
            println!(
                "  expected HAL: decode={:?} encode={:?} jpeg={:?} vdpp={:?}",
                profile.video_decode,
                profile.video_encode,
                profile.jpeg_encode,
                profile.video_post_processor
            );
            let ccu = soc::CcuTopology::detect();
            println!(
                "  CCU: decode={} encode={} jpeg={}",
                ccu.video_decode, ccu.video_encode, ccu.jpeg_encode
            );
        }
        Err(error) => println!("  soc detect failed: {error}"),
    }

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
