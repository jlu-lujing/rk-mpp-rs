#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# One command that answers "did anything regress?".
#
# Every suite below is an existing gate with its own evidence; this script only
# runs them in dependency order and prints one summary, so a change to the
# encoder cannot pass by nobody remembering to run the decoder suites (and the
# other way round).
#
# Order matters: the reference builds are checked first, because every other
# suite degrades to SKIP - and a SKIP read as a pass - when they are gone.
#
# Usage:
#   scripts/board_regress_all.sh              # quick fixed-QP matrix (6 GOP cells)
#   FULL_QP=1 scripts/board_regress_all.sh    # the whole 24-cell size x profile x QP matrix
#   SKIP=vp8,m2v scripts/board_regress_all.sh
set -uo pipefail

logdir=/tmp/rk-mpp-rs-regress
SKIP="${SKIP:-}"   # set -u: an unset SKIP is "skip nothing", not an error
ssh "$BOARD" "mkdir -p $logdir" 2>/dev/null
mkdir -p /tmp/rk-mpp-rs-regress
fail=0
summary=()

skip() {
  local name="$1"
  for item in ${SKIP//,/ }; do
    [ "$item" = "$name" ] && return 0
  done
  return 1
}

run() { # <name> <script...>
  local name="$1"; shift
  if skip "$name"; then
    summary+=("skip   $name")
    return 0
  fi
  printf '\n=== %s ===\n' "$name"
  # `< /dev/null` is not decoration: ssh without -n inherits our stdin, and when
  # this script's stdin is a pipe or a file it blocks there forever.
  if "$@" < /dev/null > "/tmp/rk-mpp-rs-regress/$name.log" 2>&1; then
    local passes
    passes=$(grep -cE '^(PASS|ok|7/7|[0-9]+/[0-9]+ )' "/tmp/rk-mpp-rs-regress/$name.log" || true)
    summary+=("ok     $name  (${passes} line(s) of evidence, log: $logdir/$name.log)")
  else
    summary+=("FAIL   $name  (log: $logdir/$name.log)")
    tail -25 "/tmp/rk-mpp-rs-regress/$name.log"
    fail=1
  fi
}

run refs scripts/setup_board_refs.sh
run h264-decode scripts/board_regress.sh
run hevc-decode scripts/board_regress_hevc.sh
run m2v-decode scripts/board_regress_m2v.sh
run mp4-decode scripts/board_regress_mp4.sh
run h263-decode scripts/board_regress_h263.sh
run vp8-decode scripts/board_regress_vp8.sh

if [ "${FULL_QP:-0}" = 1 ]; then
  run h264-encode-qp scripts/board_h264_qp_vs_mpp.sh
else
  printf '\n=== h264-encode-qp ===\n'
  if ONLY=gop scripts/board_h264_qp_vs_mpp.sh < /dev/null > /tmp/rk-mpp-rs-regress/h264-encode-qp.log 2>&1; then
    summary+=("ok     h264-encode-qp  (GOP cells only; FULL_QP=1 for all 24)")
  else
    summary+=("FAIL   h264-encode-qp  (log: $logdir/h264-encode-qp.log)")
    tail -25 /tmp/rk-mpp-rs-regress/h264-encode-qp.log
    fail=1
  fi
fi

run h264-encode-rc scripts/board_h264_cbr_vs_mpp.sh
run h264-status scripts/board_h264_status_vs_mpp.sh

printf '\n================ board regression summary ================\n'
for line in "${summary[@]}"; do printf '%s\n' "$line"; done
printf '===========================================================\n'
[ "$fail" = 0 ] && printf 'ALL SUITES OK\n' || printf 'FAILURES ABOVE\n'
exit "$fail"
