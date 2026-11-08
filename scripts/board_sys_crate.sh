#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --offline -p rk-mpp-rs --lib

native=$(mktemp -d)
stage=$(mktemp -d)
static_target=$(mktemp -d)
pkg_target=$(mktemp -d)
trap 'rm -rf "$native" "$stage" "$static_target" "$pkg_target"' EXIT

ln -sfn "$repo/target/release/librk_mpp_rs.so" "$native/librk_mpp_rs.so"
ln -sfn "$repo/target/release/librk_mpp_rs.so" "$native/librk_mpp_rs.so.1"
ln -sfn "$repo/target/release/librk_mpp_rs.a" "$native/librk_mpp_rs.a"

RK_MPP_RS_SYS_LIB_DIR="$native" \
LD_LIBRARY_PATH="$native:${LD_LIBRARY_PATH:-}" \
    cargo test --release --offline -p rk-mpp-rs-sys

RK_MPP_RS_SYS_LIB_DIR="$native" \
LD_LIBRARY_PATH="$native:${LD_LIBRARY_PATH:-}" \
    cargo run --release --offline -p rk-mpp-rs-sys \
        --example api_version

CARGO_TARGET_DIR="$static_target" \
CARGO_PROFILE_RELEASE_LTO=false \
RK_MPP_RS_SYS_LIB_DIR="$native" \
RK_MPP_RS_SYS_STATIC=1 \
    cargo test --release --offline -p rk-mpp-rs-sys \
        --test native_smoke

./scripts/install_sdk.sh \
    --prefix /usr/local \
    --destdir "$stage" \
    --profile release \
    --no-build

prefix="$stage/usr/local"
CARGO_TARGET_DIR="$pkg_target" \
RK_MPP_RS_SYS_USE_PKG_CONFIG=1 \
PKG_CONFIG_PATH="$prefix/lib/pkgconfig" \
LD_LIBRARY_PATH="$prefix/lib:${LD_LIBRARY_PATH:-}" \
    cargo test --release --offline -p rk-mpp-rs-sys \
        --test native_smoke

printf 'PASS rk-mpp-rs-sys dynamic, static and pkg-config consumers\n'
