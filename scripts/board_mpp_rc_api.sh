#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp="${MPP_REF:-$HOME/rk-mpp-oracles/mpp-old}"
if [ -f "$mpp/inc/rk_mpi.h" ] && [ -f "$mpp/build/mpp/librockchip_mpp.so.1" ]; then
    inc="$mpp/inc"
    lib="$mpp/build/mpp"
elif [ -f /usr/local/include/rockchip/rk_mpi.h ] &&
    [ -f /usr/local/lib/librockchip_mpp.so.1 ]; then
    inc=/usr/local/include/rockchip
    lib=/usr/local/lib
else
    printf 'missing reference MPP headers or library\n' >&2
    exit 2
fi

probe=/tmp/rk_mpp_rc_api_probe
vendor=/tmp/rk_mpp_rc_api_vendor.txt
ours=/tmp/rk_mpp_rc_api_ours.txt
vendor_direct=/tmp/rk_mpp_rc_api_vendor_direct.txt
ours_direct=/tmp/rk_mpp_rc_api_ours_direct.txt
overlay=$(mktemp -d /tmp/rk_mpp_rc_api.XXXXXX)
trap 'rm -rf "$overlay"' EXIT

cargo build --release --offline --lib --bin rk-mpp-rs
ln -s "$repo/target/release/librk_mpp_rs.so" \
    "$overlay/librockchip_mpp.so.1"

cc -D_GNU_SOURCE -std=c11 -O2 -Wall -Wextra -Werror \
    -I"$inc" "$repo/tools/rc_api_probe.c" \
    "$lib/librockchip_mpp.so.1" -o "$probe"

sudo -n env LD_LIBRARY_PATH="$lib" timeout 60 "$probe" \
    > "$vendor" 2> "$vendor.log"
sudo -n env LD_LIBRARY_PATH="$overlay" timeout 60 "$probe" \
    > "$ours" 2> "$ours.log"
cmp "$vendor" "$ours"
sudo -n env LD_LIBRARY_PATH="$lib" timeout 60 "$probe" direct \
    > "$vendor_direct" 2> "$vendor_direct.log"
sudo -n env LD_LIBRARY_PATH="$overlay" timeout 60 "$probe" direct \
    > "$ours_direct" 2> "$ours_direct.log"
cmp "$vendor_direct" "$ours_direct"
printf 'PASS RC API control and direct registry exports\n'
cat "$ours"
cat "$ours_direct"
