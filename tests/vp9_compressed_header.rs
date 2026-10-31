use rk_mpp_rs::codec::vp9::compressed::{CompoundPredictionMode, CompressedHeader, TxMode};
use rk_mpp_rs::codec::vp9::parser::{parse_ivf, split_superframe, Parser};
use rk_mpp_rs::codec::vp9::prob::DEFAULT_HEADER_PROBS;

#[derive(Debug, PartialEq, Eq)]
struct Probe {
    tx_mode: u8,
    allow_compound: bool,
    compound_mode: u8,
    skip: [u8; 3],
    mv_mode_00: u8,
    filter_00: u8,
    intra_0: u8,
    y_mode_00: u8,
    partition_000: u8,
    mv_joint_0: u8,
    mv_sign_0: u8,
    mv_class0_0: u8,
    mv_class0_fp_00: u8,
    mv_fp_0: [u8; 2],
    mv_hp_0: [u8; 2],
    coefficient_000: [u8; 3],
    coefficient_last: [u8; 3],
}

fn probe(header: &CompressedHeader) -> Probe {
    let probabilities = &header.probabilities;
    Probe {
        tx_mode: match header.tx_mode {
            TxMode::Only4x4 => 0,
            TxMode::Only8x8 => 1,
            TxMode::Only16x16 => 2,
            TxMode::Only32x32 => 3,
            TxMode::Select => 4,
        },
        allow_compound: header.allow_compound_prediction,
        compound_mode: match header.compound_prediction_mode {
            CompoundPredictionMode::SingleReference => 0,
            CompoundPredictionMode::CompoundReference => 1,
            CompoundPredictionMode::Select => 2,
        },
        skip: probabilities.header.skip,
        mv_mode_00: probabilities.header.mv_mode[0][0],
        filter_00: probabilities.header.filter[0][0],
        intra_0: probabilities.header.intra[0],
        y_mode_00: probabilities.header.y_mode[0][0],
        partition_000: probabilities.header.partition[0][0][0],
        mv_joint_0: probabilities.header.mv_joint[0],
        mv_sign_0: probabilities.header.mv_comp[0].sign,
        mv_class0_0: probabilities.header.mv_comp[0].class0,
        mv_class0_fp_00: probabilities.header.mv_comp[0].class0_fp[0][0],
        mv_fp_0: [
            probabilities.header.mv_comp[0].fp[0],
            probabilities.header.mv_comp[0].fp[1],
        ],
        mv_hp_0: [
            probabilities.header.mv_comp[0].class0_hp,
            probabilities.header.mv_comp[0].hp,
        ],
        coefficient_000: probabilities.coefficients[0][0][0][0][0],
        coefficient_last: probabilities.coefficients[3][1][1][5][5],
    }
}

#[test]
fn parses_a_real_profile_zero_keyframe_compressed_header() {
    let ivf = parse_ivf(include_bytes!("data/vp9_p0_64x64.ivf")).unwrap();
    let payloads = split_superframe(ivf.frames[0].data).unwrap();
    let frame = Parser::new().parse_frame(payloads[0]).unwrap();
    let compressed = frame.compressed_header.unwrap();

    assert_eq!(compressed.tx_mode, TxMode::Select);
    assert!(!compressed.allow_compound_prediction);
    assert_eq!(
        compressed.compound_prediction_mode,
        CompoundPredictionMode::SingleReference
    );
    assert_eq!(compressed.probabilities.header.skip, [235, 128, 64]);
    assert_eq!(
        compressed.probabilities.header.y_mode,
        DEFAULT_HEADER_PROBS.y_mode
    );
    assert_eq!(
        compressed.probabilities.coefficients[0][0][0][0][0],
        [195, 29, 14]
    );
}

#[test]
fn parses_compressed_headers_for_every_profile_zero_frame() {
    let ivf = parse_ivf(include_bytes!("data/vp9_p0_64x64.ivf")).unwrap();
    let mut parser = Parser::new();
    let mut seen_update = false;

    for (index, frame) in ivf.frames.iter().enumerate() {
        let payloads = split_superframe(frame.data).unwrap();
        let parsed = parser
            .parse_frame(payloads[0])
            .unwrap_or_else(|error| panic!("frame {index}: {error}"));
        let compressed = parsed.compressed_header.unwrap();
        seen_update |= compressed.probabilities != Default::default();
    }

    assert!(
        seen_update,
        "inter frames must adapt at least one probability"
    );
}

#[test]
fn parses_compressed_headers_for_every_profile_two_frame() {
    let ivf = parse_ivf(include_bytes!("data/vp9_p2_10bit_64x64.ivf")).unwrap();
    let mut parser = Parser::new();

    for (index, frame) in ivf.frames.iter().enumerate() {
        let payloads = split_superframe(frame.data).unwrap();
        parser
            .parse_frame(payloads[0])
            .unwrap_or_else(|error| panic!("frame {index}: {error}"));
    }
}

#[test]
fn compressed_header_state_matches_the_c_mpp_probe_for_every_frame() {
    let ivf = parse_ivf(include_bytes!("data/vp9_p0_64x64.ivf")).unwrap();
    let mut parser = Parser::new();
    let mut actual = Vec::new();
    for frame in &ivf.frames {
        let payloads = split_superframe(frame.data).unwrap();
        let parsed = parser.parse_frame(payloads[0]).unwrap();
        actual.push(probe(parsed.compressed_header.as_ref().unwrap()));
    }

    let expected = [
        (216, 14),
        (216, 14),
        (216, 14),
        (1, 14),
        (1, 14),
        (1, 14),
        (216, 20),
        (1, 20),
        (1, 20),
        (1, 20),
        (1, 20),
        (1, 20),
    ];
    assert_eq!(actual.len(), expected.len());
    for (index, (probe, (mv_class0, coefficient_000_count))) in
        actual.into_iter().zip(expected).enumerate()
    {
        assert_eq!(
            probe,
            Probe {
                tx_mode: 4,
                allow_compound: false,
                compound_mode: 0,
                skip: [235, 128, 64],
                mv_mode_00: 2,
                filter_00: 235,
                intra_0: 9,
                y_mode_00: 65,
                partition_000: 222,
                mv_joint_0: 32,
                mv_sign_0: 128,
                mv_class0_0: mv_class0,
                mv_class0_fp_00: 128,
                mv_fp_0: [64, 96],
                mv_hp_0: [160, 128],
                coefficient_000: [195, 29, coefficient_000_count],
                coefficient_last: [1, 16, 6],
            },
            "frame {index}"
        );
    }
}

#[test]
fn every_profile_zero_context_matches_the_c_mpp_snapshot_byte_for_byte() {
    assert_contexts_match(
        include_bytes!("data/vp9_p0_64x64.ivf"),
        include_bytes!("data/vp9_p0_contexts.bin"),
    );
}

#[test]
fn every_profile_two_context_matches_the_c_mpp_snapshot_byte_for_byte() {
    assert_contexts_match(
        include_bytes!("data/vp9_p2_10bit_64x64.ivf"),
        include_bytes!("data/vp9_p2_contexts.bin"),
    );
}

#[test]
fn hardware_probability_packets_match_c_mpp_picparams() {
    assert_picparams_probabilities_match(
        include_bytes!("data/vp9_p0_64x64.ivf"),
        include_bytes!("data/vp9_p0_picparams.bin"),
    );
    assert_picparams_probabilities_match(
        include_bytes!("data/vp9_p2_10bit_64x64.ivf"),
        include_bytes!("data/vp9_p2_picparams.bin"),
    );
}

fn assert_contexts_match(ivf_bytes: &[u8], expected: &[u8]) {
    const CONTEXT_SIZE: usize = 311 + 4 * 2 * 2 * 6 * 6 * 3;
    let ivf = parse_ivf(ivf_bytes).unwrap();
    assert_eq!(expected.len(), ivf.frames.len() * CONTEXT_SIZE);

    let mut parser = Parser::new();
    for (index, frame) in ivf.frames.iter().enumerate() {
        let payloads = split_superframe(frame.data).unwrap();
        let parsed = parser.parse_frame(payloads[0]).unwrap();
        let actual = parsed
            .compressed_header
            .unwrap()
            .probabilities
            .snapshot_bytes();
        let expected_frame = &expected[index * CONTEXT_SIZE..(index + 1) * CONTEXT_SIZE];
        if let Some(offset) = actual
            .iter()
            .zip(expected_frame)
            .position(|(actual, expected)| actual != expected)
        {
            panic!(
                "frame {index}, context byte {offset}: Rust {}, C MPP {}",
                actual[offset], expected_frame[offset]
            );
        }
        assert_eq!(actual.len(), expected_frame.len(), "frame {index}");
    }
}

fn assert_picparams_probabilities_match(ivf_bytes: &[u8], picparams: &[u8]) {
    const PICPARAMS_SIZE: usize = 19564;
    const PROB_OFFSET: usize = 208;
    const PROB_SIZE: usize = 2039;
    const UPDATE_OFFSET: usize = 15472;
    const UPDATE_SIZE: usize = 4078;

    let ivf = parse_ivf(ivf_bytes).unwrap();
    assert_eq!(picparams.len() % PICPARAMS_SIZE, 0);
    assert!(picparams.len() / PICPARAMS_SIZE >= ivf.frames.len());

    let mut parser = Parser::new();
    for (index, frame) in ivf.frames.iter().enumerate() {
        let payloads = split_superframe(frame.data).unwrap();
        let parsed = parser.parse_frame(payloads[0]).unwrap();
        let compressed = parsed.compressed_header.unwrap();
        let record = &picparams[index * PICPARAMS_SIZE..(index + 1) * PICPARAMS_SIZE];

        let probabilities = compressed.probabilities.hardware_snapshot_bytes();
        assert_bytes_equal(
            &probabilities,
            &record[PROB_OFFSET..PROB_OFFSET + PROB_SIZE],
            &format!("frame {index} hardware probabilities"),
        );

        let flags = compressed.updates.flags.hardware_snapshot_bytes();
        let deltas = compressed.updates.deltas.hardware_snapshot_bytes();
        let mut updates = Vec::with_capacity(UPDATE_SIZE);
        updates.extend_from_slice(&flags[..311]);
        updates.extend_from_slice(&deltas[..311]);
        updates.extend_from_slice(&flags[311..]);
        updates.extend_from_slice(&deltas[311..]);
        assert_bytes_equal(
            &updates,
            &record[UPDATE_OFFSET..UPDATE_OFFSET + UPDATE_SIZE],
            &format!("frame {index} probability updates"),
        );
    }
}

fn assert_bytes_equal(actual: &[u8], expected: &[u8], context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context}");
    if let Some(offset) = actual
        .iter()
        .zip(expected)
        .position(|(actual, expected)| actual != expected)
    {
        panic!(
            "{context}, byte {offset}: Rust {}, C MPP {}",
            actual[offset], expected[offset]
        );
    }
}
