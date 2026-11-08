#!/usr/bin/env bash

# RK3576 VPU720 baseline JPEG encoder gate. The C probe uses the installed
# MPP library and the same quality factor as the Rust CLI.

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

cc -O2 -w -pthread -I/usr/local/include/rockchip \
    tools/jpege_probe.c -L/usr/local/lib -lrockchip_mpp \
    -o /tmp/jpege_probe720

check_case() {
    local width="$1"
    local height="$2"
    local split="${3:-0}"
    local input="${4:-nv12}"
    local tag="${width}x${height}_${input}"
    local src="/tmp/vpu720_${tag}.raw"
    local suffix=""
    [ "$split" -eq 0 ] || suffix="_rst${split}"
    local c_out="/tmp/vpu720_${tag}${suffix}_c.jpg"
    local rust_out="/tmp/vpu720_${tag}${suffix}_rust.jpg"
    local c_log="/tmp/vpu720_${tag}${suffix}_c.log"
    local rust_log="/tmp/vpu720_${tag}${suffix}_rust.log"
    local pix_fmt mpp_fmt

    case "$input" in
        nv12)
            pix_fmt=nv12
            mpp_fmt=0
            ;;
        yuyv)
            pix_fmt=yuyv422
            mpp_fmt=8
            ;;
        uyvy)
            pix_fmt=uyvy422
            mpp_fmt=10
            ;;
        *)
            printf 'unsupported input format: %s\n' "$input" >&2
            return 2
            ;;
    esac

    ffmpeg -nostdin -hide_banner -loglevel error -y \
        -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
        -frames:v 1 -pix_fmt "$pix_fmt" -f rawvideo "$src"
    rm -f "$c_out" "$rust_out" "$c_log" "$rust_log"
    LD_LIBRARY_PATH=/usr/local/lib timeout 120 /tmp/jpege_probe720 \
        "$src" "$c_out" "$width" "$height" 50 "$split" 0 "$mpp_fmt" \
        >"$c_log" 2>&1
    local rust_extra=()
    [ "$split" -eq 0 ] || rust_extra=("rst${split}")
    timeout 120 ./target/debug/rk-mpp-rs jpeg-enc \
        "$src" "$rust_out" "$width" "$height" qf50 420 "$input" \
        "${rust_extra[@]}" >"$rust_log" 2>&1
    cmp "$c_out" "$rust_out"
    printf 'PASS %-12s split=%-3s %s bytes\n' \
        "$tag" "$split" "$(stat -c %s "$c_out")"
}

check_case 320 240 0 nv12
check_case 640 480 0 nv12
check_case 1280 720 0 nv12
check_case 320 240 60 nv12
check_case 320 240 0 yuyv
check_case 320 240 0 uyvy
printf 'PASS RK3576 VPU720 JPEG baseline matrix\n'
REMOTE
