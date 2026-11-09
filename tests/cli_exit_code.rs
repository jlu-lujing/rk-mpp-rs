use std::process::{Command, Output};

fn run_invalid_decode(command: &str) -> Output {
    let input = std::env::temp_dir().join(format!(
        "rk-mpp-rs-invalid-{command}-{}.bitstream",
        std::process::id()
    ));
    let output = std::env::temp_dir().join(format!(
        "rk-mpp-rs-invalid-{command}-{}.yuv",
        std::process::id()
    ));
    std::fs::write(
        &input,
        [
            0x00, 0x00, 0x01, 0x42, 0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        ],
    )
    .unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_rk-mpp-rs"))
        .arg(command)
        .arg(&input)
        .arg(&output)
        .output()
        .unwrap();

    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&output);
    result
}

fn assert_failed(result: Output) {
    assert!(
        !result.status.success(),
        "decode failure was reported as success: stdout={} stderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn hevc_decode_failure_returns_nonzero() {
    assert_failed(run_invalid_decode("hevcdec"));
}

#[test]
fn h264_decode_failure_returns_nonzero() {
    assert_failed(run_invalid_decode("h264dec"));
}

#[test]
fn h264_dmabuf_output_requires_dev_null() {
    let output = std::env::temp_dir().join(format!("rk-mpp-rs-dmabuf-{}.yuv", std::process::id()));
    let result = Command::new(env!("CARGO_BIN_EXE_rk-mpp-rs"))
        .arg("h264dec")
        .arg("unused.264")
        .arg(&output)
        .env("RKMPP_DECODE_DMABUF", "1")
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&output);

    assert_eq!(result.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("requires an output of /dev/null"),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
}
