#!/usr/bin/env bash

# Decode the full ten-picture VDPU383 AV1 sequence and compare every visible
# Y/U/V byte with stock C MPP.

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
cargo build --example av1d383_sequence_probe

c_raw=/tmp/rk3576_av1_383_c_sequence.nv12
c_yuv=/tmp/rk3576_av1_383_c_sequence.yuv
rust_yuv=/tmp/rk3576_av1_383_rust_sequence.yuv
rm -f "$c_raw" "$c_yuv" "$rust_yuv"

LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
    timeout 120 /usr/local/bin/mpi_dec_test \
    -i tests/data/av1_tiles_320x240_10.ivf -t 16777224 -n 10 \
    -o "$c_raw" -v q < /dev/null > /tmp/rk3576_av1_383_c_sequence.log 2>&1
ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f rawvideo -pixel_format nv12 -video_size 320x240 \
    -i "$c_raw" -frames:v 10 -pix_fmt yuv420p -f rawvideo "$c_yuv"

timeout 120 ./target/debug/examples/av1d383_sequence_probe \
    tests/data/av1_tiles_320x240_10.ivf "$rust_yuv"
test "$(stat -c %s "$c_yuv")" -eq 1152000
test "$(stat -c %s "$rust_yuv")" -eq 1152000
cmp "$c_yuv" "$rust_yuv"
sha256sum "$c_yuv" "$rust_yuv"
printf 'PASS RK3576 VDPU383 AV1 sequence (10/10 visible frames)\n'
REMOTE
