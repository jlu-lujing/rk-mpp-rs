//! Streaming H.264 decode with bounded display reordering.
//!
//! Usage: `h264_stream <in.264> [out.yuv|-] [nv12|planar]`
//! With no output path, decoded pictures are counted and released. With an
//! output path, frames are written as soon as each picture is display-ready.
//! `-` writes the frame stream to stdout for a downstream player.

use std::io::Write;
use std::time::Instant;

use rk_mpp_rs::codec::h264::{Decoder, DisplayReorder, Picture};

fn write_picture(
    picture: Picture,
    out: &mut Option<std::io::BufWriter<Box<dyn Write>>>,
    written: &mut usize,
    error: &mut Option<std::io::Error>,
) {
    if error.is_some() {
        return;
    }
    if let Some(out) = out.as_mut() {
        let frame = picture.nv12.as_deref().unwrap_or(&picture.yuv);
        if let Err(e) = out.write_all(frame) {
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
        .ok_or_else(|| "usage: h264_stream <in.264> [out.yuv|-] [nv12|planar]".to_string())?;
    let dst = args.next();
    let planar = match args.next().as_deref() {
        None | Some("nv12") => false,
        Some("planar") => true,
        Some(other) => return Err(format!("unknown output format: {other}")),
    };
    let data = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
    let aus = rk_mpp_rs::codec::h264::decoder::split_access_units(&data);
    let mut decoder = Decoder::new().map_err(|e| format!("open decoder: {e}"))?;
    decoder.set_output_nv12(!planar);
    let mut out = dst
        .map(|path| {
            if path == "-" {
                Ok(std::io::BufWriter::new(
                    Box::new(std::io::stdout()) as Box<dyn Write>
                ))
            } else {
                std::fs::File::create(path)
                    .map(|file| std::io::BufWriter::new(Box::new(file) as Box<dyn Write>))
                    .map_err(|e| format!("create output: {e}"))
            }
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
        "decoded {written} output in {:.1} ms ({:.1} fps), reorder={}, format={}",
        elapsed * 1000.0,
        written as f64 / elapsed,
        decoder.max_num_reorder_frames(),
        if planar { "planar" } else { "nv12" }
    );
    Ok(())
}
