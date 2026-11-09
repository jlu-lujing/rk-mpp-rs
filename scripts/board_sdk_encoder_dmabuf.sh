#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --offline --lib

library="$repo/target/release/librk_mpp_rs.so"
libdir=$(mktemp -d)
raw_dir=$(mktemp -d)
trap 'rm -rf "$libdir" "$raw_dir"' EXIT
ln -sfn "$library" "$libdir/librk_mpp_rs.so"
ln -sfn "$library" "$libdir/librk_mpp_rs.so.1"

gcc -std=c11 -Wall -Wextra -Werror \
    -I"$repo/include" "$repo/tests/c_api_encoder.c" \
    -L"$libdir" -lrk_mpp_rs \
    -Wl,-rpath,"$libdir" \
    -o /tmp/rk_mpp_encoder_dmabuf

ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 6 \
    -pix_fmt nv12 -f rawvideo "$raw_dir/frames.nv12"

for codec in h264 h265; do
    copied="/tmp/rk_mpp_sdk_encoder_${codec}_copied.bin"
    dmabuf="/tmp/rk_mpp_sdk_encoder_${codec}_dmabuf.bin"
    LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
        /tmp/rk_mpp_encoder_dmabuf "$codec" nv12 "$copied" \
            320 240 26 6 6 "$raw_dir/frames.nv12"
    DMABUF_INPUT=1 LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
        /tmp/rk_mpp_encoder_dmabuf "$codec" nv12 "$dmabuf" \
            320 240 26 6 6 "$raw_dir/frames.nv12"
    cmp "$copied" "$dmabuf"
done

jpeg_copied=/tmp/rk_mpp_sdk_encoder_jpeg_copied.jpg
jpeg_dmabuf=/tmp/rk_mpp_sdk_encoder_jpeg_dmabuf.jpg
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_encoder_dmabuf jpeg nv12 "$jpeg_copied" \
        320 240 10 1 1 "$raw_dir/frames.nv12"
DMABUF_INPUT=1 LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_encoder_dmabuf jpeg nv12 "$jpeg_dmabuf" \
        320 240 10 1 1 "$raw_dir/frames.nv12"
cmp "$jpeg_copied" "$jpeg_dmabuf"

printf 'PASS copied and dma-buf encoder input match for H.264, H.265 and JPEG\n'
