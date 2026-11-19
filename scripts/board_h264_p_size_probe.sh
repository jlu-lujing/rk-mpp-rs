#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# Where does the P picture stop matching the reference? A size sweep, not a guess.
#
# `board_h264_qp_vs_mpp.sh` proves the P path byte-exact at 320x240 and
# `board_h264_level_vs_mpp.sh` found it diverging at 640x480 and 1280x720 (our P
# spending more bits while the IDR in the same AU matches and the decoded
# pictures agree to 66 dB). The job registers match word for word outside the
# fourteen dynamic-address slots, so the difference is in what reaches the
# hardware, and the only way to find out which geometry quantity drives it is to
# find the threshold. Two shapes per axis: if only widths past some point fail it
# is the horizontal ME/line-buffer configuration; if heights, the vertical one.
#
# Each cell encodes two identical still pictures (a P with a clean reference -
# the least moving part that still exercises the reference read) at fixed QP 26,
# baseline, GOP 60, and reports both sides' P sizes so the trend is visible even
# where a cell fails. It compares the P picture by size, deliberately: `h264e_probe`
# is a driver of the library, not `mpi_enc_test`, and it carries no SEI NAL units,
# so a whole-access-unit compare against it would fail on the NAL sequence for
# every size and prove nothing. Byte-exact whole-stream proof at these sizes lives
# in `board_h264_qp_vs_mpp.sh`.
#
# Usage: scripts/board_h264_p_size_probe.sh
set -uo pipefail

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
    "H.264 P-size probe" "reference build missing at $mpp_ref"
fi
scp -q tools/h264e_probe.c "$BOARD":/tmp/ 2>/dev/null
ssh "$BOARD" "cc -O2 -w -pthread -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc $mpp_ref/utils/mpp_enc_roi_utils.c \
        /tmp/h264e_probe.c $mpp_lib/librockchip_mpp.so.1 -o /tmp/h264e_probe 2>/dev/null ||
        cc -O2 -w -pthread -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc $mpp_ref/utils/mpp_enc_roi_utils.c \
        /tmp/h264e_probe.c $mpp_lib/librockchip_mpp.so.0 -o /tmp/h264e_probe" || true

fail=0
# 800x600 and 1024x768 sit either side of the FBC header's 8K step (524288 coded
# pixels): the first still lands in the first row, the second crosses into the
# second, so the pair keeps the bug on either side of a threshold nobody has to
# re-derive. 1280x720 is where it was first seen.
for size in 320x240 384x288 448x352 512x384 576x416 640x480 640x256 320x512 800x600 1024x768 1280x720; do
  w="${size%%x*}"; h="${size##*x}"
  fixture=/tmp/rk-mpp-rs-psize-$size.nv12
  # Two copies of one synthetic picture: a still clip with a well-defined reference.
  ssh "$BOARD" "cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame /tmp/psize-1f.nv12 $w $h >/dev/null \
          && cp /tmp/psize-1f.nv12 $fixture && cat /tmp/psize-1f.nv12 >> $fixture"
  ssh "$BOARD" "sudo rm -f /tmp/psize_ours.264 /tmp/psize_c.264 &&
          cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264-enc $fixture \
            /tmp/psize_ours.264 $w $h 26 baseline 2 60 >/dev/null &&
          sudo env LD_LIBRARY_PATH=$mpp_lib /tmp/h264e_probe $fixture /tmp/psize_c.264 \
            $w $h 2 0 26 gop 60 >/dev/null 2>&1 &&
          sudo chmod 644 /tmp/psize_ours.264 /tmp/psize_c.264"
  scp -q "$BOARD":/tmp/psize_ours.264 /tmp/ 2>/dev/null; scp -q "$BOARD":/tmp/psize_c.264 /tmp/ 2>/dev/null
  sizes=$(python3 - "$w" "$h" <<'PY'
import re, sys
def p_sizes(path):
    d = open(path, 'rb').read()
    s = [m.start() for m in re.finditer(b"\x00\x00\x00\x01", d)] + [len(d)]
    return [len(d[s[k]+4:s[k+1]]) for k in range(len(s)-1)
            if d[s[k]+4:s[k+1]] and (d[s[k]+4] & 0x1f) == 1]
c = p_sizes("/tmp/psize_c.264"); o = p_sizes("/tmp/psize_ours.264")
same = c == o
print(("P %s vs %s  %s" % (o, c, "SAME" if same else "DIFFER")).strip(), 0 if same else 1)
PY
)
  verdict="${sizes##* }"
  printf '%-9s %s\n' "$size" "${sizes% *}"
  [ "$verdict" = "SAME" ] || fail=1
done
[ "$fail" = 0 ] && printf 'P path byte-identical at every size probed\n' || printf 'threshold evidence above\n'
exit "$fail"
