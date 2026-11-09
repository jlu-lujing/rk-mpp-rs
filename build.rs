use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/mpp_compat/standalone_log.c");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
        let object = out.join("standalone_log.o");
        let archive = out.join("librk_mpp_standalone_log.a");
        let compiler = env::var_os("CC").unwrap_or_else(|| "cc".into());
        let archiver = env::var_os("AR").unwrap_or_else(|| "ar".into());
        let status = Command::new(compiler)
            .args([
                "-std=c11",
                "-O2",
                "-fPIC",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-c",
                "src/mpp_compat/standalone_log.c",
                "-o",
            ])
            .arg(&object)
            .status()
            .expect("compile standalone MPP log shim");
        assert!(
            status.success(),
            "standalone MPP log shim compilation failed"
        );
        let status = Command::new(archiver)
            .arg("crs")
            .arg(&archive)
            .arg(&object)
            .status()
            .expect("archive standalone MPP log shim");
        assert!(status.success(), "standalone MPP log shim archiving failed");
        println!("cargo:rustc-link-search=native={}", out.display());
        println!("cargo:rustc-link-lib=static=rk_mpp_standalone_log");
        println!("cargo:rustc-cdylib-link-arg=-Wl,-soname,librk_mpp_rs.so.1");
    }
}
