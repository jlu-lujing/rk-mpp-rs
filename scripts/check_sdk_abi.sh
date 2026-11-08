#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
profile=${PROFILE:-release}
target_dir=${CARGO_TARGET_DIR:-"$repo/target"}
case "$target_dir" in
    /*) ;;
    *) target_dir="$repo/$target_dir" ;;
esac
library="$target_dir/$profile/librk_mpp_rs.so"

if [ ! -f "$library" ]; then
    printf 'missing shared library: %s\n' "$library" >&2
    exit 1
fi

temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT
ln -sfn "$library" "$temporary/librk_mpp_rs.so"
ln -sfn "$library" "$temporary/librk_mpp_rs.so.1"

gcc -std=c11 -Wall -Wextra -Werror \
    -I"$repo/include" "$repo/tests/c_api_abi.c" \
    -L"$temporary" -lrk_mpp_rs \
    -Wl,-rpath,"$temporary" \
    -o /tmp/rk_mpp_c_api_abi
/tmp/rk_mpp_c_api_abi

actual="$temporary/symbols"
nm -D --defined-only "$library" |
    awk '{print $3}' |
    grep -E '^(rk_mpp_|rk_jpeg_)' |
    sort > "$actual"
diff -u "$repo/abi/rk_mpp_rs-v1.symbols" "$actual"

soname=$(readelf -d "$library" |
    awk '/SONAME/ { gsub(/\[|\]/, "", $NF); print $NF }')
if [ "$soname" != "librk_mpp_rs.so.1" ]; then
    printf 'unexpected SONAME: %s\n' "${soname:-<missing>}" >&2
    exit 1
fi

printf 'PASS rk-mpp-rs ABI layout, symbols and SONAME\n'
