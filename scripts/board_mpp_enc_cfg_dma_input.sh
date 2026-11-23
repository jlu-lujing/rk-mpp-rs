#!/usr/bin/env bash

# Verify internal dma-heap input is submitted directly and CPU staging remains
# the exact fallback.

set -euo pipefail

board="${BOARD:?set BOARD to a Rockchip SSH alias}"
mpp="${MPP_REF:-/tmp/mpp-old}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
frames="${FRAMES:-120}"

if ! ssh "$board" "test -f $mpp/inc/rk_venc_cfg.h &&
    test -f $mpp/utils/mpp_enc_roi_utils.c"; then
    source "$repo/scripts/board_dependency.sh" \
        "MppEncCfg DMA input" "vendor headers or ROI utility missing at $mpp"
fi

ssh "$board" 'mkdir -p "$HOME/rk-mpp-rs"'
(
    cd "$repo"
    COPYFILE_DISABLE=1 tar \
        --no-xattrs \
        --exclude .git \
        --exclude target \
        --exclude .worktrees \
        --exclude refs \
        -cf - .
) | ssh "$board" 'tar -xf - --strip-components=1 -C "$HOME/rk-mpp-rs"'

ssh "$board" "MPP_REF='$mpp' FRAMES='$frames' bash -s" <<'REMOTE'
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
cd "$HOME/rk-mpp-rs"
cargo build --release --lib --bin rk-mpp-rs >/dev/null

overlay="$(mktemp -d /tmp/rk_mpp_enc_cfg_dma_input.XXXXXX)"
trap 'rm -rf "$overlay"' EXIT
for name in \
    librockchip_mpp.so \
    librockchip_mpp.so.1 \
    librk_mpp_rs.so \
    librk_mpp_rs.so.1; do
    ln -s "$PWD/target/release/librk_mpp_rs.so" "$overlay/$name"
done

if grep -q bit_target_fix "$MPP_REF/inc/mpp_rc_defs.h"; then
    rc_has_bit_target_fix=1
else
    rc_has_bit_target_fix=0
fi
common=(-O2 -w -pthread
    -DCUSTOM_RC_HAS_BIT_TARGET_FIX="$rc_has_bit_target_fix"
    -I"$MPP_REF/inc"
    -I"$MPP_REF/utils"
    -I"$MPP_REF/osal/inc")
cc "${common[@]}" "$MPP_REF/utils/mpp_enc_roi_utils.c" tools/h264e_probe.c \
    -L"$overlay" -lrockchip_mpp -Wl,-rpath,"$overlay" \
    -o /tmp/rk_mpp_h264e_input_dma
cc "${common[@]}" "$MPP_REF/utils/mpp_enc_roi_utils.c" tools/h265e_probe.c \
    -L"$overlay" -lrockchip_mpp -Wl,-rpath,"$overlay" \
    -o /tmp/rk_mpp_h265e_input_dma

fixture=/tmp/rk_mpp_enc_cfg_dma_input_720p.nv12
need=$((1280 * 720 * 3 / 2 * FRAMES))
if ! test -s "$fixture" || test "$(stat -c %s "$fixture")" -lt "$need"; then
    ffmpeg -nostdin -v error -y \
        -f lavfi -i testsrc2=size=1280x720:rate=30 -frames:v "$FRAMES" \
        -pix_fmt nv12 -f rawvideo "$fixture"
fi

run_h264_probe() {
    local output="$1"
    shift
    sudo -n env LD_LIBRARY_PATH="$overlay" GOP=60 "$@" \
        /tmp/rk_mpp_h264e_input_dma "$fixture" "$output" \
        1280 720 "$FRAMES" 0 26 3000000:2500000:3500000 \
        >/dev/null 2>&1
}

run_h265_probe() {
    local output="$1"
    shift
    sudo -n env LD_LIBRARY_PATH="$overlay" GOP=60 FRAMES="$FRAMES" "$@" \
        /tmp/rk_mpp_h265e_input_dma "$fixture" "$output" \
        1280 720 0 26 3000000:2500000:3500000 \
        >/dev/null 2>&1
}

run_h264_probe /tmp/rk_mpp_input_dma.264
run_h264_probe /tmp/rk_mpp_input_cpu.264 CPU_BUFFER=1
python3 scripts/compare_streams.py \
    /tmp/rk_mpp_input_dma.264 /tmp/rk_mpp_input_cpu.264
printf 'PASS H.264 internal dma-heap input matches CPU fallback (%s frames, %s)\n' \
    "$FRAMES" "$(sha256sum /tmp/rk_mpp_input_dma.264 | cut -d' ' -f1)"

run_h265_probe /tmp/rk_mpp_input_dma.265
run_h265_probe /tmp/rk_mpp_input_cpu.265 CPU_BUFFER=1
python3 scripts/compare_streams.py \
    /tmp/rk_mpp_input_dma.265 /tmp/rk_mpp_input_cpu.265
printf 'PASS H.265 internal dma-heap input matches CPU fallback (%s frames, %s)\n' \
    "$FRAMES" "$(sha256sum /tmp/rk_mpp_input_dma.265 | cut -d' ' -f1)"
REMOTE
