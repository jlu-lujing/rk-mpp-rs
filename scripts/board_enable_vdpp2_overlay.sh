#!/usr/bin/env bash

# Opt-in configfs overlay for RK3576 boards whose base DTB leaves VDPP2 disabled.

set -euo pipefail

board="${BOARD:?set BOARD to a Rockchip SSH alias}"
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
        config=$(zcat /proc/config.gz 2>/dev/null |
            sed -n "s/^CONFIG_ROCKCHIP_MPP_VDPP=//p; s/^# CONFIG_ROCKCHIP_MPP_VDPP is not set$/n/p")
        overlay=$(zcat /proc/config.gz 2>/dev/null |
            sed -n "s/^CONFIG_OF_OVERLAY=//p; s/^# CONFIG_OF_OVERLAY is not set$/n/p")
        printf "dt status: "
        if [ -f /proc/device-tree/vdpp@27961000/status ]; then
            tr -d "\0" < /proc/device-tree/vdpp@27961000/status
            echo
        else
            echo unavailable
        fi
        printf "kernel CONFIG_ROCKCHIP_MPP_VDPP: %s\n" "${config:-unavailable}"
        printf "kernel CONFIG_OF_OVERLAY: %s\n" "${overlay:-unavailable}"
        platform_device=
        for candidate in \
            /sys/bus/platform/devices/27961000.vdpp \
            /sys/bus/platform/devices/vdpp@27961000; do
            if [ -e "$candidate" ]; then
                platform_device=$candidate
                break
            fi
        done
        if [ -n "$platform_device" ]; then
            printf "platform device: present\n"
            if [ -e "$platform_device/driver" ]; then
                printf "driver: %s\n" \
                    "$(basename "$(readlink -f "$platform_device/driver")")"
            else
                echo "driver: unbound"
            fi
        else
            echo "platform device: absent"
        fi
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
    config=$(zcat /proc/config.gz 2>/dev/null |
        sed -n "s/^CONFIG_ROCKCHIP_MPP_VDPP=//p; s/^# CONFIG_ROCKCHIP_MPP_VDPP is not set$/n/p")
    if [ "${config:-n}" = n ]; then
        echo "CONFIG_ROCKCHIP_MPP_VDPP is not built into this kernel" >&2
        echo "a DTB overlay cannot create the missing platform driver" >&2
        exit 1
    fi
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
