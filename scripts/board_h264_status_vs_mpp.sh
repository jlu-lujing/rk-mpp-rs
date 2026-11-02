#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# Compare this driver's STATUS read-back against the C encoder's, field by field.
#
# `Statistics` takes its positions from `Vepu580Status_t`, but the check that
# matters is behavioural: encode the same pictures with both encoders and require
# the window to agree. Everything downstream - the scene motion level, and rate
# control once it lands - reads these words, and an offset that is one word off
# feeds it a plausible zero rather than failing anything.
#
# One word is expected to differ: reg4107, `st_bsb.bsbw_addr`, the bitstream
# address the hardware wrote to. Two processes have different dma-bufs, so that
# difference is the point of comparing fields rather than the whole window.
#
# Usage: scripts/board_h264_status_vs_mpp.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
fixture=/tmp/rk-mpp-rs-status-2f.nv12

if ! ssh "$BOARD" "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.0 && test -f /tmp/mpp_trace.so"; then
  printf 'SKIP STATUS (reference build or ioctl tracer missing)\n'
  exit 0
fi

ssh "$BOARD" "cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame /tmp/rk-mpp-rs-1f.nv12 320 240 >/dev/null &&
        rm -f $fixture && cp /tmp/rk-mpp-rs-1f.nv12 $fixture && cat /tmp/rk-mpp-rs-1f.nv12 >> $fixture &&
        sudo rm -f /tmp/stat_c.txt /tmp/stat_rust.txt && stat -c "%n %s" $fixture"

# The tracer prints the whole window per read; keep only the STATUS lines so the
# comparison input stays small enough to reason about.
ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib LD_PRELOAD=/tmp/mpp_trace.so timeout 180 $mpp_bin \
    -i $fixture -o /tmp/rk-mpp-rs-status-c.264 -w 320 -h 240 -t 7 -f 0 -n 2 \
    -rc 2 -qc 26:26:26:26:26 -v q 2>&1 |
    grep -A1 'read_complete\[1\]@16384' > /tmp/stat_c.txt"

ssh "$BOARD" "cd ~/rk-mpp-rs && sudo env RKMPP_STAT_DUMP=/tmp/stat_rust.txt \
    ./target/release/rk-mpp-rs h264-enc $fixture /tmp/rk-mpp-rs-status-rust.264 \
    320 240 26 baseline 2 120" >/dev/null 2>&1
ssh "$BOARD" "sudo chmod 644 /tmp/stat_rust.txt"
scp -q scripts/stat_compare.py "$BOARD":/tmp/ 2>/dev/null

if ssh "$BOARD" "python3 /tmp/stat_compare.py /tmp/stat_c.txt /tmp/stat_rust.txt"; then
  printf 'PASS STATUS (every field the encoder consumes agrees with C)\n'
else
  printf 'FAIL STATUS (statistics disagree)\n'
  exit 1
fi
