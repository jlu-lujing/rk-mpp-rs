#!/usr/bin/env bash

# RK3576 VDPU383 H.265 bring-up gate. Functional test only; the Rust side is
# built without LTO and no throughput claim is made.
#
# Usage:
#   BOARD=luck2 scripts/board_rk3576_h265_vs_mpp.sh

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
    ssh "$board" "tar -xf - -C \$HOME/rk-mpp-rs"

ssh "$board" 'bash -s' <<'REMOTE'
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
cd "$HOME/rk-mpp-rs"
cargo build
cp scripts/p010_to_planar.py /tmp/p010_to_planar.py

check_h265_case() {
    local name="$1"
    local width="$2"
    local height="$3"
    local frames="$4"
    local depth="$5"
    local params="$6"
    local input="/tmp/rk3576_h265_${name}.265"
    local c_raw="/tmp/rk3576_h265_${name}_c.raw"
    local c_yuv="/tmp/rk3576_h265_${name}_c.yuv"
    local rust_yuv="/tmp/rk3576_h265_${name}_rust.yuv"

    local pix_fmt=()
    if [ "$depth" = 10 ]; then
        pix_fmt=(-pix_fmt yuv420p10le)
    fi
    ffmpeg -nostdin -y -hide_banner -loglevel error \
        -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
        -frames:v "$frames" "${pix_fmt[@]}" -c:v libx265 -preset veryfast \
        -x265-params "$params" -f hevc "$input"

    rm -f "$c_raw" "$c_yuv" "$rust_yuv"
    LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
        /usr/local/bin/mpi_dec_test \
        -i "$input" -t 16777220 -n "$frames" -o "$c_raw" -v q < /dev/null
    if [ "$depth" = 10 ]; then
        python3 /tmp/p010_to_planar.py "$c_raw" "$c_yuv" "$width" "$height" < /dev/null
    else
        ffmpeg -nostdin -hide_banner -loglevel error -y \
            -f rawvideo -pixel_format nv12 -video_size "${width}x${height}" \
            -i "$c_raw" -pix_fmt yuv420p -f rawvideo "$c_yuv"
    fi
    timeout 180 ./target/debug/rk-mpp-rs hevcdec "$input" "$rust_yuv" < /dev/null
    cmp "$c_yuv" "$rust_yuv"
    printf 'PASS %-16s %sx%s %s frames depth=%s\n' \
        "$name" "$width" "$height" "$frames" "$depth"
}

check_h265_case base320 320 240 1 8 \
    'keyint=30:min-keyint=30:bframes=0:scenecut=0:repeat-headers=1:log-level=warning'
check_h265_case ippp640 640 480 6 8 \
    'keyint=30:min-keyint=30:bframes=0:scenecut=0:repeat-headers=1:log-level=warning'
check_h265_case bframes640 640 480 6 8 \
    'keyint=30:min-keyint=30:bframes=3:b-pyramid=1:scenecut=0:repeat-headers=1:log-level=warning'
check_h265_case main10 320 240 6 10 \
    'keyint=30:min-keyint=30:bframes=3:b-pyramid=1:scenecut=0:repeat-headers=1:log-level=warning'
check_h265_case tiles 640 360 6 8 \
    'keyint=30:min-keyint=30:bframes=0:tiles=2x2:wpp=0:scenecut=0:repeat-headers=1:log-level=warning'
check_h265_case wpp 640 360 6 8 \
    'pools=4:keyint=30:min-keyint=30:bframes=0:wpp=1:scenecut=0:repeat-headers=1:log-level=warning'
ffmpeg -nostdin -hide_banner -loglevel trace \
    -i /tmp/rk3576_h265_wpp.265 -c copy -bsf:v trace_headers -f null - 2>&1 |
    grep -c 'entropy_coding_sync_enabled_flag[[:space:]]*1 = 1' >/dev/null
check_h265_case hd720 1280 720 6 8 \
    'pools=4:keyint=30:min-keyint=30:bframes=0:wpp=1:scenecut=0:repeat-headers=1:log-level=warning'
ffmpeg -nostdin -hide_banner -loglevel trace \
    -i /tmp/rk3576_h265_hd720.265 -c copy -bsf:v trace_headers -f null - 2>&1 |
    grep -c 'entropy_coding_sync_enabled_flag[[:space:]]*1 = 1' >/dev/null

printf 'PASS RK3576 VDPU383 H.265 matrix\n'
REMOTE
