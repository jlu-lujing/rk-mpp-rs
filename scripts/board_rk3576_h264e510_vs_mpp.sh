#!/usr/bin/env bash

# First RK3576 VEPU510 H.264 hardware gate: one 320x240 QP26 Baseline IDR.

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

cc -O2 -w -pthread -DH264E_PROBE_NO_ROI \
    -I/usr/local/include/rockchip tools/h264e_probe.c \
    -L/usr/local/lib -lrockchip_mpp -o /tmp/h264e_probe510_self

ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 1 \
    -pix_fmt nv12 -f rawvideo /tmp/h264e510_320x240.nv12
ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 2 \
    -pix_fmt nv12 -f rawvideo /tmp/h264e510_2x320x240.nv12
ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 6 \
    -pix_fmt nv12 -f rawvideo /tmp/h264e510_6x320x240.nv12

rm -f /tmp/h264e510_c.264 /tmp/h264e510_rust.264
rm -f /tmp/h264e510_c.log /tmp/h264e510_rust.log
rm -f /tmp/h264e510_gop2_c.264 /tmp/h264e510_gop2_rust.264
rm -f /tmp/h264e510_gop2_c.log /tmp/h264e510_gop2_rust.log
rm -f /tmp/h264e510_gop6_c.264 /tmp/h264e510_gop6_rust.264
rm -f /tmp/h264e510_gop6_c.log /tmp/h264e510_gop6_rust.log
rm -f /tmp/h264e510_cbr6_c.264 /tmp/h264e510_cbr6_rust.264
rm -f /tmp/h264e510_cbr6_c.log /tmp/h264e510_cbr6_rust.log
rm -f /tmp/h264e510_vbr6_c.264 /tmp/h264e510_vbr6_rust.264
rm -f /tmp/h264e510_vbr6_c.log /tmp/h264e510_vbr6_rust.log
rm -f /tmp/h264e510_avbr6_c.264 /tmp/h264e510_avbr6_rust.264
rm -f /tmp/h264e510_avbr6_c.log /tmp/h264e510_avbr6_rust.log

LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h264e_probe510_self \
    /tmp/h264e510_320x240.nv12 /tmp/h264e510_c.264 \
    320 240 1 0 26 seioff > /tmp/h264e510_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h264e510 \
    /tmp/h264e510_320x240.nv12 /tmp/h264e510_rust.264 \
    > /tmp/h264e510_rust.log 2>&1

cmp /tmp/h264e510_c.264 /tmp/h264e510_rust.264
sha256sum /tmp/h264e510_c.264 /tmp/h264e510_rust.264
printf 'PASS RK3576 VEPU510 H.264 fixed-QP IDR: %s bytes\n' \
    "$(stat -c %s /tmp/h264e510_c.264)"

LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h264e_probe510_self \
    /tmp/h264e510_2x320x240.nv12 /tmp/h264e510_gop2_c.264 \
    320 240 2 0 26 seioff > /tmp/h264e510_gop2_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h264e510-gop2 \
    /tmp/h264e510_2x320x240.nv12 /tmp/h264e510_gop2_rust.264 \
    > /tmp/h264e510_gop2_rust.log 2>&1

cmp /tmp/h264e510_gop2_c.264 /tmp/h264e510_gop2_rust.264
sha256sum /tmp/h264e510_gop2_c.264 /tmp/h264e510_gop2_rust.264
printf 'PASS RK3576 VEPU510 H.264 fixed-QP GOP2: %s bytes\n' \
    "$(stat -c %s /tmp/h264e510_gop2_c.264)"

LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h264e_probe510_self \
    /tmp/h264e510_6x320x240.nv12 /tmp/h264e510_gop6_c.264 \
    320 240 6 0 26 seioff > /tmp/h264e510_gop6_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h264e510-gop6 \
    /tmp/h264e510_6x320x240.nv12 /tmp/h264e510_gop6_rust.264 \
    > /tmp/h264e510_gop6_rust.log 2>&1

cmp /tmp/h264e510_gop6_c.264 /tmp/h264e510_gop6_rust.264
sha256sum /tmp/h264e510_gop6_c.264 /tmp/h264e510_gop6_rust.264
printf 'PASS RK3576 VEPU510 H.264 fixed-QP GOP6: %s bytes\n' \
    "$(stat -c %s /tmp/h264e510_gop6_c.264)"

LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h264e_probe510_self \
    /tmp/h264e510_6x320x240.nv12 /tmp/h264e510_cbr6_c.264 \
    320 240 6 1 -1 seioff > /tmp/h264e510_cbr6_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h264e510-cbr6 \
    /tmp/h264e510_6x320x240.nv12 /tmp/h264e510_cbr6_rust.264 \
    > /tmp/h264e510_cbr6_rust.log 2>&1

cmp /tmp/h264e510_cbr6_c.264 /tmp/h264e510_cbr6_rust.264
sha256sum /tmp/h264e510_cbr6_c.264 /tmp/h264e510_cbr6_rust.264
printf 'PASS RK3576 VEPU510 H.264 CBR GOP6: %s bytes\n' \
    "$(stat -c %s /tmp/h264e510_cbr6_c.264)"

LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h264e_probe510_self \
    /tmp/h264e510_6x320x240.nv12 /tmp/h264e510_vbr6_c.264 \
    320 240 6 2 -1 seioff > /tmp/h264e510_vbr6_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h264e510-vbr6 \
    /tmp/h264e510_6x320x240.nv12 /tmp/h264e510_vbr6_rust.264 \
    > /tmp/h264e510_vbr6_rust.log 2>&1
cmp /tmp/h264e510_vbr6_c.264 /tmp/h264e510_vbr6_rust.264
sha256sum /tmp/h264e510_vbr6_c.264 /tmp/h264e510_vbr6_rust.264
printf 'PASS RK3576 VEPU510 H.264 VBR GOP6: %s bytes\n' \
    "$(stat -c %s /tmp/h264e510_vbr6_c.264)"

LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h264e_probe510_self \
    /tmp/h264e510_6x320x240.nv12 /tmp/h264e510_avbr6_c.264 \
    320 240 6 3 -1 seioff > /tmp/h264e510_avbr6_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h264e510-avbr6 \
    /tmp/h264e510_6x320x240.nv12 /tmp/h264e510_avbr6_rust.264 \
    > /tmp/h264e510_avbr6_rust.log 2>&1
cmp /tmp/h264e510_avbr6_c.264 /tmp/h264e510_avbr6_rust.264
sha256sum /tmp/h264e510_avbr6_c.264 /tmp/h264e510_avbr6_rust.264
printf 'PASS RK3576 VEPU510 H.264 AVBR GOP6: %s bytes\n' \
    "$(stat -c %s /tmp/h264e510_avbr6_c.264)"
REMOTE
