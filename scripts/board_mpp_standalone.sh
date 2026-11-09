#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp_root="${MPP_REF:-/tmp/mpp-develop}"
dec_test="${DEC_TEST:-$mpp_root/build/test/mpi_dec_test}"
vendor_lib="${MPP_LIB:-$mpp_root/build/mpp}"
h264="${H264_STREAM:-/tmp/rk_mpp_standalone_h264.264}"
overlay=$(mktemp -d /tmp/rk_mpp_standalone.XXXXXX)
trap 'rm -rf "$overlay"' EXIT

if [ -z "${DEC_TEST:-}" ] && [ ! -x "$dec_test" ] &&
    [ -x /usr/local/bin/mpi_dec_test ]; then
    dec_test=/usr/local/bin/mpi_dec_test
fi
if [ -z "${MPP_LIB:-}" ] && [ ! -f "$vendor_lib/librockchip_mpp.so.1" ] &&
    [ -f /usr/local/lib/librockchip_mpp.so.1 ]; then
    vendor_lib=/usr/local/lib
fi
if [ ! -x "$dec_test" ] || [ ! -f "$vendor_lib/librockchip_mpp.so.1" ]; then
    printf 'missing vendor mpi_dec_test or comparison library under %s\n' \
        "$mpp_root" >&2
    exit 2
fi

cargo build --release --offline --lib
ln -s "$repo/target/release/librk_mpp_rs.so" \
    "$overlay/librockchip_mpp.so.1"

if ! LD_LIBRARY_PATH="$overlay" ldd "$dec_test" |
    grep -F "librockchip_mpp.so.1 => $overlay/librockchip_mpp.so.1" >/dev/null; then
    printf 'standalone library did not replace vendor librockchip_mpp\n' >&2
    exit 1
fi

run_case() {
    local name="$1"
    local input="$2"
    local coding="$3"
    local frames="$4"
    local width="$5"
    local height="$6"
    local golden="/tmp/rk_mpp_standalone_${name}_vendor.yuv"
    local ours="/tmp/rk_mpp_standalone_${name}_ours.yuv"

    sudo -n rm -f "$golden" "$ours"
    sudo -n env LD_LIBRARY_PATH="$vendor_lib:${LD_LIBRARY_PATH:-}" \
        timeout 120 "$dec_test" -i "$input" -o "$golden" \
        -w "$width" -h "$height" -t "$coding" -f 0 -n "$frames" -v q \
        < /dev/null > "/tmp/rk_mpp_standalone_${name}_vendor.log" 2>&1
    sudo -n env LD_LIBRARY_PATH="$overlay" \
        timeout 120 "$dec_test" -i "$input" -o "$ours" \
        -w "$width" -h "$height" -t "$coding" -f 0 -n "$frames" -v q \
        < /dev/null > "/tmp/rk_mpp_standalone_${name}_ours.log" 2>&1
    cmp "$golden" "$ours"
    printf 'PASS %-8s standalone decoder (%s, %s bytes)\n' \
        "$name" "$(sha256sum "$ours" | cut -d' ' -f1)" "$(stat -c %s "$ours")"
}

ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=640x480:rate=30" -frames:v 6 \
    -pix_fmt yuv420p -c:v libx264 -preset veryfast \
    -x264-params "keyint=30:min-keyint=30:bframes=3:scenecut=0:repeat-headers=1" \
    -f h264 "$h264"
run_case h264 "$h264" 7 6 640 480

run_case h265 tests/data/h265e_c_fixqp_qp26_320x240.265 16777220 1 320 240
run_case vp9 tests/data/vp9_p0_lossless_320x240_12.ivf 10 12 320 240
run_case av1 tests/data/av1_320x240_10.ivf 16777224 10 320 240

mjpeg=/tmp/rk_mpp_standalone_mjpeg.jpg
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=1" -frames:v 1 \
    -pix_fmt yuvj420p -strict unofficial -c:v mjpeg -q:v 3 "$mjpeg"
run_case mjpeg "$mjpeg" 8 1 320 240
