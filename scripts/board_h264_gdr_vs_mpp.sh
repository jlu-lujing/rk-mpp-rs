#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# GDR / progressive intra refresh against the reference.
#
# The mechanism under test, all three pieces at once:
#
#   * the schedule - `mpp_enc_refs.cpp` skips `cleanup_cpb` while a refresh
#     length is set, so only picture 0 is an IDR and the IGOP boundary becomes
#     a sweep instead; the window starts only after the first IGOP
#     (`cpb.seq_cnt > 1`);
#   * the base table - `setup_vepu580_intra_refresh` refills the force-intra
#     records every refresh picture, the band sliding by `16 * refresh_num` and
#     carrying the two-line overlap (`y = idx * 16 * n - 32`, `h = 16 * n + 32`)
#     on every band but the first;
#   * the registers - `roi_en` and `roi_addr` over the refresh table without
#     `roi_qp_en`, and the CIME search on the refresh axis clamped to 1.
#
# The GOP is 12 and 25 pictures are coded, so the second IGOP's sweep (frames
# 12..15) and the third IGOP's boundary (frame 24) are both inside the stream.
# The whole file is compared byte for byte.
#
# Note the header question this crate deliberately does NOT "fix": the C gates
# the recovery-point SEI and the repeated header on `hdr_status` having no
# added bits yet, and the opening IDR sets those bits. In the reference's own
# steady state no sweep boundary re-prefixes anything, and neither do we. If a
# cell below differs exactly at a sweep boundary's first byte, that gate is the
# first thing to re-read in `mpp_enc_impl.cpp`.
#
# Usage: scripts/board_h264_gdr_vs_mpp.sh
set -uo pipefail

SIZE="320 240"
FRAMES=25
GOP=12
fixture=/tmp/rk-mpp-rs-move-320x240.nv12

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"

if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  printf 'SKIP gdr (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

scp -q tools/h264e_probe.c "$BOARD":/tmp/ 2>/dev/null
ssh "$BOARD" "cc -O2 -w -pthread -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc $mpp_ref/utils/mpp_enc_roi_utils.c \
        /tmp/h264e_probe.c $mpp_lib/librockchip_mpp.so.1 -o /tmp/h264e_probe 2>/dev/null ||
        cc -O2 -w -pthread -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc $mpp_ref/utils/mpp_enc_roi_utils.c \
        /tmp/h264e_probe.c $mpp_lib/librockchip_mpp.so.0 -o /tmp/h264e_probe" || true

ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc=size=320x240:rate=30 -frames:v $FRAMES \
    -pix_fmt nv12 -f rawvideo -an $fixture" >/dev/null 2>&1

fail=0
run_cell() {
  local name="$1" ours_args="$2" probe_args="$3"
  ssh "$BOARD" "sudo rm -f /tmp/gdr_ours_$name.264 /tmp/gdr_c_$name.264 &&
          cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264-enc $fixture \
            /tmp/gdr_ours_$name.264 $SIZE $ours_args >/dev/null &&
          sudo env LD_LIBRARY_PATH=$mpp_lib /tmp/h264e_probe $fixture \
            /tmp/gdr_c_$name.264 320 240 $probe_args >/dev/null 2>&1 &&
          sudo chmod 644 /tmp/gdr_ours_$name.264 /tmp/gdr_c_$name.264"
  if ! ssh "$BOARD" "test -s /tmp/gdr_ours_$name.264 && test -s /tmp/gdr_c_$name.264"; then
    printf 'FAIL gdr/%s (one side produced no stream)\n' "$name"; fail=1; return
  fi
  scp -q "$BOARD":"/tmp/gdr_ours_$name.264" "$BOARD":"/tmp/gdr_c_$name.264" /tmp/ 2>/dev/null
  if python3 scripts/compare_streams.py "/tmp/gdr_c_$name.264" "/tmp/gdr_ours_$name.264"; then
    printf 'PASS gdr/%s\n' "$name"
  else
    printf 'FAIL gdr/%s (bitstreams differ)\n' "$name"; fail=1
  fi
}

# Row refresh, 4 rows a picture: refresh_length = ceil(15/4) = 4 pictures of a
# 12-frame IGOP, two full sweeps inside the run.
run_cell row_n4 "26 baseline $FRAMES $GOP gdr:row:4" \
  "$FRAMES 0 26 288000:1500000:2500000 gop $GOP gdr 0 4"
# Column refresh, 4 columns a picture: the other axis, clamping cme_srch_h.
run_cell col_n4 "26 baseline $FRAMES $GOP gdr:col:4" \
  "$FRAMES 0 26 288000:1500000:2500000 gop $GOP gdr 1 4"
# 8 rows a picture: the short sweep, length ceil(15/8) = 2.
run_cell row_n8 "26 baseline $FRAMES $GOP gdr:row:8" \
  "$FRAMES 0 26 288000:1500000:2500000 gop $GOP gdr 0 8"
# 1 row a picture: length 15 capped at the GOP of 12, which lands ON the GOP -
# the `refresh_length >= igop` corner `mpp_enc_refs_set_refresh_length` answers
# by clamping and every picture after the first IGOP refreshes a single row.
run_cell row_n1 "26 baseline $FRAMES $GOP gdr:row:1" \
  "$FRAMES 0 26 288000:1500000:2500000 gop $GOP gdr 0 1"

exit "$fail"
