#!/usr/bin/env bash
# Extended RK3588 4K encoder feature matrix. Complements board_4k_matrix.sh,
# which covers the codec baseline. This gate focuses on feature interactions
# that only become expensive, overflow-prone or stride-sensitive at 4K.
set -uo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

: "${BOARD:?set BOARD to the SSH alias of an RK3588 board}"
MPP_REF="${MPP_REF:-/tmp/mpp-old}"
export BOARD MPP_REF

if ! ssh "$BOARD" "test -d $MPP_REF/inc && test -d $MPP_REF/build"; then
    printf 'missing reference MPP at %s on %s\n' "$MPP_REF" "$BOARD" >&2
    exit 2
fi

if [ "${SKIP_BUILD:-0}" != 1 ]; then
    COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target \
        --exclude Cargo.lock --exclude .worktrees --exclude refs \
        --exclude '._*' -cf - . |
        ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
    ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'
fi

passed=0
failed=0

run_gate() {
    local name="$1"
    shift
    printf '\n== %s ==\n' "$name"
    if "$@"; then
        passed=$((passed + 1))
    else
        failed=$((failed + 1))
    fi
}

run_remote() {
    local name="$1"
    local command="$2"
    printf '\n== %s ==\n' "$name"
    if ssh "$BOARD" "cd ~/rk-mpp-rs && $command"; then
        passed=$((passed + 1))
    else
        failed=$((failed + 1))
    fi
}

run_gate "H.264 fixed QP: profiles x QP" \
    env SKIP_BUILD=1 ONLY=sizes SIZES=3840x2160 \
    scripts/board_h264_qp_vs_mpp.sh

run_gate "H.264 rate control and hierarchical QP" \
    env SKIP_BUILD=1 SIZE='3840 2160' FRAMES=2 \
    scripts/board_h264_cbr_vs_mpp.sh

run_gate "H.264 ROI / QP map" \
    env SKIP_BUILD=1 SIZE='3840 2160' FRAMES=2 BPS_FIXQP=31104000 \
    BPS='300000:250000:350000' \
    ROI1='480 320 1280 1600 0 1 24' \
    ROI2='1600 640 960 1280 1 1 10' \
    OURS_ROI1='roi:480:320:1280:1600:0:1:24' \
    OURS_ROI2='roi:1600:640:960:1280:1:1:10' \
    scripts/board_h264_roi_vs_mpp.sh

run_gate "H.264 slice split past the FIFO" \
    env SKIP_BUILD=1 SIZE='3840 2160' FRAMES=2 BPS_FIXQP=31104000 \
    SLICE_CELLS='ctu_96 byte_256' scripts/board_h264_slice_vs_mpp.sh

run_gate "H.265 CBR / VBR / AVBR / smart GOP" \
    env SKIP_BUILD=1 SIZES=3840x2160 scripts/board_h265_cbr_vs_mpp.sh

run_gate "H.265 ROI / QP map" \
    env SKIP_BUILD=1 RUN_4K=1 scripts/board_h265_roi_vs_mpp.sh

run_gate "H.265 slice split" \
    env SKIP_BUILD=1 RUN_4K=1 scripts/board_h265_slice_vs_mpp.sh

run_gate "H.265 auto tiles" \
    env RUN_4K=1 RUN_4K_ONLY=1 scripts/board_h265_tile_vs_mpp.sh

run_gate "H.265 GDR / intra refresh" \
    env SKIP_BUILD=1 RUN_4K=1 FRAMES=8 scripts/board_h265_gdr_vs_mpp.sh

run_remote "H.264/H.265 4K OSD" \
    "SIZE=3840x2160 FRAMES=2 SKIP_BUILD=1 scripts/board_mpp_osd.sh"

run_remote "H.265 forced IDR / header mode" \
    "SIZE=3840x2160 FRAMES=6 SKIP_BUILD=1 scripts/board_h265_force_idr.sh"

run_remote "H.264/H.265 custom RC" \
    "SIZE=3840x2160 SKIP_BUILD=1 scripts/board_mpp_custom_rc.sh"

run_remote "4K long GOP / DPB soak" \
    "SKIP_BUILD=1 scripts/board_4k_soak.sh"

run_gate "MJPEG multi-core and rotation" \
    env SKIP_BUILD=1 RUN_4K=1 scripts/board_jpege_multicore_vs_mpp.sh

run_gate "MJPEG RGB input" \
    env SKIP_BUILD=1 RUN_4K=1 scripts/board_jpege_rgb_vs_mpp.sh

printf '\nSUMMARY 4K features passed=%d failed=%d\n' "$passed" "$failed"
exit "$failed"
