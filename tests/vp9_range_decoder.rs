use rk_mpp_rs::codec::vp9::parser::Vp9Error;
use rk_mpp_rs::codec::vp9::range::RangeDecoder;

const REAL_KEYFRAME_FIRST_PARTITION: &[u8] = &[
    0x03, 0xb0, 0x7c, 0xb1, 0x7a, 0xdf, 0x49, 0xe2, 0x6f, 0x75, 0xc2, 0x78, 0x4f, 0x06, 0xde, 0x97,
    0x82, 0xf4, 0xdd, 0x47, 0x47, 0x73, 0xa6, 0x37, 0x03, 0xdf, 0xcd, 0x83, 0xf9, 0xff, 0xb7, 0xef,
    0xc7, 0xeb, 0xfd, 0x67, 0x4e, 0xf5, 0xfe, 0x93, 0xa4, 0xfa, 0x7f, 0x59, 0xe9, 0xfd, 0x27, 0x45,
    0xe9, 0x9e, 0xa7, 0xd3, 0x6e, 0x9f, 0x53, 0x76, 0x3e, 0x02, 0x1a,
];

#[test]
fn rejects_a_partition_shorter_than_its_initial_value() {
    assert!(matches!(
        RangeDecoder::new(&[0; 2]),
        Err(Vp9Error::Truncated { bit: 0, width: 24 })
    ));
}

#[test]
fn decodes_real_keyframe_header_fields() {
    let mut decoder = RangeDecoder::new(REAL_KEYFRAME_FIRST_PARTITION).unwrap();

    assert!(!decoder.read_bool(128).unwrap(), "marker bit");
    assert_eq!(decoder.read_bits(2).unwrap(), 0, "TX_4X4");
    assert!(
        !decoder.read_bool(128).unwrap(),
        "coefficient probability update"
    );
    for context in 0..3 {
        assert!(
            !decoder.read_bool(252).unwrap(),
            "skip probability [{context}]"
        );
    }
}

#[test]
fn decodes_raw_bits_across_buffer_refills() {
    let mut decoder = RangeDecoder::new(REAL_KEYFRAME_FIRST_PARTITION).unwrap();
    let mut output = [0u8; 12];
    for bit in 0..96 {
        output[bit / 8] = (output[bit / 8] << 1) | u8::from(decoder.read_bool(128).unwrap());
    }

    assert_eq!(
        output,
        [0x03, 0xb0, 0x7c, 0xb1, 0x7a, 0xdf, 0x49, 0xe2, 0x6f, 0x75, 0xc2, 0x78,]
    );
}

#[test]
fn applies_the_mpp_probability_delta_code() {
    let mut decoder = RangeDecoder::new(REAL_KEYFRAME_FIRST_PARTITION).unwrap();
    assert!(!decoder.read_bool(128).unwrap());
    assert_eq!(decoder.read_bits(2).unwrap(), 0);
    assert!(!decoder.read_bool(128).unwrap());
    for _ in 0..3 {
        assert!(!decoder.read_bool(252).unwrap());
    }

    let old_probabilities = [100, 120, 50, 200, 128, 1, 255, 77];
    let expected = [
        (7, 149),
        (31, 113),
        (14, 190),
        (247, 8),
        (12, 46),
        (8, 112),
        (4, 196),
        (0, 73),
    ];
    for (probability, (delta, updated)) in old_probabilities.into_iter().zip(expected) {
        let update = decoder.diff_update_prob(probability).unwrap();
        assert_eq!((update.delta, update.probability), (delta, updated));
    }
}
