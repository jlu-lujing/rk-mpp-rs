use std::hint::black_box;
use std::time::Instant;

use rk_mpp_rs::codec::yuv::nv12_to_planar;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        return Err(format!(
            "usage: {} <session.nv12> <width> <height> <frames> [samples]",
            args[0]
        ));
    }
    let width = args[2]
        .parse::<usize>()
        .map_err(|error| format!("width: {error}"))?;
    let height = args[3]
        .parse::<usize>()
        .map_err(|error| format!("height: {error}"))?;
    let frames = args[4]
        .parse::<usize>()
        .map_err(|error| format!("frames: {error}"))?;
    let samples = args
        .get(5)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(7);
    let frame_size = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .map(|bytes| bytes / 2)
        .ok_or_else(|| "frame size overflow".to_string())?;
    let input = std::fs::read(&args[1]).map_err(|error| format!("{}: {error}", args[1]))?;
    if input.len() < frame_size * frames {
        return Err(format!(
            "{}: need {} bytes for {frames} frames, has {}",
            args[1],
            frame_size * frames,
            input.len()
        ));
    }

    let mut times = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let mut checksum = 0u64;
        for frame in input.chunks_exact(frame_size).take(frames) {
            let pixels = width * height;
            let output = nv12_to_planar(&frame[..pixels], &frame[pixels..], width, height, width);
            checksum ^= u64::from(output[0])
                ^ u64::from(output[pixels / 2])
                ^ u64::from(*output.last().unwrap());
            black_box(output);
        }
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        black_box(checksum);
        times.push(elapsed);
    }
    times.sort_by(f64::total_cmp);
    let median = times[times.len() / 2];
    let mut checksum = 0xcbf2_9ce4_8422_2325u64;
    for frame in input.chunks_exact(frame_size).take(frames) {
        let pixels = width * height;
        let output = nv12_to_planar(&frame[..pixels], &frame[pixels..], width, height, width);
        for byte in output {
            checksum ^= u64::from(byte);
            checksum = checksum.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    println!(
        "nv12_to_planar {width}x{height} {frames} frames: {median:.3} ms ({:.1} fps), fnv={checksum:016x}",
        frames as f64 * 1000.0 / median,
    );
    Ok(())
}
