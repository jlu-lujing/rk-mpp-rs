use rk_mpp_rs::codec::h264::decoder::{split_access_units, Decoder as H264Decoder};
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
