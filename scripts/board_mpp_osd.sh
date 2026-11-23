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

overlay=$(mktemp -d /tmp/rk_mpp_osd.XXXXXX)
frames="${FRAMES:-2}"
size="${SIZE:-320x240}"
width="${size%%x*}"
height="${size##*x}"
raw="/tmp/rk_mpp_osd_${frames}-${size}.nv12"
trap 'rm -rf "$overlay"' EXIT

if grep -qi rk3576 /proc/device-tree/compatible 2>/dev/null; then
    printf 'SKIP VEPU510 OSD: register path is not implemented\n'
    exit 0
fi

if [ "${SKIP_BUILD:-0}" != 1 ]; then
    cargo build --release --offline --lib --bin rk-mpp-rs
fi
ln -s "$repo/target/release/librk_mpp_rs.so" \
    "$overlay/librockchip_mpp.so.1"
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=${size}:rate=30" -frames:v "$frames" \
    -pix_fmt nv12 -f rawvideo "$raw"

run_encoder() {
    local library="$1"
    local preload="$2"
    local output="$3"
    local type="$4"
    local decoder="$5"
    local force_cpu="$6"
    local yuv="${output}.yuv"
    local input_mode=()

    if [ "$force_cpu" = direct ]; then
        input_mode=(RKMPP_COMPAT_OSD_DMABUF=1)
    elif [ "$force_cpu" = fallback ]; then
        input_mode=(RKMPP_COMPAT_CPU_INPUT=1)
    fi
    sudo -n rm -f "$output" "$yuv"
    sudo -n env osd_enable=1 "${input_mode[@]}" \
        LD_PRELOAD="$preload" LD_LIBRARY_PATH="$library" \
        timeout 60 "$enc_test" -i "$raw" -o "$output" \
        -w "$width" -h "$height" -n "$frames" -rc 2 -fps 30 -g 0:60 -t "$type" -qc 26 \
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
    direct="/tmp/rk_mpp_osd_${name}_direct.stream"
    fallback="/tmp/rk_mpp_osd_${name}_fallback.stream"
    run_encoder "$lib" "" "$vendor" "$type" "$decoder" 0
    run_encoder "$overlay" "$overlay/librockchip_mpp.so.1" \
        "$direct" "$type" "$decoder" direct
    run_encoder "$overlay" "$overlay/librockchip_mpp.so.1" \
        "$fallback" "$type" "$decoder" fallback
    python3 "$repo/scripts/compare_streams.py" "$vendor" "$direct"
    python3 "$repo/scripts/compare_streams.py" "$direct" "$fallback"
    cmp "$vendor.yuv" "$direct.yuv"
    cmp "$direct.yuv" "$fallback.yuv"
    printf 'PASS %s OSD direct/fallback stream+reconstruction (%s frames, %s)\n' \
        "$name" "$frames" "$(sha256sum "$direct.yuv" | cut -d' ' -f1)"
done
