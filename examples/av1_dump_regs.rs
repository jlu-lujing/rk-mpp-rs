use std::mem::offset_of;
use std::path::{Path, PathBuf};

use rk_mpp_rs::codec::av1::assembler::{Av1PacketEvent, Av1PacketParser};
use rk_mpp_rs::codec::av1::picparams::{Av1PicParams, PicParamsPointers};
use rk_mpp_rs::hal::av1d::{
    build_film_grain_memory, build_global_model, build_probability_buffer, build_tile_info,
    display_layout, Av1DmaBuffers, Av1RegisterState, Av1Registers, FILM_GRAIN_BUFFER_SIZE,
};

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

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

    let mut state = Av1RegisterState::default();
    for (job_index, (coded, packet)) in jobs.iter().enumerate() {
        let expected = std::fs::read(format!(
            "tests/data/av1_picparams/picparams_{job_index:03}.bin"
        ))
        .ok();
        let current_slot = expected
            .as_ref()
            .map(|bytes| u16_at(bytes, offset_of!(Av1PicParams, curr_pic_texture_index)) as u8)
            .unwrap_or(job_index as u8);
        let mut reference_texture_slots = [0x7f; 8];
        for logical in 0..7 {
            let source = coded.header.ref_frame_idx[logical] as usize;
            reference_texture_slots[source] = expected
                .as_ref()
                .map(|bytes| bytes[offset_of!(Av1PicParams, frame_refs) + logical * 108 + 48] as u8)
                .unwrap_or(job_index as u8);
        }
        if let Some(expected) = &expected {
            for (slot, value) in reference_texture_slots.iter_mut().enumerate() {
                let mapped = expected[offset_of!(Av1PicParams, ref_frame_map_texture_index) + slot];
                if mapped != 0xff {
                    *value = mapped;
                }
            }
        }

        let params = Av1PicParams::build(
            parser.sequence().unwrap(),
            &coded.header,
            &coded.references_at_header,
            &coded.cdf,
            &coded.tile_ranges,
            coded.frame_start,
            coded.frame_tag_size,
            current_slot,
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
        let buffers = Av1DmaBuffers {
            stream_len: coded.stream_len,
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
