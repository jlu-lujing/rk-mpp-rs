#!/usr/bin/env bash
BOARD="${BOARD:?set BOARD to a Rockchip SSH alias}"
# PD/DECT inverse-telecine path of IEP2 client 28.
#
# `iep2_test` hard-codes IEP2_DIL_MODE_I5O2, which never reaches iep2_check_pd or
# iep2_check_ffo. `tools/iep2_dect_probe.c` is the stock test with the mode set to
# DECT and one line per frame printed from `struct iep2_api_info`, so both the
# pixels and the state transitions have a reference:
#
#   frame N pd_flag P pd_types T dil_order O frm_mode F ratio R
#
# Cells: ffmpeg's 3:2 telecine (a confirmed PULLDOWN 3:2:3:2 with a cycling
# pd_flag) and plain progressive content (the machines stay at UNKNOWN).
set -uo pipefail

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"
test_dir="$mpp_ref/build/mpp/vproc/iep2/test"
fail=0

if ! ssh "$BOARD" "test -d $mpp_ref && test -f $mpp_lib/librockchip_mpp.so.1"; then
    source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
        "IEP PD/DECT" "reference build missing at $mpp_ref"
fi

scp -q tools/iep2_dect_probe.c "$BOARD":/tmp/ 2>/dev/null
if ! ssh "$BOARD" "set -e; B=$test_dir
    DEFS=\$(grep '^C_DEFINES' \$B/CMakeFiles/iep2_test.dir/flags.make | cut -d= -f2-)
    INC=\$(grep '^C_INCLUDES' \$B/CMakeFiles/iep2_test.dir/flags.make | cut -d= -f2-)
    cc -O3 -DNDEBUG -fPIC -w \$DEFS \$INC -c /tmp/iep2_dect_probe.c -o /tmp/iep2_dect_probe.o
    sed -e 's#CMakeFiles/iep2_test.dir/iep2_test.c.o#/tmp/iep2_dect_probe.o#' \
        -e 's#-o iep2_test#-o /tmp/iep2_dect_probe#' \
        \$B/CMakeFiles/iep2_test.dir/link.txt > /tmp/iep2_dect_link.sh
    cd \$B && sh /tmp/iep2_dect_link.sh" 2>/dev/null; then
    printf 'FAIL iep-dect (reference probe did not build)\n'
    exit 1
fi

run_cell() {
    local name="$1" fixture="$2" frames="$3" width="$4" height="$5"
    local c=/tmp/iep_dect_${name}_c.yuv
    local r=/tmp/iep_dect_${name}_r.yuv
    local c_log=/tmp/iep_dect_${name}_c.log
    local r_log=/tmp/iep_dect_${name}_r.log
    sudo_cmd="sudo"
    ssh "$BOARD" "sudo rm -f $c $r $c_log $r_log"
    if ! ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib mpp_syslog_perror=1 \\
        timeout 300 /tmp/iep2_dect_probe -w $width -h $height -c yuv420p -i $fixture \\
        -C yuv420sp -o $c >$c_log 2>&1"; then
        printf 'FAIL iep-dect/%s (reference failed)\n' "$name"
        fail=1
        return
    fi
    if ! ssh "$BOARD" "cd ~/rk-mpp-rs && sudo timeout 300 ./target/release/rk-mpp-rs \\
        iep-dect $fixture $r $width $height $frames >$r_log 2>&1"; then
        printf 'FAIL iep-dect/%s (rust failed)\n' "$name"
        fail=1
        return
    fi
    ssh "$BOARD" "sudo chmod 644 $c $r $c_log $r_log"
    scp -q "$BOARD":$c "$BOARD":$r "$BOARD":$c_log "$BOARD":$r_log /tmp/ 2>/dev/null
    if ! cmp -s "/tmp/iep_dect_${name}_c.yuv" "/tmp/iep_dect_${name}_r.yuv"; then
        printf 'FAIL iep-dect/%s (pixels differ)\n' "$name"
        fail=1
        return
    fi
    grep -oE 'frame [0-9]+ pd_flag [-0-9]+ pd_types [-0-9]+ dil_order [-0-9]+ frm_mode [-0-9]+ ratio [-0-9]+' \
        "/tmp/iep_dect_${name}_c.log" > "/tmp/iep_dect_${name}_c.frames"
    grep -E '^frame ' "/tmp/iep_dect_${name}_r.log" > "/tmp/iep_dect_${name}_r.frames"
    if ! diff -q "/tmp/iep_dect_${name}_c.frames" "/tmp/iep_dect_${name}_r.frames" >/dev/null; then
        printf 'FAIL iep-dect/%s (pd state differs)\n' "$name"
        diff "/tmp/iep_dect_${name}_c.frames" "/tmp/iep_dect_${name}_r.frames" | head -6
        fail=1
        return
    fi
    local bytes
    bytes=$(wc -c < "/tmp/iep_dect_${name}_c.yuv" | tr -d ' ')
    printf 'PASS iep-dect/%s (%s bytes, %s frames of pd state)\n' "$name" "$bytes" \
        "$(wc -l < "/tmp/iep_dect_${name}_c.frames" | tr -d ' ')"
}

# 3:2 pulldown: 24 fps source telecined to 30 fps. Confirms PULLDOWN 3:2:3:2
# on the sixth check and then cycles CC/NC/NON.
ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc2=size=720x480:rate=24000/1001 \
    -vf telecine -frames:v 20 -pix_fmt yuv420p -f rawvideo /tmp/iep_dect_telecine.yuv"
run_cell telecine /tmp/iep_dect_telecine.yuv 20 720 480

# Progressive content: no pulldown, so both sides must stay at pd_types 4.
ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc2=size=720x480:rate=30 \
    -frames:v 12 -pix_fmt yuv420p -f rawvideo /tmp/iep_dect_progressive.yuv"
run_cell progressive /tmp/iep_dect_progressive.yuv 12 720 480

# Geometry edges: sizes that are not multiples of 16, including an odd chroma
# count (322x242). The probe's stride is the picture width, so padded-stride
# layouts still have no oracle and are not claimed.
for cell in 322x242 722x482 1366x768; do
    w="${cell%x*}"
    h="${cell#*x}"
    ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc2=size=${cell}:rate=30 \
        -frames:v 8 -pix_fmt yuv420p -f rawvideo /tmp/iep_dect_${cell}.yuv"
    run_cell "$cell" "/tmp/iep_dect_${cell}.yuv" 8 "$w" "$h"
done

exit "$fail"
