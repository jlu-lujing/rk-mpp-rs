use std::path::PathBuf;
use std::process::Command;

const BUS: &str = "/home/user/projects/rknn-projects-test/baseline/images/bus.jpg";

fn run_binary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rk-mpp-rs"))
        .args(args)
        .output()
        .expect("run rk-mpp-rs")
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |mut hash, byte| {
        hash ^= u64::from(*byte);
        hash.wrapping_mul(0x100000001b3)
    })
}

/// The async task queue + refcounted buffer pool must produce exactly what
/// the synchronous submit+poll path produces: job 0 of the async run lands
/// on disk and is compared byte for byte against `decode`. The demo's own
/// PASS line covers the remaining jobs against its sync oracle.
#[test]
#[ignore = "requires an RK3588 jpegd, dma-heap and /dev/mpp_service access"]
fn async_queue_decodes_byte_identical_to_sync_path() {
    let pid = std::process::id();
    let sync_path = PathBuf::from(format!("/tmp/rk-mpp-rs-framework-sync-{pid}.rgb"));
    let async_path = PathBuf::from(format!("/tmp/rk-mpp-rs-framework-async-{pid}.rgb"));

    let sync = run_binary(&["decode", BUS, sync_path.to_str().unwrap(), "1"]);
    assert!(sync.status.success(), "sync decode failed");

    let demo = run_binary(&["async-demo", BUS, "6", "3", async_path.to_str().unwrap()]);
    let log = String::from_utf8_lossy(&demo.stdout);
    assert!(
        demo.status.success() && log.contains("async-demo: PASS"),
        "async demo failed: {log}"
    );
    assert!(
        log.contains("callbacks=6"),
        "one completion callback per job: {log}"
    );

    let sync_bytes = std::fs::read(&sync_path).unwrap();
    let async_bytes = std::fs::read(&async_path).unwrap();
    std::fs::remove_file(&sync_path).ok();
    std::fs::remove_file(&async_path).ok();
    assert_eq!(async_bytes.len(), 640 * 640 * 3);
    assert_eq!(
        fnv1a(&async_bytes),
        fnv1a(&sync_bytes),
        "async job 0 differs from the sync decode"
    );
}
