#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"

input=/tmp/rk_mpp_sdk_decode_scale.264
output=/tmp/rk_mpp_sdk_decode_scale.rgb
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=1280x720:rate=30" -frames:v 6 \
    -pix_fmt yuv420p -c:v libx264 -preset veryfast \
    -x264-params "keyint=6:min-keyint=6:scenecut=0:repeat-headers=1:bframes=2" \
    -f h264 "$input"

cargo run --release --offline --example sdk_decode_scale -- "$input" "$output"
test "$(stat -c %s "$output")" -eq $((640 * 640 * 3))

printf 'PASS CPU and DMA-BUF decode -> NV12 scale -> RGB model input\n'
