#!/usr/bin/env bash

# One AV1 keyframe through VDPU383, byte-compared with stock C MPP.

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
cargo build --example av1d383_probe

c_raw=/tmp/rk3576_av1_383_c_first.nv12
c_yuv=/tmp/rk3576_av1_383_c_first.yuv
rust_yuv=/tmp/rk3576_av1_383_rust_first.yuv
rm -f "$c_raw" "$c_yuv" "$rust_yuv"

LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
    timeout 60 /usr/local/bin/mpi_dec_test \
    -i tests/data/av1_tiles_320x240_10.ivf -t 16777224 -n 1 \
    -o "$c_raw" -v q < /dev/null > /tmp/rk3576_av1_383_c_first.log 2>&1
ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f rawvideo -pixel_format nv12 -video_size 320x240 \
    -i "$c_raw" -frames:v 1 -pix_fmt yuv420p -f rawvideo "$c_yuv"

timeout 60 ./target/debug/examples/av1d383_probe \
    tests/data/av1_tiles_320x240_10.ivf "$rust_yuv"
test "$(stat -c %s "$c_yuv")" -eq 115200
test "$(stat -c %s "$rust_yuv")" -eq 115200
cmp "$c_yuv" "$rust_yuv"
sha256sum "$c_yuv" "$rust_yuv"
printf 'PASS RK3576 VDPU383 AV1 keyframe\n'
REMOTE
