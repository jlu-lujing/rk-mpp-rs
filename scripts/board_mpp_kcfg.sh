#!/usr/bin/env bash

# Validate the vendor init-kcfg ABI against the standalone Rust library.

set -euo pipefail

board="${BOARD:-t6}"
mpp="${MPP_REF:-/tmp/mpp-develop}"
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
) |
    ssh "$board" 'tar -xf - --strip-components=1 -C "$HOME/rk-mpp-rs"'

scp -q "$repo/tools/mpp_kcfg_probe.c" "$board":/tmp/mpp_kcfg_probe.c

ssh "$board" "MPP_REF='$mpp' bash -s" <<'REMOTE'
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
cd "$HOME/rk-mpp-rs"
test -f "$MPP_REF/inc/rk_mpi.h"
test -f "$MPP_REF/inc/rk_venc_kcfg.h"
cargo build --release --lib >/dev/null
overlay="$(mktemp -d /tmp/rk_mpp_kcfg.XXXXXX)"
trap 'rm -rf "$overlay"' EXIT
ln -s "$PWD/target/release/librk_mpp_rs.so" "$overlay/librockchip_mpp.so"
ln -s "$PWD/target/release/librk_mpp_rs.so" "$overlay/librockchip_mpp.so.0"
ln -s "$PWD/target/release/librk_mpp_rs.so" "$overlay/librockchip_mpp.so.1"
ln -s "$PWD/target/release/librk_mpp_rs.so" "$overlay/librk_mpp_rs.so.0"
ln -s "$PWD/target/release/librk_mpp_rs.so" "$overlay/librk_mpp_rs.so.1"
cc -O2 -Wall -Wextra -Werror \
    -I"$MPP_REF/inc" /tmp/mpp_kcfg_probe.c \
    -L"$overlay" -lrockchip_mpp -o /tmp/mpp_kcfg_probe
sudo -n env LD_LIBRARY_PATH="$overlay" timeout 30 /tmp/mpp_kcfg_probe < /dev/null
REMOTE
