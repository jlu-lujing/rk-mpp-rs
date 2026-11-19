#!/usr/bin/env bash

# Verify hierarchical QP arrays from MppEncCfg reach the Rust encoder.

set -euo pipefail

board="${BOARD:-t6}"
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

scp -q "$repo/tools/h264e_probe.c" "$board":/tmp/mpp_hier_qp_probe.c
scp -q "$repo/tools/custom_rc_probe.h" "$board":/tmp/custom_rc_probe.h
scp -q "$repo/scripts/strip_version_sei.py" "$board":/tmp/strip_version_sei.py

ssh "$board" "MPP_REF='$mpp' bash -s" <<'REMOTE'
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
cd "$HOME/rk-mpp-rs"
test -f "$MPP_REF/inc/rk_mpi.h"
test -f "$MPP_REF/build/mpp/librockchip_mpp.so.1"
cargo build --release --lib >/dev/null

overlay="$(mktemp -d /tmp/rk_mpp_hier_qp.XXXXXX)"
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

if grep -q 'bit_target_fix' "$MPP_REF/inc/mpp_rc_defs.h"; then
    rc_has_bit_target_fix=1
else
    rc_has_bit_target_fix=0
fi

cc -D_GNU_SOURCE -DCUSTOM_RC_HAS_BIT_TARGET_FIX="$rc_has_bit_target_fix" \
    -DH264E_PROBE_NO_ROI -std=c11 -O2 -Wall -Wextra -Werror -pthread \
    -I"$MPP_REF/inc" /tmp/mpp_hier_qp_probe.c \
    "$MPP_REF/build/mpp/librockchip_mpp.so.1" -o /tmp/mpp_hier_qp_probe

raw=/tmp/rk_mpp_hier_qp_320x240_33.nv12
vendor=/tmp/rk_mpp_hier_qp_vendor.264
rust=/tmp/rk_mpp_hier_qp_rust.264
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 33 \
    -pix_fmt nv12 -f rawvideo "$raw"

args=("$raw" "$vendor" 320 240 33 1 26 300000:250000:350000
    gop 60 hier 1:2:3:4:1:1:1:1)
sudo -n rm -f "$vendor" "$rust"
sudo -n env LD_LIBRARY_PATH="$MPP_REF/build/mpp" \
    /tmp/mpp_hier_qp_probe "${args[@]}" >/tmp/rk_mpp_hier_qp_vendor.log 2>&1
args[1]="$rust"
sudo -n env LD_LIBRARY_PATH="$overlay" \
    /tmp/mpp_hier_qp_probe "${args[@]}" >/tmp/rk_mpp_hier_qp_rust.log 2>&1

sudo -n python3 /tmp/strip_version_sei.py "$vendor" /tmp/rk_mpp_hier_qp_vendor.norm
sudo -n python3 /tmp/strip_version_sei.py "$rust" /tmp/rk_mpp_hier_qp_rust.norm
sudo -n cmp -s /tmp/rk_mpp_hier_qp_vendor.norm /tmp/rk_mpp_hier_qp_rust.norm
sudo -n stat -c '%n %s' "$vendor" "$rust"
echo "PASS mpp/hier-qp arrays through MPP_ENC_SET_CFG"
REMOTE
