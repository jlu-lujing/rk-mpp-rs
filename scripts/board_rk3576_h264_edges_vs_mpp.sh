#!/usr/bin/env bash

# RK3576 VDPU383 H.264 edge matrix. The three normal rows use the installed C
# MPP as oracle; PAFF uses FFmpeg because the stock RK3576 C harness produces a
# field-order output that differs from both FFmpeg and the Rust path.

set -euo pipefail

board="${BOARD:?set BOARD to a Rockchip SSH alias}"

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

check_case() {
    local name="$1"
    local width="$2"
    local height="$3"
    local frames="$4"
    local oracle="$5"
    local input="/tmp/${name}"
    local rust="/tmp/${name}_rust.yuv"
    local c_raw="/tmp/${name}_c.nv12"
    local c_planar="/tmp/${name}_c.yuv"
    local reference="/tmp/${name}_reference.yuv"
    local frame_bytes=$(( width * height * 3 / 2 ))
    local expected_bytes=$(( frame_bytes * frames ))

    if [ ! -s "$input" ]; then
        source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
            "RK3576 H.264 edge $name" "input missing at $input"
    fi

    rm -f "$rust" "$c_raw" "$c_planar" "$reference"
    timeout 600 ./target/debug/rk-mpp-rs h264dec "$input" "$rust" \
        > "/tmp/${name}_rust.log" 2>&1
    test "$(stat -c %s "$rust")" -eq "$expected_bytes"

    if [ "$oracle" = "ffmpeg" ]; then
        ffmpeg -nostdin -hide_banner -loglevel error -y -i "$input" \
            -pix_fmt yuv420p -f rawvideo "$reference"
    else
        LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
            /usr/local/bin/mpi_dec_test \
            -i "$input" -t 7 -n "$frames" -o "$c_raw" -w "$width" -h "$height" \
            -v q < /dev/null > "/tmp/${name}_c.log" 2>&1
        ffmpeg -nostdin -hide_banner -loglevel error -y \
            -f rawvideo -pixel_format nv12 -video_size "${width}x${height}" \
            -i "$c_raw" -pix_fmt yuv420p -f rawvideo "$reference"
    fi

    test "$(stat -c %s "$reference")" -eq "$expected_bytes"
    cmp "$reference" "$rust"
    printf 'PASS %-24s %sx%s %s frames oracle=%s\n' \
        "$name" "$width" "$height" "$frames" "$oracle"
}

check_missing_reference_case() {
    local name="mr1_missing_au20.264"
    local width=176
    local height=144
    local decode_frames=61
    local output_frames=22
    local input="/tmp/${name}"
    local rust="/tmp/${name}_rust.yuv"
    local c_raw="/tmp/${name}_c.nv12"
    local c_planar="/tmp/${name}_c.yuv"
    local frame_bytes=$(( width * height * 3 / 2 ))
    local expected_bytes=$(( frame_bytes * output_frames ))

    if [ ! -s "$input" ]; then
        source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
            "RK3576 H.264 edge $name" "input missing at $input"
    fi
    rm -f "$rust" "$c_raw" "$c_planar"
    timeout 600 ./target/debug/rk-mpp-rs h264dec "$input" "$rust" \
        > "/tmp/${name}_rust.log" 2>&1
    test "$(stat -c %s "$rust")" -eq "$expected_bytes"
    LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
        /usr/local/bin/mpi_dec_test \
        -i "$input" -t 7 -n "$decode_frames" -o "$c_raw" \
        -w "$width" -h "$height" -v q < /dev/null \
        > "/tmp/${name}_c.log" 2>&1
    ffmpeg -nostdin -hide_banner -loglevel error -y \
        -f rawvideo -pixel_format nv12 -video_size "${width}x${height}" \
        -i "$c_raw" -pix_fmt yuv420p -f rawvideo "$c_planar"
    test "$(stat -c %s "$c_planar")" -eq "$expected_bytes"
    cmp "$c_planar" "$rust"
    printf 'PASS %-24s %sx%s %s AU / %s output frames oracle=c-mpp\n' \
        "$name" "$width" "$height" "$decode_frames" "$output_frames"
}

check_case test720.264 1280 720 30 c-mpp
check_case test_hard.264 1920 1080 90 c-mpp
check_case field_paff.264 720 480 15 ffmpeg
check_case mr1_longterm.264 176 144 62 c-mpp
check_missing_reference_case

printf 'PASS RK3576 VDPU383 H.264 edge matrix\n'
REMOTE
