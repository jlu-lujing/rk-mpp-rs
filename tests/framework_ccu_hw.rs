use rk_mpp_rs::hal::jpege::{EncoderConfig, Vepu2Jpeg};
use rk_mpp_rs::service::dma_heap::DmaBuf;
use rk_mpp_rs::service::soc::CcuTopology;

fn nv12_input(width: usize, height: usize, shift: u8) -> Vec<u8> {
    let mut frame = vec![0u8; width * height * 3 / 2];
    for y in 0..height {
        for x in 0..width {
            frame[y * width + x] = (x as u8).wrapping_mul(3) ^ (y as u8).wrapping_mul(5) ^ shift;
        }
    }
    frame[width * height..].fill(128u8.wrapping_add(shift));
    frame
}

fn input_buffer(config: EncoderConfig, bytes: &[u8]) -> DmaBuf {
    let size = config.input_size().unwrap() + config.hor_stride * 64;
    let mut buffer = DmaBuf::alloc("system", size, false).unwrap();
    buffer.sync_write_begin().unwrap();
    buffer.as_mut_slice()[..bytes.len()].copy_from_slice(bytes);
    buffer.sync_write_end().unwrap();
    buffer
}

#[test]
#[ignore = "requires RK3588 VEPU2 and /dev/mpp_service"]
fn two_jpeg_sessions_submit_through_one_ccu_batch() {
    assert!(
        CcuTopology::detect().jpeg_encode,
        "t6 device tree must advertise the JPEG CCU"
    );
    let config = EncoderConfig::new(640, 480);
    let left_frame = nv12_input(config.width, config.height, 17);
    let right_frame = nv12_input(config.width, config.height, 193);
    let left_input = input_buffer(config, &left_frame);
    let right_input = input_buffer(config, &right_frame);

    let expected_left = Vepu2Jpeg::open()
        .unwrap()
        .encode_fd(left_input.fd(), config)
        .unwrap();
    let expected_right = Vepu2Jpeg::open()
        .unwrap()
        .encode_fd(right_input.fd(), config)
        .unwrap();

    let mut left = Vepu2Jpeg::open().unwrap();
    let mut right = Vepu2Jpeg::open().unwrap();
    let actual = Vepu2Jpeg::encode_fd_pair(
        &mut left,
        left_input.fd(),
        config,
        &mut right,
        right_input.fd(),
        config,
    )
    .unwrap();

    assert!(actual.batched, "t6 must expose SET_SESSION_FD");
    assert_eq!(actual.left.bytes, expected_left);
    assert_eq!(actual.right.bytes, expected_right);
    assert_eq!(actual.left.packets, vec![0..actual.left.bytes.len()]);
    assert_eq!(actual.right.packets, vec![0..actual.right.bytes.len()]);
}
