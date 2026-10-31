use rk_mpp_rs::codec::vp9::decoder::Decoder;
use rk_mpp_rs::codec::vp9::parser::parse_ivf;

fn decode_ivf(ivf_bytes: &[u8]) -> (usize, Vec<u8>) {
    let ivf = parse_ivf(ivf_bytes).unwrap();
    let mut decoder = Decoder::new().unwrap();
    let mut output = Vec::new();
    let mut shown = 0;

    for frame in &ivf.frames {
        for picture in decoder.decode_packet(frame.data).unwrap() {
            assert_eq!(picture.err_info, 0);
            output.extend_from_slice(&picture.yuv);
            shown += 1;
        }
    }
    (shown, output)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |mut hash, byte| {
        hash ^= u64::from(*byte);
        hash.wrapping_mul(0x100000001b3)
    })
}

#[test]
#[ignore = "requires an RK3588 rkvdec and /dev/mpp_service access"]
fn show_existing_frame_repeats_the_cached_picture() {
    let ivf = parse_ivf(include_bytes!("data/vp9_p0_64x64.ivf")).unwrap();
    let mut decoder = Decoder::new().unwrap();

    let first = decoder.decode_packet(ivf.frames[0].data).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!((first[0].width, first[0].height), (64, 64));
    assert_eq!(first[0].err_info, 0);

    // show_existing_frame=1, profile=0, frame_to_show_map_idx=0
    let repeated = decoder.decode_packet(&[0x88, 0x00]).unwrap();
    assert_eq!(repeated.len(), 1);
    assert_eq!((repeated[0].width, repeated[0].height), (64, 64));
    assert_eq!(repeated[0].poc, first[0].poc);
    assert_eq!(repeated[0].err_info, 0);
    assert_eq!(repeated[0].yuv, first[0].yuv);
}

#[test]
#[ignore = "requires an RK3588 rkvdec and /dev/mpp_service access"]
fn error_resilient_segmentation_resets_past_segment_maps() {
    let (shown, output) = decode_ivf(include_bytes!("data/vp9_p0_720p_errres_seg.ivf"));
    assert_eq!(shown, 6);
    assert_eq!(output.len(), 6 * 1280 * 720 * 3 / 2);
    assert_eq!(fnv1a(&output), 0x56e987488cb08627);
}

#[test]
#[ignore = "requires an RK3588 rkvdec and /dev/mpp_service access"]
fn lossless_and_tiled_streams_match_software_output() {
    let (shown, profile0_lossless) =
        decode_ivf(include_bytes!("data/vp9_p0_lossless_320x240_12.ivf"));
    assert_eq!(shown, 12);
    assert_eq!(profile0_lossless.len(), 12 * 320 * 240 * 3 / 2);
    assert_eq!(fnv1a(&profile0_lossless), 0xfa56535a6614bff3);

    let (shown, profile2_lossless) =
        decode_ivf(include_bytes!("data/vp9_p2_10bit_lossless_320x240_12.ivf"));
    assert_eq!(shown, 12);
    assert_eq!(profile2_lossless.len(), 12 * 320 * 240 * 3);
    assert_eq!(fnv1a(&profile2_lossless), 0xfab4bf9ecd8e2d4a);

    let (shown, error_resilient_tiles) =
        decode_ivf(include_bytes!("data/vp9_p0_errres_tiles_640x360_12.ivf"));
    assert_eq!(shown, 12);
    assert_eq!(error_resilient_tiles.len(), 12 * 640 * 360 * 3 / 2);
    assert_eq!(fnv1a(&error_resilient_tiles), 0x0c35574d8f686bf0);

    let (shown, odd_lossless) = decode_ivf(include_bytes!("data/vp9_p0_lossless_321x241_2.ivf"));
    assert_eq!(shown, 2);
    assert_eq!(odd_lossless.len(), 2 * (321 * 241 + 2 * 161 * 121));
    assert_eq!(fnv1a(&odd_lossless), 0xf99534b276b1f2d9);
}
