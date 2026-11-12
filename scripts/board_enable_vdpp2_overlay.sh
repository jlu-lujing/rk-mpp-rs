#!/usr/bin/env bash

# Opt-in configfs overlay for RK3576 boards whose base DTB leaves VDPP2 disabled.

set -euo pipefail

board="${BOARD:-lf1}"
action="${1:-status}"
overlay=tools/rk3576-vdpp2-overlay.dts

case "$action" in
    status|enable|disable) ;;
    *)
        printf 'usage: %s [status|enable|disable]\n' "$0" >&2
        exit 2
        ;;
esac

if [ "$action" = status ]; then
    ssh "$board" '
        printf "vdpp status: "
        cat /proc/device-tree/vdpp@27961000/status 2>/dev/null || echo unavailable
        if [ -d /sys/kernel/config/device-tree/overlays/vdpp2 ]; then
            echo "configfs overlay: loaded"
        else
            echo "configfs overlay: absent"
        fi
    '
    exit
fi

if [ "$action" = disable ]; then
    ssh "$board" '
        if [ -d /sys/kernel/config/device-tree/overlays/vdpp2 ]; then
            sudo rmdir /sys/kernel/config/device-tree/overlays/vdpp2
            echo "removed configfs overlay"
        else
            echo "configfs overlay is not loaded"
        fi
    '
    exit
fi

scp -q "$overlay" "$board":/tmp/rk3576-vdpp2-overlay.dts
ssh "$board" '
    set -eu
    command -v dtc >/dev/null || {
        echo "dtc is required to build the VDPP2 overlay" >&2
        exit 1
    }
    if [ ! -d /sys/kernel/config/device-tree/overlays ]; then
        echo "kernel configfs device-tree overlays are unavailable" >&2
        echo "enable CONFIG_OF_OVERLAY and CONFIG_OF_CONFIGFS in the kernel" >&2
        exit 1
    fi
    sudo dtc -@ -I dts -O dtb -o /tmp/rk3576-vdpp2-overlay.dtbo \
        /tmp/rk3576-vdpp2-overlay.dts
    sudo mkdir /sys/kernel/config/device-tree/overlays/vdpp2
    sudo cp /tmp/rk3576-vdpp2-overlay.dtbo \
        /sys/kernel/config/device-tree/overlays/vdpp2/dtbo
    sleep 1
    printf "vdpp status after overlay: "
    cat /proc/device-tree/vdpp@27961000/status
    echo
'
