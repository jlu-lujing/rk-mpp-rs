#!/usr/bin/env bash

set -euo pipefail

board="${BOARD:?set BOARD to a Rockchip SSH alias}"

ssh "$board" 'mkdir -p "$HOME/rk-mpp-rs"'
COPYFILE_DISABLE=1 tar \
    --no-xattrs \
    --exclude .git \
    --exclude target \
    --exclude .worktrees \
    --exclude refs \
    -cf - . |
    ssh "$board" 'tar -xmf - -C "$HOME/rk-mpp-rs"'

ssh "$board" 'RUST_MIN_STACK=16777216 CARGO_BUILD_JOBS=1 bash -s' <<'REMOTE'
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
cd "$HOME/rk-mpp-rs"
cargo build --release --offline
PROFILE=release ./scripts/check_sdk_abi.sh

ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 2 \
    -pix_fmt yuv420p -c:v libx264 -preset veryfast \
    -x264-params "keyint=1:repeat-headers=1" -f h264 /tmp/rk_mpp_sdk_example.264
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 2 \
    -pix_fmt nv12 -f rawvideo /tmp/rk_mpp_sdk_example.nv12
cargo run --release --offline --example sdk_decode -- \
    h264 /tmp/rk_mpp_sdk_example.264 /tmp/rk_mpp_sdk_decode.yuv
cargo run --release --offline --example sdk_encode -- \
    h264 /tmp/rk_mpp_sdk_example.nv12 /tmp/rk_mpp_sdk_encode.264 320 240 26 6
test -s /tmp/rk_mpp_sdk_decode.yuv
test -s /tmp/rk_mpp_sdk_encode.264

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
./scripts/install_sdk.sh \
    --prefix /usr/local \
    --destdir "$stage" \
    --profile release \
    --no-build

installed="$stage/usr/local"
api_minor=$(awk '
    /#define RK_MPP_API_VERSION_MINOR/ { gsub(/u/, "", $3); print $3 }
' "$installed/include/rk_mpp_rs.h")
test -L "$installed/lib/librk_mpp_rs.so"
test -L "$installed/lib/librk_mpp_rs.so.1"
test -f "$installed/lib/librk_mpp_rs.so.1.$api_minor.0"
test -f "$installed/lib/librk_mpp_rs.a"
test -f "$installed/include/rk_mpp_rs.h"
test -f "$installed/lib/pkgconfig/rk-mpp-rs.pc"
test -f "$installed/lib/cmake/rk-mpp-rs/rk-mpp-rsConfig.cmake"

g++ -std=c++17 -Wall -Wextra -Werror \
    $(PKG_CONFIG_PATH="$installed/lib/pkgconfig" pkg-config --cflags rk-mpp-rs) \
    tests/c_api_decoder_smoke.cpp \
    $(PKG_CONFIG_PATH="$installed/lib/pkgconfig" pkg-config --libs rk-mpp-rs) \
    -Wl,-rpath,"$installed/lib" \
    -o /tmp/rk_mpp_pkgconfig_consumer
/tmp/rk_mpp_pkgconfig_consumer

cmake_dir=/tmp/rk_mpp_cmake_consumer_build
rm -rf "$cmake_dir"
cmake -S tests/cmake_consumer -B "$cmake_dir" \
    -DCMAKE_PREFIX_PATH="$installed" \
    -DCMAKE_BUILD_TYPE=Release
cmake --build "$cmake_dir" --parallel
LD_LIBRARY_PATH="$installed/lib:${LD_LIBRARY_PATH:-}" \
    "$cmake_dir/rk_mpp_cmake_consumer"
"$cmake_dir/rk_mpp_cmake_consumer_static"

printf 'PASS rk-mpp-rs install, pkg-config, CMake and ABI package\n'
REMOTE
