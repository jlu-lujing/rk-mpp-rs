#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# C consumer that uses only official MPP headers plus this crate's library.
# Covers the migration-critical surface added for the drop-in path: log
# level/callback, frame SAR/HDR metadata and the mpp_compat table.
set -euo pipefail
tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock \
    --exclude .worktrees --exclude refs -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'
inc=$(ssh "$BOARD" 'for d in /tmp/mpp-develop/inc "$HOME/rk-mpp-oracles/mpp-develop/inc"; do
    [ -f "$d/mpp_frame.h" ] && printf "%s\n" "$d" && break; done')
if [ -z "$inc" ]; then
    source "$(cd "$(dirname "$0")" && pwd)/scripts/board_dependency.sh" \
        "MPP migration" "official MPP headers not found"
fi
ssh "$BOARD" "cd ~/rk-mpp-rs &&
  gcc -std=c11 -Wall -Wextra -Werror -Wno-unused-parameter -I$inc \
      tests/c_api_migration.c -Ltarget/release -lrk_mpp_rs \
      -Wl,-rpath,\$PWD/target/release -o /tmp/rk_mpp_migration &&
  mkdir -p /tmp/rk_mpp_migration_lib &&
  ln -sf \$PWD/target/release/librk_mpp_rs.so /tmp/rk_mpp_migration_lib/librk_mpp_rs.so.1 &&
  LD_LIBRARY_PATH=/tmp/rk_mpp_migration_lib /tmp/rk_mpp_migration"
