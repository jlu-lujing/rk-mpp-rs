#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"

size="${SIZE:-1280x720}"
width="${size%%x*}"
height="${size##*x}"
frames="${FRAMES:-6}"
input="/tmp/rk_mpp_sdk_decode_scale_${size}.264"
output="/tmp/rk_mpp_sdk_decode_scale_${size}.rgb"
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=${size}:rate=30" -frames:v "$frames" \
    -pix_fmt yuv420p -c:v libx264 -preset veryfast \
    -x264-params "keyint=6:min-keyint=6:scenecut=0:repeat-headers=1:bframes=2" \
    -f h264 "$input"

if [ "${SKIP_BUILD:-0}" != 1 ]; then
    cargo build --release --offline --example sdk_decode_scale
fi
./target/release/examples/sdk_decode_scale "$input" "$output"
test "$(stat -c %s "$output")" -eq $((640 * 640 * 3))

printf 'PASS CPU and DMA-BUF decode -> NV12 scale -> RGB model input (%sx%s, %s frames)\n' \
    "$width" "$height" "$frames"
