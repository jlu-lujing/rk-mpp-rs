#!/usr/bin/env bash

# Run the vendor MppEncCfg self-test against librk_mpp_rs.so.

set -euo pipefail

board="${BOARD:-t6}"
mpp="${MPP_REF:-/tmp/mpp-develop}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
test_bin="$mpp/build/mpp/base/test/mpp_enc_cfg_test"

if ! ssh "$board" "test -x $test_bin"; then
    source "$repo/scripts/board_dependency.sh" \
        "MppEncCfg self-test" "vendor test binary missing at $test_bin"
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
cargo build --release --lib >/dev/null

overlay="$(mktemp -d /tmp/rk_mpp_enc_cfg_test.XXXXXX)"
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

test_bin="$MPP_REF/build/mpp/base/test/mpp_enc_cfg_test"
sudo -n env LD_LIBRARY_PATH="$MPP_REF/build/mpp" \
    "$test_bin" >/tmp/rk_mpp_enc_cfg_vendor.log 2>&1
sudo -n env LD_LIBRARY_PATH="$overlay" \
    "$test_bin" >/tmp/rk_mpp_enc_cfg_rust.log 2>&1

common=(-O2 -Wall -Wextra -Werror
    -I"$MPP_REF/inc"
    -I"$MPP_REF/mpp/inc"
    -I"$MPP_REF/mpp/base/inc"
    -I"$MPP_REF/kmpp/base/inc"
    -I"$MPP_REF/osal/inc")
cc "${common[@]}" tools/mpp_enc_cfg_kcfg_array_probe.c \
    -L"$overlay" -lrockchip_mpp \
    -o /tmp/rk_mpp_enc_cfg_kcfg_array_probe
sudo -n env LD_LIBRARY_PATH="$overlay" \
    /tmp/rk_mpp_enc_cfg_kcfg_array_probe

grep -q 'test kobj roundtrip          success' /tmp/rk_mpp_enc_cfg_rust.log
grep -q 'done success' /tmp/rk_mpp_enc_cfg_rust.log
test "$(grep -o 'mpp_enc_cfg_test: test' /tmp/rk_mpp_enc_cfg_rust.log | wc -l)" -eq 9
tail -n 1 /tmp/rk_mpp_enc_cfg_rust.log
echo "PASS mpp/enc-cfg self-test 9/9"
REMOTE
