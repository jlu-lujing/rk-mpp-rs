use std::path::PathBuf;
use std::process::Command;

const BUS: &str = "/home/user/projects/rknn-projects-test/baseline/images/bus.jpg";

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |mut hash, byte| {
        hash ^= u64::from(*byte);
        hash.wrapping_mul(0x100000001b3)
    })
}

fn decode(factor: usize, nv12: bool) -> Vec<u8> {
    let path = PathBuf::from(format!(
        "/tmp/rk-mpp-rs-jpeg-{}-{factor}-{nv12}.raw",
        std::process::id()
    ));
    let mut command = Command::new(env!("CARGO_BIN_EXE_rk-mpp-rs"));
    command
        .args(["decode", BUS])
        .arg(&path)
        .arg(factor.to_string())
        .env_remove("RKMPP_SCALE")
        .env_remove("RKMPP_NV12");
    if nv12 {
        command.env("RKMPP_NV12", "1");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "decoder failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    bytes
}

#[test]
#[ignore = "requires an RK3588 jpegd and /dev/mpp_service access"]
fn downscale_geometry_and_dma_visible_pixels_are_stable() {
    let full = decode(1, false);
    let half = decode(2, false);
    let quarter = decode(4, false);
    let eighth = decode(8, false);
    let half_nv12 = decode(2, true);

    assert_eq!(full.len(), 640 * 640 * 3);
    assert_eq!(half.len(), 320 * 320 * 3);
    assert_eq!(quarter.len(), 160 * 160 * 3);
    assert_eq!(eighth.len(), 80 * 80 * 3);
    assert_eq!(half_nv12.len(), 320 * 320 * 3 / 2);

    // The full frame also catches a missing DMA_BUF_SYNC_READ invalidation:
    // hardware can report success while stale cache lines produce horizontal
    // bands. The scaled hashes exercise 1/2, 1/4, 1/8 and NV12 layout.
    assert_eq!(fnv1a(&full), 0x4a30124fe4759188);
    assert_eq!(fnv1a(&half), 0x60953649429bc92d);
    assert_eq!(fnv1a(&quarter), 0xc51b94fb7c3a76af);
    assert_eq!(fnv1a(&eighth), 0xf06b56cddf7c8b06);
    assert_eq!(fnv1a(&half_nv12), 0x903239c211f26599);
}
