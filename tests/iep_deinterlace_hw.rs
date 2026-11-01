use std::process::Command;

fn run_binary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rk-mpp-rs"))
        .args(args)
        .output()
        .expect("run rk-mpp-rs")
}

/// The capability survey records client 28 as binding but has never driven a
/// job through it. This is the first on-board assertion: opening the session,
/// announcing `IEP_CLIENT_TYPE`, and the two commands the deinterlace path
/// needs must all answer. Run with `cargo test -- --ignored` on RK3588.
#[test]
#[ignore = "requires an RK3588 IEP2 driver and /dev/mpp_service access"]
fn iep_client_28_binds_and_supports_the_deinterlace_commands() {
    let out = run_binary(&["iep-probe"]);
    let log = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "iep-probe failed: {log}");
    assert!(
        log.contains("session init (INIT_CLIENT_TYPE):         binds"),
        "client 28 did not bind:\n{log}"
    );
    assert!(
        log.contains("POLL_HW_FINISH:                          yes"),
        "POLL_HW_FINISH missing:\n{log}"
    );
}

/// Drive one I5O2 deinterlace end to end: a synthetic three-frame YUV420-SP
/// input must yield two reconstructed frames of exactly the input frame size.
/// Byte-exactness against the C `iep2_test` is the shell script's job; this
/// test only proves the ioctl sequence completes and the hardware writes out.
#[test]
#[ignore = "requires an RK3588 IEP2 driver and /dev/dma_heap access"]
fn iep_deinterlace_writes_two_frames_of_input_size() {
    let (w, h) = (320u32, 240u32);
    let frame = (w as usize) * (h as usize) * 3 / 2;
    let pid = std::process::id();
    let in_path = format!("/tmp/rk-mpp-rs-iep-in-{pid}.yuv");
    let out_path = format!("/tmp/rk-mpp-rs-iep-out-{pid}.yuv");

    // prev, current, next: three flat frames differing only in luma level.
    let mut data = Vec::with_capacity(frame * 3);
    for n in 0..3u8 {
        let level = 40u8 + n * 30;
        data.extend_from_slice(&vec![level; frame]);
    }
    std::fs::write(&in_path, &data).unwrap();

    let run = run_binary(&[
        "iep-dei",
        &in_path,
        &out_path,
        &w.to_string(),
        &h.to_string(),
        "TFF",
    ]);
    let log = String::from_utf8_lossy(&run.stdout);
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "iep-dei failed: stdout={log} stderr={err}"
    );

    let out_bytes = std::fs::read(&out_path).unwrap();
    std::fs::remove_file(&in_path).ok();
    std::fs::remove_file(&out_path).ok();
    assert_eq!(
        out_bytes.len(),
        frame * 2,
        "two deinterlaced frames expected"
    );
}
