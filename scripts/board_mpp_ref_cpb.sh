#!/usr/bin/env bash

# Compare encoder reference CPB metadata against the vendor library.

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

scp -q "$repo/tools/mpp_ref_cpb_probe.c" "$board":/tmp/mpp_ref_cpb_probe.c

ssh "$board" "MPP_REF='$mpp' bash -s" <<'REMOTE'
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
cd "$HOME/rk-mpp-rs"
test -f "$MPP_REF/inc/rk_mpi.h"
test -f "$MPP_REF/mpp/base/inc/mpp_enc_ref.h"
test -f "$MPP_REF/build/mpp/librockchip_mpp.so.1"
cargo build --release --lib >/dev/null

overlay="$(mktemp -d /tmp/rk_mpp_ref_cpb.XXXXXX)"
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
    -I"$MPP_REF/mpp/base/inc"
    -I"$MPP_REF/osal/inc")
format_flags=()
if [ -f "$MPP_REF/inc/rk_mpp_cfg.h" ] &&
    grep -q 'mpp_enc_ref_cfg_apply_file' "$MPP_REF/inc/rk_venc_ref.h"; then
    format_flags=(-DRK_MPP_HAS_REF_CFG_FORMATS=1)
fi
cc "${common[@]}" "${format_flags[@]}" /tmp/mpp_ref_cpb_probe.c \
    "$MPP_REF/build/mpp/librockchip_mpp.so.1" \
    -o /tmp/mpp_ref_cpb_vendor
cc "${common[@]}" "${format_flags[@]}" /tmp/mpp_ref_cpb_probe.c \
    -L"$overlay" -lrockchip_mpp \
    -o /tmp/mpp_ref_cpb_rs

sudo -n env LD_LIBRARY_PATH="$MPP_REF/build/mpp" \
    /tmp/mpp_ref_cpb_vendor > /tmp/mpp_ref_cpb_vendor.txt
sudo -n env LD_LIBRARY_PATH="$overlay" \
    /tmp/mpp_ref_cpb_rs > /tmp/mpp_ref_cpb_rs.txt
diff -u /tmp/mpp_ref_cpb_vendor.txt /tmp/mpp_ref_cpb_rs.txt
cat /tmp/mpp_ref_cpb_rs.txt

if [ "${#format_flags[@]}" -ne 0 ]; then
    sudo -n env LD_LIBRARY_PATH="$MPP_REF/build/mpp" \
        /tmp/mpp_ref_cpb_vendor dump-formats /tmp/mpp_ref_fmt_vendor
    sudo -n env LD_LIBRARY_PATH="$overlay" \
        /tmp/mpp_ref_cpb_rs dump-formats /tmp/mpp_ref_fmt_rust
    for format in log json toml; do
        sudo -n env LD_LIBRARY_PATH="$overlay" \
            /tmp/mpp_ref_cpb_rs apply-format "$format" \
            "/tmp/mpp_ref_fmt_vendor.$format" > "/tmp/mpp_ref_rust_reads_$format.txt"
        if [ "$format" = toml ]; then
            cmp -s /tmp/mpp_ref_fmt_vendor.toml /tmp/mpp_ref_fmt_rust.toml
            if sudo -n env LD_LIBRARY_PATH="$MPP_REF/build/mpp" \
                /tmp/mpp_ref_cpb_vendor apply-format toml \
                /tmp/mpp_ref_fmt_rust.toml >/dev/null 2>&1; then
                echo "C unexpectedly accepted its own TOML ref-cfg" >&2
                exit 1
            fi
            printf 'PASS ref-cfg/toml extraction; vendor self-apply remains a C-BUG\n'
            continue
        fi
        sudo -n env LD_LIBRARY_PATH="$MPP_REF/build/mpp" \
            /tmp/mpp_ref_cpb_vendor apply-format "$format" \
            "/tmp/mpp_ref_fmt_rust.$format" > "/tmp/mpp_ref_vendor_reads_$format.txt"
        diff -u "/tmp/mpp_ref_vendor_reads_$format.txt" \
            "/tmp/mpp_ref_rust_reads_$format.txt"
        printf 'PASS ref-cfg/%s bidirectional strings\n' "$format"
    done

    json="$MPP_REF/test/cfg/enc/ref_cfg_gop3_tsvc4.json"
    if [ -f "$json" ]; then
        sudo -n env LD_LIBRARY_PATH="$MPP_REF/build/mpp" \
            /tmp/mpp_ref_cpb_vendor "$json" > /tmp/mpp_ref_cpb_vendor_json.txt
        sudo -n env LD_LIBRARY_PATH="$overlay" \
            /tmp/mpp_ref_cpb_rs "$json" > /tmp/mpp_ref_cpb_rs_json.txt
        diff -u /tmp/mpp_ref_cpb_vendor_json.txt /tmp/mpp_ref_cpb_rs_json.txt
        cat /tmp/mpp_ref_cpb_rs_json.txt
    fi
fi
REMOTE
