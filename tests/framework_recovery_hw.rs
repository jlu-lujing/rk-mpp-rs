use rk_mpp_rs::codec::h264::decoder::{split_access_units, Decoder as H264Decoder};
use rk_mpp_rs::codec::h265::encoder::SyntaxOptions;
use rk_mpp_rs::hal::h265e_session::Vepu580H265;
use rk_mpp_rs::service::{Client, MppService};

#[test]
#[ignore = "requires RK3588 /dev/mpp_service access"]
fn session_replacement_is_idempotent_without_reset() {
    let mut service = MppService::open(Client::RkvDec as u32).unwrap();
    for expected in 1..=16 {
        service.restart_after_hardware_error().unwrap();
        assert_eq!(
            service.recovery_stats().session_resets,
            0,
            "normal recovery must not poison the driver with RESET_SESSION"
        );
        assert_eq!(service.recovery_stats().session_reopens, expected);
        assert!(service.hw_id().unwrap() > 0);
    }
    assert_eq!(service.recovery_stats().reset_failures, 0);
}

fn decode_h264(data: &[u8], reset_first: bool) -> Vec<Vec<u8>> {
    let mut decoder = H264Decoder::new().unwrap();
    if reset_first {
        decoder.recover_session().unwrap();
    }
    let mut frames = Vec::new();
    for access_unit in split_access_units(data) {
        if let Some(picture) = decoder.decode_access_unit(access_unit).unwrap() {
            assert_eq!(picture.err_info, 0);
            frames.push(if let Some(nv12) = picture.nv12 {
                nv12
            } else {
                picture.yuv
            });
        }
    }
    frames
}

#[test]
#[ignore = "requires RK3588 /dev/mpp_service access"]
fn session_replacement_before_the_next_job_keeps_decode_output_identical() {
    let data = include_bytes!("data/h264_i32_cavlc.264");
    let baseline = decode_h264(data, false);
    let recovered = decode_h264(data, true);
    assert!(!baseline.is_empty());
    assert_eq!(
        recovered, baseline,
        "session replacement keeps the next decode job byte-identical"
    );
}

fn tiled_h265_session() -> Vepu580H265 {
    let mut encoder = Vepu580H265::open_gop_short_term(320, 240, 26).unwrap();
    encoder.set_syntax_options(SyntaxOptions {
        auto_tile: true,
        ..SyntaxOptions::default()
    });
    encoder.set_gop(60).unwrap();
    encoder
}

fn moving_nv12(frame: usize) -> Vec<u8> {
    const WIDTH: usize = 320;
    const HEIGHT: usize = 240;
    let mut nv12 = vec![0u8; WIDTH * HEIGHT * 3 / 2];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            nv12[y * WIDTH + x] = ((x + y + frame * 7) & 0xff) as u8;
        }
    }
    for y in 0..HEIGHT / 2 {
        for x in 0..WIDTH / 2 {
            let at = WIDTH * HEIGHT + y * WIDTH + x * 2;
            nv12[at] = 96u8.wrapping_add((frame * 5) as u8);
            nv12[at + 1] = 160u8.wrapping_sub((frame * 3) as u8);
        }
    }
    nv12
}

#[test]
#[ignore = "requires RK3588 /dev/mpp_service access"]
fn h265_tile_restarts_with_idr_after_an_error_reopens_the_service() {
    let frames = [moving_nv12(0), moving_nv12(1), moving_nv12(2)];

    // A fresh stream defines the expected recovery IDR.
    let mut baseline = tiled_h265_session();
    let expected = baseline.encode_frame(&frames[2]).unwrap();

    let mut recovered = tiled_h265_session();
    recovered.encode_frame(&frames[0]).unwrap();
    recovered.encode_frame(&frames[1]).unwrap();
    assert_eq!(recovered.recovery_stats().session_reopens, 0);

    // A positive but unmapped fd reaches the real submit path and makes the
    // kernel reject the batch. submit_batch_recovering then closes and reopens
    // the service fd, leaving this encoder object and its tile buffers alive.
    let error = recovered.encode_frame_fd(0x3fff_ffff).unwrap_err();
    assert!(
        error.contains("hardware error mask") || error.contains("completed ioctl without enc_done"),
        "unexpected fd failure: {error}"
    );
    let stats = recovered.recovery_stats();
    assert_eq!(
        stats.session_resets, 0,
        "recovery must not issue RESET_SESSION"
    );
    assert!(stats.session_reopens >= 1, "{stats:?}");

    // The next picture starts a clean IDR epoch on the replacement fd. It must
    // carry the parameter sets, execute the discarded tiled prepass, and match
    // a newly opened encoder.
    let actual = recovered.encode_frame(&frames[2]).unwrap();
    assert_eq!(
        actual, expected,
        "recovery did not restart a clean tiled IDR epoch"
    );
}
