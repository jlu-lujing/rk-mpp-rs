#!/usr/bin/env bash

# Verify that MppEncCfg rc:refresh_* fields reach the Rust encoder. The
# unmodified vendor probes set the keys, while the Rust CLI uses the typed
# EncoderConfig::gdr path; both outputs must agree.

set -euo pipefail

board="${BOARD:?set BOARD to a Rockchip SSH alias}"
mpp="${MPP_REF:-/tmp/mpp-old}"
repo="$(cd "$(dirname "$0")/.." && pwd)"

if ! ssh "$board" "test -f $mpp/inc/rk_venc_cfg.h &&
    test -f $mpp/utils/mpp_enc_roi_utils.c"; then
    source "$repo/scripts/board_dependency.sh" \
        "MppEncCfg GDR mapping" "vendor headers or ROI utility missing at $mpp"
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

overlay="$(mktemp -d /tmp/rk_mpp_enc_cfg_gdr.XXXXXX)"
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
    -o /tmp/rk_mpp_h264e_cfg_probe
cc "${common[@]}" "$MPP_REF/utils/mpp_enc_roi_utils.c" tools/h265e_probe.c \
    -L"$overlay" -lrockchip_mpp -Wl,-rpath,"$overlay" \
    -o /tmp/rk_mpp_h265e_cfg_probe

ffmpeg -nostdin -v error -y \
    -f lavfi -i testsrc=size=320x240:rate=30 -frames:v 25 \
    -pix_fmt nv12 -f rawvideo /tmp/rk_mpp_enc_cfg_gdr.nv12

sudo -n ./target/release/rk-mpp-rs h264-enc \
    /tmp/rk_mpp_enc_cfg_gdr.nv12 /tmp/rk_mpp_enc_cfg_gdr_h264_cli.264 \
    320 240 26 baseline 25 12 gdr:row:4 >/dev/null
sudo -n env LD_LIBRARY_PATH="$overlay" /tmp/rk_mpp_h264e_cfg_probe \
    /tmp/rk_mpp_enc_cfg_gdr.nv12 /tmp/rk_mpp_enc_cfg_gdr_h264_obj.264 \
    320 240 25 0 26 288000:1500000:2500000 gop 12 gdr 0 4 \
    >/tmp/rk_mpp_enc_cfg_gdr_h264_obj.log 2>&1
python3 scripts/compare_streams.py \
    /tmp/rk_mpp_enc_cfg_gdr_h264_cli.264 \
    /tmp/rk_mpp_enc_cfg_gdr_h264_obj.264
printf 'PASS H.264 MppEncCfg rc:refresh_* -> GDR (%s)\n' \
    "$(sha256sum /tmp/rk_mpp_enc_cfg_gdr_h264_obj.264 | cut -d' ' -f1)"

sudo -n env H265_GOP=12 ./target/release/rk-mpp-rs h265-gop \
    /tmp/rk_mpp_enc_cfg_gdr.nv12 /tmp/rk_mpp_enc_cfg_gdr_h265_cli.265 \
    320 240 25 26 row:4 >/dev/null
sudo -n env LD_LIBRARY_PATH="$overlay" REF_CFG=1 REFRESH_EN=1 \
    REFRESH_MODE=0 REFRESH_NUM=4 GOP=12 FRAMES=25 \
    /tmp/rk_mpp_h265e_cfg_probe \
    /tmp/rk_mpp_enc_cfg_gdr.nv12 /tmp/rk_mpp_enc_cfg_gdr_h265_obj.265 \
    320 240 0 26 288000:1500000:2500000 \
    >/tmp/rk_mpp_enc_cfg_gdr_h265_obj.log 2>&1
cmp /tmp/rk_mpp_enc_cfg_gdr_h265_cli.265 \
    /tmp/rk_mpp_enc_cfg_gdr_h265_obj.265
printf 'PASS H.265 MppEncCfg rc:refresh_* -> GDR (%s)\n' \
    "$(sha256sum /tmp/rk_mpp_enc_cfg_gdr_h265_obj.265 | cut -d' ' -f1)"
REMOTE
