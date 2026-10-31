use rk_mpp_rs::codec::av1::decoder::Decoder;
use rk_mpp_rs::codec::av1::parser::parse_ivf;

fn decode_ivf(ivf_bytes: &[u8]) -> (usize, u32, Vec<u8>) {
    let ivf = parse_ivf(ivf_bytes).unwrap();
    let mut decoder = Decoder::new().unwrap();
    let mut output = Vec::new();
    let mut shown = 0;
    let mut bit_depth = 0;

    for frame in &ivf.frames {
        for picture in decoder.decode_packet(frame.data).unwrap() {
            assert_eq!(picture.err_info, 0);
            if shown > 0 {
                assert_eq!(picture.bit_depth, bit_depth);
            }
            bit_depth = picture.bit_depth;
            output.extend_from_slice(&picture.yuv);
            shown += 1;
        }
    }
    (shown, bit_depth, output)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |mut hash, byte| {
        hash ^= u64::from(*byte);
        hash.wrapping_mul(0x100000001b3)
    })
}

#[test]
#[ignore = "requires an RK3588 rkvdec and /dev/mpp_service access"]
fn film_grain_8_and_10_bit_outputs_match_libaom() {
    let (shown, bit_depth, grain8) =
        decode_ivf(include_bytes!("data/av1_film_grain_320x240_10.ivf"));
    assert_eq!(shown, 10);
    assert_eq!(bit_depth, 8);
    assert_eq!(grain8.len(), 10 * 320 * 240 * 3 / 2);
    assert_eq!(fnv1a(&grain8), 0x6f7f7f6a9fda3ee9);

    let (shown, bit_depth, grain10) = decode_ivf(include_bytes!("data/film_grain_10bit.ivf"));
    assert_eq!(shown, 10);
    assert_eq!(bit_depth, 10);
    assert_eq!(grain10.len(), 10 * 320 * 240 * 3);
    assert_eq!(fnv1a(&grain10), 0x8d9f4d8ee7c2cd40);
}
