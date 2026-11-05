use std::env;
use std::fs;
use std::path::Path;

use rk_mpp_rs::reference::av1::cdf_state::CdfState;
use rk_mpp_rs::reference::av1::inter_frame::{reconstruct_inter_frame, ReferenceFrameStore};
use rk_mpp_rs::reference::av1::inter_sink::{InterTileBlockSink, InterTileConfig};
use rk_mpp_rs::reference::av1::intra_frame::{
    apply_intra_cdef, apply_intra_deblock, apply_intra_loop_restoration, reconstruct_intra_frame,
    upscale_frame, IntraFrame,
};
use rk_mpp_rs::reference::av1::intra_sink::{IntraTileBlockSink, IntraTileConfig};
use rk_mpp_rs::reference::av1::obu::parse_ivf;
use rk_mpp_rs::reference::av1::stream::{ReferenceStream, StreamEvent};
use rk_mpp_rs::reference::av1::tile_entropy::decode_tile_partitions_with_cdfs;

fn write_plane<W: std::io::Write>(
    output: &mut W,
    data: &[u16],
    stride: usize,
    width: usize,
    height: usize,
    bit_depth: u32,
) -> std::io::Result<()> {
    for row in data.chunks_exact(stride).take(height) {
        if bit_depth <= 8 {
            output.write_all(&row[..width].iter().map(|v| *v as u8).collect::<Vec<_>>())?;
        } else {
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

fn main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: av1_inter_reference_dump <input.ivf> <out-dir>")?;
    let output = args
        .next()
        .ok_or("usage: av1_inter_reference_dump <input.ivf> <out-dir>")?;
    let bytes = fs::read(input).map_err(|error| error.to_string())?;
    let ivf = parse_ivf(&bytes).map_err(|error| format!("{error:?}"))?;
    let mut stream = ReferenceStream::new();
    let events = stream
        .parse_ivf(&ivf)
        .map_err(|error| format!("{error:?}"))?;
    let sequence = stream.sequence().ok_or("missing sequence header")?;
    let output = Path::new(&output);
    fs::create_dir_all(output).map_err(|error| error.to_string())?;

    let mut references = ReferenceFrameStore::new();
    let mut shown = 0usize;
    for event in events.iter().flatten() {
        let StreamEvent::CodedFrame(frame) = event else {
            continue;
        };
        let initial_cdfs = if frame.header.primary_ref_frame
            == rk_mpp_rs::reference::av1::frame_header::PRIMARY_REF_NONE
        {
            CdfState::new()
        } else {
            let slot = frame.header.ref_frame_idx[frame.header.primary_ref_frame as usize] as usize;
            references
                .load_cdfs(slot)
                .ok_or_else(|| format!("missing saved CDF slot {slot}"))?
        };
        let (picture, final_cdfs) = if frame.header.frame_is_intra {
            let config = IntraTileConfig::from_headers(sequence, &frame.header);
            let sb_size = if config.superblock_mi == 32 { 128 } else { 64 };
            let tile = &frame.layout.tiles[0];
            let decoded = decode_tile_partitions_with_cdfs(
                &frame.payload[tile.bytes.clone()],
                &frame.header,
                tile.tile,
                sb_size,
                false,
                true,
                initial_cdfs,
                IntraTileBlockSink::new(config),
            )
            .map_err(|error| format!("{error:?}"))?;
            let mut picture =
                reconstruct_intra_frame(sequence, &frame.header, &decoded.sink.blocks)?;
            if frame.header.loop_filter_level[0] != 0 || frame.header.loop_filter_level[1] != 0 {
                apply_intra_deblock(sequence, &frame.header, &decoded.sink.blocks, &mut picture);
            }
            let deblocked = picture.clone();
            if sequence.enable_cdef && !frame.header.coded_lossless {
                apply_intra_cdef(
                    sequence,
                    &frame.header,
                    &decoded.sink.blocks,
                    decoded.sink.cdef_grid(),
                    &mut picture,
                );
            }
            let cdef = picture.clone();
            if frame.header.use_superres {
                picture = upscale_frame(sequence, &frame.header, &picture)?;
            }
            apply_intra_loop_restoration(
                sequence,
                &frame.header,
                decoded.sink.lr_params(),
                &deblocked,
                &mut picture,
            )?;
            if frame.header.use_superres {
                let _ = cdef;
            }
            (picture, decoded.cdfs)
        } else {
            let config = InterTileConfig::from_headers(sequence, &frame.header);
            let sb_size = if config.superblock_mi == 32 { 128 } else { 64 };
            let tile = &frame.layout.tiles[0];
            let decoded = decode_tile_partitions_with_cdfs(
                &frame.payload[tile.bytes.clone()],
                &frame.header,
                tile.tile,
                sb_size,
                false,
                true,
                initial_cdfs,
                InterTileBlockSink::new(config),
            )
            .map_err(|error| format!("{error:?}"))?;
            let picture =
                reconstruct_inter_frame(sequence, &frame.header, &decoded.sink.blocks, &references)
                    .map_err(|error| error.to_string())?;
            println!(
                "frame {shown}: {} inter blocks, {} bytes",
                decoded.sink.blocks.len(),
                frame.payload[tile.bytes.clone()].len()
            );
            (picture, decoded.cdfs)
        };
        references.refresh(
            frame.header.refresh_frame_flags,
            &picture,
            frame.header.base_q_idx,
            &final_cdfs,
        );
        write_frame(&output.join(format!("{shown:02}.yuv")), &picture)
            .map_err(|error| error.to_string())?;
        shown += 1;
    }
    Ok(())
}
