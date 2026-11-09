#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp_include="${MPP_INCLUDE:-$repo/refs/mpp/inc}"
if [ ! -f "$mpp_include/rk_mpi.h" ]; then
    for candidate in /usr/local/include /usr/local/include/rockchip; do
        if [ -f "$candidate/rk_mpi.h" ]; then
            mpp_include=$candidate
            break
        fi
    done
fi
if [ ! -f "$mpp_include/rk_mpi.h" ]; then
    printf 'missing official MPP headers under %s\n' "$mpp_include" >&2
    exit 2
fi

cargo build --release --offline --lib
libdir=$(mktemp -d)
trap 'rm -rf "$libdir"' EXIT
ln -sfn "$repo/target/release/librk_mpp_rs.so" "$libdir/librk_mpp_rs.so"
ln -sfn "$repo/target/release/librk_mpp_rs.so" "$libdir/librk_mpp_rs.so.1"

gcc -std=c11 -Wall -Wextra -Werror \
    -I"$mpp_include" "$repo/tests/c_mpp_meta.c" \
    -L"$libdir" -lrk_mpp_rs \
    -Wl,-rpath,"$libdir" \
    -o /tmp/rk_mpp_official_meta
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" /tmp/rk_mpp_official_meta

meta_test="${MPP_META_TEST:-}"
if [ -n "$meta_test" ] && [ -x "$meta_test" ]; then
    LD_PRELOAD="$libdir/librk_mpp_rs.so" \
        "$meta_test" -loop "${MPP_META_LOOPS:-10}" \
        -threads "${MPP_META_THREADS:-2}"
fi
