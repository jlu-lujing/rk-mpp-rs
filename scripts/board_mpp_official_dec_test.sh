#!/usr/bin/env bash
# Run the vendor mpi_dec_test binary unchanged while LD_PRELOAD switches the
# public MPP entry points and object API to rk-mpp-rs. Vendor helper symbols
# (log, MppDecCfg, OSAL) intentionally remain supplied by librockchip_mpp.
set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp_ref="${MPP_REF:-/tmp/mpp-develop}"
dec_test="${DEC_TEST:-$mpp_ref/build/test/mpi_dec_test}"
mpp_lib="${MPP_LIB:-$mpp_ref/build/mpp}"
preload="$repo/target/release/librk_mpp_rs.so"
h264="${H264_STREAM:-/tmp/rk_mpp_official_h264.264}"
h265="$repo/tests/data/h265e_c_fixqp_qp26_320x240.265"

if [ ! -x "$dec_test" ] || [ ! -f "$mpp_lib/librockchip_mpp.so.0" ]; then
    printf 'missing vendor mpi_dec_test under %s\n' "$mpp_ref" >&2
    exit 2
fi

cargo build --release --offline --lib

run_case() {
    local name="$1"
    local input="$2"
    local coding="$3"
    local frames="$4"
    local size="$5"
    local expected_sha="$6"
    local output="/tmp/rk_mpp_official_${name}.yuv"
    local log="/tmp/rk_mpp_official_${name}.log"

    rm -f "$output" "$log"
    LD_PRELOAD="$preload" LD_LIBRARY_PATH="$mpp_lib:${LD_LIBRARY_PATH:-}" \
        timeout 60 "$dec_test" -i "$input" -o "$output" \
        -w 320 -h 240 -t "$coding" -f 0 -n "$frames" -v q \
        < /dev/null > "$log" 2>&1
    test "$(stat -c %s "$output")" -eq "$size"
    local actual
    actual="$(sha256sum "$output" | cut -d' ' -f1)"
    if [ "$actual" != "$expected_sha" ]; then
        printf '%s output SHA mismatch: got %s expected %s\n' \
            "$name" "$actual" "$expected_sha" >&2
        tail -40 "$log" >&2
        exit 1
    fi
    printf 'PASS %s through unchanged vendor mpi_dec_test (%s)\n' \
        "$name" "$actual"
}

if command -v ffmpeg >/dev/null 2>&1; then
    ffmpeg -nostdin -y -hide_banner -loglevel error \
        -f lavfi -i "testsrc2=size=640x480:rate=30" -frames:v 6 \
        -pix_fmt yuv420p -c:v libx264 -preset veryfast \
        -x264-params "keyint=30:min-keyint=30:bframes=3:scenecut=0:repeat-headers=1" \
        -f h264 "$h264"
    h264_golden=/tmp/rk_mpp_official_h264_c.yuv
    h264_ours=/tmp/rk_mpp_official_h264_ours.yuv
    LD_LIBRARY_PATH="$mpp_lib:${LD_LIBRARY_PATH:-}" \
        timeout 60 "$dec_test" -i "$h264" -o "$h264_golden" \
        -w 640 -h 480 -t 7 -f 0 -n 6 -v q \
        < /dev/null > /tmp/rk_mpp_official_h264_c.log 2>&1
    LD_PRELOAD="$preload" LD_LIBRARY_PATH="$mpp_lib:${LD_LIBRARY_PATH:-}" \
        timeout 60 "$dec_test" -i "$h264" -o "$h264_ours" \
        -w 640 -h 480 -t 7 -f 0 -n 6 -v q \
        < /dev/null > /tmp/rk_mpp_official_h264_ours.log 2>&1
    test "$(stat -c %s "$h264_golden")" -eq 2764800
    cmp "$h264_golden" "$h264_ours"
    printf 'PASS h264 through unchanged vendor mpi_dec_test (%s)\n' \
        "$(sha256sum "$h264_ours" | cut -d' ' -f1)"
else
    source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
        "official MPI decoder" "ffmpeg missing"
fi
run_case h265 "$h265" 16777220 1 115200 \
    fcedb435aa69996d93a2bd87b2c5ffb5f22ccc48d86ea7fe3d935d0311d805e0

mjpeg=/tmp/rk_mpp_official_mjpeg.jpg
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=1" -frames:v 1 \
    -pix_fmt yuvj420p -strict unofficial -c:v mjpeg -q:v 3 "$mjpeg"
run_case mjpeg "$mjpeg" 8 1 115200 \
    98c5f9ccd1c19b02e0d16c434aeaef7e7fbca3d6e3803c7a43cc084162a689f8

# MJPEG uses the official advanced loop, which requests a preallocated output
# frame through MppMeta. The compatibility layer must return that same frame.
if grep -q 'dec_advanced mismatch frame' \
    /tmp/rk_mpp_official_mjpeg.log; then
    printf 'FAIL advanced MJPEG output frame identity changed\n' >&2
    exit 1
fi
