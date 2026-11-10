#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp=${MPP_REF:-$HOME/rk-mpp-oracles/mpp-old}
lib="$mpp/build/mpp"
probe=/tmp/rk_mpp_h265_force_idr_probe
raw=/tmp/rk_mpp_h265_force_idr_6.nv12
vendor=/tmp/rk_mpp_h265_force_idr_vendor.265
ours=/tmp/rk_mpp_h265_force_idr_ours.265
overlay=$(mktemp -d /tmp/rk_mpp_h265_force_idr.XXXXXX)
trap 'rm -rf "$overlay"' EXIT

if [ ! -f "$mpp/inc/rk_mpi.h" ] || [ ! -f "$lib/librockchip_mpp.so.1" ]; then
    printf 'missing reference MPP under %s\n' "$mpp" >&2
    exit 2
fi

cargo build --release --offline --lib --bin rk-mpp-rs
ln -s "$repo/target/release/librk_mpp_rs.so" "$overlay/librockchip_mpp.so.1"

cc -D_GNU_SOURCE -std=c11 -O2 -Wall -Wextra -Werror -pthread \
    -I"$mpp/inc" "$repo/tools/h265e_probe.c" \
    "$lib/librockchip_mpp.so.1" -o "$probe"

ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 6 \
    -pix_fmt nv12 -f rawvideo "$raw"

run_probe() {
    local library="$1"
    local output="$2"

    sudo -n rm -f "$output"
    sudo -n env FRAMES=6 GOP=60 LD_LIBRARY_PATH="$library" timeout 60 \
        "$probe" "$raw" "$output" 320 240 0 26 288000:1500000:2500000 \
        forceidr 2,4 headereach seioff \
        < /dev/null > "$output.log" 2>&1
}

run_probe "$lib" "$vendor"
run_probe "$overlay" "$ours"

python3 "$repo/scripts/compare_streams.py" "$vendor" "$ours"
sudo -n "$repo/target/release/rk-mpp-rs" hevcdec "$vendor" "$vendor.yuv" \
    > /dev/null
sudo -n "$repo/target/release/rk-mpp-rs" hevcdec "$ours" "$ours.yuv" \
    > /dev/null
cmp "$vendor.yuv" "$ours.yuv"
printf 'PASS h265 forced IDR at frames 2,4 (%s)\n' \
    "$(sha256sum "$ours.yuv" | cut -d' ' -f1)"
