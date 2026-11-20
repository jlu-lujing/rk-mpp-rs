#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# Probe the advertised H.265 10-bit encode path. VEPU580 accepts the MPP config
# and the 10-bit frame format, but the hardware job on RK3588 terminates with
# int_sta=0x100 and writes no packet. The Rust API therefore rejects non-8-bit
# configurations instead of silently reading a 10-bit file as 8-bit.
set -uo pipefail

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"

scp -q tools/h265e_probe.c "$BOARD":/tmp/h265e_probe10.c
ssh "$BOARD" "cc -O2 -w -pthread -I$mpp_ref/inc /tmp/h265e_probe10.c \
  $mpp_lib/librockchip_mpp.so.1 -o /tmp/h265e_probe10 2>/dev/null ||
  cc -O2 -w -pthread -I$mpp_ref/inc /tmp/h265e_probe10.c \
  $mpp_lib/librockchip_mpp.so.0 -o /tmp/h265e_probe10" || true
ssh "$BOARD" "sudo rm -f /tmp/h265_10bit.265 /tmp/h265_10bit.trace &&
  dd if=/dev/zero of=/tmp/h265_10bit.nv12 bs=230400 count=1 status=none &&
  sudo env LD_LIBRARY_PATH=$mpp_lib LD_PRELOAD=${TRACER:-/tmp/mpp_trace.so} \
    BIT_DEPTH=10 /tmp/h265e_probe10 /tmp/h265_10bit.nv12 \
      /tmp/h265_10bit.265 320 240 0 26 >/dev/null 2>/tmp/h265_10bit.trace;
  sudo chmod 644 /tmp/h265_10bit.265 /tmp/h265_10bit.trace" || true

if ssh "$BOARD" "test -s /tmp/h265_10bit.265"; then
  printf 'FAIL h265-10bit (hardware unexpectedly produced a stream)\n'
  exit 1
fi
if ! ssh "$BOARD" "grep -q 'read_complete\\[0\\]@44 words=1 00000100' /tmp/h265_10bit.trace"; then
  printf 'FAIL h265-10bit (hardware did not report the expected int_sta=0x100)\n'
  exit 1
fi
printf 'PASS h265-10bit (VEPU580 rejects 10-bit with int_sta=0x100, no packet)\n'
