//! The AV1 submit path arms its address offsets by filtering the register
//! build's declared offsets into a reused scratch vector. This test locks
//! that the scratch pass emits exactly the list the original per-frame
//! `filter().map().collect()` produced, for every frame of a real
//! multi-tile stream.

use rk_mpp_rs::codec::av1::assembler::{Av1PacketEvent, Av1PacketParser};
use rk_mpp_rs::codec::av1::picparams::{Av1PicParams, PicParamsPointers};
use rk_mpp_rs::hal::av1d::{Av1DmaBuffers, Av1RegisterState, Av1Registers};

#[test]
fn the_scratch_offset_list_matches_the_original_collect_pass() {
    let data = std::fs::read("tests/data/av1_tiles_320x240_10.ivf").unwrap();
    let ivf = rk_mpp_rs::codec::av1::parser::parse_ivf(&data).unwrap();
    let mut parser = Av1PacketParser::new();
    let mut coded_index = 0;

    for packet in &ivf.frames {
        for event in parser.parse_packet(packet.data).unwrap() {
            let Av1PacketEvent::Coded(frame) = event else {
                continue;
            };
            let mut reference_texture_slots = [0x7fu8; 8];
            for logical in 0..7 {
                let source = frame.header.ref_frame_idx[logical] as usize;
                let candidate = (source as u8) + 1;
                if reference_texture_slots[source] == 0x7f {
                    reference_texture_slots[source] = candidate;
                }
            }
            let pointers = PicParamsPointers {
                cdfs: 0,
                intrabc_cdfs: 0,
            };
            let sequence = parser.sequence().unwrap();
            let params = Av1PicParams::build(
                sequence,
                &frame.header,
                &frame.references_at_header,
                &frame.cdf,
                &frame.tile_ranges,
                frame.frame_start,
                frame.frame_tag_size,
                0,
                &reference_texture_slots,
                pointers,
            );
            let mut state = Av1RegisterState::default();
            let registers = Av1Registers::build(&params, &Av1DmaBuffers::default(), &mut state)
                .expect("register build for a parsed frame");

            // The original per-frame allocation pass, spelled out.
            let expected: Vec<_> = registers
                .address_offsets()
                .iter()
                .filter(|offset| offset.offset != 0)
                .map(|offset| (offset.reg_idx, offset.offset))
                .collect();
            let mut scratch = vec![rk_mpp_rs::service::RegOffsetInfo {
                reg_idx: 0xffff_ffff,
                offset: 0,
            }]; // dirty scratch must be fully rebuilt
            scratch.clear();
            rk_mpp_rs::hal::av1d::write_armed_offsets(&registers, &mut scratch);
            let got: Vec<_> = scratch
                .iter()
                .map(|entry| (entry.reg_idx, entry.offset))
                .collect();
            assert_eq!(
                got, expected,
                "coded frame {coded_index} (order_hint={})",
                frame.header.order_hint
            );
            assert!(
                !expected.is_empty(),
                "every AV1 job arms at least the stream offset"
            );
            coded_index += 1;
        }
    }
    assert_eq!(coded_index, 11);
}
