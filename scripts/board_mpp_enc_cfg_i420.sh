#!/usr/bin/env bash

# Verify MppEncCfg prep:format=MPP_FMT_YUV420P reaches the typed I420 path.

set -euo pipefail

board="${BOARD:-t6}"
mpp="${MPP_REF:-/tmp/mpp-old}"
repo="$(cd "$(dirname "$0")/.." && pwd)"

if ! ssh "$board" "test -f $mpp/inc/rk_venc_cfg.h"; then
    source "$repo/scripts/board_dependency.sh" \
        "MppEncCfg I420 input" "vendor headers missing at $mpp"
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

ssh "$board" "MPP_REF='$mpp' bash -s" <<'REMOTE'
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
cd "$HOME/rk-mpp-rs"
cargo build --release --lib --bin rk-mpp-rs >/dev/null

overlay="$(mktemp -d /tmp/rk_mpp_enc_cfg_i420.XXXXXX)"
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
cc -O2 -w -pthread \
    -DCUSTOM_RC_HAS_BIT_TARGET_FIX="$rc_has_bit_target_fix" \
    -I"$MPP_REF/inc" -I"$MPP_REF/utils" -I"$MPP_REF/osal/inc" \
    "$MPP_REF/utils/mpp_enc_roi_utils.c" tools/h264e_probe.c \
    -L"$overlay" -lrockchip_mpp -Wl,-rpath,"$overlay" \
    -o /tmp/rk_mpp_h264_i420_probe

ffmpeg -nostdin -v error -y -f lavfi \
    -i testsrc=size=320x240:rate=30 -frames:v 1 \
    -pix_fmt nv12 -f rawvideo /tmp/rk_mpp_i420_nv12.yuv
ffmpeg -nostdin -v error -y -f lavfi \
    -i testsrc=size=320x240:rate=30 -frames:v 1 \
    -pix_fmt yuv420p -f rawvideo /tmp/rk_mpp_i420_planar.yuv

sudo -n env LD_LIBRARY_PATH="$overlay" MPP_INPUT_FORMAT=0 \
    /tmp/rk_mpp_h264_i420_probe /tmp/rk_mpp_i420_nv12.yuv \
    /tmp/rk_mpp_i420_nv12.264 320 240 1 0 26 288000:1500000:2500000 \
    >/tmp/rk_mpp_i420_nv12.log 2>&1
sudo -n env LD_LIBRARY_PATH="$overlay" MPP_INPUT_FORMAT=4 \
    /tmp/rk_mpp_h264_i420_probe /tmp/rk_mpp_i420_planar.yuv \
    /tmp/rk_mpp_i420_planar.264 320 240 1 0 26 288000:1500000:2500000 \
    >/tmp/rk_mpp_i420_planar.log 2>&1
python3 scripts/compare_streams.py \
    /tmp/rk_mpp_i420_nv12.264 /tmp/rk_mpp_i420_planar.264
printf 'PASS H.264 MppEncCfg NV12/I420 produce identical stream (%s)\n' \
    "$(sha256sum /tmp/rk_mpp_i420_planar.264 | cut -d' ' -f1)"
REMOTE
