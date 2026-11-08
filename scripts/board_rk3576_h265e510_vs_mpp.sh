#!/usr/bin/env bash

# First RK3576 VEPU510 H.265 hardware gate: one 320x240 QP26 Main IDR.

set -euo pipefail

board="${BOARD:-luck2}"

ssh "$board" 'mkdir -p "$HOME/rk-mpp-rs"'
COPYFILE_DISABLE=1 tar \
    --no-xattrs \
    --exclude .git \
    --exclude target \
    --exclude .worktrees \
    --exclude refs \
    -cf - . |
    ssh "$board" 'tar -xf - -C "$HOME/rk-mpp-rs"'

ssh "$board" 'bash -s' <<'REMOTE'
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
cd "$HOME/rk-mpp-rs"
cargo build

cc -O2 -w -pthread -I/usr/local/include/rockchip tools/h265e_probe.c \
    -L/usr/local/lib -lrockchip_mpp -o /tmp/h265e_probe510

ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 1 \
    -pix_fmt nv12 -f rawvideo /tmp/h265e510_320x240.nv12
ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 2 \
    -pix_fmt nv12 -f rawvideo /tmp/h265e510_2x320x240.nv12
ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 6 \
    -pix_fmt nv12 -f rawvideo /tmp/h265e510_6x320x240.nv12

rm -f /tmp/h265e510_c.265 /tmp/h265e510_rust.265
rm -f /tmp/h265e510_c.log /tmp/h265e510_rust.log
rm -f /tmp/h265e510_gop2_c.265 /tmp/h265e510_gop2_rust.265
rm -f /tmp/h265e510_gop2_c.log /tmp/h265e510_gop2_rust.log
rm -f /tmp/h265e510_gop6_c.265 /tmp/h265e510_gop6_rust.265
rm -f /tmp/h265e510_gop6_c.log /tmp/h265e510_gop6_rust.log
rm -f /tmp/h265e510_cbr6_c.265 /tmp/h265e510_cbr6_rust.265
rm -f /tmp/h265e510_cbr6_c.log /tmp/h265e510_cbr6_rust.log

LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h265e_probe510 \
    /tmp/h265e510_320x240.nv12 /tmp/h265e510_c.265 \
    320 240 0 26 > /tmp/h265e510_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h265e510 \
    /tmp/h265e510_320x240.nv12 /tmp/h265e510_rust.265 \
    > /tmp/h265e510_rust.log 2>&1

cmp /tmp/h265e510_c.265 /tmp/h265e510_rust.265
sha256sum /tmp/h265e510_c.265 /tmp/h265e510_rust.265
printf 'PASS RK3576 VEPU510 H.265 fixed-QP IDR: %s bytes\n' \
    "$(stat -c %s /tmp/h265e510_c.265)"

FRAMES=2 GOP=60 LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h265e_probe510 \
    /tmp/h265e510_2x320x240.nv12 /tmp/h265e510_gop2_c.265 \
    320 240 0 26 > /tmp/h265e510_gop2_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h265e510-gop2 \
    /tmp/h265e510_2x320x240.nv12 /tmp/h265e510_gop2_rust.265 \
    > /tmp/h265e510_gop2_rust.log 2>&1

cmp /tmp/h265e510_gop2_c.265 /tmp/h265e510_gop2_rust.265
sha256sum /tmp/h265e510_gop2_c.265 /tmp/h265e510_gop2_rust.265
printf 'PASS RK3576 VEPU510 H.265 fixed-QP GOP2: %s bytes\n' \
    "$(stat -c %s /tmp/h265e510_gop2_c.265)"

FRAMES=6 GOP=60 LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h265e_probe510 \
    /tmp/h265e510_6x320x240.nv12 /tmp/h265e510_gop6_c.265 \
    320 240 0 26 > /tmp/h265e510_gop6_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h265e510-gop6 \
    /tmp/h265e510_6x320x240.nv12 /tmp/h265e510_gop6_rust.265 \
    > /tmp/h265e510_gop6_rust.log 2>&1

cmp /tmp/h265e510_gop6_c.265 /tmp/h265e510_gop6_rust.265
sha256sum /tmp/h265e510_gop6_c.265 /tmp/h265e510_gop6_rust.265
printf 'PASS RK3576 VEPU510 H.265 fixed-QP GOP6: %s bytes\n' \
    "$(stat -c %s /tmp/h265e510_gop6_c.265)"

FRAMES=6 GOP=60 LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h265e_probe510 \
    /tmp/h265e510_6x320x240.nv12 /tmp/h265e510_cbr6_c.265 \
    320 240 1 26 > /tmp/h265e510_cbr6_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h265e510-cbr6 \
    /tmp/h265e510_6x320x240.nv12 /tmp/h265e510_cbr6_rust.265 \
    > /tmp/h265e510_cbr6_rust.log 2>&1

cmp /tmp/h265e510_cbr6_c.265 /tmp/h265e510_cbr6_rust.265
sha256sum /tmp/h265e510_cbr6_c.265 /tmp/h265e510_cbr6_rust.265
printf 'PASS RK3576 VEPU510 H.265 CBR6: %s bytes\n' \
    "$(stat -c %s /tmp/h265e510_cbr6_c.265)"
REMOTE
