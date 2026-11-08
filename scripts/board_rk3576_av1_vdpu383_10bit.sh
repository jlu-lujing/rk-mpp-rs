#!/usr/bin/env bash

# Decode the ten-frame 10-bit AV1 fixture and compare Rust's planar u16 output
# with stock C MPP. The current C VDPU383 HAL does not enable film grain, so
# this gate locks the common skip-grain path.

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

c_raw=/tmp/rk3576_av1_383_c_10bit.raw
c_yuv=/tmp/rk3576_av1_383_c_10bit.yuv
rust_yuv=/tmp/rk3576_av1_383_rust_10bit.yuv
rm -f "$c_raw" "$c_yuv" "$rust_yuv"

LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
    timeout 120 /usr/local/bin/mpi_dec_test \
    -i tests/data/film_grain_10bit.ivf -t 16777224 -n 10 \
    -o "$c_raw" -v q < /dev/null > /tmp/rk3576_av1_383_c_10bit.log 2>&1
python3 scripts/p010_to_planar.py "$c_raw" "$c_yuv" 320 240

timeout 120 ./target/debug/examples/av1d383_sequence_probe \
    tests/data/film_grain_10bit.ivf "$rust_yuv"
test "$(stat -c %s "$c_yuv")" -eq 2304000
test "$(stat -c %s "$rust_yuv")" -eq 2304000
cmp "$c_yuv" "$rust_yuv"
sha256sum "$c_yuv" "$rust_yuv"
printf 'PASS RK3576 VDPU383 AV1 10-bit sequence (10/10 visible frames)\n'
REMOTE
