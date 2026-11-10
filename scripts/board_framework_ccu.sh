#!/usr/bin/env bash
set -euo pipefail

BOARD="${BOARD:-t6}"
REPEATS="${REPEATS:-3}"

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
      two_jpeg_sessions_submit_through_one_ccu_batch \
      -- --ignored --nocapture --test-threads=1
  done
'
