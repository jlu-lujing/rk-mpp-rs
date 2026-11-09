#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp_ref="${MPP_REF:-$HOME/rk-mpp-rs/refs/mpp}"
if [ ! -f "$mpp_ref/inc/rk_mpi.h" ]; then
    printf 'missing official MPP headers under %s\n' "$mpp_ref" >&2
    exit 2
fi

cargo build --release --offline --lib
libdir=$(mktemp -d)
trap 'rm -rf "$libdir"' EXIT
ln -sfn "$repo/target/release/librk_mpp_rs.so" "$libdir/librk_mpp_rs.so"
ln -sfn "$repo/target/release/librk_mpp_rs.so" "$libdir/librk_mpp_rs.so.1"

gcc -std=c11 -Wall -Wextra -Werror \
    -I"$mpp_ref/inc" "$repo/tests/c_mpp_lifecycle.c" \
    -L"$libdir" -lrk_mpp_rs \
    -Wl,-rpath,"$libdir" \
    -o /tmp/rk_mpp_official_lifecycle
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_official_lifecycle
