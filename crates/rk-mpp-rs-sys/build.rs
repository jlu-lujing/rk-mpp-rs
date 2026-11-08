use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const REQUESTED_VERSION: &str = "1.10";

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rerun-if-env-changed=RK_MPP_RS_SYS_LIB_DIR");
    println!("cargo:rerun-if-env-changed=RK_MPP_RS_SYS_STATIC");
    println!("cargo:rerun-if-env-changed=RK_MPP_RS_SYS_USE_PKG_CONFIG");
    let header = manifest.join("../../include/rk_mpp_rs.h");
    if header.is_file() {
        println!("cargo:rerun-if-changed={}", header.display());
    }

    let link_kind = if env::var_os("RK_MPP_RS_SYS_STATIC").is_some() {
        "static"
    } else {
        "dylib"
    };

    if let Some(dir) = env::var_os("RK_MPP_RS_SYS_LIB_DIR").map(PathBuf::from) {
        emit_search(&dir);
        emit_link(link_kind);
        return;
    }

    if env::var_os("RK_MPP_RS_SYS_USE_PKG_CONFIG").is_some() {
        if let Some(dir) = pkg_config_library_dir() {
            emit_search(&dir);
            emit_link(link_kind);
            return;
        }
        panic!(
            "RK_MPP_RS_SYS_USE_PKG_CONFIG was set, but pkg-config could not find \
             rk-mpp-rs {REQUESTED_VERSION} or newer"
        );
    }

    let workspace = manifest
        .parent()
        .and_then(Path::parent)
        .expect("crate must live under <workspace>/crates/rk-mpp-rs-sys");
    let profile = env::var("PROFILE").unwrap();
    let target_dir = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                workspace.join(path)
            }
        })
        .unwrap_or_else(|| workspace.join("target"));
    let cross_profile = env::var("TARGET")
        .ok()
        .map(|target| target_dir.join(target).join(&profile));
    let host_profile = target_dir.join(&profile);

    let selected = [cross_profile, Some(host_profile)]
        .into_iter()
        .flatten()
        .find(|dir| native_library_exists(dir));
    let selected = selected.unwrap_or_else(|| target_dir.join(profile));
    emit_search(&selected);
    emit_link(link_kind);
}

fn emit_search(dir: &Path) {
    println!("cargo:rustc-link-search=native={}", dir.display());
}

fn emit_link(kind: &str) {
    println!("cargo:rustc-link-lib={kind}=rk_mpp_rs");
}

fn native_library_exists(dir: &Path) -> bool {
    dir.join("librk_mpp_rs.so").is_file() || dir.join("librk_mpp_rs.a").is_file()
}

fn pkg_config_library_dir() -> Option<PathBuf> {
    let output = Command::new("pkg-config")
        .args(["--modversion", "rk-mpp-rs"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?;
    if !version_at_least(version.trim(), REQUESTED_VERSION) {
        return None;
    }

    let output = Command::new("pkg-config")
        .args(["--libs-only-L", "rk-mpp-rs"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .split_whitespace()
        .find_map(|token| token.strip_prefix("-L"))
        .map(PathBuf::from)
}

fn version_at_least(actual: &str, required: &str) -> bool {
    fn parts(version: &str) -> Vec<u32> {
        version
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    }

    let mut actual = parts(actual);
    let mut required = parts(required);
    let len = actual.len().max(required.len());
    actual.resize(len, 0);
    required.resize(len, 0);
    actual >= required
}
