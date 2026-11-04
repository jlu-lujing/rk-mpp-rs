use std::env;
use std::fs;
use std::path::Path;

use rk_mpp_rs::reference::av1::intra_frame::{
    apply_intra_cdef, apply_intra_deblock, apply_intra_loop_restoration, reconstruct_intra_frame,
    upscale_frame, IntraFrame, PlaneBuffer,
};
use rk_mpp_rs::reference::av1::intra_sink::{IntraTileBlockSink, IntraTileConfig};
use rk_mpp_rs::reference::av1::obu::parse_ivf;
use rk_mpp_rs::reference::av1::stream::{ReferenceStream, StreamEvent};
use rk_mpp_rs::reference::av1::tile_entropy::decode_tile_partitions;

fn write_plane<W: std::io::Write>(
    output: &mut W,
    data: &[u16],
    stride: usize,
    width: usize,
    height: usize,
    bit_depth: u32,
) -> std::io::Result<()> {
    if bit_depth <= 8 {
        for row in data.chunks_exact(stride).take(height) {
            let samples = row[..width].iter().map(|sample| *sample as u8);
            output.write_all(&samples.collect::<Vec<_>>())?;
        }
    } else {
        for row in data.chunks_exact(stride).take(height) {
            for sample in &row[..width] {
                output.write_all(&sample.to_le_bytes())?;
            }
        }
    }
    Ok(())
}

fn write_frame(path: &Path, frame: &IntraFrame) -> std::io::Result<()> {
    let mut output = fs::File::create(path)?;
    for plane in [Some(&frame.y), frame.u.as_ref(), frame.v.as_ref()]
        .into_iter()
        .flatten()
    {
        write_plane(
            &mut output,
            &plane.data,
            plane.stride,
            plane.width,
            plane.height,
            frame.bit_depth,
        )?;
    }
    Ok(())
}

fn print_hashes(stage: &str, frame: &IntraFrame) {
    let hash = |data: &[u16]| {
        data.iter().fold(0xcbf29ce484222325u64, |hash, sample| {
            (hash ^ u64::from(*sample)).wrapping_mul(0x100000001b3)
        })
    };
    eprintln!(
        "rk stage={stage} hashes={:x?}",
        (
            hash(&frame.y.data),
            hash(&frame.u.as_ref().unwrap().data),
            hash(&frame.v.as_ref().unwrap().data),
        )
    );
}

fn read_plane<W: std::io::Read>(
    input: &mut W,
    plane: &mut PlaneBuffer,
    bit_depth: u32,
) -> std::io::Result<()> {
    for row in 0..plane.height {
        let start = row * plane.stride;
        let samples = &mut plane.data[start..start + plane.width];
        if bit_depth <= 8 {
            let mut bytes = vec![0u8; plane.width];
            input.read_exact(&mut bytes)?;
            for (sample, byte) in samples.iter_mut().zip(bytes) {
                *sample = u16::from(byte);
            }
        } else {
            for sample in samples {
                let mut bytes = [0u8; 2];
                input.read_exact(&mut bytes)?;
                *sample = u16::from_le_bytes(bytes);
            }
        }
    }
    Ok(())
}

fn replace_with_reference_deblock(path: &str, frame: &mut IntraFrame) -> Result<(), String> {
    let mut input = fs::File::open(path).map_err(|error| error.to_string())?;
    read_plane(&mut input, &mut frame.y, frame.bit_depth).map_err(|error| error.to_string())?;
    if let Some(u) = frame.u.as_mut() {
        read_plane(&mut input, u, frame.bit_depth).map_err(|error| error.to_string())?;
    }
    if let Some(v) = frame.v.as_mut() {
        read_plane(&mut input, v, frame.bit_depth).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: av1_reference_dump <input.ivf> <out-prefix> [reference-deblock.yuv]")?;
    let prefix = args
        .next()
        .ok_or("usage: av1_reference_dump <input.ivf> <out-prefix> [reference-deblock.yuv]")?;
    let reference_deblock = args.next();

    let bytes = fs::read(input).map_err(|error| error.to_string())?;
    let ivf = parse_ivf(&bytes).map_err(|error| format!("{error:?}"))?;
    let mut stream = ReferenceStream::new();
    let packets = stream
        .parse_ivf(&ivf)
        .map_err(|error| format!("{error:?}"))?;
    let sequence = stream.sequence().ok_or("missing sequence header")?;
    let frame = packets
        .iter()
        .flatten()
        .find_map(|event| match event {
            StreamEvent::CodedFrame(frame) => Some(frame),
            _ => None,
        })
        .ok_or("missing coded frame")?;

    let config = IntraTileConfig::from_headers(sequence, &frame.header);
    let superblock_mi = config.superblock_mi;
    let sb_size = if superblock_mi == 32 { 128 } else { 64 };
    let tile = &frame.layout.tiles[0];
    let decoded = decode_tile_partitions(
        &frame.payload[tile.bytes.clone()],
        &frame.header,
        tile.tile,
        sb_size,
        false,
        true,
        IntraTileBlockSink::new(config),
    )
    .map_err(|error| format!("{error:?}"))?;
    let prefix = Path::new(&prefix);
    let mut picture = reconstruct_intra_frame(sequence, &frame.header, &decoded.sink.blocks)?;
    write_frame(&prefix.with_extension("pre.yuv"), &picture).map_err(|error| error.to_string())?;
    print_hashes("pre", &picture);

    apply_intra_deblock(sequence, &frame.header, &decoded.sink.blocks, &mut picture);
    write_frame(&prefix.with_extension("deblock.yuv"), &picture)
        .map_err(|error| error.to_string())?;
    print_hashes("deblock", &picture);
    let deblocked = picture.clone();
    if let Some(reference_deblock) = reference_deblock {
        replace_with_reference_deblock(&reference_deblock, &mut picture)?;
    }
    apply_intra_cdef(
        sequence,
        &frame.header,
        &decoded.sink.blocks,
        decoded.sink.cdef_grid(),
        &mut picture,
    );
    write_frame(&prefix.with_extension("cdef.yuv"), &picture).map_err(|error| error.to_string())?;
    print_hashes("cdef", &picture);
    if frame.header.use_superres {
        let upscaled = upscale_frame(sequence, &frame.header, &picture)?;
        write_frame(&prefix.with_extension("cdef-upscaled.yuv"), &upscaled)
            .map_err(|error| error.to_string())?;
    }

    apply_intra_loop_restoration(
        sequence,
        &frame.header,
        decoded.sink.lr_params(),
        &deblocked,
        &mut picture,
    )?;
    write_frame(&prefix.with_extension("lr.yuv"), &picture).map_err(|error| error.to_string())?;
    print_hashes("lr", &picture);
    Ok(())
}
