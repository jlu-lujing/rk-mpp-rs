#!/usr/bin/env bash

# Verify MppEncCfg h265:* syntax keys reach the typed H.265 encoder.

set -euo pipefail

board="${BOARD:-t6}"
mpp="${MPP_REF:-/tmp/mpp-old}"
repo="$(cd "$(dirname "$0")/.." && pwd)"

if ! ssh "$board" "test -f $mpp/inc/rk_venc_cfg.h &&
    test -f $mpp/utils/mpp_enc_roi_utils.c"; then
    source "$repo/scripts/board_dependency.sh" \
        "MppEncCfg H.265 syntax" "vendor headers or ROI utility missing at $mpp"
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

overlay="$(mktemp -d /tmp/rk_mpp_enc_cfg_h265_syntax.XXXXXX)"
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
cc "${common[@]}" "$MPP_REF/utils/mpp_enc_roi_utils.c" tools/h265e_probe.c \
    -L"$overlay" -lrockchip_mpp -Wl,-rpath,"$overlay" \
    -o /tmp/rk_mpp_h265e_syntax_probe

ffmpeg -nostdin -v error -y \
    -f lavfi -i testsrc2=size=320x240:rate=30 -frames:v 2 \
    -pix_fmt nv12 -f rawvideo /tmp/rk_mpp_enc_cfg_h265_syntax.nv12

run_case() {
    local name="$1"
    local env_args="$2"
    local cli_args="$3"
    local cli=/tmp/rk_mpp_h265_syntax_${name}_cli.265
    local object=/tmp/rk_mpp_h265_syntax_${name}_obj.265

    sudo -n env H265_GOP=60 ./target/release/rk-mpp-rs h265-gop \
        /tmp/rk_mpp_enc_cfg_h265_syntax.nv12 "$cli" \
        320 240 2 26 shortterm $cli_args >/dev/null
    sudo -n env LD_LIBRARY_PATH="$overlay" FRAMES=2 GOP=60 $env_args \
        /tmp/rk_mpp_h265e_syntax_probe \
        /tmp/rk_mpp_enc_cfg_h265_syntax.nv12 "$object" \
        320 240 0 26 288000:1500000:2500000 >/dev/null 2>&1
    cmp "$cli" "$object"
    printf 'PASS H.265 MppEncCfg %s -> typed syntax (%s)\n' \
        "$name" "$(sha256sum "$object" | cut -d' ' -f1)"
}

run_case beta_tc_sao_chroma \
    "DBLK_DISABLE=0 DBLK_BETA=3 DBLK_TC=-2 SAO_LUMA_DISABLE=0 SAO_CHROMA_DISABLE=1 LPF_ACS_SLI=1 LPF_ACS_TILE_DISABLE=1" \
    "dblk:0:3:-2 sao:0:1 lpf:1:1"
run_case sao_off \
    "DBLK_DISABLE=0 SAO_LUMA_DISABLE=1 SAO_CHROMA_DISABLE=1 LPF_ACS_SLI=0 LPF_ACS_TILE_DISABLE=1" \
    "dblk:0:0:0 sao:1:1 lpf:0:1"
REMOTE
