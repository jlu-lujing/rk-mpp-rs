#!/usr/bin/env bash
# H.264 IDR at several fixed QPs: our VEPU580 register file vs the C HAL's.
#
# The encoder used to replay one captured job, so QP 26 was the only value that
# could produce correct bytes. This script is what proves the QP words are now
# generated rather than recorded: the same probe frame goes to both encoders, and
# the two bitstreams have to agree byte for byte at every QP.
#
# QP 26 is the control. It must reproduce the stream the locked golden hashes in
# tests/h264_encoder_hw.rs describe; if that one fails, the C invocation is wrong
# and the other rows mean nothing.
#
# Usage: scripts/board_h264_qp_vs_mpp.sh
set -euo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
  ssh t6 'tar -xf - -C ~/rk-mpp-rs'
ssh t6 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

mpp_enc=/tmp/mpp-develop/build/test/mpi_enc_test
mpp_lib=/tmp/mpp-develop/build/mpp
frame=/tmp/rk-mpp-rs-probe-320x240.nv12
cfg=/tmp/rk-mpp-rs-h264-baseline.json
fail=0

if ! ssh t6 "test -x $mpp_enc"; then
  printf 'SKIP H.264 QP comparison (mpi_enc_test missing)\n'
  exit 0
fi

# Both sides need the same NV12 bytes, with the stride layout our encoder is
# given: rows aligned to 16 pixels.
ssh t6 "cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame $frame 320 240" >/dev/null

# `mpi_enc_test` defaults to High profile with CABAC and transform-8x8 at level
# 4.0. Our reference job is Constrained Baseline, CAVLC, no transform-8x8, level
# 3.0, GOP 1, and the same POC and frame-num ranges, so the comparison needs the
# cfg file to say so - otherwise every row differs for reasons that have nothing
# to do with QP. The QP goes in here as well rather than only in `-qc`, because
# the cfg is applied after the command line and its rc defaults would put
# qp_init back to -1, which means "let the rate control choose".
write_cfg() {
  local qp="$1"
  ssh t6 "cat > $cfg" <<JSON
{
    "codec" : { "type" : 7 },
    "rc"    : { "mode" : 2, "gop" : 1, "qp_init" : ${qp},
                "qp_min" : ${qp}, "qp_max" : ${qp},
                "qp_min_i" : ${qp}, "qp_max_i" : ${qp},
                "fps_in_num" : 30, "fps_in_denom" : 1,
                "fps_out_num" : 30, "fps_out_denom" : 1 },
    "h264"  : { "stream_type" : 0, "profile" : 66, "level" : 30,
                "poc_type" : 0, "log2_max_poc_lsb" : 12, "log2_max_frm_num" : 12,
                "gaps_not_allowed" : 0, "cabac_en" : 0, "cabac_idc" : 0,
                "trans8x8" : 0, "const_intra" : 0, "scaling_list" : 0,
                "cb_qp_offset" : -6, "cr_qp_offset" : -6,
                "max_ltr" : 0, "constraint_set" : 0, "vui_en" : 1,
                "qp_init" : ${qp}, "qp_min" : ${qp}, "qp_max" : ${qp},
                "qp_min_i" : ${qp}, "qp_max_i" : ${qp} }
}
JSON
}

for qp in 26 18 32 42; do
  write_cfg "$qp"
  c_out="/tmp/rk-mpp-rs-h264-c-qp${qp}.264"
  rust_out="/tmp/rk-mpp-rs-h264-rust-qp${qp}.264"
  ssh t6 "sudo rm -f $c_out $rust_out"
  # -rc 2 is fixed QP; -qc takes qp_init:min:max:min_i:max_i, all pinned here so
  # the encoder cannot drift the QP between the two runs.
  if ! ssh t6 "sudo env LD_LIBRARY_PATH=$mpp_lib $mpp_enc -i $frame -o $c_out \
      -w 320 -h 240 -t 7 -f 0 -n 1 -rc 2 -qc ${qp}:${qp}:${qp}:${qp}:${qp} \
      -cfg $cfg -v q" \
      >/dev/null 2>&1; then
    printf 'FAIL qp %2d (C encode failed)\n' "$qp"
    fail=1
    continue
  fi
  if ! ssh t6 "cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264-enc $frame $rust_out 320 240 $qp" \
      >/dev/null 2>&1; then
    printf 'FAIL qp %2d (Rust encode failed)\n' "$qp"
    fail=1
    continue
  fi
  c_bytes=$(ssh t6 "stat -c %s $c_out")
  rust_bytes=$(ssh t6 "stat -c %s $rust_out")
  # C's mpi_enc_test writes an Annex-B stream without a trailing pad; compare the
  # shorter prefix and then require the lengths to agree, so a missing tail is a
  # failure rather than a shorter pass.
  if ssh t6 "cmp -s <(head -c $((c_bytes < rust_bytes ? c_bytes : rust_bytes)) $c_out) \
                <(head -c $((c_bytes < rust_bytes ? c_bytes : rust_bytes)) $rust_out)" \
     && [ "$c_bytes" = "$rust_bytes" ]; then
    printf 'PASS qp %2d (%s bytes, byte-exact vs C MPP)\n' "$qp" "$c_bytes"
  else
    printf 'FAIL qp %2d (C %s bytes, Rust %s bytes)\n' "$qp" "$c_bytes" "$rust_bytes"
    if [ "$qp" = 26 ]; then
      # QP 26 is the value the captured register image came with, and our output
      # for it is the stream locked in tests/h264_encoder_hw.rs. If C disagrees at
      # that QP, it is a different encoder build, not a different QP.
      printf '     note: the control row disagrees, so the C build is not the one the\n'
      printf '     register job was captured from; see the M7 note in ROADMAP.md.\n'
    fi
    fail=1
  fi
done

exit "$fail"
