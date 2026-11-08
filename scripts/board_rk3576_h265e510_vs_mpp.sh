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
rm -f /tmp/h265e510_roi2_c.265 /tmp/h265e510_roi2_rust.265
rm -f /tmp/h265e510_roi2_c.log /tmp/h265e510_roi2_rust.log
rm -f /tmp/h265e510_split_ctu2_c.265 /tmp/h265e510_split_ctu2_rust.265
rm -f /tmp/h265e510_split_ctu2_c.log /tmp/h265e510_split_ctu2_rust.log
rm -f /tmp/h265e510_split_byte2_c.265 /tmp/h265e510_split_byte2_rust.265
rm -f /tmp/h265e510_split_byte2_c.log /tmp/h265e510_split_byte2_rust.log
rm -f /tmp/h265e510_splitout_ctu2_c.265 /tmp/h265e510_splitout_ctu2_rust.265
rm -f /tmp/h265e510_splitout_ctu2_c.log /tmp/h265e510_splitout_ctu2_rust.log
rm -f /tmp/h265e510_splitout_ctu2_c.packets /tmp/h265e510_splitout_ctu2_rust.packets
rm -f /tmp/h265e510_splitout_byte2_c.265 /tmp/h265e510_splitout_byte2_rust.265
rm -f /tmp/h265e510_splitout_byte2_c.log /tmp/h265e510_splitout_byte2_rust.log
rm -f /tmp/h265e510_splitout_byte2_c.packets /tmp/h265e510_splitout_byte2_rust.packets
rm -f /tmp/h265e510_gdr_row_c.265 /tmp/h265e510_gdr_row_rust.265
rm -f /tmp/h265e510_gdr_row_c.log /tmp/h265e510_gdr_row_rust.log
rm -f /tmp/h265e510_gdr_col_c.265 /tmp/h265e510_gdr_col_rust.265
rm -f /tmp/h265e510_gdr_col_c.log /tmp/h265e510_gdr_col_rust.log
rm -f /tmp/h265e510_gop6_c.265 /tmp/h265e510_gop6_rust.265
rm -f /tmp/h265e510_gop6_c.log /tmp/h265e510_gop6_rust.log
rm -f /tmp/h265e510_cbr6_c.265 /tmp/h265e510_cbr6_rust.265
rm -f /tmp/h265e510_cbr6_c.log /tmp/h265e510_cbr6_rust.log
rm -f /tmp/h265e510_vbr6_c.265 /tmp/h265e510_vbr6_rust.265
rm -f /tmp/h265e510_vbr6_c.log /tmp/h265e510_vbr6_rust.log
rm -f /tmp/h265e510_avbr6_c.265 /tmp/h265e510_avbr6_rust.265
rm -f /tmp/h265e510_avbr6_c.log /tmp/h265e510_avbr6_rust.log

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

FRAMES=2 GOP=60 LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h265e_probe510 \
    /tmp/h265e510_2x320x240.nv12 /tmp/h265e510_roi2_c.265 \
    320 240 0 26 roi 0 0 160 120 1 1 30 \
    > /tmp/h265e510_roi2_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h265e510-roi2 \
    /tmp/h265e510_2x320x240.nv12 /tmp/h265e510_roi2_rust.265 \
    > /tmp/h265e510_roi2_rust.log 2>&1
cmp /tmp/h265e510_roi2_c.265 /tmp/h265e510_roi2_rust.265
sha256sum /tmp/h265e510_roi2_c.265 /tmp/h265e510_roi2_rust.265
printf 'PASS RK3576 VEPU510 H.265 ROI GOP2: %s bytes\n' \
    "$(stat -c %s /tmp/h265e510_roi2_c.265)"

FRAMES=2 GOP=60 LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h265e_probe510 \
    /tmp/h265e510_2x320x240.nv12 /tmp/h265e510_split_ctu2_c.265 \
    320 240 0 26 split 2 8 > /tmp/h265e510_split_ctu2_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h265e510-split2 \
    /tmp/h265e510_2x320x240.nv12 /tmp/h265e510_split_ctu2_rust.265 ctu:8 \
    > /tmp/h265e510_split_ctu2_rust.log 2>&1
cmp /tmp/h265e510_split_ctu2_c.265 /tmp/h265e510_split_ctu2_rust.265
sha256sum /tmp/h265e510_split_ctu2_c.265 /tmp/h265e510_split_ctu2_rust.265
printf 'PASS RK3576 VEPU510 H.265 BY_CTU=8 GOP2: %s bytes\n' \
    "$(stat -c %s /tmp/h265e510_split_ctu2_c.265)"

FRAMES=2 GOP=60 LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h265e_probe510 \
    /tmp/h265e510_2x320x240.nv12 /tmp/h265e510_split_byte2_c.265 \
    320 240 0 26 split 1 256 > /tmp/h265e510_split_byte2_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h265e510-split2 \
    /tmp/h265e510_2x320x240.nv12 /tmp/h265e510_split_byte2_rust.265 byte:256 \
    > /tmp/h265e510_split_byte2_rust.log 2>&1
cmp /tmp/h265e510_split_byte2_c.265 /tmp/h265e510_split_byte2_rust.265
sha256sum /tmp/h265e510_split_byte2_c.265 /tmp/h265e510_split_byte2_rust.265
printf 'PASS RK3576 VEPU510 H.265 BY_BYTE=256 GOP2: %s bytes\n' \
    "$(stat -c %s /tmp/h265e510_split_byte2_c.265)"

FRAMES=2 GOP=60 LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h265e_probe510 \
    /tmp/h265e510_2x320x240.nv12 /tmp/h265e510_splitout_ctu2_c.265 \
    320 240 0 26 split 2 8 splitout 1 \
    > /tmp/h265e510_splitout_ctu2_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h265e510-split2 \
    /tmp/h265e510_2x320x240.nv12 /tmp/h265e510_splitout_ctu2_rust.265 ctu:8 lowdelay \
    > /tmp/h265e510_splitout_ctu2_rust.log 2>&1
cmp /tmp/h265e510_splitout_ctu2_c.265 /tmp/h265e510_splitout_ctu2_rust.265
awk '/^picture [0-9]+ packet [0-9]+: [0-9]+ bytes/ { print $5 }' \
    /tmp/h265e510_splitout_ctu2_c.log > /tmp/h265e510_splitout_ctu2_c.packets
awk '/^h265e510-split2 packet lengths:/ { for (i = 4; i <= NF; i++) print $i }' \
    /tmp/h265e510_splitout_ctu2_rust.log > /tmp/h265e510_splitout_ctu2_rust.packets
test -s /tmp/h265e510_splitout_ctu2_c.packets
cmp /tmp/h265e510_splitout_ctu2_c.packets /tmp/h265e510_splitout_ctu2_rust.packets
sha256sum /tmp/h265e510_splitout_ctu2_c.265 /tmp/h265e510_splitout_ctu2_rust.265
printf 'PASS RK3576 VEPU510 H.265 LOWDELAY BY_CTU=8 GOP2: %s bytes, %s packets\n' \
    "$(stat -c %s /tmp/h265e510_splitout_ctu2_c.265)" \
    "$(wc -l < /tmp/h265e510_splitout_ctu2_c.packets)"

FRAMES=2 GOP=60 LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h265e_probe510 \
    /tmp/h265e510_2x320x240.nv12 /tmp/h265e510_splitout_byte2_c.265 \
    320 240 0 26 split 1 256 splitout 1 \
    > /tmp/h265e510_splitout_byte2_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h265e510-split2 \
    /tmp/h265e510_2x320x240.nv12 /tmp/h265e510_splitout_byte2_rust.265 byte:256 lowdelay \
    > /tmp/h265e510_splitout_byte2_rust.log 2>&1
cmp /tmp/h265e510_splitout_byte2_c.265 /tmp/h265e510_splitout_byte2_rust.265
awk '/^picture [0-9]+ packet [0-9]+: [0-9]+ bytes/ { print $5 }' \
    /tmp/h265e510_splitout_byte2_c.log > /tmp/h265e510_splitout_byte2_c.packets
awk '/^h265e510-split2 packet lengths:/ { for (i = 4; i <= NF; i++) print $i }' \
    /tmp/h265e510_splitout_byte2_rust.log > /tmp/h265e510_splitout_byte2_rust.packets
test -s /tmp/h265e510_splitout_byte2_c.packets
cmp /tmp/h265e510_splitout_byte2_c.packets /tmp/h265e510_splitout_byte2_rust.packets
sha256sum /tmp/h265e510_splitout_byte2_c.265 /tmp/h265e510_splitout_byte2_rust.265
printf 'PASS RK3576 VEPU510 H.265 LOWDELAY BY_BYTE=256 GOP2: %s bytes, %s packets\n' \
    "$(stat -c %s /tmp/h265e510_splitout_byte2_c.265)" \
    "$(wc -l < /tmp/h265e510_splitout_byte2_c.packets)"

REF_CFG=1 REFRESH_EN=1 REFRESH_MODE=0 REFRESH_NUM=2 FRAMES=10 GOP=4 \
    LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h265e_probe510 \
    /tmp/h264e510_13x320x240.nv12 /tmp/h265e510_gdr_row_c.265 \
    320 240 0 26 > /tmp/h265e510_gdr_row_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h265e510-gdr \
    /tmp/h264e510_13x320x240.nv12 /tmp/h265e510_gdr_row_rust.265 row:2:4:10 \
    > /tmp/h265e510_gdr_row_rust.log 2>&1
cmp /tmp/h265e510_gdr_row_c.265 /tmp/h265e510_gdr_row_rust.265
sha256sum /tmp/h265e510_gdr_row_c.265 /tmp/h265e510_gdr_row_rust.265
printf 'PASS RK3576 VEPU510 H.265 GDR row=2 GOP4: %s bytes\n' \
    "$(stat -c %s /tmp/h265e510_gdr_row_c.265)"

REF_CFG=1 REFRESH_EN=1 REFRESH_MODE=1 REFRESH_NUM=4 FRAMES=13 GOP=6 \
    LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h265e_probe510 \
    /tmp/h264e510_13x320x240.nv12 /tmp/h265e510_gdr_col_c.265 \
    320 240 0 26 > /tmp/h265e510_gdr_col_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h265e510-gdr \
    /tmp/h264e510_13x320x240.nv12 /tmp/h265e510_gdr_col_rust.265 col:4:6:13 \
    > /tmp/h265e510_gdr_col_rust.log 2>&1
cmp /tmp/h265e510_gdr_col_c.265 /tmp/h265e510_gdr_col_rust.265
sha256sum /tmp/h265e510_gdr_col_c.265 /tmp/h265e510_gdr_col_rust.265
printf 'PASS RK3576 VEPU510 H.265 GDR col=4 GOP6: %s bytes\n' \
    "$(stat -c %s /tmp/h265e510_gdr_col_c.265)"

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

FRAMES=6 GOP=60 LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h265e_probe510 \
    /tmp/h265e510_6x320x240.nv12 /tmp/h265e510_vbr6_c.265 \
    320 240 2 26 > /tmp/h265e510_vbr6_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h265e510-vbr6 \
    /tmp/h265e510_6x320x240.nv12 /tmp/h265e510_vbr6_rust.265 \
    > /tmp/h265e510_vbr6_rust.log 2>&1
cmp /tmp/h265e510_vbr6_c.265 /tmp/h265e510_vbr6_rust.265
sha256sum /tmp/h265e510_vbr6_c.265 /tmp/h265e510_vbr6_rust.265
printf 'PASS RK3576 VEPU510 H.265 VBR6: %s bytes\n' \
    "$(stat -c %s /tmp/h265e510_vbr6_c.265)"

FRAMES=6 GOP=60 LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h265e_probe510 \
    /tmp/h265e510_6x320x240.nv12 /tmp/h265e510_avbr6_c.265 \
    320 240 3 26 > /tmp/h265e510_avbr6_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h265e510-avbr6 \
    /tmp/h265e510_6x320x240.nv12 /tmp/h265e510_avbr6_rust.265 \
    > /tmp/h265e510_avbr6_rust.log 2>&1
cmp /tmp/h265e510_avbr6_c.265 /tmp/h265e510_avbr6_rust.265
sha256sum /tmp/h265e510_avbr6_c.265 /tmp/h265e510_avbr6_rust.265
printf 'PASS RK3576 VEPU510 H.265 AVBR6: %s bytes\n' \
    "$(stat -c %s /tmp/h265e510_avbr6_c.265)"
REMOTE
