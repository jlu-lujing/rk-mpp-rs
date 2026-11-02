#!/usr/bin/env bash
BOARD="${BOARD:-t62}"
# Capture what the C encoder's rate control writes, as the target for the port.
#
# Nothing here compares anything yet - this crate cannot produce a rate-controlled
# job. What it does is record the reference's own decisions in a shape the port can
# be measured against: the fixed-QP job and the CBR job of the same picture, side
# by side, and the statistics the hardware reported back per job.
#
# Two things were checked before this was treated as an oracle. The per-job report
# is identical across runs even though the raw trace is not (the tracer prints a
# per-call buffer address, so the logs differ at byte 58 while meaning the same
# thing), and the CBR bitstream is byte-identical across runs - md5 of the six
# frames at 320x240 / 300 kbps is c34eba92a3196865d04ac23253b2f7af.
#
# REFRESH=1 rewrites the two fixtures under tests/data with the current output:
# the report, and the two jobs as register words.
#
# Usage: scripts/board_h264_rc_capture.sh
set -uo pipefail

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
fixture=/tmp/rk-mpp-rs-move-320x240.nv12
out=tests/data/vepu580_rc_c_move6_cbr.txt
regs=tests/data/vepu580_rc_c_move6_cbr_regs.txt

if ! ssh "$BOARD" "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.0 && test -f /tmp/mpp_trace.so"; then
  printf 'SKIP rate-control capture (reference build or ioctl tracer missing)\n'
  exit 0
fi

# The moving fixture the GOP matrix already uses, so one source serves both.
ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc=size=320x240:rate=30 -frames:v 6 \
    -pix_fmt nv12 -f rawvideo -an $fixture" >/dev/null 2>&1

run() { # <name> <extra mpi_enc_test args...>
  local name="$1"; shift
  ssh "$BOARD" "sudo rm -f /tmp/rc_$name.log"
  ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib LD_PRELOAD=/tmp/mpp_trace.so timeout 180 $mpp_bin \
      -i $fixture -o /tmp/rk-mpp-rs-rc-$name.264 -w 320 -h 240 -t 7 -f 0 -n 6 -v q $*" \
      >/tmp/rc_$name.log 2>&1
  ssh "$BOARD" "sudo chmod 644 /tmp/rc_$name.log"
  scp -q "$BOARD":"/tmp/rc_$name.log" /tmp/ 2>/dev/null
}

run fix "-rc 2 -qc 26:26:26:26:26"
run cbr "-rc 1 -bps 300000:250000:350000"

report() { python3 tools/rc_trace_report.py "/tmp/rc_$1.log" "${@:2}"; }

{
  printf '# What the C encoder'\''s rate control writes for the same six frames it\n'
  printf '# encodes at fixed QP, captured on t6 with tools/mpp_ioctl_trace.c.\n'
  printf '# Regenerate with: REFRESH=1 scripts/board_h264_rc_capture.sh\n'
  printf '# The CBR stream those jobs produced is 6283 bytes, md5\n'
  printf '# c34eba92a3196865d04ac23253b2f7af, and both it and this report repeat\n'
  printf '# byte for byte across runs.\n\n'
  report cbr --against /tmp/rc_fix.log
} > /tmp/rc_report.txt

cat /tmp/rc_report.txt

# The same two jobs as raw register words. The report says which fields move; the
# words are what the register-level port is checked against, word for word.
{
  printf '# The C encoder'\''s two CBR jobs as register words: the intra job and the\n'
  printf '# next P job, in window order, addresses included (those differ per\n'
  printf '# process). The I job is the fixed-QP job plus the thirty rate-control\n'
  printf '# fields, so rebuilding those fields has to reproduce these words.\n'
  printf '# Regenerate with: REFRESH=1 scripts/board_h264_rc_capture.sh\n\n'
  for job in 0 1; do
    printf 'job %s\n' "$job"
    python3 tools/rc_trace_report.py /tmp/rc_cbr.log --dump-job "$job"
    printf '\n'
  done
} > /tmp/rc_regs.txt

if [ "${REFRESH:-0}" = 1 ]; then
  cp /tmp/rc_report.txt "$out"
  cp /tmp/rc_regs.txt "$regs"
  printf '\nwrote %s and %s\n' "$out" "$regs"
fi
