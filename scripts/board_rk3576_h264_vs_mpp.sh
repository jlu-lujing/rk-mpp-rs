#!/usr/bin/env bash

# RK3576 VDPU383 H.264 bring-up gate.
#
# The board's stock MPP is the oracle. This script intentionally builds the
# Rust side without LTO and reports no performance claim: it proves the
# VDPU383 register/table/stride path over a real IPPP stream.
#
# Usage:
#   BOARD=my-rk3576-host scripts/board_rk3576_h264_vs_mpp.sh

set -euo pipefail

board="${BOARD:?set BOARD to a Rockchip SSH alias}"
source_stream=/tmp/rk3576_h264_640x480.264
c_nv12=/tmp/rk3576_h264_640x480_c.nv12
c_yuv=/tmp/rk3576_h264_640x480_c.yuv
r_yuv=/tmp/rk3576_h264_640x480_rust.yuv

ssh "$board" 'mkdir -p "$HOME/rk-mpp-rs"'
COPYFILE_DISABLE=1 tar \
    --no-xattrs \
    --exclude .git \
    --exclude target \
    --exclude .worktrees \
    --exclude refs \
    -cf - . |
    ssh "$board" "tar -xf - -C \$HOME/rk-mpp-rs"

ssh "$board" '
    set -eu
    export PATH="$HOME/.cargo/bin:$PATH"
    cd "$HOME/rk-mpp-rs"
    cargo build

    if ! test -s /tmp/rk3576_h264_640x480.264; then
        ffmpeg -y -hide_banner -loglevel error \
            -f lavfi -i testsrc2=size=640x480:rate=30 \
            -frames:v 12 -c:v libx264 -preset veryfast -qp 24 \
            -pix_fmt yuv420p /tmp/rk3576_h264_640x480.264
    fi

    rm -f /tmp/rk3576_h264_640x480_c.nv12 \
          /tmp/rk3576_h264_640x480_c.yuv \
          /tmp/rk3576_h264_640x480_rust.yuv
    LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
        /usr/local/bin/mpi_dec_test \
        -i /tmp/rk3576_h264_640x480.264 -t 7 -n 12 \
        -o /tmp/rk3576_h264_640x480_c.nv12 -v q
    ffmpeg -hide_banner -loglevel error -y \
        -f rawvideo -pixel_format nv12 -video_size 640x480 \
        -i /tmp/rk3576_h264_640x480_c.nv12 \
        -pix_fmt yuv420p -f rawvideo \
        /tmp/rk3576_h264_640x480_c.yuv
    ./target/debug/rk-mpp-rs \
        h264dec /tmp/rk3576_h264_640x480.264 \
        /tmp/rk3576_h264_640x480_rust.yuv
    cmp \
        /tmp/rk3576_h264_640x480_c.yuv \
        /tmp/rk3576_h264_640x480_rust.yuv
    sha256sum \
        /tmp/rk3576_h264_640x480_c.yuv \
        /tmp/rk3576_h264_640x480_rust.yuv
    printf "PASS RK3576 VDPU383 H.264 640x480 12 frames byte-identical\n"
'
