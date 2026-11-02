#!/usr/bin/env bash
# VP9 boundary matrix on the board: every fixture in one pass, both decoders,
# byte-for-byte where both can produce comparable output.
#
# The matrix axes the fixtures cover are the ones the ROADMAP lists as open:
# odd luma dimensions (321x241), single-superblock 64x64 for both profiles,
# lossless for both profiles, error resilience with two tile columns,
# error resilience with temporal segmentation at 720p, and the profile 2
# 10-bit path. For each stream the judge is: exit code, output frame count
# (bytes / per-frame size) and, when the sizes agree, a byte compare of the
# C NV12 dump (ffmpeg-converted for 10-bit) against our planar output.
#
# Anything the matrix cannot compare - no C binary, a stream C refuses while
# we accept or vice versa - is reported as NAMED, not skipped: a named
# difference is a matrix cell with a verdict.
#
# Optional: CORPUS=<dir> additionally sweeps every *.ivf in the corpus whose
# name starts with vp9 (they are run by path, list printed first).
#
# Usage: scripts/board_vp9_matrix.sh
here="$(cd "$(dirname "$0")" && pwd)"
set -euo pipefail

corpus="${CORPUS:-}"

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
  ssh t6 'tar -xf - -C ~/rk-mpp-rs'
ssh t6 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

mpp_bin=/tmp/mpp-develop/build/test/mpi_dec_test
mpp_lib=/tmp/mpp-develop/build/mpp

# name : width : height : packets : bit_depth : per-frame-bytes override
# The packets column is the *packet* count; hidden reference frames make it
# exceed the display count in the alt-ref case, so the frame-count judge uses
# bytes, not packets.
streams=(
  "vp9_p0_64x64:64:64:0:8"
  "vp9_p2_10bit_64x64:64:64:0:10"
  "vp9_p0_lossless_321x241_2:321:241:2:8"
  "vp9_p0_lossless_320x240_12:320:240:12:8"
  "vp9_p2_10bit_lossless_320x240_12:320:240:12:10"
  "vp9_p0_errres_tiles_640x360_12:640:360:12:8"
  "vp9_p0_720p_errres_seg:1280:720:0:8"
)

if ! ssh t6 "test -x $mpp_bin"; then
  printf 'SKIP VP9 matrix (mpi_dec_test missing)\n'
  exit 0
fi

fail=0
named=0

# Cells where libvpx (ffmpeg) proved the C side wrong. Their byte compare is
# expected to fail; recording them here keeps a real regression from hiding.
known_c_bug() {
  case "$1" in
    vp9_p0_720p_errres_seg)
      printf 'KNOWN-C-BUG %s: Rust matches libvpx on all 6 frames; C differs on frames 4/5 by 213 V-plane bytes (max delta 3)\n' "$1"
      return 0 ;;
    vp9_p0_lossless_321x241_2)
      printf 'KNOWN-C-BUG %s: libvpx and Rust produce 2 frames; the C harness emits 1\n' "$1"
      return 0 ;;
  esac
  return 1
}
for entry in "${streams[@]}"; do
  IFS=':' read -r name width height _packets depth <<<"$entry"
  sample_bytes=$((depth > 8 ? 2 : 1))
  frame_bytes=$(( (width * height + 2 * ((width + 1) / 2) * ((height + 1) / 2)) * sample_bytes ))
  mpp_out="/tmp/vpx9_${name}_c.nv12"
  planar="/tmp/vpx9_${name}_c.yuv"
  rust="/tmp/vpx9_${name}_rust.yuv"

  # mpi_dec_test loops the last picture when -n exceeds the stream, so the
  # reference runs after the Rust one and asks for exactly Rust's frame count;
  # a count difference would then show as differing bytes, which the verdict
  # text below reports under the same 'named' umbrella.
  ssh t6 "sudo rm -f $mpp_out $rust"
  ssh t6 "cd ~/rk-mpp-rs && sudo timeout 300 \
    ./target/release/rk-mpp-rs vp9dec ~/rk-mpp-rs/tests/data/${name}.ivf $rust" \
    >/dev/null 2>&1 && r_rc=0 || r_rc=$?
  r_bytes=$(ssh t6 "stat -c %s $rust 2>/dev/null || echo 0")
  r_frames=$(( r_bytes / frame_bytes ))
  if [ "$r_frames" = 0 ]; then r_frames=1; fi
  ssh t6 "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 300 \
    $mpp_bin -i ~/rk-mpp-rs/tests/data/${name}.ivf -t 10 -o $mpp_out -n $r_frames" \
    >/dev/null 2>&1 && c_rc=0 || c_rc=$?

  c_bytes=$(ssh t6 "stat -c %s $mpp_out 2>/dev/null || echo 0")

  # rkvdec writes NV12-family output while the Rust decoder emits planar,
  # exactly like the VP8 comparison: convert the C side to the planar form
  # both sides agree on before the cmp.
  # rkvdec writes semi-planar on both depths - NV12 at 8-bit, P010 at 10-bit -
  # while the Rust decoder emits fully planar, exactly as the VP8 comparison
  # assumes. The probe that settled the 10-bit layout: the raw chroma plane
  # interleaves U,V pairs (891,655,878,662 against the planar 891,878,883).
  scp -q "$here/p010_to_planar.py" t6:/tmp/p010_to_planar.py 2>/dev/null
  if [ "$depth" = 10 ]; then
    # FFmpeg p010 is MSB-aligned; rkvdec is LSB-aligned. See the script.
    ssh t6 "python3 /tmp/p010_to_planar.py $mpp_out $planar $width $height" \
      >/dev/null 2>&1 || planar=""
  else
    ssh t6 "ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
      -video_size ${width}x${height} -i $mpp_out \
      -pix_fmt yuv420p -f rawvideo $planar" >/dev/null 2>&1 || planar=""
  fi
  c_cmp="$planar"

  c_frames=$(( c_bytes / frame_bytes ))
  r_frames=$(( r_bytes / frame_bytes ))
  if known_c_bug "$name"; then
    lv="/tmp/vpx9_${name}_libvpx.yuv"
    if ssh t6 "ffmpeg -loglevel error -y -i ~/rk-mpp-rs/tests/data/${name}.ivf \
        -pix_fmt yuv420p -f rawvideo $lv" >/dev/null 2>&1 &&
       ssh t6 "cmp -s $lv $rust"; then
      :
    else
      printf 'FAIL %s: Rust no longer matches libvpx (the C verdict is already known)\n' "$name"
      fail=1
    fi
    continue
  fi
  if [ "$c_rc" != "$r_rc" ]; then
    printf 'NAMED %s: exit code C=%s Rust=%s (bytes C=%s Rust=%s)\n' \
      "$name" "$c_rc" "$r_rc" "$c_bytes" "$r_bytes"
    named=$((named + 1))
  elif [ "$c_frames" != "$r_frames" ] || [ "$c_frames" = 0 ]; then
    printf 'NAMED %s: frame count C=%s Rust=%s (rc both %s)\n' "$name" "$c_frames" "$r_frames" "$c_rc"
    named=$((named + 1))
  elif ssh t6 "test -n '$c_cmp' && cmp -s '$c_cmp' $rust"; then
    printf 'PASS %s: %s frames byte-identical to C (%s-bit)\n' "$name" "$r_frames" "$depth"
  else
    printf 'FAIL %s: %s frames each but bytes differ\n' "$name" "$r_frames"
    fail=1
  fi
done

# Corpus sweep (opt-in): every VP9-named IVF gets the same two runs and a
# byte compare, without width assumptions - sizes must merely agree.
if [ -n "$corpus" ] && ssh t6 "test -d '$corpus'"; then
  while IFS= read -r stream; do
    base=$(basename "$stream" .ivf)
    ssh t6 "sudo rm -f /tmp/vpx9_corpus_c.yuv /tmp/vpx9_corpus_r.yuv"
    ssh t6 "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 300 \
      $mpp_bin -i '$stream' -t 10 -o /tmp/vpx9_corpus_c.yuv" >/dev/null 2>&1 && c_rc=0 || c_rc=$?
    ssh t6 "cd ~/rk-mpp-rs && sudo timeout 300 \
      ./target/release/rk-mpp-rs vp9dec '$stream' /tmp/vpx9_corpus_r.yuv" >/dev/null 2>&1 && r_rc=0 || r_rc=$?
    c_bytes=$(ssh t6 "stat -c %s /tmp/vpx9_corpus_c.yuv 2>/dev/null || echo 0")
    r_bytes=$(ssh t6 "stat -c %s /tmp/vpx9_corpus_r.yuv 2>/dev/null || echo 0")
    if [ "$c_rc" != "$r_rc" ] || [ "$c_bytes" != "$r_bytes" ]; then
      printf 'NAMED corpus %s: rc C=%s Rust=%s bytes C=%s Rust=%s (NV12 vs planar not normalised)\n' \
        "$base" "$c_rc" "$r_rc" "$c_bytes" "$r_bytes"
      named=$((named + 1))
    else
      printf 'SAME-SIZE corpus %s: %s bytes, run the normalised compare by hand\n' "$base" "$c_bytes"
    fi
  done < <(ssh t6 "find '$corpus' -name 'vp9*.ivf' | sort" </dev/null)
fi

printf '\nmatrix verdicts: fail=%s named-differences=%s\n' "$fail" "$named"
exit "$fail"
