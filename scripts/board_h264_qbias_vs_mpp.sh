#!/usr/bin/env bash
# Verify hw:qbias_en/i/p through the native H.264 encoder.
set -euo pipefail

BOARD="${BOARD:?set BOARD to the RK3588 SSH alias}"
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
remote_repo="${REMOTE_REPO:-\$HOME/rk-mpp-rs}"
mpp_ref="${MPP_REF:-\$HOME/rk-mpp-oracles/mpp-old}"
frames="${FRAMES:-60}"
fixture="${FIXTURE:-/tmp/rk_mpp_h264_qbias_noise.nv12}"
probe=/tmp/h264e_probe_qbias

ssh "$BOARD" "ffmpeg -nostdin -y -hide_banner -loglevel error \
  -f lavfi -i 'testsrc2=size=320x240:rate=30,noise=alls=100:allf=t+u' \
  -frames:v '$frames' -pix_fmt nv12 -f rawvideo '$fixture'"

ssh "$BOARD" "cc -O2 -w -pthread -DCUSTOM_RC_HAS_BIT_TARGET_FIX=0 \
  -I\"$mpp_ref/inc\" -I\"$mpp_ref/utils\" -I\"$mpp_ref/osal/inc\" \
  \"$mpp_ref/utils/mpp_enc_roi_utils.c\" \"$remote_repo/tools/h264e_probe.c\" \
  \"$mpp_ref/build/mpp/librockchip_mpp.so.0\" -o '$probe'"

ssh "$BOARD" "sudo env LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" \
  QBIAS_EN=1 QBIAS_I=700 QBIAS_P=300 timeout 120 '$probe' '$fixture' \
  /tmp/rk_mpp_h264_qbias_c.264 320 240 '$frames' 0 26 \
  288000:1500000:2500000 headereach </dev/null >/dev/null \
  2>/tmp/rk_mpp_h264_qbias_c.log"

ssh "$BOARD" "sudo env H264_QBIAS_EN=1 H264_QBIAS_I=700 H264_QBIAS_P=300 \
  timeout 120 \"$remote_repo/target/release/rk-mpp-rs\" h264-enc '$fixture' \
  /tmp/rk_mpp_h264_qbias_r.264 320 240 26 baseline '$frames' 60 \
  </dev/null >/dev/null 2>/tmp/rk_mpp_h264_qbias_r.log"

scp -q "$BOARD":/tmp/rk_mpp_h264_qbias_c.264 \
  "$BOARD":/tmp/rk_mpp_h264_qbias_r.264 /tmp/
python3 "$repo/scripts/compare_streams.py" \
  /tmp/rk_mpp_h264_qbias_c.264 /tmp/rk_mpp_h264_qbias_r.264
printf 'PASS H.264 VEPU580 qbias\n'
