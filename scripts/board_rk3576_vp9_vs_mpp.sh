#!/usr/bin/env bash

# RK3576 VDPU383 VP9 matrix against the installed C MPP. This is a functional
# gate; the Rust binary is intentionally a debug build so a slow board LLVM
# LTO host cannot hide a decoder result.

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
cp scripts/p010_to_planar.py /tmp/p010_to_planar.py

check_case() {
    local name="$1"
    local width="$2"
    local height="$3"
    local depth="$4"
    local input="tests/data/${name}.ivf"
    local rust="/tmp/vp9_${name}_rust.yuv"
    local c_raw="/tmp/vp9_${name}_c.nv12"
    local c_planar="/tmp/vp9_${name}_c.yuv"
    local libvpx="/tmp/vp9_${name}_libvpx.yuv"
    local sample_bytes=1
    local frame_bytes
    local rust_bytes
    local frames
    local c_bytes
    local c_frames

    if [ "$depth" = 10 ]; then
        sample_bytes=2
    fi
    frame_bytes=$(( (width * height + 2 * ((width + 1) / 2) * ((height + 1) / 2)) * sample_bytes ))

    rm -f "$rust" "$c_raw" "$c_planar" "$libvpx"
    timeout 300 ./target/debug/rk-mpp-rs vp9dec "$input" "$rust" \
        > "/tmp/vp9_${name}_rust.log" 2>&1
    rust_bytes=$(stat -c %s "$rust" 2>/dev/null || echo 0)
    frames=$((rust_bytes / frame_bytes))
    if [ "$frames" -le 0 ]; then
        printf 'FAIL %-28s Rust produced no complete frame\n' "$name"
        return 1
    fi

    LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
        /usr/local/bin/mpi_dec_test \
        -i "$input" -t 10 -o "$c_raw" -n "$frames" -v q < /dev/null \
        > "/tmp/vp9_${name}_c.log" 2>&1
    if [ "$depth" = 10 ]; then
        python3 /tmp/p010_to_planar.py "$c_raw" "$c_planar" "$width" "$height"
    else
        ffmpeg -nostdin -hide_banner -loglevel error -y \
            -f rawvideo -pixel_format nv12 -video_size "${width}x${height}" \
            -i "$c_raw" -pix_fmt yuv420p -f rawvideo "$c_planar"
    fi
    c_bytes=$(stat -c %s "$c_planar")
    c_frames=$((c_bytes / frame_bytes))

    case "$name" in
        vp9_p0_lossless_321x241_2)
            ffmpeg -nostdin -hide_banner -loglevel error -y -i "$input" \
                -pix_fmt yuv420p -f rawvideo "$libvpx"
            cmp "$libvpx" "$rust"
            printf 'PASS %-28s %sx%s %s frames (libvpx; known C frame-count bug)\n' \
                "$name" "$width" "$height" "$frames"
            return
            ;;
        vp9_p0_720p_errres_seg)
            ffmpeg -nostdin -hide_banner -loglevel error -y -i "$input" \
                -pix_fmt yuv420p -f rawvideo "$libvpx"
            cmp "$libvpx" "$rust"
            printf 'PASS %-28s %sx%s %s frames (libvpx; known C byte bug)\n' \
                "$name" "$width" "$height" "$frames"
            return
            ;;
    esac

    if [ "$c_frames" -ne "$frames" ]; then
        printf 'FAIL %-28s C frames=%s Rust frames=%s\n' \
            "$name" "$c_frames" "$frames"
        return 1
    fi
    cmp "$c_planar" "$rust"
    printf 'PASS %-28s %sx%s %s frames depth=%s\n' \
        "$name" "$width" "$height" "$frames" "$depth"
}

check_case vp9_p0_64x64 64 64 8
check_case vp9_p2_10bit_64x64 64 64 10
check_case vp9_p0_lossless_321x241_2 321 241 8
check_case vp9_p0_lossless_320x240_12 320 240 8
check_case vp9_p2_10bit_lossless_320x240_12 320 240 10
check_case vp9_p0_errres_tiles_640x360_12 640 360 8
check_case vp9_p0_720p_errres_seg 1280 720 8

printf 'PASS RK3576 VDPU383 VP9 matrix\n'
REMOTE
