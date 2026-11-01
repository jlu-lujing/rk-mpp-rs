#!/usr/bin/env bash
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

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
  ssh t6 'tar -xf - -C ~/rk-mpp-rs'
ssh t6 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

# The IEP2 oracle lives with whichever reference build carries vproc tests.
for ref in /tmp/mpp-old /tmp/mpp /home/user/rk-mpp-oracles/mpp; do
  cand="$ref/build/test/iep2_test"
  lib="$ref/build/mpp"
  if ssh t6 "test -x $cand && test -f $lib/librockchip_mpp.so.0"; then
    MPP_BIN="$cand"; MPP_LIB="$lib"; break
  fi
  if ssh t6 "test -x $cand && test -f $lib/librockchip_mpp.so.1"; then
    MPP_BIN="$cand"; MPP_LIB="$lib"; break
  fi
done

if [ -z "${MPP_BIN:-}" ]; then
  printf 'SKIP IEP (no iep2_test oracle build found; build mpp with -DIEP2_TEST=ON)\n'
  exit 0
fi

in=/tmp/rk-mpp-rs-iep-in.yuv
out_c=/tmp/rk-mpp-rs-iep-c.yuv
out_r=/tmp/rk-mpp-rs-iep-rust.yuv

# A deterministic 3-frame YUV420-SP fixture (I5O2 wants prev/current/next).
ssh t6 "python3 - '$W' '$H' '$in' <<'PY'
import sys, struct
w, h, path = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
frame = w*h*3//2
with open(path,'wb') as f:
    for n in range(3):
        for y in range(h):
            f.write(bytes(((y*3 + n*7) & 0xff) for _ in range(w)))
        for cy in range(h//2):
            f.write(bytes((128 + ((cy*5 + n*3) & 0x1f) - 16) & 0xff for _ in range(w)))
PY"

# Reference: TFF source, three frames in, two frames out.
ssh t6 "cd ~/rk-mpp-rs && sudo env LD_LIBRARY_PATH=$MPP_LIB timeout 180 \
    $MPP_BIN -w $W -h $H -c yuv420sp -i $in -C yuv420sp -o $out_c -f TFF >/dev/null 2>&1"
ssh t6 "sudo chmod 644 $out_c 2>/dev/null || true"

# This driver, same picture.
ssh t6 "cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs iep-dei $in $out_r $W $H TFF"

if ! ssh t6 "test -s $out_c && test -s $out_r"; then
  printf 'FAIL IEP (one side produced no output; C=%s rust=%s)\n' \
    "$(ssh t6 'stat -c %s '"$out_c"' 2>/dev/null || echo missing)" \
    "$(ssh t6 'stat -c %s '"$out_r"' 2>/dev/null || echo missing)"
  exit 1
fi

if ssh t6 "cmp -s $out_c $out_r"; then
  printf 'PASS IEP deinterlace (both reconstruct %sx%s, %s bytes identical)\n' \
    "$W" "$H" "$(ssh t6 'stat -c %s '"$out_c"' 2>/dev/null)"
else
  printf 'FAIL IEP deinterlace (Rust output differs from C iep2_test)\n'
  ssh t6 "cmp -l $out_c $out_r | head -20"
  exit 1
fi
