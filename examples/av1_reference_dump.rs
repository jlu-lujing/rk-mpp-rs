use std::env;
use std::fs;
use std::path::Path;

use rk_mpp_rs::reference::av1::cdef::{
    cdef_direction, cdef_filter_block, cdef_luma_primary_strength, CdefRegion, PlaneView,
};
use rk_mpp_rs::reference::av1::intra_frame::{
    apply_intra_cdef, apply_intra_deblock, apply_intra_loop_restoration, reconstruct_intra_frame,
    IntraFrame, PlaneBuffer,
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

fn trace_cdef(frame: &IntraFrame) {
    let luma = PlaneView::new(&frame.y.data, frame.y.stride, frame.y.width, frame.y.height);
    for row in 0..8 {
        let values: Vec<_> = (0..8)
            .map(|col| {
                let direction = cdef_direction(luma, col * 8, row * 8, frame.bit_depth);
                format!("{}:{}", direction.dir, direction.variance)
            })
            .collect();
        eprintln!("rk cdef row={row} {}", values.join(" "));
    }
    let direction = cdef_direction(luma, 24, 8, frame.bit_depth);
    let primary = cdef_luma_primary_strength(6, direction.variance);
    eprintln!(
        "rk cdef input x=24 y=8 {:?}",
        &luma.samples[8 * luma.stride + 24..8 * luma.stride + 32]
    );
    let block = cdef_filter_block(
        luma,
        CdefRegion::new(80, 60),
        2,
        6,
        0,
        0,
        primary,
        2,
        4,
        direction.dir,
        frame.bit_depth,
    );
    eprintln!(
        "rk cdef block x=24 y=8 primary={primary} direction={} {block:?}",
        direction.dir
    );
}

fn trace_blocks(blocks: &[rk_mpp_rs::reference::av1::intra_sink::IntraSyntaxBlock]) {
    use rk_mpp_rs::reference::av1::intra::UvMode;

    for block in blocks {
        if block.block.row < 8
            || block.block.row > 12
            || block.block.col < 20
            || block.block.col > 28
        {
            continue;
        }
        eprintln!(
            "rk syntax row={} col={} size={:?} y={:?}/{} uv={:?}/{} alpha={}/{} tx={:?} c={}",
            block.block.row,
            block.block.col,
            block.block.size.name,
            block.y_mode,
            block.angle_y,
            block.uv_mode,
            block.angle_uv,
            block.cfl_alphas.alpha_u,
            block.cfl_alphas.alpha_v,
            block
                .transforms
                .iter()
                .map(|tx| (tx.plane, tx.x, tx.y, tx.tx, tx.all_zero))
                .collect::<Vec<_>>(),
            block.uv_mode == Some(UvMode::Cfl),
        );
        if block.block.row == 8 && block.block.col == 20 {
            for tx in &block.transforms {
                eprintln!(
                    "rk quant plane={} x={} y={} tx={:?} type={:?} eob={} quant={:?}",
                    tx.plane, tx.x, tx.y, tx.tx, tx.tx_type, tx.eob, tx.quant
                );
            }
        }
    }
}

fn main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: av1_reference_dump <input.ivf> <out-prefix>")?;
    let prefix = args
        .next()
        .ok_or("usage: av1_reference_dump <input.ivf> <out-prefix>")?;
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
    if env::var_os("RK_AV1_SYNTAX_TRACE").is_some() {
        trace_blocks(&decoded.sink.blocks);
    }

    let prefix = Path::new(&prefix);
    let mut picture = reconstruct_intra_frame(sequence, &frame.header, &decoded.sink.blocks)?;
    if env::var_os("RK_AV1_SYNTAX_TRACE").is_some() {
        use rk_mpp_rs::reference::av1::intra::IntraMode;
        use rk_mpp_rs::reference::av1::predict::{cfl_predict, predict_intra, IntraEdges};

        let u = picture.u.as_ref().unwrap();
        let edges = IntraEdges::from_plane(
            &u.data,
            u.stride,
            48,
            16,
            4,
            4,
            true,
            true,
            true,
            true,
            u.width - 1,
            u.height - 1,
            picture.bit_depth,
        );
        let dc = predict_intra(IntraMode::Dc, &edges, 2, 2, 0, false, 0, picture.bit_depth);
        eprintln!(
            "rk cfl target above={:?} left={:?} dc={:?}",
            &edges.above[..4],
            &edges.left[..4],
            dc
        );
        for filter_type in [0, 1] {
            for enable_filter in [false, true] {
                let edges = IntraEdges::from_plane(
                    &u.data,
                    u.stride,
                    40,
                    16,
                    8,
                    4,
                    true,
                    true,
                    true,
                    true,
                    u.width - 1,
                    u.height - 1,
                    picture.bit_depth,
                );
                let prediction = predict_intra(
                    IntraMode::H,
                    &edges,
                    3,
                    2,
                    -1,
                    enable_filter,
                    filter_type,
                    picture.bit_depth,
                );
                eprintln!(
                    "rk directional filter_type={filter_type} enable={enable_filter} {prediction:?}"
                );
            }
        }
        let cfl = cfl_predict(
            &picture.y.data,
            picture.y.stride,
            48,
            16,
            4,
            4,
            1,
            1,
            picture.y.width,
            picture.y.height,
            -5,
            &dc,
            picture.bit_depth,
        );
        eprintln!("rk cfl target prediction={cfl:?}");
    }
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
    if env::var_os("RK_CDEF_TRACE").is_some() {
        eprintln!(
            "rk cdef params index={:?} y={}/{} uv={}/{} damping={}",
            decoded.sink.cdef_grid().get(0, 0),
            frame.header.cdef_y_pri_strength[0],
            frame.header.cdef_y_sec_strength[0],
            frame.header.cdef_uv_pri_strength[0],
            frame.header.cdef_uv_sec_strength[0],
            frame.header.cdef_damping
        );
        trace_cdef(&picture);
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
