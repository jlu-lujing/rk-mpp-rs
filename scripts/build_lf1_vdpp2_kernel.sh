#!/usr/bin/env bash

# Build a VDPP-enabled RK3576 kernel and repack the current lf1 U-Boot FIT.
# This script never writes to the board or boot partition.
set -euo pipefail

usage() {
    cat >&2 <<'EOF'
usage: build_lf1_vdpp2_kernel.sh \
  <kernel-source-or-tarball> <board-config> <current-boot.img> <output-dir>

The build host must be x86_64 Linux with:
  aarch64-linux-gnu-gcc, make, dtc, fdtget, fdtput, dumpimage, mkimage

Set CROSS_COMPILE to use another toolchain prefix, for example:
  CROSS_COMPILE=aarch64-none-linux-gnu-
EOF
    exit 2
}

[ "$#" -eq 4 ] || usage

source_input=$1
board_config=$2
current_boot=$3
output_dir=$4
cross_compile=${CROSS_COMPILE:-aarch64-linux-gnu-}
c_compiler="${cross_compile}gcc"
nm_tool="${cross_compile}nm"

for tool in make "$c_compiler" dtc fdtget fdtput dumpimage mkimage sha256sum; do
    command -v "$tool" >/dev/null || {
        echo "missing required tool: $tool" >&2
        exit 1
    }
done
[ -f "$board_config" ] || {
    echo "board config not found: $board_config" >&2
    exit 1
}
[ -f "$current_boot" ] || {
    echo "current boot image not found: $current_boot" >&2
    exit 1
}
expected_version=$(sed -n \
    's/^# Linux\/arm64 \([0-9][0-9.]*\) Kernel Configuration$/\1/p' \
    "$board_config")
[ -n "$expected_version" ] || {
    echo "cannot determine the kernel version from $board_config" >&2
    exit 1
}

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/lf1-vdpp2-kernel.XXXXXX")
cleanup() {
    if [ "${KEEP_WORK:-0}" != 1 ]; then
        rm -rf "$work_dir"
    fi
}
trap cleanup EXIT

case "$source_input" in
    *.tar|*.tar.gz|*.tgz|*.tar.xz)
        mkdir -p "$work_dir/source"
        tar -xf "$source_input" -C "$work_dir/source"
        mapfile -t top_level < <(find "$work_dir/source" -mindepth 1 -maxdepth 1)
        [ "${#top_level[@]}" -eq 1 ] || {
            echo "kernel archive must contain one top-level directory" >&2
            exit 1
        }
        kernel_src=${top_level[0]}
        ;;
    *)
        [ -d "$source_input" ] || {
            echo "kernel source is neither a directory nor a supported archive" >&2
            exit 1
        }
        kernel_src=$source_input
        ;;
esac

[ -x "$kernel_src/scripts/config" ] || {
    echo "not a Linux kernel source tree: $kernel_src" >&2
    exit 1
}
source_version=$(awk -F' = ' '
    /^VERSION = / { version = $2 }
    /^PATCHLEVEL = / { patch = $2 }
    /^SUBLEVEL = / { sublevel = $2 }
    END { print version "." patch "." sublevel }
' "$kernel_src/Makefile")
[ "$source_version" = "$expected_version" ] || {
    echo "kernel source version is $source_version, board expects $expected_version" >&2
    exit 1
}
vdpp_source="$kernel_src/drivers/video/rockchip/mpp/mpp_vdpp.c"
[ -f "$vdpp_source" ] || {
    echo "kernel source has no Rockchip mpp_vdpp.c" >&2
    exit 1
}
grep -q 'rockchip,vdpp-rk3576' "$vdpp_source" || {
    echo "kernel source predates RK3576 VDPP support" >&2
    exit 1
}

mkdir -p "$output_dir"
output_dir=$(cd "$output_dir" && pwd)
build_dir="$work_dir/build"
mkdir -p "$build_dir"
expected_config="$work_dir/expected.config"
cp "$board_config" "$expected_config"
"$kernel_src/scripts/config" --file "$expected_config" \
    --enable ROCKCHIP_MPP_VDPP
cp "$expected_config" "$build_dir/.config"

make -C "$kernel_src" O="$build_dir" ARCH=arm64 \
    CROSS_COMPILE="$cross_compile" olddefconfig
grep -q '^CONFIG_ROCKCHIP_MPP_VDPP=y$' "$build_dir/.config" || {
    echo "CONFIG_ROCKCHIP_MPP_VDPP was not enabled" >&2
    exit 1
}

make -C "$kernel_src" O="$build_dir" ARCH=arm64 \
    CROSS_COMPILE="$cross_compile" -j"$(nproc)" Image

kernel_image="$build_dir/arch/arm64/boot/Image"
vmlinux="$build_dir/vmlinux"
[ -s "$kernel_image" ] || {
    echo "kernel Image was not produced" >&2
    exit 1
}
if ! "$nm_tool" "$vmlinux" |
    grep -E 'vdpp_(probe|remove)|rockchip_vdpp_driver|vdpp2_data' \
        > "$output_dir/mpp_vdpp-symbols.txt"
then
    echo "mpp_vdpp symbols are absent from vmlinux" >&2
    exit 1
fi

kernel_config="$work_dir/kernel.config"
"$kernel_src/scripts/extract-ikconfig" "$kernel_image" > "$kernel_config"
grep -q '^CONFIG_ROCKCHIP_MPP_VDPP=y$' "$kernel_config" || {
    echo "rebuilt Image does not embed CONFIG_ROCKCHIP_MPP_VDPP=y" >&2
    exit 1
}
awk '/^(CONFIG_|# CONFIG_)/' "$expected_config" \
    > "$work_dir/expected-config.txt"
awk '/^(CONFIG_|# CONFIG_)/' "$kernel_config" \
    > "$work_dir/kernel-config.txt"
if ! diff -u "$work_dir/expected-config.txt" \
    "$work_dir/kernel-config.txt" > "$output_dir/kernel-config.diff"; then
    echo "rebuilt kernel config differs beyond CONFIG_ROCKCHIP_MPP_VDPP" >&2
    cat "$output_dir/kernel-config.diff" >&2
    exit 1
fi
actual_version=$(sed -n \
    's/^# Linux\/arm64 \([0-9][0-9.]*\) Kernel Configuration$/\1/p' \
    "$kernel_config")
[ "$actual_version" = "$expected_version" ] || {
    echo "kernel version mismatch: board=$expected_version rebuilt=$actual_version" >&2
    echo "select the matching board SDK source revision" >&2
    exit 1
}

fdt="$work_dir/current-fdt.dtb"
resource="$work_dir/current-resource.bin"
dumpimage -T flat_dt -p 0 -o "$fdt" "$current_boot" >/dev/null
dumpimage -T flat_dt -p 2 -o "$resource" "$current_boot" >/dev/null

cp "$kernel_image" "$output_dir/Image"
cp "$fdt" "$output_dir/board.dtb"
cp "$resource" "$output_dir/resource.bin"
cp "$kernel_config" "$output_dir/kernel.config"

fit_data_dir="$work_dir/fit"
fit_template="$work_dir/current-fit.dtb"
fit_its="$fit_data_dir/lf1-vdpp2-boot.its"
mkdir -p "$fit_data_dir"
cp "$current_boot" "$fit_template"

mapfile -t fit_images < <(fdtget -l "$current_boot" /images)
[ "${#fit_images[@]}" -gt 0 ] || {
    echo "current boot image has no FIT images" >&2
    exit 1
}
for image_name in "${fit_images[@]}"; do
    image_source=
    case "$image_name" in
        fdt) image_source=$fdt ;;
        kernel) ;;
        resource) image_source=$resource ;;
        *)
            echo "unsupported FIT image node: $image_name" >&2
            exit 1
            ;;
    esac
    if [ -n "$image_source" ]; then
        cp "$image_source" "$fit_data_dir/$image_name"
    fi
    fdtput -t s "$fit_template" "/images/$image_name" data \
        "/INCBIN/($image_name)"
done
cp "$kernel_image" "$fit_data_dir/kernel"

# Match Rockchip's scripts/mkimg repack path: preserve the current FIT
# structure, replace image data, remove generated values, and rebuild it as
# an externally stored FIT. This keeps the vendor signature declaration and
# the original configuration node intact.
dtc -I dtb -O dts "$fit_template" -o "$fit_its" >/dev/null 2>&1
sed -i \
    -e 's#data = "/INCBIN/(\(.*\))";#data = /incbin/("\1");#' \
    -e '/memreserve/d' \
    -e '/timestamp/d' \
    -e '/data-size/d' \
    -e '/data-position/d' \
    -e '/value/d' \
    -e '/hashed-strings/d' \
    -e '/hashed-nodes/d' \
    -e '/signer-version/d' \
    -e '/signer-name/d' \
    "$fit_its"
cp "$fit_its" "$output_dir/lf1-vdpp2-boot.its"

mkimage_args=(-E -p 0x800)
mkimage_help=$(mkimage 2>&1 || true)
if grep -q '\[-B size\]' <<< "$mkimage_help"; then
    mkimage_args+=(-B 0x200)
fi
boot_output="$output_dir/lf1-vdpp2-boot.img"
(
    cd "$fit_data_dir"
    mkimage "${mkimage_args[@]}" -f "$fit_its" "$boot_output"
) > "$output_dir/mkimage.log" 2>&1
dumpimage -l "$boot_output" > "$output_dir/lf1-vdpp2-boot.manifest.txt"

sign_algo=$(fdtget -t s "$boot_output" \
    /configurations/conf/signature algo)
sign_padding=$(fdtget -t s "$boot_output" \
    /configurations/conf/signature padding)
[ "$sign_algo" = "sha256,rsa2048" ] || {
    echo "new FIT does not preserve the vendor signature algorithm" >&2
    exit 1
}
[ "$sign_padding" = "pss" ] || {
    echo "new FIT does not preserve the vendor signature padding" >&2
    exit 1
}

current_size=$(stat -c %s "$current_boot")
output_size=$(stat -c %s "$boot_output")
if [ "$output_size" -gt "$current_size" ]; then
    echo "new FIT is $output_size bytes, larger than the $current_size-byte partition" >&2
    exit 1
fi

(
    cd "$output_dir"
    sha256sum Image board.dtb resource.bin kernel.config \
        lf1-vdpp2-boot.its lf1-vdpp2-boot.img mpp_vdpp-symbols.txt \
        > SHA256SUMS
)

printf 'PASS lf1 VDPP2 kernel artifact: %s\n' "$boot_output"
printf 'size=%s/%s bytes\n' "$output_size" "$current_size"
printf 'kernel=%s\n' "$actual_version"
grep -E '^(Image|fdt|resource|kernel):|Hash value:|Data Size:' \
    "$output_dir/lf1-vdpp2-boot.manifest.txt"
