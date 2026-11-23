#!/usr/bin/env bash

# Verify advanced H.264 MppEncCfg RC fields reach the Rust encoder.

set -euo pipefail

board="${BOARD:?set BOARD to a Rockchip SSH alias}"
mpp="${MPP_REF:-/tmp/mpp-old}"
repo="$(cd "$(dirname "$0")/.." && pwd)"

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

scp -q "$repo/tools/rc_probe.c" "$board":/tmp/mpp_rc_fields_probe.c
scp -q "$repo/scripts/strip_version_sei.py" "$board":/tmp/strip_version_sei.py

ssh "$board" "MPP_REF='$mpp' bash -s" <<'REMOTE'
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
cd "$HOME/rk-mpp-rs"
test -f "$MPP_REF/inc/rk_mpi.h"
test -f "$MPP_REF/build/mpp/librockchip_mpp.so.1"
cargo build --release --lib >/dev/null

overlay="$(mktemp -d /tmp/rk_mpp_rc_fields.XXXXXX)"
trap 'rm -rf "$overlay"' EXIT
for name in \
    librockchip_mpp.so \
    librockchip_mpp.so.0 \
    librockchip_mpp.so.1 \
    librk_mpp_rs.so \
    librk_mpp_rs.so.0 \
    librk_mpp_rs.so.1; do
    ln -s "$PWD/target/release/librk_mpp_rs.so" "$overlay/$name"
done

cc -O2 -w -I"$MPP_REF/inc" /tmp/mpp_rc_fields_probe.c \
    "$MPP_REF/build/mpp/librockchip_mpp.so.1" -o /tmp/mpp_rc_fields_probe

raw12=/tmp/rk_mpp_rc_fields_320x240_12.nv12
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 12 \
    -pix_fmt nv12 -f rawvideo "$raw12"

run_case() {
    local label="$1" frames="$2"
    shift 2
    local vendor="/tmp/rk_mpp_${label}_vendor.264"
    local rust="/tmp/rk_mpp_${label}_rust.264"
    local bps_target="${BPS_TARGET:-300000}"
    local bps_min="${BPS_MIN:-250000}"
    local bps_max="${BPS_MAX:-350000}"
    sudo -n rm -f "$vendor" "$rust"
    sudo -n env LD_LIBRARY_PATH="$MPP_REF/build/mpp" "$@" \
        /tmp/mpp_rc_fields_probe "$raw12" "$vendor" 320 240 "$frames" \
        "$bps_target" "$bps_min" "$bps_max" 1 30 0 \
        >"/tmp/${label}_vendor.log" 2>&1
    sudo -n env LD_LIBRARY_PATH="$overlay" "$@" \
        /tmp/mpp_rc_fields_probe "$raw12" "$rust" 320 240 "$frames" \
        "$bps_target" "$bps_min" "$bps_max" 1 30 0 \
        >"/tmp/${label}_rust.log" 2>&1
    sudo -n python3 /tmp/strip_version_sei.py "$vendor" "$vendor.norm"
    sudo -n python3 /tmp/strip_version_sei.py "$rust" "$rust.norm"
    sudo -n cmp -s "$vendor.norm" "$rust.norm"
    sudo -n stat -c '%n %s' "$vendor" "$rust"
    printf 'PASS mpp/%s through MPP_ENC_SET_CFG\n' "$label"
}

run_case debreath 12 DEBREATH_EN=1 DEBREATH_STRENGTH=20 MAX_REENC=0
run_case super-reenc 6 MAX_REENC=1 SUPER_MODE=2 SUPER_I_THD=10000 \
    SUPER_P_THD=1000 RC_PRIORITY=0
BPS_TARGET=10000 BPS_MIN=8333 BPS_MAX=11666 \
    run_case drop-pskip 6 MAX_REENC=1 DROP_MODE=2 DROP_THD=0 DROP_GAP=1
REMOTE
