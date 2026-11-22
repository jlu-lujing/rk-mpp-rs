use std::mem::offset_of;
use std::path::{Path, PathBuf};

use rk_mpp_rs::codec::av1::assembler::{Av1PacketEvent, Av1PacketParser};
use rk_mpp_rs::codec::av1::picparams::{Av1PicParams, PicParamsPointers};
use rk_mpp_rs::hal::av1d::{
    build_film_grain_memory, build_global_model, build_probability_buffer, build_tile_info,
    display_layout, Av1DmaBuffers, Av1RegisterState, Av1Registers, FILM_GRAIN_BUFFER_SIZE,
};

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn write_words(path: &Path, words: &[u32]) {
    use std::fmt::Write as _;

    let mut text = String::with_capacity(words.len() * 23);
    for (index, word) in words.iter().enumerate() {
        writeln!(text, "reg[{index:3}] = {word:08x}").unwrap();
    }
    std::fs::write(path, text).unwrap();
}

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .unwrap_or_else(|| "tests/data/av1_320x240_10.ivf".into());
    let job_limit = args.next().map(|value| value.parse().unwrap()).unwrap_or(2);
    let dump_directory = args.next().map(PathBuf::from);
    if let Some(directory) = &dump_directory {
        std::fs::create_dir_all(directory).unwrap();
    }
    let data = std::fs::read(input).unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let mut parser = Av1PacketParser::new();
    let mut jobs = Vec::new();
    for packet in &ivf.frames {
        for event in parser.parse_packet(packet.data).unwrap() {
            if let Av1PacketEvent::Coded(frame) = event {
                jobs.push((frame, packet.data));
                if jobs.len() == job_limit {
                    break;
                }
            }
        }
        if jobs.len() == job_limit {
            break;
        }
    }
    if std::env::var_os("AV1_HEADER_TRACE").is_some() {
        for (index, (coded, _)) in jobs.iter().enumerate() {
            eprintln!(
                "job {index}: primary={} allow_warped={} q={} dq={}/{} dlf={}/{}/{} intrabc={} global={:?}",
                coded.header.primary_ref_frame,
                coded.header.allow_warped_motion,
                coded.header.base_q_idx,
                coded.header.delta_q_present,
                coded.header.delta_q_res,
                coded.header.delta_lf_present,
                coded.header.delta_lf_res,
                coded.header.delta_lf_multi,
                coded.header.allow_intrabc,
                coded.header.global_motion
            );
            for (slot, reference) in coded.references_at_header.iter().enumerate() {
                if reference.valid {
                    eprintln!(
                        "  slot {slot}: order={} warped={:?}",
                        reference.order_hint, reference.warped_motion
                    );
                }
            }
        }
    }

    let mut state = Av1RegisterState::default();
    let mut reference_frames: [Option<usize>; 8] = [None; 8];
    let mut allocated_frames = 0usize;
    for (job_index, (coded, packet)) in jobs.iter().enumerate() {
        let expected = std::fs::read(format!(
            "tests/data/av1_picparams/picparams_{job_index:03}.bin"
        ))
        .ok();
        let live: Vec<usize> = reference_frames.iter().flatten().copied().collect();
        let current_slot = (0..allocated_frames)
            .find(|index| !live.contains(index))
            .unwrap_or(allocated_frames);
        if current_slot == allocated_frames {
            allocated_frames += 1;
        }
        let reference_texture_slots =
            reference_frames.map(|reference| reference.map_or(0x7f, |index| index as u8));

        let params = Av1PicParams::build(
            parser.sequence().unwrap(),
            &coded.header,
            &coded.references_at_header,
            &coded.cdf,
            &coded.tile_ranges,
            coded.frame_start,
            coded.frame_tag_size,
            current_slot as u8,
            &reference_texture_slots,
            PicParamsPointers {
                cdfs: expected
                    .as_ref()
                    .map(|bytes| u64_at(bytes, offset_of!(Av1PicParams, cdfs)))
                    .unwrap_or(0),
                intrabc_cdfs: expected
                    .as_ref()
                    .map(|bytes| u64_at(bytes, offset_of!(Av1PicParams, cdfs_ndvc)))
                    .unwrap_or(0),
            },
        );

        let display = display_layout(
            usize::from(params.width),
            usize::from(params.height),
            u32::from(params.bitdepth),
        )
        .unwrap();
        let mut reference_fds = [None; 24];
        for reference in reference_frames.iter().flatten().copied() {
            reference_fds[reference] = Some(0x1100 + reference as u32);
        }
        let buffers = Av1DmaBuffers {
            current_output: 0x1000,
            references: reference_fds,
            stream_len: coded.stream_len,
            stream: 0x1200,
            global_model: 0x1300,
            tile_info: 0x1301,
            probability: 0x1302,
            probability_out: 0x1303,
            filter: 0x1304,
            tile: 0x1305,
            film_grain: 0x1306,
            display_output: 0x1400,
            display_hor_stride: display.hor_stride as u32,
            display_ver_stride: display.ver_stride as u32,
            display_yuv420sp: params.bitdepth <= 8,
            ..Av1DmaBuffers::default()
        };
        let regs = Av1Registers::build(&params, &buffers, &mut state).unwrap();
        let intra = matches!(params.format.frame_type, 0 | 2);
        let probability = build_probability_buffer(&coded.cdf, intra);
        let global_model = build_global_model(&params);
        let tile_info = build_tile_info(&params, true);
        let frame_start = coded.frame_start as usize;
        let frame_end = frame_start + coded.stream_len as usize;
        let stream = &packet[frame_start..frame_end];

        if let Some(directory) = &dump_directory {
            std::fs::write(
                directory.join(format!("picparams_{job_index:03}.bin")),
                params.as_bytes(),
            )
            .unwrap();
            std::fs::write(
                directory.join(format!("prob_in_{job_index:03}.bin")),
                probability,
            )
            .unwrap();
            std::fs::write(
                directory.join(format!("global_model_{job_index:03}.bin")),
                global_model,
            )
            .unwrap();
            std::fs::write(
                directory.join(format!("tile_info_{job_index:03}.bin")),
                tile_info,
            )
            .unwrap();
            std::fs::write(directory.join(format!("stream_{job_index:03}.bin")), stream).unwrap();
            let film_grain = build_film_grain_memory(&params).unwrap();
            let mut film_grain_buffer = [0u8; FILM_GRAIN_BUFFER_SIZE];
            film_grain_buffer[..film_grain.len()].copy_from_slice(&film_grain);
            std::fs::write(
                directory.join(format!("film_grain_mem_{job_index:03}.bin")),
                film_grain_buffer,
            )
            .unwrap();
            write_words(
                &directory.join(format!("reg_{job_index:03}_in.txt")),
                regs.words(),
            );
            use std::fmt::Write as _;
            let mut offsets = String::new();
            for offset in regs.address_offsets() {
                writeln!(
                    offsets,
                    "reg[{}] += 0x{:08x}",
                    offset.reg_idx, offset.offset
                )
                .unwrap();
            }
            std::fs::write(
                directory.join(format!("offsets_{job_index:03}.txt")),
                offsets,
            )
            .unwrap();
        }
        for (slot, reference) in reference_frames.iter_mut().enumerate() {
            if coded.header.refresh_frame_flags & (1 << slot) != 0 {
                *reference = Some(current_slot);
            }
        }

        eprintln!(
            "job={job_index} type={} coding={:016x} start={} len={} reg5={:08x} warp_bit={} grain={} seed={:04x} y/cb/cr={}/{}/{} lag={} chroma_luma={}",
            params.format.frame_type,
            params.coding,
            coded.frame_start,
            coded.stream_len,
            regs.words()[5],
            (params.coding >> 16) & 1,
            params.film_grain.apply_grain,
            params.film_grain.grain_seed,
            params.film_grain.num_y_points,
            params.film_grain.num_cb_points,
            params.film_grain.num_cr_points,
            params.film_grain.ar_coeff_lag,
            params.film_grain.chroma_scaling_from_luma,
        );
        println!("JOB {job_index}");
        for (index, word) in regs.words().iter().enumerate() {
            println!("reg[{index:03}] = {word:08x}");
        }
        for offset in regs.address_offsets() {
            println!("offset[{}] = {:08x}", offset.reg_idx, offset.offset);
        }
    }
}
