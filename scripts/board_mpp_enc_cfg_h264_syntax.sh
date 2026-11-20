#!/usr/bin/env bash

# Verify MppEncCfg h264:cabac_en / trans8x8 / cabac_idc overrides reach the typed encoder.

set -euo pipefail

board="${BOARD:-t6}"
mpp="${MPP_REF:-/tmp/mpp-old}"
repo="$(cd "$(dirname "$0")/.." && pwd)"

if ! ssh "$board" "test -f $mpp/inc/rk_venc_cfg.h &&
    test -f $mpp/utils/mpp_enc_roi_utils.c"; then
    source "$repo/scripts/board_dependency.sh" \
        "MppEncCfg H.264 syntax" "vendor headers or ROI utility missing at $mpp"
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

overlay="$(mktemp -d /tmp/rk_mpp_enc_cfg_h264_syntax.XXXXXX)"
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
    "$MPP_REF/build/mpp/librockchip_mpp.so.1" \
    -o /tmp/rk_mpp_h264e_syntax_vendor
cc "${common[@]}" "$MPP_REF/utils/mpp_enc_roi_utils.c" tools/h264e_probe.c \
    -L"$overlay" -lrockchip_mpp -Wl,-rpath,"$overlay" \
    -o /tmp/rk_mpp_h264e_syntax_rust

ffmpeg -nostdin -v error -y \
    -f lavfi -i testsrc=size=320x240:rate=30 -frames:v 1 \
    -pix_fmt nv12 -f rawvideo /tmp/rk_mpp_enc_cfg_h264_syntax.nv12

run_case() {
    local name="$1" profile="$2" cabac="$3" transform="$4"
    local vendor=/tmp/rk_mpp_h264_syntax_${name}_vendor.264
    local rust=/tmp/rk_mpp_h264_syntax_${name}_rust.264
    sudo -n env LD_LIBRARY_PATH="$MPP_REF/build/mpp" \
        H264_PROFILE="$profile" H264_CABAC="$cabac" H264_TRANS8X8="$transform" \
        /tmp/rk_mpp_h264e_syntax_vendor \
        /tmp/rk_mpp_enc_cfg_h264_syntax.nv12 "$vendor" \
        320 240 1 0 26 288000:1500000:2500000 >/dev/null 2>&1
    sudo -n env LD_LIBRARY_PATH="$overlay" \
        H264_PROFILE="$profile" H264_CABAC="$cabac" H264_TRANS8X8="$transform" \
        /tmp/rk_mpp_h264e_syntax_rust \
        /tmp/rk_mpp_enc_cfg_h264_syntax.nv12 "$rust" \
        320 240 1 0 26 288000:1500000:2500000 >/dev/null 2>&1
    python3 scripts/compare_streams.py "$vendor" "$rust"
    printf 'PASS H.264 MppEncCfg %s -> typed syntax (%s)\n' \
        "$name" "$(sha256sum "$rust" | cut -d' ' -f1)"
}

run_case main_cavlc_no8x8 77 0 0
run_case high_cabac_no8x8 100 1 0

ffmpeg -nostdin -v error -y \
    -f lavfi -i testsrc2=size=320x240:rate=30 -frames:v 2 \
    -pix_fmt nv12 -f rawvideo /tmp/rk_mpp_enc_cfg_h264_cabac_idc.nv12

run_cabac_idc() {
    local idc="$1"
    local vendor=/tmp/rk_mpp_h264_cabac_idc_${idc}_vendor.264
    local rust=/tmp/rk_mpp_h264_cabac_idc_${idc}_rust.264
    local cli=/tmp/rk_mpp_h264_cabac_idc_${idc}_cli.264
    sudo -n env LD_LIBRARY_PATH="$MPP_REF/build/mpp" \
        H264_PROFILE=77 H264_CABAC=1 H264_TRANS8X8=0 H264_CABAC_IDC="$idc" \
        /tmp/rk_mpp_h264e_syntax_vendor \
        /tmp/rk_mpp_enc_cfg_h264_cabac_idc.nv12 "$vendor" \
        320 240 2 0 26 288000:1500000:2500000 >/dev/null 2>&1
    sudo -n env LD_LIBRARY_PATH="$overlay" \
        H264_PROFILE=77 H264_CABAC=1 H264_TRANS8X8=0 H264_CABAC_IDC="$idc" \
        /tmp/rk_mpp_h264e_syntax_rust \
        /tmp/rk_mpp_enc_cfg_h264_cabac_idc.nv12 "$rust" \
        320 240 2 0 26 288000:1500000:2500000 >/dev/null 2>&1
    sudo -n env H264_GOP=60 ./target/release/rk-mpp-rs h264-enc \
        /tmp/rk_mpp_enc_cfg_h264_cabac_idc.nv12 "$cli" \
        320 240 26 main 2 60 "cabac_idc:$idc" >/dev/null
    python3 scripts/compare_streams.py "$vendor" "$rust"
    python3 scripts/compare_streams.py "$rust" "$cli"
    printf 'PASS H.264 MppEncCfg cabac_idc=%s -> typed syntax (%s)\n' \
        "$idc" "$(sha256sum "$rust" | cut -d' ' -f1)"
}

run_cabac_idc 1
run_cabac_idc 2

run_constraint_set() {
    local raw="$1"
    local name="${raw#0x}"
    local vendor=/tmp/rk_mpp_h264_constraint_${name}_vendor.264
    local rust=/tmp/rk_mpp_h264_constraint_${name}_rust.264
    local cli=/tmp/rk_mpp_h264_constraint_${name}_cli.264
    sudo -n env LD_LIBRARY_PATH="$MPP_REF/build/mpp" \
        H264_PROFILE=77 H264_CABAC=1 H264_TRANS8X8=0 H264_CONSTRAINT_SET="$raw" \
        /tmp/rk_mpp_h264e_syntax_vendor \
        /tmp/rk_mpp_enc_cfg_h264_syntax.nv12 "$vendor" \
        320 240 1 0 26 288000:1500000:2500000 >/dev/null 2>&1
    sudo -n env LD_LIBRARY_PATH="$overlay" \
        H264_PROFILE=77 H264_CABAC=1 H264_TRANS8X8=0 H264_CONSTRAINT_SET="$raw" \
        /tmp/rk_mpp_h264e_syntax_rust \
        /tmp/rk_mpp_enc_cfg_h264_syntax.nv12 "$rust" \
        320 240 1 0 26 288000:1500000:2500000 >/dev/null 2>&1
    sudo -n env H264_GOP=60 ./target/release/rk-mpp-rs h264-enc \
        /tmp/rk_mpp_enc_cfg_h264_syntax.nv12 "$cli" \
        320 240 26 main 1 60 "constraint_set:$raw" >/dev/null
    python3 scripts/compare_streams.py "$vendor" "$rust"
    python3 scripts/compare_streams.py "$rust" "$cli"
    printf 'PASS H.264 MppEncCfg constraint_set=%s -> typed syntax (%s)\n' \
        "$raw" "$(sha256sum "$rust" | cut -d' ' -f1)"
}

run_constraint_set 0x020000
run_constraint_set 0x010001
REMOTE
