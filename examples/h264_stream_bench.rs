//! Streaming H.264 decode with bounded display reordering. An optional output
//! path writes planar YUV as frames become display-ready instead of retaining
//! the entire stream in memory.

use std::io::Write;
use std::time::Instant;

use rk_mpp_rs::codec::h264::{Decoder, DisplayReorder, Picture};

fn write_picture(
    picture: Picture,
    out: &mut Option<std::io::BufWriter<std::fs::File>>,
    written: &mut usize,
    error: &mut Option<std::io::Error>,
) {
    if error.is_some() {
        return;
    }
    if let Some(out) = out.as_mut() {
        if let Err(e) = out.write_all(&picture.yuv) {
            *error = Some(e);
            return;
        }
    }
    *written += 1;
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or_else(|| "usage: h264_stream_bench <in.264> [out.yuv]".to_string())?;
    let dst = args.next();
    let data = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
    let aus = rk_mpp_rs::codec::h264::decoder::split_access_units(&data);
    let mut decoder = Decoder::new().map_err(|e| format!("open decoder: {e}"))?;
    let mut out = dst
        .map(|path| {
            std::fs::File::create(path)
                .map(std::io::BufWriter::new)
                .map_err(|e| format!("create output: {e}"))
        })
        .transpose()?;

    let start = Instant::now();
    let mut written = 0usize;
    let mut write_error = None;
    let mut reorder = DisplayReorder::new(decoder.max_num_reorder_frames());
    for au in aus {
        if let Some(picture) = decoder
            .decode_access_unit(au)
            .map_err(|e| format!("decode: {e}"))?
        {
            reorder.set_max_reorder(decoder.max_num_reorder_frames());
            reorder.push(picture, |picture| {
                write_picture(picture, &mut out, &mut written, &mut write_error);
            });
        }
    }
    reorder.flush(|picture| {
        write_picture(picture, &mut out, &mut written, &mut write_error);
    });
    if let Some(out) = out.as_mut() {
        out.flush().map_err(|e| format!("flush output: {e}"))?;
    }
    if let Some(error) = write_error {
        return Err(format!("write output: {error}"));
    }
    let elapsed = start.elapsed().as_secs_f64();
    eprintln!(
        "decoded {written} output in {:.1} ms ({:.1} fps), reorder={}",
        elapsed * 1000.0,
        written as f64 / elapsed,
        decoder.max_num_reorder_frames()
    );
    Ok(())
}
