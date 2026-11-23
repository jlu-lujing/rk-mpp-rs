#!/usr/bin/env bash
BOARD="${BOARD:?set BOARD to a Rockchip SSH alias}"
# W5-8 recovery gate: failed session replacement, fd lifecycle, H.264 reuse
# after recovery, and the legacy VDPU2 STATUS-error path.
#
# The reset-only probe is intentionally not repeated: on RK3588 it returned success
# but poisoned the old fd for the next decode job. This gate exercises the
# replacement-session implementation instead.
#
# Usage: scripts/board_framework_recovery.sh
set -euo pipefail

ssh -n "$BOARD" 'mkdir -p "$HOME/rk-mpp-rs"'
COPYFILE_DISABLE=1 tar --no-xattrs \
  --exclude .git --exclude target --exclude .worktrees --exclude refs \
  -cf - . |
  ssh "$BOARD" 'tar -xf - -C "$HOME/rk-mpp-rs"'

ssh -n "$BOARD" '
  export PATH="$HOME/.cargo/bin:$PATH"
  cd "$HOME/rk-mpp-rs"
  cargo test --test framework_recovery_hw \
    session_replacement -- --ignored --nocapture --test-threads=1
'

ssh -n "$BOARD" '
  export PATH="$HOME/.cargo/bin:$PATH"
  cd "$HOME/rk-mpp-rs"
  timeout 120 cargo test --test framework_recovery_hw \
    h265_tile_restarts_with_idr_after_an_error_reopens_the_service \
    -- --ignored --nocapture --test-threads=1
'

ssh -n "$BOARD" '
  export PATH="$HOME/.cargo/bin:$PATH"
  cd "$HOME/rk-mpp-rs"
  cargo test --test vp_streams \
    vp8_stream_policy_marks_a_damaged_middle_packet \
    -- --ignored --nocapture --test-threads=1
'
