use rk_mpp_rs::codec::vp9::parser::{parse_ivf, split_superframe, Parser};
use rk_mpp_rs::codec::vp9::picparams::{build, ReferenceSlot, PIC_PARAMS_SIZE};

#[test]
fn profile_zero_picparams_match_c_mpp_for_every_frame() {
    assert_picparams_match(
        include_bytes!("data/vp9_p0_64x64.ivf"),
        include_bytes!("data/vp9_p0_picparams.bin"),
    );
}

#[test]
fn profile_two_picparams_match_c_mpp_for_every_frame() {
    assert_picparams_match(
        include_bytes!("data/vp9_p2_10bit_64x64.ivf"),
        include_bytes!("data/vp9_p2_picparams.bin"),
    );
}

fn assert_picparams_match(ivf_bytes: &[u8], snapshots: &[u8]) {
    let ivf = parse_ivf(ivf_bytes).unwrap();
    assert_eq!(snapshots.len() % PIC_PARAMS_SIZE, 0);
    assert!(snapshots.len() / PIC_PARAMS_SIZE >= ivf.frames.len());

    let mut parser = Parser::new();
    for (index, frame) in ivf.frames.iter().enumerate() {
        let payloads = split_superframe(frame.data).unwrap();
        let parsed = parser
            .parse_frame(payloads[0])
            .unwrap_or_else(|error| panic!("frame {index}: {error}"));
        let compressed = parsed.compressed_header.as_ref().unwrap();
        let expected = &snapshots[index * PIC_PARAMS_SIZE..(index + 1) * PIC_PARAMS_SIZE];
        // Buffer-slot assignment belongs to the decoder's frame pool and is
        // intentionally unrelated to VP9 syntax. Feed MPP's allocation into
        // the builder so every other field and byte offset is still checked.
        let current_slot = expected[0];
        let references = std::array::from_fn(|reference| {
            ReferenceSlot::new(
                expected[16 + reference],
                u32::from_le_bytes(
                    expected[24 + reference * 4..28 + reference * 4]
                        .try_into()
                        .unwrap(),
                ),
                u32::from_le_bytes(
                    expected[56 + reference * 4..60 + reference * 4]
                        .try_into()
                        .unwrap(),
                ),
                parsed.header.bit_depth,
            )
        });
        let actual = build(
            &parsed.header,
            compressed,
            current_slot,
            &references,
            parsed.header.use_prev_in_find_mv_refs,
        );
        if let Some(offset) = actual
            .iter()
            .zip(expected)
            .position(|(actual, expected)| actual != expected)
        {
            panic!(
                "frame {index}, PicParams byte {offset}: Rust {}, C MPP {} \
                 (kind={:?}, high_precision_mv={}, refresh_context={}, parallel={})",
                actual[offset],
                expected[offset],
                parsed.header.frame_kind,
                parsed.header.allow_high_precision_mv,
                parsed.header.refresh_frame_context,
                parsed.header.frame_parallel_mode,
            );
        }
    }
}
