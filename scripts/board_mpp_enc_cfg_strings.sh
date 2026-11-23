#!/usr/bin/env bash

# Compare scalar MppEncCfg LOG/JSON/TOML extraction and application.

set -euo pipefail

board="${BOARD:?set BOARD to a Rockchip SSH alias}"
mpp="${MPP_REF:-/tmp/mpp-develop}"
repo="$(cd "$(dirname "$0")/.." && pwd)"

if ssh "$board" "test ! -f $mpp/inc/rk_mpp_cfg.h ||
    ! grep -q mpp_enc_cfg_apply_file $mpp/inc/rk_venc_cfg.h"; then
    source "$repo/scripts/board_dependency.sh" \
        "MppEncCfg string formats" "cfg-string API missing at $mpp"
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

scp -q "$repo/tools/mpp_enc_cfg_probe.c" "$board":/tmp/mpp_enc_cfg_probe.c

ssh "$board" "MPP_REF='$mpp' bash -s" <<'REMOTE'
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
cd "$HOME/rk-mpp-rs"
test -f "$MPP_REF/inc/rk_mpp_cfg.h"
test -f "$MPP_REF/inc/rk_venc_cfg.h"
test -f "$MPP_REF/build/mpp/librockchip_mpp.so.1"
cargo build --release --lib >/dev/null

overlay="$(mktemp -d /tmp/rk_mpp_enc_cfg.XXXXXX)"
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

common=(-O2 -Wall -Wextra -Werror
    -I"$MPP_REF/inc"
    -I"$MPP_REF/mpp/inc"
    -I"$MPP_REF/mpp/base/inc"
    -I"$MPP_REF/kmpp/base/inc"
    -I"$MPP_REF/osal/inc")
cc "${common[@]}" /tmp/mpp_enc_cfg_probe.c \
    "$MPP_REF/build/mpp/librockchip_mpp.so.1" \
    -o /tmp/mpp_enc_cfg_vendor
cc "${common[@]}" /tmp/mpp_enc_cfg_probe.c \
    -L"$overlay" -lrockchip_mpp \
    -o /tmp/mpp_enc_cfg_rust

sudo -n env LD_LIBRARY_PATH="$MPP_REF/build/mpp" \
    /tmp/mpp_enc_cfg_vendor dump /tmp/mpp_enc_cfg_vendor
sudo -n env LD_LIBRARY_PATH="$overlay" \
    /tmp/mpp_enc_cfg_rust dump /tmp/mpp_enc_cfg_rust

for format in log json toml; do
    sudo -n env LD_LIBRARY_PATH="$MPP_REF/build/mpp" \
        /tmp/mpp_enc_cfg_vendor apply "$format" \
        "/tmp/mpp_enc_cfg_rust.$format" > "/tmp/mpp_enc_cfg_vendor_reads_$format.txt"
    sudo -n env LD_LIBRARY_PATH="$overlay" \
        /tmp/mpp_enc_cfg_rust apply "$format" \
        "/tmp/mpp_enc_cfg_vendor.$format" > "/tmp/mpp_enc_cfg_rust_reads_$format.txt"
    diff -u "/tmp/mpp_enc_cfg_vendor_reads_$format.txt" \
        "/tmp/mpp_enc_cfg_rust_reads_$format.txt"
    printf 'PASS enc-cfg/%s bidirectional values\n' "$format"
done
REMOTE
