#!/usr/bin/env bash
BOARD="${BOARD:-t62}"
# Put the board's C reference builds somewhere a cleanup cannot reach.
#
# The oracles - the 2024-04 MPP the encoder is byte-compared against, the master
# build the decoders are compared against, and the ioctl tracer - were built into
# /tmp. `/tmp` on t6 is not a tmpfs (the root filesystem is an overlay with
# upperdir=/data/root), but Debian's tmpfiles policy ships `D /tmp 1777 root root -`
# and `systemd-tmpfiles-clean.timer` is active, and the reference trees live no
# protection there. Rebuilding the 2024-04 library costs the most of anything on
# this board, so the authoritative copies now live under ~/rk-mpp-oracles and /tmp
# is only a working copy this script can refill.
#
# Idempotent: verifies first, restores only what is missing.
#
# Usage: scripts/setup_board_refs.sh [--fresh]   # --fresh re-verifies verbosely
set -uo pipefail

# The board user's home, not the local one: every path below is interpolated into
# an `ssh "$BOARD"` command line, so a `$HOME` here would be the Mac's.
ORACLE_HOME="${ORACLE_HOME:-~/rk-mpp-oracles}"
MPP_OLD_URL="https://github.com/rockchip-linux/mpp"
MPP_OLD_COMMIT="80957d266"

ssh "$BOARD" "mkdir -p $ORACLE_HOME"

check() {   # <label> <test command>
  if ssh "$BOARD" "$2" >/dev/null 2>&1; then
    printf 'ok    %s\n' "$1"; return 0
  fi
  printf 'miss  %s\n' "$1"; return 1
}

restore() { # <name> - copy from the oracle home into /tmp
  ssh "$BOARD" "test -e $ORACLE_HOME/$1 && cp -a $ORACLE_HOME/$1 /tmp/ 2>/dev/null || true"
}

bump() {    # <name> - copy the working tree in /tmp up to the oracle home
  ssh "$BOARD" "test -e /tmp/$1 && cp -a /tmp/$1 $ORACLE_HOME/ 2>/dev/null || true"
}

fail=0

# The encoder oracle.
if ! check "encoder oracle (/tmp/mpp-old)" "test -x /tmp/mpp-old/build/test/mpi_enc_test && test -f /tmp/mpp-old/build/mpp/librockchip_mpp.so.1"; then
  if ssh "$BOARD" "test -x $ORACLE_HOME/mpp-old/build/test/mpi_enc_test"; then
    echo "restoring /tmp/mpp-old from $ORACLE_HOME"
    restore mpp-old
  else
    echo "no copy anywhere: cloning $MPP_OLD_COMMIT and building (needs network + cmake)"
    ssh "$BOARD" "git clone $MPP_OLD_URL $ORACLE_HOME/mpp-old &&
            git -C $ORACLE_HOME/mpp-old checkout $MPP_OLD_COMMIT &&
            cmake -S $ORACLE_HOME/mpp-old -B $ORACLE_HOME/mpp-old/build -DCMAKE_BUILD_TYPE=Release &&
            make -C $ORACLE_HOME/mpp-old/build -j8 rockchip_mpp mpi_enc_test" || fail=1
    restore mpp-old
  fi
fi
check "encoder oracle (/tmp/mpp-old)" "test -x /tmp/mpp-old/build/test/mpi_enc_test && test -f /tmp/mpp-old/build/mpp/librockchip_mpp.so.1" || fail=1

# The decoder oracle.
if ! check "decoder oracle (/tmp/mpp-develop)" "test -x /tmp/mpp-develop/build/test/mpi_dec_test"; then
  if ssh "$BOARD" "test -x $ORACLE_HOME/mpp-develop/build/test/mpi_dec_test"; then
    echo "restoring /tmp/mpp-develop from $ORACLE_HOME"
    restore mpp-develop
  else
    echo "no copy anywhere: cloning master and building (needs network + cmake)"
    ssh "$BOARD" "git clone $MPP_OLD_URL $ORACLE_HOME/mpp-develop &&
            cmake -S $ORACLE_HOME/mpp-develop -B $ORACLE_HOME/mpp-develop/build -DCMAKE_BUILD_TYPE=Release &&
            make -C $ORACLE_HOME/mpp-develop/build -j8 rockchip_mpp mpi_dec_test" || fail=1
    restore mpp-develop
  fi
fi
check "decoder oracle (/tmp/mpp-develop)" "test -x /tmp/mpp-develop/build/test/mpi_dec_test" || fail=1

# The ioctl tracer, rebuilt from this repository's own source.
if ! check "ioctl tracer (/tmp/mpp_trace.so)" "test -f /tmp/mpp_trace.so"; then
  if ssh "$BOARD" "test -f $ORACLE_HOME/mpp_trace.so"; then
    echo "restoring /tmp/mpp_trace.so from $ORACLE_HOME"
    restore mpp_trace.so
  else
    echo "building the tracer from tools/mpp_ioctl_trace.c"
    scp -q tools/mpp_ioctl_trace.c "$BOARD":/tmp/ 2>/dev/null
    ssh "$BOARD" "cc -O2 -shared -fPIC /tmp/mpp_ioctl_trace.c -o $ORACLE_HOME/mpp_trace.so" || fail=1
    restore mpp_trace.so
  fi
fi
check "ioctl tracer (/tmp/mpp_trace.so)" "test -f /tmp/mpp_trace.so" || fail=1

# The stream corpus and the reference outputs the decoder regressions compare to.
# Streams are small (a dozen MB in total) and are not reconstructible without the
# original encoders and settings; the reference YUVs are derivable but expensive to
# re-derive by hand, so both are kept. Our own decoder output (`*_rust.yuv`) and
# scratch dumps are not - they are what the tests produce.
CORPUS="$ORACLE_HOME/corpus"
ssh "$BOARD" "mkdir -p $CORPUS"
ssh "$BOARD" "cd /tmp && cp -a *.264 *.hevc *.ivf *.m2v *.m4v *.263 *.jpg $CORPUS/ 2>/dev/null || true"
ssh "$BOARD" "cd /tmp && for f in *_ffmpeg.yuv *_ffmpeg_*.yuv *_mpp.yuv *_mpp_planar.yuv *_ref*.yuv *_planar.yuv; do
        case \"\$f\" in *_rust.yuv|*_now.yuv|*_repro.yuv|*_trace*.yuv|*src*.yuv) continue;; esac
        test -f \"\$f\" && cp -n \"\$f\" $CORPUS/ 2>/dev/null
      done; true"
if ! ssh "$BOARD" "cd /tmp && ls *.264 >/dev/null 2>&1"; then
  echo "restoring the corpus into /tmp"
  ssh "$BOARD" "cd $CORPUS && cp -n * /tmp/ 2>/dev/null; true"
fi

# Keep the oracle home as current as the working copies, so the next restore is
# the newest verified build rather than an old one.
bump mpp-old
bump mpp-develop
ssh "$BOARD" "test -f /tmp/mpp_trace.so && cp -a /tmp/mpp_trace.so $ORACLE_HOME/mpp_trace.so 2>/dev/null || true"

if [ "$fail" = 0 ]; then
  printf '\nreferences ready; oracle home: %s\n' "$ORACLE_HOME"
fi
exit "$fail"
