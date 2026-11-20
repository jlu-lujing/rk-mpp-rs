#!/usr/bin/env bash

set -euo pipefail

usage() {
    cat <<'EOF'
usage: scripts/install_sdk.sh [--prefix PATH] [--destdir PATH]
                              [--profile release|debug] [--no-build]
                              [--mpp-compat] [--mpp-include PATH]
EOF
}

prefix=/usr/local
destdir=
profile=release
build=1
mpp_compat=0
mpp_include=

while [ "$#" -gt 0 ]; do
    case "$1" in
        --prefix)
            prefix=${2:?--prefix needs a path}
            shift 2
            ;;
        --destdir)
            destdir=${2:?--destdir needs a path}
            shift 2
            ;;
        --profile)
            profile=${2:?--profile needs release or debug}
            shift 2
            ;;
        --no-build)
            build=0
            shift
            ;;
        --mpp-compat)
            mpp_compat=1
            shift
            ;;
        --mpp-include)
            mpp_include=${2:?--mpp-include needs a path}
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
done

case "$profile" in
    release|debug) ;;
    *)
        printf 'unsupported profile: %s\n' "$profile" >&2
        exit 2
        ;;
esac
case "$prefix" in
    /*) ;;
    *)
        printf 'prefix must be absolute: %s\n' "$prefix" >&2
        exit 2
        ;;
esac

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
header="$repo/include/rk_mpp_rs.h"
api_major=$(awk '
    /#define RK_MPP_API_VERSION_MAJOR/ { gsub(/u/, "", $3); print $3 }
' "$header")
api_minor=$(awk '
    /#define RK_MPP_API_VERSION_MINOR/ { gsub(/u/, "", $3); print $3 }
' "$header")
if [ -z "$api_major" ] || [ -z "$api_minor" ]; then
    printf 'could not read API version from %s\n' "$header" >&2
    exit 1
fi
api_version="$api_major.$api_minor.0"

if [ "$build" -eq 1 ]; then
    (
        cd "$repo"
        if [ "$profile" = release ]; then
            cargo build --release --offline --lib
        else
            cargo build --offline --lib
        fi
    )
fi

if [ "$mpp_compat" -eq 1 ] && [ -z "$mpp_include" ]; then
    for candidate in /usr/local/include /usr/include; do
        if [ -f "$candidate/rockchip/rk_mpi.h" ]; then
            mpp_include=$candidate
            break
        fi
    done
fi
if [ "$mpp_compat" -eq 1 ] &&
    { [ -z "$mpp_include" ] || [ ! -f "$mpp_include/rockchip/rk_mpi.h" ]; }; then
    printf 'MPP compatibility install needs --mpp-include PATH containing rockchip/rk_mpi.h\n' >&2
    exit 2
fi

target_dir=${CARGO_TARGET_DIR:-"$repo/target"}
case "$target_dir" in
    /*) ;;
    *)
        target_dir="$repo/$target_dir"
        ;;
esac
library_dir="$target_dir/$profile"
shared="$library_dir/librk_mpp_rs.so"
archive="$library_dir/librk_mpp_rs.a"
for file in "$header" "$shared" "$archive"; do
    if [ ! -f "$file" ]; then
        printf 'missing build product: %s\n' "$file" >&2
        exit 1
    fi
done

root="${destdir}${prefix}"
install -d \
    "$root/share/doc/rk-mpp-rs" \
    "$root/include" \
    "$root/lib/cmake/rk-mpp-rs" \
    "$root/lib/pkgconfig"
install -m 0644 "$header" "$root/include/rk_mpp_rs.h"
install -m 0644 "$repo/CHANGELOG.md" "$root/share/doc/rk-mpp-rs/CHANGELOG.md"
install -m 0644 "$repo/NOTICE" "$root/share/doc/rk-mpp-rs/NOTICE"
install -m 0644 "$repo/LICENSE-MIT" "$root/share/doc/rk-mpp-rs/LICENSE-MIT"
install -m 0644 "$repo/LICENSE-APACHE" "$root/share/doc/rk-mpp-rs/LICENSE-APACHE"
install -m 0644 "$archive" "$root/lib/librk_mpp_rs.a"
install -m 0755 "$shared" "$root/lib/librk_mpp_rs.so.$api_version"
ln -sfn "librk_mpp_rs.so.$api_version" "$root/lib/librk_mpp_rs.so.$api_major"
ln -sfn "librk_mpp_rs.so.$api_major" "$root/lib/librk_mpp_rs.so"
if [ "$mpp_compat" -eq 1 ]; then
    install -d "$root/include/rockchip"
    cp -R "$mpp_include/rockchip/." "$root/include/rockchip/"
    ln -sfn "librk_mpp_rs.so.$api_version" "$root/lib/librockchip_mpp.so"
    ln -sfn "librk_mpp_rs.so.$api_version" "$root/lib/librockchip_mpp.so.$api_major"
fi

sed "s/@VERSION@/$api_major.$api_minor/g" \
    "$repo/packaging/rk-mpp-rs.pc.in" \
    > "$root/lib/pkgconfig/rk-mpp-rs.pc"
if [ "$mpp_compat" -eq 1 ]; then
    sed "s/@VERSION@/$api_major.$api_minor.0/g" \
        "$repo/packaging/rockchip_mpp.pc.in" \
        > "$root/lib/pkgconfig/rockchip_mpp.pc"
fi
sed "s/@VERSION@/$api_major.$api_minor/g" \
    "$repo/packaging/rk-mpp-rsConfig.cmake" \
    > "$root/lib/cmake/rk-mpp-rs/rk-mpp-rsConfig.cmake"
sed "s/@VERSION@/$api_major.$api_minor.0/g" \
    "$repo/packaging/rk-mpp-rsConfigVersion.cmake" \
    > "$root/lib/cmake/rk-mpp-rs/rk-mpp-rsConfigVersion.cmake"

soname=$(readelf -d "$root/lib/librk_mpp_rs.so.$api_version" |
    awk '/SONAME/ { gsub(/\[|\]/, "", $NF); print $NF }')
if [ "$soname" != "librk_mpp_rs.so.$api_major" ]; then
    printf 'unexpected SONAME: %s\n' "${soname:-<missing>}" >&2
    exit 1
fi

printf 'installed rk-mpp-rs API %s\n' "$api_major.$api_minor"
printf '  root:   %s\n' "$root"
printf '  header: include/rk_mpp_rs.h\n'
printf '  lib:    lib/librk_mpp_rs.so.%s\n' "$api_version"
printf '  pc:     lib/pkgconfig/rk-mpp-rs.pc\n'
printf '  cmake:  lib/cmake/rk-mpp-rs\n'
if [ "$mpp_compat" -eq 1 ]; then
    printf '  compat: lib/librockchip_mpp.so{,.%s}\n' "$api_major"
    printf '  compat: lib/pkgconfig/rockchip_mpp.pc (headers from %s)\n' \
        "$mpp_include"
fi
