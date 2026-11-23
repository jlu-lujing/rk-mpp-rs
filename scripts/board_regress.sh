#!/usr/bin/env bash
BOARD="${BOARD:?set BOARD to a Rockchip SSH alias}"
# H.264 regression: sync, build on the board, decode every stream, compare
# with the reference decoder output.
# Fixtures may live in /tmp or in the oracle corpus; both roots are searched so
# a fresh boot does not turn the whole suite into silent SKIPs.
# Usage: scripts/board_regress.sh
set -euo pipefail

streams=(
  "/tmp/test720.264:/tmp/o720_ffmpeg_full.yuv"
  "/tmp/test_hard.264:/tmp/o1080_hard_ffmpeg.yuv"
  "/tmp/test720_slices.264:/tmp/o_slices_ref_planar.yuv"
  "/tmp/test720_i.264:/tmp/o_i_ffmpeg.yuv"
  "/tmp/field_paff.264:/tmp/o_paff_ffmpeg.yuv"
  "/tmp/mr1_longterm.264:/tmp/o_mr1_ffmpeg.yuv"
  "/tmp/mr1_missing_au20.264:/tmp/o_mr1_missing_mpp_planar.yuv"
)

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

# Absolute corpus root on the board; `~` is expanded remotely via $HOME.
corpus=$(ssh "$BOARD" 'printf "%s\n" "$HOME/rk-mpp-oracles/corpus"')

# Resolve a fixture name against the search roots, printing the first hit.
remote_find() {
  local name=$1 root found
  for root in /tmp "$corpus"; do
    found=$(ssh "$BOARD" "p=\"$root/$name\"; if test -f \"\$p\"; then printf '%s\n' \"\$p\"; fi")
    if [ -n "$found" ]; then
      printf '%s\n' "$found"
      return 0
    fi
  done
  return 1
}

fail=0
for pair in "${streams[@]}"; do
  in_name="${pair%%:*}"; in_name="${in_name##*/}"
  ref_name="${pair##*:}"; ref_name="${ref_name##*/}"
  if ! in=$(remote_find "$in_name"); then
    source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
      "H.264 regression" "stream missing at $in_name"
  fi
  if ! ref=$(remote_find "$ref_name"); then
    source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
      "H.264 regression" "reference output $ref_name missing"
  fi
  out="/tmp/${ref_name%.yuv}_rust.yuv"
  ssh "$BOARD" "sudo rm -f $out"
  if ! ssh "$BOARD" "cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264dec $in $out" >/dev/null 2>&1; then
    printf 'FAIL %s (decoder exited non-zero)\n' "$in"
    fail=1
    continue
  fi
  if ssh "$BOARD" "cmp -s $ref $out"; then
    printf 'PASS %s\n' "$in"
  else
    printf 'FAIL %s\n' "$in"
    fail=1
  fi
done
exit $fail
