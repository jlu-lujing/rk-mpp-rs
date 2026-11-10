#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp=${MPP_REF:-$HOME/rk-mpp-oracles/mpp-old}
if [ -f "$mpp/inc/rk_mpi.h" ] && [ -f "$mpp/build/mpp/librockchip_mpp.so.1" ]; then
    inc="$mpp/inc"
    lib="$mpp/build/mpp"
elif [ -f /usr/local/include/rockchip/rk_mpi.h ] &&
    [ -f /usr/local/lib/librockchip_mpp.so.1 ]; then
    inc=/usr/local/include/rockchip
    lib=/usr/local/lib
else
    printf 'missing reference MPP headers or library\n' >&2
    exit 2
fi
probe=/tmp/rk_mpp_h265_force_idr_probe
raw=/tmp/rk_mpp_h265_force_idr_6.nv12
vendor=/tmp/rk_mpp_h265_force_idr_vendor.265
ours=/tmp/rk_mpp_h265_force_idr_ours.265
direct=/tmp/rk_mpp_h265_force_idr_direct.265
overlay=$(mktemp -d /tmp/rk_mpp_h265_force_idr.XXXXXX)
trap 'rm -rf "$overlay"' EXIT

cargo build --release --offline --lib --bin rk-mpp-rs
ln -s "$repo/target/release/librk_mpp_rs.so" "$overlay/librockchip_mpp.so.1"

cc -D_GNU_SOURCE -std=c11 -O2 -Wall -Wextra -Werror -pthread \
    -I"$inc" "$repo/tools/h265e_probe.c" \
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

if grep -qi rk3588 /proc/device-tree/compatible 2>/dev/null; then
    python3 "$repo/scripts/compare_streams.py" "$vendor" "$ours"
    sudo -n "$repo/target/release/rk-mpp-rs" hevcdec "$vendor" "$vendor.yuv" \
        > /dev/null
    sudo -n "$repo/target/release/rk-mpp-rs" hevcdec "$ours" "$ours.yuv" \
        > /dev/null
    cmp "$vendor.yuv" "$ours.yuv"
    printf 'PASS h265 forced IDR at frames 2,4 (%s)\n' \
        "$(sha256sum "$ours.yuv" | cut -d' ' -f1)"
else
    # The vendor MPP object path and the VEPU510 direct template have
    # independently stable streams on lf1. Compare object controls by NAL
    # sequence, then prove the full control payload through the direct path.
    python3 "$repo/scripts/compare_streams.py" --sequence-only "$vendor" "$ours"
    sudo -n rm -f "$direct" "$direct.yuv"
    sudo -n "$repo/target/release/rk-mpp-rs" h265e510-control \
        "$raw" "$direct" 2,4 off > /dev/null
    python3 "$repo/scripts/compare_streams.py" "$vendor" "$direct"
    sudo -n "$repo/target/release/rk-mpp-rs" hevcdec "$vendor" "$vendor.yuv" \
        > /dev/null
    sudo -n "$repo/target/release/rk-mpp-rs" hevcdec "$direct" "$direct.yuv" \
        > /dev/null
    cmp "$vendor.yuv" "$direct.yuv"
    printf 'PASS h265 VEPU510 object NAL sequence and direct forced-IDR stream (%s)\n' \
        "$(sha256sum "$direct.yuv" | cut -d' ' -f1)"
fi
