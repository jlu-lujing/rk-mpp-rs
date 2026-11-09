#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"

cargo build --release --offline -p rk-mpp-rs
cargo build --offline --lib
ln -sfn librk_mpp_rs.so target/debug/librk_mpp_rs.so.1

native=$(mktemp -d)
trap 'rm -rf "$native"' EXIT
ln -s "$repo/target/release/librk_mpp_rs.so" "$native/librk_mpp_rs.so"
ln -s "$repo/target/release/librk_mpp_rs.so" "$native/librk_mpp_rs.so.1"

if ! RK_MPP_RS_SYS_LIB_DIR="$native" \
    LD_LIBRARY_PATH="$native:${LD_LIBRARY_PATH:-}" \
        cargo test --workspace --all-targets --offline \
        > /tmp/rk-mpp-workspace-tests.log 2>&1
then
    tail -200 /tmp/rk-mpp-workspace-tests.log
    exit 1
fi

awk '/test result: ok\./ {p+=$4;f+=$6;i+=$8;n++}
     END {printf "SUMMARY suites=%d passed=%d failed=%d ignored=%d\n",n,p,f,i}' \
    /tmp/rk-mpp-workspace-tests.log
cargo fmt --check
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --offline
printf 'PASS workspace gate\n'
