#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# Drive one IEP2 (client 28) I5O2 deinterlace and compare it against the C
# `iep2_test` that ships in the reference MPP build.
#
# The capability survey records client 28 as binding, but no job has ever been
# driven through it from this crate, so this script answers the question the
# survey could not: when a real SET_REG_WRITE(struct iep2_params) +
# SET_REG_READ(struct iep2_output) + POLL_HW_FINISH sequence is issued, does the
# hardware reconstruct the same two frames the C library reconstructs?
#
# Both sides read the same raw YUV420-SP file and emit two progressive frames.
# The C test's first dump order for a TFF source is dst[0] then dst[1]; the Rust
# `iep-dei` writes the same two frames in the same order, so a byte compare is
# the check. Motion-history and env-tunable thresholds are at their defaults on
# both sides (md_lambda=4 off RK3528), so frame 0 should agree exactly; if it
# does not, that is a real finding for the ROADMAP, not a script bug.
#
# This script is provided for on-board use and is NEVER run from CI or a host:
# it sshes to t6 and needs /dev/mpp_service + /dev/dma_heap + the IEP2 driver.
#
# Usage: scripts/board_iep_dei_vs_mpp.sh [WIDTH HEIGHT]
set -uo pipefail

W="${1:-720}"
H="${2:-480}"

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

# The IEP2 oracle lives with whichever reference build carries vproc tests.
for ref in /tmp/mpp-old /tmp/mpp-develop '~/rk-mpp-oracles/mpp-old' '~/rk-mpp-oracles/mpp-develop'; do
  for cand in "$ref/build/test/iep2_test" "$ref/build/mpp/vproc/iep2/test/iep2_test"; do
  lib="$ref/build/mpp"
  if ssh "$BOARD" "test -x $cand && test -f $lib/librockchip_mpp.so.0"; then
    MPP_BIN="$cand"; MPP_LIB="$lib"; break
  fi
  if ssh "$BOARD" "test -x $cand && test -f $lib/librockchip_mpp.so.1"; then
    MPP_BIN="$cand"; MPP_LIB="$lib"; break
  fi
  if [ -n "${MPP_BIN:-}" ]; then break; fi
 done
  if [ -n "${MPP_BIN:-}" ]; then break; fi
done

if [ -z "${MPP_BIN:-}" ]; then
  printf 'SKIP IEP (no iep2_test oracle build found; build mpp with -DIEP2_TEST=ON)\n'
  exit 0
fi

in=/tmp/rk-mpp-rs-iep-in.yuv
out_c=/tmp/rk-mpp-rs-iep-c.yuv
out_r=/tmp/rk-mpp-rs-iep-rust.yuv

# A deterministic 3-frame YUV420-SP fixture (I5O2 wants prev/current/next).
# A heredoc inside a double-quoted ssh argument is a bash 3.2 trap (HANDOFF
# 6.11): ship the generator as its own file instead.
here="$(cd "$(dirname "$0")" && pwd)"
scp -q "$here/gen_iep_fixture.py" "$BOARD":/tmp/gen_iep_fixture.py
ssh "$BOARD" "python3 /tmp/gen_iep_fixture.py $in $W $H 3"

# Reference: TFF source, three frames in, two frames out.
ssh "$BOARD" "cd ~/rk-mpp-rs && sudo env LD_LIBRARY_PATH=$MPP_LIB timeout 180 \
    $MPP_BIN -w $W -h $H -c yuv420sp -i $in -C yuv420sp -o $out_c -f TFF >/dev/null 2>&1"
ssh "$BOARD" "sudo chmod 644 $out_c 2>/dev/null || true"

# This driver, same picture.
ssh "$BOARD" "cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs iep-dei $in $out_r $W $H TFF"

c_size=$(ssh "$BOARD" "stat -c %s $out_c 2>/dev/null" || echo missing)
r_size=$(ssh "$BOARD" "stat -c %s $out_r 2>/dev/null" || echo missing)
if [ -z "$c_size" ] || [ "$c_size" = 0 ] || [ -z "$r_size" ] || [ "$r_size" = 0 ]; then
  printf 'FAIL IEP (one side produced no output; C=%s rust=%s)\n' "$c_size" "$r_size"
  exit 1
fi

if ssh "$BOARD" "cmp -s $out_c $out_r"; then
  printf 'PASS IEP deinterlace (both reconstruct %sx%s, %s bytes identical)\n' \
    "$W" "$H" "$c_size"
else
  printf 'FAIL IEP deinterlace (Rust output differs from C iep2_test)\n'
  ssh "$BOARD" "cmp -l $out_c $out_r | head -20"
  exit 1
fi
