#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --offline --lib
cargo test --release --offline --test dma_heap_import_hw -- --ignored

library="$repo/target/release/librk_mpp_rs.so"
libdir=$(mktemp -d)
jpeg_dir=$(mktemp -d)
trap 'rm -rf "$libdir" "$jpeg_dir"' EXIT
ln -sfn "$library" "$libdir/librk_mpp_rs.so"
ln -sfn "$library" "$libdir/librk_mpp_rs.so.1"

gcc -std=c11 -Wall -Wextra -Werror \
    -I"$repo/include" "$repo/tests/c_api_decoder.c" \
    -L"$libdir" -lrk_mpp_rs \
    -Wl,-rpath,"$libdir" \
    -o /tmp/rk_mpp_decoder_dmabuf

ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 4 \
    -pix_fmt yuvj420p -strict unofficial -c:v mjpeg -q:v 3 \
    "$jpeg_dir/frame_%02d.jpg"

REQUIRE_DMABUF=1 \
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_decoder_dmabuf mjpeg nv12 97 \
        /tmp/rk_mpp_sdk_dmabuf.nv12 "$jpeg_dir"/*.jpg

test -s /tmp/rk_mpp_sdk_dmabuf.nv12
EXPECT_NO_DMABUF=1 \
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_decoder_dmabuf mjpeg nv12 211 \
        /tmp/rk_mpp_sdk_cpu_frame.nv12 "$jpeg_dir"/*.jpg
test -s /tmp/rk_mpp_sdk_cpu_frame.nv12
printf 'PASS rk-mpp-rs DMA-BUF import and MJPEG frame export\n'
