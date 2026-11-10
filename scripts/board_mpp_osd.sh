#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp=${MPP_REF:-$HOME/rk-mpp-oracles/mpp-old}
if [ -f "$mpp/inc/rk_mpi.h" ] && [ -f "$mpp/build/mpp/librockchip_mpp.so.1" ]; then
    lib="$mpp/build/mpp"
    enc_test="$mpp/build/test/mpi_enc_test"
elif [ -f /usr/local/include/rockchip/rk_mpi.h ] &&
    [ -f /usr/local/lib/librockchip_mpp.so.1 ]; then
    lib=/usr/local/lib
    enc_test=/usr/local/bin/mpi_enc_test
else
    printf 'missing reference MPP headers or library\n' >&2
    exit 2
fi

raw=/tmp/rk_mpp_osd_2.nv12
overlay=$(mktemp -d /tmp/rk_mpp_osd.XXXXXX)
trap 'rm -rf "$overlay"' EXIT

if grep -qi rk3576 /proc/device-tree/compatible 2>/dev/null; then
    printf 'SKIP VEPU510 OSD: register path is not implemented\n'
    exit 0
fi

cargo build --release --offline --lib --bin rk-mpp-rs
ln -s "$repo/target/release/librk_mpp_rs.so" \
    "$overlay/librockchip_mpp.so.1"
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 2 \
    -pix_fmt nv12 -f rawvideo "$raw"

run_encoder() {
    local library="$1"
    local preload="$2"
    local output="$3"
    local type="$4"
    local decoder="$5"
    local yuv="${output}.yuv"

    sudo -n rm -f "$output" "$yuv"
    sudo -n env osd_enable=1 LD_PRELOAD="$preload" LD_LIBRARY_PATH="$library" \
        timeout 60 "$enc_test" -i "$raw" -o "$output" \
        -w 320 -h 240 -n 2 -rc 2 -fps 30 -g 0:60 -t "$type" -qc 26 \
        < /dev/null > "$output.log" 2>&1
    sudo -n "$repo/target/release/rk-mpp-rs" "$decoder" "$output" "$yuv" \
        > /dev/null
}

for case in "h264 7 h264dec" "h265 16777220 hevcdec"; do
    set -- $case
    name="$1"
    type="$2"
    decoder="$3"
    vendor="/tmp/rk_mpp_osd_${name}_vendor.stream"
    ours="/tmp/rk_mpp_osd_${name}_ours.stream"
    run_encoder "$lib" "" "$vendor" "$type" "$decoder"
    run_encoder "$overlay" "$overlay/librockchip_mpp.so.1" \
        "$ours" "$type" "$decoder"
    python3 "$repo/scripts/compare_streams.py" "$vendor" "$ours"
    cmp "$vendor.yuv" "$ours.yuv"
    printf 'PASS %s OSD stream/reconstruction (%s)\n' \
        "$name" "$(sha256sum "$ours.yuv" | cut -d' ' -f1)"
done
