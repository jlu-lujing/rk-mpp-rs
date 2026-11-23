#!/usr/bin/env bash
BOARD="${BOARD:?set BOARD to a Rockchip SSH alias}"
# SPS level: our encoder against the reference library, NAL by NAL.
#
# The level is the last encoder property that used to be frozen into the recorded
# register image. What it reaches is `mv_limit`, the CIME vertical search window,
# `rect_size` and the SPS's own `level_idc` - and MPP raises the level to whatever
# the coded size needs (`h264e_sps_update`), which the HAL fields then read. That
# bump is covered deliberately by the second size: at 320x240 a request of 1.0
# becomes 1.1 and the request reads through almost as-is, while at 1280x720 every
# request below 3.1 is raised to it, so the rows there prove the HAL follows the
# raised value (a request of 1.0 there must produce mv_limit 2, `rect_size` 0 and
# the full 28-block CIME window, not what 1.0 itself would give).
#
# Two frames in the 320x240 cells, because `mv_limit` and the CIME window only reach a
# byte when a P slice runs motion search; the fixture moves so the search is
# not trivially at zero.
#
# The reference side is `mpi_enc_test` with the level patched into its source
# the way `board_h264_qp_vs_mpp.sh` patches the profile: fixed QP 26, Baseline,
# both sides configure nothing else beyond the level.
#
# Usage: scripts/board_h264_level_vs_mpp.sh
set -uo pipefail

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_src="$mpp_ref/test/mpi_enc_test.c"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
fail=0

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

if ! ssh "$BOARD" "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.0 && test -f $mpp_src"; then
  source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
    "H.264 level matrix" "reference build missing at $mpp_ref"
fi
ssh "$BOARD" "test -f /tmp/mpi_enc_test.c.orig || cp $mpp_src /tmp/mpi_enc_test.c.orig"

configure_reference_level() {
  ssh "$BOARD" "cp /tmp/mpi_enc_test.c.orig $mpp_src &&
          sed -i -e 's/\"h264:profile\", [0-9]*/\"h264:profile\", 66/' \
                 -e 's/\"h264:cabac_en\", [0-9]*/\"h264:cabac_en\", 0/' \
                 -e 's/\"h264:trans8x8\", [0-9]*/\"h264:trans8x8\", 0/' \
                 -e 's/\"h264:level\", [0-9]*/\"h264:level\", $1/' $mpp_src &&
          make -C $mpp_ref/build -j8 mpi_enc_test" >/dev/null 2>&1
}

compare() {
  scp -q scripts/compare_streams.py "$BOARD":/tmp/ 2>/dev/null
  ssh "$BOARD" "python3 /tmp/compare_streams.py /tmp/$1 /tmp/$2"
}

# size:frames:levels - 320x240 raises to 1.1, so rows below it exercise the
# per-level switch with a P frame as well. 1280x720 raises to 3.1, proving the
# HAL follows the raised level, but compares the IDR only: byte-exactness of P
# slices above 320x240 is an open item of its own (ROADMAP), unrelated to the
# level.
run_size() {
  local width="$1" height="$2" frames="$3" levels="$4"
  local frame="/tmp/rk-mpp-rs-lvl-move-${width}x${height}.nv12"
  if ! ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc=size=${width}x${height}:rate=30 \
      -frames:v "$frames" -pix_fmt nv12 -f rawvideo -an $frame" >/dev/null 2>&1; then
    printf 'FAIL %sx%s (could not build the moving fixture)\n' "$width" "$height"
    fail=1
    return
  fi
  for level in $levels; do
    local tag="${width}x${height}-lvl${level}"
    local c_out="/tmp/lvl-${tag}-c.264"
    local rust_out="/tmp/lvl-${tag}-rust.264"
    configure_reference_level "$level" ||
      { printf 'FAIL %-20s (could not configure reference)\n' "$tag"; fail=1; continue; }
    ssh "$BOARD" "sudo rm -f $c_out $rust_out"
    if ! ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 $mpp_bin -i $frame -o $c_out \
        -w $width -h $height -t 7 -f 0 -n "$frames" -rc 2 -qc 26:26:26:26:26 -v q" \
        >/dev/null 2>&1; then
      printf 'FAIL %-20s (reference encode failed)\n' "$tag"; fail=1; continue
    fi
    if ! ssh "$BOARD" "cd ~/rk-mpp-rs && timeout 120 sudo env RKMPP_LEVEL=$level \
        ./target/release/rk-mpp-rs h264-enc $frame $rust_out $width $height 26 baseline "$frames" 60" \
        >/dev/null 2>&1; then
      printf 'FAIL %-20s (our encode failed)\n' "$tag"; fail=1; continue
    fi
    ssh "$BOARD" "sudo chmod 644 $c_out $rust_out"
    scp -q "$BOARD":"$c_out" "$BOARD":"$rust_out" /tmp/ 2>/dev/null
    if compare "${c_out##*/}" "${rust_out##*/}"; then
      if [ "$frames" -gt 1 ]; then
        printf 'PASS %-20s (SPS, PPS, SEI, IDR and P byte-identical)\n' "$tag"
      else
        printf 'PASS %-20s (SPS, PPS, SEI and IDR byte-identical)\n' "$tag"
      fi
    else
      printf 'FAIL %-20s (bitstreams differ)\n' "$tag"; fail=1
    fi
  done
}

if ssh "$BOARD" 'ps -eo comm | grep -q "^mpi_"'; then
  printf 'ABORT: a leftover mpi_* process holds the encoder (see HANDOFF 6.14)\n'
  exit 1
fi

run_size 320 240 2 "9 10 11 20 31 40 42 51 52"
run_size 1280 720 1 "10 20 31 52"

# Leave the reference harness on the Baseline pin at the default level.
configure_reference_level 30
exit "$fail"
