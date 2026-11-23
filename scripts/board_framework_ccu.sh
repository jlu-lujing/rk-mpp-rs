#!/usr/bin/env bash
set -euo pipefail

BOARD="${BOARD:?set BOARD to a Rockchip SSH alias}"
REPEATS="${REPEATS:-3}"

compatible=$(ssh "$BOARD" 'tr -d "\0" < /proc/device-tree/compatible')
case "$compatible" in
  *rk3588*) TEST=two_jpeg_sessions_submit_through_one_ccu_batch ;;
  *rk3576*) TEST=two_vepu510_sessions_submit_through_one_ccu_batch ;;
  *)
    printf 'unsupported CCU SoC on %s: %s\n' "$BOARD" "$compatible" >&2
    exit 2
    ;;
esac

COPYFILE_DISABLE=1 tar --no-xattrs \
  --exclude .git --exclude target --exclude .worktrees --exclude refs \
  -cf - . |
  ssh "$BOARD" 'tar -xf - -C "$HOME/rk-mpp-rs"'

ssh -n "$BOARD" '
  export PATH="$HOME/.cargo/bin:$PATH"
  cd "$HOME/rk-mpp-rs"
  for run in $(seq 1 '"$REPEATS"'); do
    printf "CCU run %s/%s\n" "$run" '"$REPEATS"'
    timeout 180 cargo test --test framework_ccu_hw \
      '"$TEST"' \
      -- --ignored --nocapture --test-threads=1
  done
'
