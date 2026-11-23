#!/usr/bin/env bash
BOARD="${BOARD:?set BOARD to a Rockchip SSH alias}"
# H.265 fixed-QP encode throughput against the same official C harness used by
# the correctness matrix. The h265e_probe is not suitable as a benchmark oracle:
# it leaves a different rc-cfg SEI target and its first P length differs from
# mpi_enc_test even though both are C MPP.
set -uo pipefail

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
rust_bin="${RUST_BIN:-~/rk-mpp-rs/target/release/rk-mpp-rs}"
mpp_lib="$mpp_ref/build/mpp"
samples="${SAMPLES:-5}"
frames="${FRAMES:-60}"
allow_skips="${ALLOW_SKIPS:-0}"
sizes=(320x240 640x480 1280x720 1920x1088)

if ! ssh "$BOARD" "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.1"; then
  printf 'missing H.265 reference encoder at %s (set MPP_REF or run setup_board_refs.sh)\n' \
    "$mpp_ref" >&2
  exit 2
fi
if ! ssh "$BOARD" "test -x $rust_bin"; then
  printf 'missing Rust H.265 encoder at %s (set RUST_BIN)\n' "$rust_bin" >&2
  exit 2
fi

median() {
  local values
  values="$(printf '%s\n' "$@" | sort -n)"
  local n
  n=$(printf '%s\n' "$values" | wc -l)
  printf '%s\n' "$values" | sed -n "$((n / 2 + 1))p"
}

timed() {
  local command="$1"
  ssh "$BOARD" "start=\$(date +%s%N); $command >/dev/null 2>&1; end=\$(date +%s%N); \
    echo \$(( (end - start) / 1000000 ))" 2>/dev/null
}

fail=0
printf '%-12s %10s | %-22s | %-22s | rust\n' 'size' 'pics' 'C MPP' 'this crate'
for entry in "${sizes[@]}"; do
  width="${entry%%x*}"
  height="${entry##*x}"
  file="/tmp/enc_${entry}_${frames}.nv12"
  if ! ssh "$BOARD" "test -s $file"; then
    ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc2=size=$entry:rate=30 \
      -frames:v $frames -pix_fmt nv12 -f rawvideo $file" >/dev/null 2>&1
  fi
  if ! ssh "$BOARD" "test -s $file"; then
    if [ "$allow_skips" = 1 ]; then
      printf 'SKIP %-12s (no input frames)\n' "$entry"
    else
      printf 'FAIL %-12s (no input frames)\n' "$entry"
      fail=1
    fi
    continue
  fi

  c_times=()
  rust_times=()
  for _ in $(seq 1 "$samples"); do
    c=$(timed "sudo env LD_LIBRARY_PATH=$mpp_lib $mpp_bin \
      -i $file -o /dev/null -w $width -h $height -t 16777220 -f 0 \
      -n $frames -g 120 -rc 2 -qc 26:26:26:26:26")
    r=$(timed "sudo env H265_GOP=120 \
      $rust_bin h265-gop $file /dev/null \
      $width $height $frames 26")
    [[ -n "$c" ]] && c_times+=("$c")
    [[ -n "$r" ]] && rust_times+=("$r")
  done
  if [[ ${#c_times[@]} -eq 0 || ${#rust_times[@]} -eq 0 ]]; then
    printf 'SKIP %-12s (C=%s rust=%s samples)\n' \
      "$entry" "${#c_times[@]}" "${#rust_times[@]}"
    fail=1
    continue
  fi

  c_ms=$(median "${c_times[@]}")
  rust_ms=$(median "${rust_times[@]}")
  delta=$(awk -v c="$c_ms" -v r="$rust_ms" 'BEGIN { print (c-r)/r*100 }')
  printf '%-12s %3s pics | C MPP %8s ms (%6.1f fps) | rust %8s ms (%6.1f fps) | %+.1f%%\n' \
    "$entry" "$frames" "$c_ms" \
    "$(awk -v f="$frames" -v t="$c_ms" 'BEGIN { print f*1000/t }')" \
    "$rust_ms" \
    "$(awk -v f="$frames" -v t="$rust_ms" 'BEGIN { print f*1000/t }')" \
    "$delta"
  if ! awk -v c="$c_ms" -v r="$rust_ms" 'BEGIN { exit !(r <= c * 1.05) }'; then
    printf 'FAIL h265-throughput-%s (rust exceeds C by more than 5%%)\n' "$entry"
    fail=1
  else
    printf 'PASS h265-throughput-%s\n' "$entry"
  fi
done

exit "$fail"
