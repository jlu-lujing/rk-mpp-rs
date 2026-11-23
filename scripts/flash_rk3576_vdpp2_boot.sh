#!/usr/bin/env bash

# Safely stage or apply a replacement RK3576 boot FIT.
# Dry-run is the default. Set APPLY=1 to write /dev/mmcblk2p3 and REBOOT=1 to
# reboot after the write has been read back and verified.
set -euo pipefail

usage() {
    cat >&2 <<'EOF'
usage: flash_rk3576_vdpp2_boot.sh <boot.img> <expected-sha256>

Dry-run:
  BOARD=my-rk3576-host ./scripts/flash_rk3576_vdpp2_boot.sh <image> <sha256>

Apply and reboot:
  BOARD=my-rk3576-host APPLY=1 REBOOT=1 \
    ./scripts/flash_rk3576_vdpp2_boot.sh <image> <sha256>
EOF
    exit 2
}

[ "$#" -eq 2 ] || usage

board="${BOARD:?set BOARD to a Rockchip SSH alias}"
image=$1
expected_sha=$2
apply=${APPLY:-0}
reboot=${REBOOT:-0}
allow_other_kernel=${ALLOW_OTHER_KERNEL:-0}
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

[ -f "$image" ] || {
    echo "boot image not found: $image" >&2
    exit 1
}

image_sha=$(shasum -a 256 "$image" | awk '{print $1}')
[ "$image_sha" = "$expected_sha" ] || {
    echo "boot image SHA-256 mismatch" >&2
    echo "expected: $expected_sha" >&2
    echo "actual:   $image_sha" >&2
    exit 1
}

image_size=$(wc -c <"$image" | tr -d ' ')
remote=$(
    ssh "$board" '
        set -eu
        kernel=$(uname -r)
        partition_bytes=$(sudo blockdev --getsize64 /dev/mmcblk2p3)
        dt_status=$(tr -d "\0" < /proc/device-tree/vdpp@27961000/status 2>/dev/null || echo unavailable)
        sudo -n true
        printf "kernel=%s\npartition_bytes=%s\ndt_status=%s\n" \
            "$kernel" "$partition_bytes" "$dt_status"
    '
)
printf '%s\n' "$remote"
remote_kernel=$(sed -n 's/^kernel=//p' <<<"$remote")
partition_bytes=$(sed -n 's/^partition_bytes=//p' <<<"$remote")
dt_status=$(sed -n 's/^dt_status=//p' <<<"$remote")

if [ "$remote_kernel" != 6.1.75 ] && [ "$allow_other_kernel" != 1 ]; then
    echo "expected RK3576 kernel 6.1.75, got $remote_kernel" >&2
    echo "set ALLOW_OTHER_KERNEL=1 only after reviewing the target board" >&2
    exit 1
fi
if [ "$dt_status" != okay ] && [ "$allow_other_kernel" != 1 ]; then
    echo "target DTB does not expose /vdpp@27961000 as okay" >&2
    exit 1
fi
if [ "$image_size" -gt "$partition_bytes" ]; then
    echo "boot image is $image_size bytes; p3 is only $partition_bytes bytes" >&2
    exit 1
fi

printf 'verified image: %s bytes sha256=%s\n' "$image_size" "$image_sha"
printf 'target: %s /dev/mmcblk2p3 (%s bytes)\n' "$board" "$partition_bytes"

if [ "$apply" != 1 ]; then
    echo "DRY RUN: no data written; set APPLY=1 to flash"
    exit 0
fi

timestamp=$(date +%Y%m%d-%H%M%S)
remote_image="/tmp/rk3576-vdpp2-boot-$timestamp.img"
remote_readback="/tmp/rk3576-vdpp2-readback-$timestamp.img"
local_backup="$repo/target/rk3576-vdpp2/rk3576-p3-backup-$timestamp.img"

cleanup_remote() {
    ssh "$board" "sudo rm -f '$remote_image' '$remote_readback'" ||
        true
}
trap cleanup_remote EXIT

mkdir -p "$(dirname "$local_backup")"
scp -q "$image" "$board:$remote_image"
ssh "$board" "printf '%s  %s\n' '$image_sha' '$remote_image' | sha256sum -c -"

echo "backing up current p3 to $local_backup"
ssh "$board" "sudo dd if=/dev/mmcblk2p3 bs=1M status=none 2>/dev/null" \
    >"$local_backup"
[ "$(wc -c <"$local_backup" | tr -d ' ')" -eq "$partition_bytes" ] || {
    echo "local p3 backup size mismatch; refusing to flash" >&2
    exit 1
}
shasum -a 256 "$local_backup"

echo "writing $remote_image to /dev/mmcblk2p3"
ssh "$board" "sudo dd if='$remote_image' of=/dev/mmcblk2p3 bs=4M conv=fsync status=progress && sync"

blocks=$((image_size / 1048576))
[ $((image_size % 1048576)) -eq 0 ] || blocks=$((blocks + 1))
readback_sha=$(
    ssh "$board" "
        sudo dd if=/dev/mmcblk2p3 of='$remote_readback' \
            bs=1M count=$blocks status=none 2>/dev/null &&
        sudo truncate -s $image_size '$remote_readback' &&
        sudo sha256sum '$remote_readback' | cut -d ' ' -f 1
    "
)
[ "$readback_sha" = "$image_sha" ] || {
    echo "p3 readback SHA-256 mismatch" >&2
    echo "expected: $image_sha" >&2
    echo "readback: $readback_sha" >&2
    echo "restore from $local_backup before rebooting" >&2
    exit 1
}
echo "PASS p3 readback sha256=$readback_sha"

cleanup_remote
trap - EXIT

if [ "$reboot" = 1 ]; then
    echo "rebooting $board"
    ssh "$board" "sudo reboot" || true
else
    echo "write complete; set REBOOT=1 or reboot manually"
fi
