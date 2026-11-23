#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
if [ "${SKIP_BUILD:-0}" != 1 ]; then
    cargo build --release --offline --lib
fi

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

size="${SIZE:-320x240}"
width="${size%%x*}"
height="${size##*x}"
frames="${FRAMES:-6}"
jpeg_frames="${JPEG_FRAMES:-1}"
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=${size}:rate=30" -frames:v "$frames" \
    -pix_fmt nv12 -f rawvideo "$raw_dir/frames.nv12"

for codec in h264 h265; do
    copied="/tmp/rk_mpp_sdk_encoder_${codec}_${size}_copied.bin"
    dmabuf="/tmp/rk_mpp_sdk_encoder_${codec}_${size}_dmabuf.bin"
    LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
        /tmp/rk_mpp_encoder_dmabuf "$codec" nv12 "$copied" \
            "$width" "$height" 26 "$frames" "$frames" "$raw_dir/frames.nv12"
    DMABUF_INPUT=1 LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
        /tmp/rk_mpp_encoder_dmabuf "$codec" nv12 "$dmabuf" \
            "$width" "$height" 26 "$frames" "$frames" "$raw_dir/frames.nv12"
    cmp "$copied" "$dmabuf"
done

jpeg_copied="/tmp/rk_mpp_sdk_encoder_jpeg_${size}_copied.jpg"
jpeg_dmabuf="/tmp/rk_mpp_sdk_encoder_jpeg_${size}_dmabuf.jpg"
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_encoder_dmabuf jpeg nv12 "$jpeg_copied" \
        "$width" "$height" 10 "$jpeg_frames" 1 "$raw_dir/frames.nv12"
DMABUF_INPUT=1 LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_encoder_dmabuf jpeg nv12 "$jpeg_dmabuf" \
        "$width" "$height" 10 "$jpeg_frames" 1 "$raw_dir/frames.nv12"
cmp "$jpeg_copied" "$jpeg_dmabuf"

if command -v strace >/dev/null 2>&1; then
    trace="$raw_dir/fcntl.trace"
    strace -f -e trace=fcntl -o "$trace" \
        env LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" DMABUF_INPUT=1 \
        /tmp/rk_mpp_encoder_dmabuf h264 nv12 /tmp/rk_mpp_sdk_encoder_strace.bin \
            "$width" "$height" 26 "$frames" "$frames" \
            "$raw_dir/frames.nv12" >/dev/null
    if grep -q F_DUPFD_CLOEXEC "$trace"; then
        echo "FAIL dma-buf C submit duplicated the caller fd" >&2
        grep F_DUPFD_CLOEXEC "$trace" >&2
        exit 1
    fi
fi

printf 'PASS copied and dma-buf encoder input match for H.264, H.265 and JPEG (%s, %s frames)\n' \
    "$size" "$frames"
