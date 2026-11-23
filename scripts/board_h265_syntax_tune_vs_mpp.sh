#!/usr/bin/env bash
BOARD="${BOARD:?set BOARD to a Rockchip SSH alias}"
# H.265 deblock/SAO/loop-filter controls against the C encoder. The C probe and
# Rust use the same harness bps triple so the rc-cfg SEI is comparable too.
set -uo pipefail

FRAMES=2
GOP=60
mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"
tracer="${TRACER:-/tmp/mpp_trace.so}"
fixture=/tmp/rk-mpp-rs-h265-tune-320x240.nv12

scp -q tools/h265e_probe.c "$BOARD":/tmp/h265e_probe_tune.c
ssh "$BOARD" "cc -O2 -w -pthread -I$mpp_ref/inc /tmp/h265e_probe_tune.c \
  $mpp_lib/librockchip_mpp.so.1 -o /tmp/h265e_probe_tune 2>/dev/null ||
  cc -O2 -w -pthread -I$mpp_ref/inc /tmp/h265e_probe_tune.c \
  $mpp_lib/librockchip_mpp.so.0 -o /tmp/h265e_probe_tune" || true
ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc2=size=320x240:rate=30 \
  -frames:v $FRAMES -pix_fmt nv12 -f rawvideo $fixture" >/dev/null 2>&1

fail=0
run_cell() {
  local name="$1"
  local c_env="$2"
  local rust_opts="$3"
  local c=/tmp/h265_tune_${name}_c.265
  local ours=/tmp/h265_tune_${name}_r.265
  local log=/tmp/h265_tune_${name}_c.log

  ssh "$BOARD" "sudo rm -f $c $ours $log &&
    sudo env LD_LIBRARY_PATH=$mpp_lib LD_PRELOAD=$tracer \
      FRAMES=$FRAMES GOP=$GOP $c_env \
      /tmp/h265e_probe_tune $fixture $c 320 240 0 26 \
        288000:1500000:2500000 >/dev/null 2>$log &&
    cd ~/rk-mpp-rs && sudo env H265_GOP=$GOP \
      ./target/release/rk-mpp-rs h265-gop $fixture $ours \
      320 240 $FRAMES 26 shortterm $rust_opts >/dev/null &&
    sudo chmod 644 $c $ours"
  if ! ssh "$BOARD" "test -s $c && test -s $ours"; then
    printf 'FAIL h265-tune/%s (one side produced no stream)\n' "$name"
    fail=1
    return
  fi
  scp -q "$BOARD":$c "$BOARD":$ours /tmp/ 2>/dev/null
  if python3 scripts/compare_streams.py "$c" "$ours"; then
    printf 'PASS h265-tune/%s\n' "$name"
  else
    printf 'FAIL h265-tune/%s (bitstreams differ)\n' "$name"
    fail=1
  fi
}

run_cell default \
  "" \
  "dblk:0:0:0 sao:0:0 lpf:0:0"
run_cell beta_tc_sao_chroma \
  "DBLK_DISABLE=0 DBLK_BETA=3 DBLK_TC=-2 SAO_LUMA_DISABLE=0 SAO_CHROMA_DISABLE=1 LPF_ACS_SLI=1 LPF_ACS_TILE_DISABLE=1" \
  "dblk:0:3:-2 sao:0:1 lpf:1:1"
run_cell deblock_off \
  "DBLK_DISABLE=1 SAO_LUMA_DISABLE=0 SAO_CHROMA_DISABLE=0 LPF_ACS_SLI=0 LPF_ACS_TILE_DISABLE=0" \
  "dblk:1:0:0 sao:0:0 lpf:0:0"
run_cell sao_off \
  "DBLK_DISABLE=0 SAO_LUMA_DISABLE=1 SAO_CHROMA_DISABLE=1 LPF_ACS_SLI=0 LPF_ACS_TILE_DISABLE=1" \
  "dblk:0:0:0 sao:1:1 lpf:0:1"

exit "$fail"
