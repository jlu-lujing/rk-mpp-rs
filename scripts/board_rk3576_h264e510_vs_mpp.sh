#!/usr/bin/env bash

# First RK3576 VEPU510 H.264 hardware gate: one 320x240 QP26 Baseline IDR.

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
ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 13 \
    -pix_fmt nv12 -f rawvideo /tmp/h264e510_13x320x240.nv12

rm -f /tmp/h264e510_c.264 /tmp/h264e510_rust.264
rm -f /tmp/h264e510_c.log /tmp/h264e510_rust.log
rm -f /tmp/h264e510_gop2_c.264 /tmp/h264e510_gop2_rust.264
rm -f /tmp/h264e510_gop2_c.log /tmp/h264e510_gop2_rust.log
rm -f /tmp/h264e510_roi2_c.264 /tmp/h264e510_roi2_rust.264
rm -f /tmp/h264e510_roi2_c.log /tmp/h264e510_roi2_rust.log
rm -f /tmp/h264e510_split_ctu2_c.264 /tmp/h264e510_split_ctu2_rust.264
rm -f /tmp/h264e510_split_ctu2_c.log /tmp/h264e510_split_ctu2_rust.log
rm -f /tmp/h264e510_split_byte2_c.264 /tmp/h264e510_split_byte2_rust.264
rm -f /tmp/h264e510_split_byte2_c.log /tmp/h264e510_split_byte2_rust.log
rm -f /tmp/h264e510_splitout_ctu2_c.264 /tmp/h264e510_splitout_ctu2_rust.264
rm -f /tmp/h264e510_splitout_ctu2_c.log /tmp/h264e510_splitout_ctu2_rust.log
rm -f /tmp/h264e510_splitout_ctu2_c.packets /tmp/h264e510_splitout_ctu2_rust.packets
rm -f /tmp/h264e510_splitout_byte2_c.264 /tmp/h264e510_splitout_byte2_rust.264
rm -f /tmp/h264e510_splitout_byte2_c.log /tmp/h264e510_splitout_byte2_rust.log
rm -f /tmp/h264e510_splitout_byte2_c.packets /tmp/h264e510_splitout_byte2_rust.packets
rm -f /tmp/h264e510_gdr_row_c.264 /tmp/h264e510_gdr_row_rust.264
rm -f /tmp/h264e510_gdr_row_c.log /tmp/h264e510_gdr_row_rust.log
rm -f /tmp/h264e510_gdr_row_c.packets /tmp/h264e510_gdr_row_rust.packets
rm -f /tmp/h264e510_gdr_col_c.264 /tmp/h264e510_gdr_col_rust.264
rm -f /tmp/h264e510_gdr_col_c.log /tmp/h264e510_gdr_col_rust.log
rm -f /tmp/h264e510_gdr_col_c.packets /tmp/h264e510_gdr_col_rust.packets
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
    /tmp/h264e510_2x320x240.nv12 /tmp/h264e510_roi2_c.264 \
    320 240 2 0 26 roi 0 0 160 120 1 1 30 seioff \
    > /tmp/h264e510_roi2_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h264e510-roi2 \
    /tmp/h264e510_2x320x240.nv12 /tmp/h264e510_roi2_rust.264 \
    > /tmp/h264e510_roi2_rust.log 2>&1
cmp /tmp/h264e510_roi2_c.264 /tmp/h264e510_roi2_rust.264
sha256sum /tmp/h264e510_roi2_c.264 /tmp/h264e510_roi2_rust.264
printf 'PASS RK3576 VEPU510 H.264 ROI GOP2: %s bytes\n' \
    "$(stat -c %s /tmp/h264e510_roi2_c.264)"

LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h264e_probe510_self \
    /tmp/h264e510_2x320x240.nv12 /tmp/h264e510_split_ctu2_c.264 \
    320 240 2 0 26 split 2 96 seioff \
    > /tmp/h264e510_split_ctu2_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h264e510-split2 \
    /tmp/h264e510_2x320x240.nv12 /tmp/h264e510_split_ctu2_rust.264 mb:96 \
    > /tmp/h264e510_split_ctu2_rust.log 2>&1
cmp /tmp/h264e510_split_ctu2_c.264 /tmp/h264e510_split_ctu2_rust.264
sha256sum /tmp/h264e510_split_ctu2_c.264 /tmp/h264e510_split_ctu2_rust.264
printf 'PASS RK3576 VEPU510 H.264 BY_CTU=96 GOP2: %s bytes\n' \
    "$(stat -c %s /tmp/h264e510_split_ctu2_c.264)"

LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h264e_probe510_self \
    /tmp/h264e510_2x320x240.nv12 /tmp/h264e510_split_byte2_c.264 \
    320 240 2 0 26 split 1 256 seioff \
    > /tmp/h264e510_split_byte2_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h264e510-split2 \
    /tmp/h264e510_2x320x240.nv12 /tmp/h264e510_split_byte2_rust.264 byte:256 \
    > /tmp/h264e510_split_byte2_rust.log 2>&1
cmp /tmp/h264e510_split_byte2_c.264 /tmp/h264e510_split_byte2_rust.264
sha256sum /tmp/h264e510_split_byte2_c.264 /tmp/h264e510_split_byte2_rust.264
printf 'PASS RK3576 VEPU510 H.264 BY_BYTE=256 GOP2: %s bytes\n' \
    "$(stat -c %s /tmp/h264e510_split_byte2_c.264)"

LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h264e_probe510_self \
    /tmp/h264e510_2x320x240.nv12 /tmp/h264e510_splitout_ctu2_c.264 \
    320 240 2 0 26 split 2 96 splitout 1 seioff \
    > /tmp/h264e510_splitout_ctu2_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h264e510-split2 \
    /tmp/h264e510_2x320x240.nv12 /tmp/h264e510_splitout_ctu2_rust.264 mb:96 lowdelay \
    > /tmp/h264e510_splitout_ctu2_rust.log 2>&1
cmp /tmp/h264e510_splitout_ctu2_c.264 /tmp/h264e510_splitout_ctu2_rust.264
awk '/^picture [0-9]+ packet [0-9]+: [0-9]+ bytes/ { print $5 }' \
    /tmp/h264e510_splitout_ctu2_c.log > /tmp/h264e510_splitout_ctu2_c.packets
awk '/^h264e510-split2 packet lengths:/ { for (i = 4; i <= NF; i++) print $i }' \
    /tmp/h264e510_splitout_ctu2_rust.log > /tmp/h264e510_splitout_ctu2_rust.packets
test -s /tmp/h264e510_splitout_ctu2_c.packets
cmp /tmp/h264e510_splitout_ctu2_c.packets /tmp/h264e510_splitout_ctu2_rust.packets
sha256sum /tmp/h264e510_splitout_ctu2_c.264 /tmp/h264e510_splitout_ctu2_rust.264
printf 'PASS RK3576 VEPU510 H.264 LOWDELAY BY_CTU=96 GOP2: %s bytes, %s packets\n' \
    "$(stat -c %s /tmp/h264e510_splitout_ctu2_c.264)" \
    "$(wc -l < /tmp/h264e510_splitout_ctu2_c.packets)"

LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h264e_probe510_self \
    /tmp/h264e510_2x320x240.nv12 /tmp/h264e510_splitout_byte2_c.264 \
    320 240 2 0 26 split 1 256 splitout 1 seioff \
    > /tmp/h264e510_splitout_byte2_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h264e510-split2 \
    /tmp/h264e510_2x320x240.nv12 /tmp/h264e510_splitout_byte2_rust.264 byte:256 lowdelay \
    > /tmp/h264e510_splitout_byte2_rust.log 2>&1
cmp /tmp/h264e510_splitout_byte2_c.264 /tmp/h264e510_splitout_byte2_rust.264
awk '/^picture [0-9]+ packet [0-9]+: [0-9]+ bytes/ { print $5 }' \
    /tmp/h264e510_splitout_byte2_c.log > /tmp/h264e510_splitout_byte2_c.packets
awk '/^h264e510-split2 packet lengths:/ { for (i = 4; i <= NF; i++) print $i }' \
    /tmp/h264e510_splitout_byte2_rust.log > /tmp/h264e510_splitout_byte2_rust.packets
test -s /tmp/h264e510_splitout_byte2_c.packets
cmp /tmp/h264e510_splitout_byte2_c.packets /tmp/h264e510_splitout_byte2_rust.packets
sha256sum /tmp/h264e510_splitout_byte2_c.264 /tmp/h264e510_splitout_byte2_rust.264
printf 'PASS RK3576 VEPU510 H.264 LOWDELAY BY_BYTE=256 GOP2: %s bytes, %s packets\n' \
    "$(stat -c %s /tmp/h264e510_splitout_byte2_c.264)" \
    "$(wc -l < /tmp/h264e510_splitout_byte2_c.packets)"

LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h264e_probe510_self \
    /tmp/h264e510_13x320x240.nv12 /tmp/h264e510_gdr_row_c.264 \
    320 240 10 0 26 gop 4 gdr 0 2 seioff \
    > /tmp/h264e510_gdr_row_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h264e510-gdr \
    /tmp/h264e510_13x320x240.nv12 /tmp/h264e510_gdr_row_rust.264 row:2:4:10 \
    > /tmp/h264e510_gdr_row_rust.log 2>&1
cmp /tmp/h264e510_gdr_row_c.264 /tmp/h264e510_gdr_row_rust.264
awk '/^picture [0-9]+ packet [0-9]+: [0-9]+ bytes/ { print $5 }' \
    /tmp/h264e510_gdr_row_c.log > /tmp/h264e510_gdr_row_c.packets
awk '/^h264e510-gdr packet lengths:/ { for (i = 4; i <= NF; i++) print $i }' \
    /tmp/h264e510_gdr_row_rust.log > /tmp/h264e510_gdr_row_rust.packets
test -s /tmp/h264e510_gdr_row_c.packets
cmp /tmp/h264e510_gdr_row_c.packets /tmp/h264e510_gdr_row_rust.packets
sha256sum /tmp/h264e510_gdr_row_c.264 /tmp/h264e510_gdr_row_rust.264
printf 'PASS RK3576 VEPU510 H.264 GDR row=2 GOP4: %s bytes, %s pictures\n' \
    "$(stat -c %s /tmp/h264e510_gdr_row_c.264)" \
    "$(wc -l < /tmp/h264e510_gdr_row_c.packets)"

LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/h264e_probe510_self \
    /tmp/h264e510_13x320x240.nv12 /tmp/h264e510_gdr_col_c.264 \
    320 240 13 0 26 gop 6 gdr 1 4 seioff \
    > /tmp/h264e510_gdr_col_c.log 2>&1
timeout 120 ./target/debug/rk-mpp-rs h264e510-gdr \
    /tmp/h264e510_13x320x240.nv12 /tmp/h264e510_gdr_col_rust.264 col:4:6:13 \
    > /tmp/h264e510_gdr_col_rust.log 2>&1
cmp /tmp/h264e510_gdr_col_c.264 /tmp/h264e510_gdr_col_rust.264
awk '/^picture [0-9]+ packet [0-9]+: [0-9]+ bytes/ { print $5 }' \
    /tmp/h264e510_gdr_col_c.log > /tmp/h264e510_gdr_col_c.packets
awk '/^h264e510-gdr packet lengths:/ { for (i = 4; i <= NF; i++) print $i }' \
    /tmp/h264e510_gdr_col_rust.log > /tmp/h264e510_gdr_col_rust.packets
test -s /tmp/h264e510_gdr_col_c.packets
cmp /tmp/h264e510_gdr_col_c.packets /tmp/h264e510_gdr_col_rust.packets
sha256sum /tmp/h264e510_gdr_col_c.264 /tmp/h264e510_gdr_col_rust.264
printf 'PASS RK3576 VEPU510 H.264 GDR col=4 GOP6: %s bytes, %s pictures\n' \
    "$(stat -c %s /tmp/h264e510_gdr_col_c.264)" \
    "$(wc -l < /tmp/h264e510_gdr_col_c.packets)"

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
