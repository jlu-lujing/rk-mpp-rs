#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# H.265 horizontal tiles. RK3588 emits two uniform columns for auto_tile below
# 4K and for any width above 4096. Four columns exist only above 8192, which is
# outside the H.265 13-bit picture-width limit, so the reachable matrix is two.
set -uo pipefail

GOP=60
mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"

scp -q tools/h265e_probe.c "$BOARD":/tmp/h265e_probe_tile.c
ssh "$BOARD" "cc -O2 -w -pthread -I$mpp_ref/inc /tmp/h265e_probe_tile.c \
  $mpp_lib/librockchip_mpp.so.1 -o /tmp/h265e_probe_tile 2>/dev/null ||
  cc -O2 -w -pthread -I$mpp_ref/inc /tmp/h265e_probe_tile.c \
  $mpp_lib/librockchip_mpp.so.0 -o /tmp/h265e_probe_tile" || true

fail=0
run_cell() {
  local name="$1" width="$2" height="$3" frames="$4" auto_tile="$5" rust_tile="$6"
  local fixture=/tmp/h265_tile_${name}.nv12
  local c=/tmp/h265_tile_${name}_c.265
  local ours=/tmp/h265_tile_${name}_r.265
  # `mpi_enc_test`'s own default target when `-bps` is absent. Fixed QP does not
  # spend it, but `update_rc_cfg_log` prints it into the rc-cfg SEI, and the Rust
  # CLI derives the same number from the picture size, so the harness has to hand
  # C the matching value rather than a hard-coded one.
  local bps=$((width * height * 30 / 8))

  ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc2=size=${width}x${height}:rate=30 \
    -frames:v $frames -pix_fmt nv12 -f rawvideo $fixture" >/dev/null 2>&1
  # The reference is bimodal for the first tiled picture of a session: the same
  # probe with the same arguments emits `9724d029..` right after one of our
  # sessions and `b27d68dc..` right after its own, and the two decode to
  # different pixels (`7dc42fe5..` against `c9fcd4f7..`) because the VEPU580
  # carries dual-core state between processes and the first tiled job reads it.
  # Every observation so far says the first state is the reproducible one and
  # that it equals ours, so the C side is measured in that state by design: one
  # throwaway encode of ours immediately before it, rather than whatever the
  # previous script happened to leave behind. Without this the cell passes or
  # fails depending on the run order of unrelated scripts.
  ssh "$BOARD" "cd ~/rk-mpp-rs && sudo env H265_GOP=$GOP \
    ./target/release/rk-mpp-rs h265-gop $fixture /tmp/h265_tile_${name}_warm.265 \
    $width $height $frames 26 shortterm $rust_tile >/dev/null 2>&1"
  ssh "$BOARD" "sudo rm -f $c $ours &&
    sudo env LD_LIBRARY_PATH=$mpp_lib FRAMES=$frames GOP=$GOP AUTO_TILE=$auto_tile \
      /tmp/h265e_probe_tile $fixture $c $width $height 0 26 \
        $bps:1500000:2500000 >/dev/null 2>&1 &&
    cd ~/rk-mpp-rs && sudo env H265_GOP=$GOP \
      ./target/release/rk-mpp-rs h265-gop $fixture $ours \
      $width $height $frames 26 shortterm $rust_tile >/dev/null &&
    sudo chmod 644 $c $ours"
  if ! ssh "$BOARD" "test -s $c && test -s $ours"; then
    printf 'FAIL h265-tile/%s (one side produced no stream)\n' "$name"
    fail=1
    return
  fi
  scp -q "$BOARD":$c "$BOARD":$ours /tmp/ 2>/dev/null
  if python3 scripts/compare_streams.py "$c" "$ours"; then
    printf 'PASS h265-tile/%s\n' "$name"
  else
    printf 'FAIL h265-tile/%s (bitstreams differ)\n' "$name"
    fail=1
  fi
}

# AUTO_TILE=1 at 320x240 covers two tiles across an IDR and P picture.
run_cell auto320 320 240 2 1 "tile:auto"
# 4608x64 automatically selects two tiles. Neither encoder produces a second
# picture at that shape - the C stream stays at its one-picture size and Rust
# reports int_sta=0x00000100 - so the reachable cell here is IDR-only.
run_cell wide4608 4608 64 1 0 ""

exit "$fail"
