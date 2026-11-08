#!/usr/bin/env bash

# Deterministic H.265 corruption matrix on RK3576 VDPU383.

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
cargo build

base=/tmp/rk3576_h265_corrupt_base.265
case_dir=/tmp/rk3576_h265_corrupt_cases
rm -rf "$case_dir"
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=640x480:rate=30" -frames:v 6 \
    -pix_fmt yuv420p -c:v libx265 -preset veryfast \
    -x265-params "keyint=30:min-keyint=30:bframes=0:scenecut=0:repeat-headers=1:log-level=warning" \
    -f hevc "$base"
python3 scripts/make_hevc_corrupt_matrix.py "$base" "$case_dir"

passed=0
c_timeouts=0
for input in "$case_dir"/*.265; do
    name="$(basename "$input" .265)"
    c_raw="/tmp/rk3576_h265_corrupt_${name}_c.raw"
    c_yuv="/tmp/rk3576_h265_corrupt_${name}_c.yuv"
    rust_yuv="/tmp/rk3576_h265_corrupt_${name}_rust.yuv"
    c_log="/tmp/rk3576_h265_corrupt_${name}_c.log"
    rust_log="/tmp/rk3576_h265_corrupt_${name}_rust.log"
    rm -f "$c_raw" "$c_yuv" "$rust_yuv" "$c_log" "$rust_log"

    expect_c_timeout=0
    c_timeout=30
    case "$name" in
        sps_mid|vcl1_start)
            expect_c_timeout=1
            c_timeout=8
            ;;
    esac

    set +e
    LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
        timeout "$c_timeout" /usr/local/bin/mpi_dec_test \
        -i "$input" -t 16777220 -n 6 -o "$c_raw" -v q \
        < /dev/null > "$c_log" 2>&1
    c_rc=$?
    timeout 30 ./target/debug/rk-mpp-rs hevcdec "$input" "$rust_yuv" \
        < /dev/null > "$rust_log" 2>&1
    rust_rc=$?
    set -e

    c_bytes="$(stat -c %s "$c_raw" 2>/dev/null || echo 0)"
    rust_bytes="$(stat -c %s "$rust_yuv" 2>/dev/null || echo 0)"

    if [ "$expect_c_timeout" -eq 1 ]; then
        test "$c_rc" -eq 124
        test "$rust_rc" -ne 124
        test "$c_bytes" -eq 0
        test "$rust_bytes" -eq 0
        c_timeouts=$((c_timeouts + 1))
        printf 'PASS %-14s C timeout, Rust rc=%s empty\n' "$name" "$rust_rc"
    else
        test "$c_rc" -eq 0
        test "$rust_rc" -eq 0
        test "$c_bytes" -eq "$rust_bytes"
        if [ "$c_bytes" -ne 0 ]; then
            test $((c_bytes % 460800)) -eq 0
            ffmpeg -nostdin -hide_banner -loglevel error -y \
                -f rawvideo -pixel_format nv12 -video_size 640x480 \
                -i "$c_raw" -pix_fmt yuv420p -f rawvideo "$c_yuv"
            cmp "$c_yuv" "$rust_yuv"
        fi
        printf 'PASS %-14s %s frames\n' "$name" "$((c_bytes / 460800))"
    fi
    passed=$((passed + 1))
done

printf 'PASS RK3576 VDPU383 H.265 corruption matrix (%s cases, %s C timeouts)\n' \
    "$passed" "$c_timeouts"
REMOTE
