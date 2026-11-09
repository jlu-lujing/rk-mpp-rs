#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp_ref="${MPP_REF:-$HOME/rk-mpp-rs/refs/mpp}"
stream="${H264_STREAM:-/tmp/h264_multi_idr.264}"
output="${H264_OUTPUT:-/tmp/rk_mpp_compat_data.yuv}"

if [ ! -f "$mpp_ref/inc/rk_mpi.h" ]; then
    printf 'missing official MPP headers under %s\n' "$mpp_ref" >&2
    exit 2
fi
cargo build --release --offline --lib
libdir=$(mktemp -d)
trap 'rm -rf "$libdir"' EXIT
ln -sfn "$repo/target/release/librk_mpp_rs.so" "$libdir/librk_mpp_rs.so"
ln -sfn "$repo/target/release/librk_mpp_rs.so" "$libdir/librk_mpp_rs.so.1"

gcc -std=c11 -Wall -Wextra -Werror \
    -I"$mpp_ref/inc" "$repo/tests/c_mpp_data_path.c" \
    -L"$libdir" -lrk_mpp_rs \
    -Wl,-rpath,"$libdir" \
    -o /tmp/rk_mpp_official_data
run_case() {
    local name="$1"
    local input="$2"
    local coding="$3"
    local frames="$4"
    local expected_size="$5"
    local expected_sha="$6"
    local case_output="${output%.*}-${name}.yuv"

    if [ ! -s "$input" ]; then
        printf 'SKIP %s (missing %s)\n' "$name" "$input"
        return 0
    fi
    LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
        /tmp/rk_mpp_official_data "$input" "$case_output" "$coding" "$frames"
    test "$(stat -c %s "$case_output")" -eq "$expected_size"
    local actual_sha
    actual_sha="$(sha256sum "$case_output" | cut -d' ' -f1)"
    if [ "$actual_sha" != "$expected_sha" ]; then
        printf 'output SHA mismatch for %s: got %s expected %s\n' \
            "$name" "$actual_sha" "$expected_sha" >&2
        exit 1
    fi
    printf 'PASS %s official data path matches C MPP (%s)\n' \
        "$name" "$actual_sha"
}

run_case h264-multiframe "$stream" 7 15 1728000 \
    22a8a315f31eeb8715c0551a305aa310642d1be3a704af0a098595af3c5523f7
run_case h265-portable "$repo/tests/data/h265e_c_fixqp_qp26_320x240.265" \
    16777220 1 115200 \
    fcedb435aa69996d93a2bd87b2c5ffb5f22ccc48d86ea7fe3d935d0311d805e0

mjpeg_input=/tmp/rk_mpp_compat_mjpeg.jpg
mjpeg_golden=/tmp/rk_mpp_compat_mjpeg_golden.nv12
mjpeg_output=/tmp/rk_mpp_compat_mjpeg_ours.nv12
if command -v ffmpeg >/dev/null 2>&1 &&
    [ -x /usr/local/bin/mpi_dec_test ]; then
    ffmpeg -nostdin -y -hide_banner -loglevel error \
        -f lavfi -i "testsrc2=size=320x240:rate=1" -frames:v 1 \
        -pix_fmt yuvj420p -strict unofficial -c:v mjpeg -q:v 3 \
        "$mjpeg_input"
    rm -f "$mjpeg_golden" "$mjpeg_output"
    if LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
        timeout 60 /usr/local/bin/mpi_dec_test -i "$mjpeg_input" \
        -t 8 -w 320 -h 240 -n 1 -o "$mjpeg_golden" -v q \
        < /dev/null > /tmp/rk_mpp_compat_mjpeg_c.log 2>&1; then
        LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
            /tmp/rk_mpp_official_data "$mjpeg_input" "$mjpeg_output" 8 1
        cmp "$mjpeg_golden" "$mjpeg_output"
        printf 'PASS mjpeg official data path matches C MPP (%s)\n' \
            "$(sha256sum "$mjpeg_output" | cut -d' ' -f1)"
    else
        printf 'SKIP mjpeg (system C decoder oracle failed)\n'
    fi
else
    printf 'SKIP mjpeg (ffmpeg or system C decoder oracle missing)\n'
fi
