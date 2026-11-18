#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# Encode throughput: this crate vs the C encoder, on the same input frames.
#
# Nothing has ever measured the encoder against MPP, which leaves "never slower
# than the C library" unverified for the whole encode side. Same discipline as
# the decode bench: the two binaries run interleaved sample by sample, because
# clock and thermal state drift, and only medians are reported. A lone pass of
# the C tool has previously printed numbers six times off the interleaved median.
#
# What is compared is "encode N pictures, end to end", each side with its own
# input reading and its own bitstream sink: both write to /dev/null, so the
# measurement covers submit, poll, the reconstruction bookkeeping and the
# parameter sets, and not a file write. Sizes are macroblock multiples, which is
# what both tools want a raw NV12 frame to be - `mpi_enc_test` reads
# hor_stride x ver_stride too, so 1080 is represented by its coded 1088 lines.
#
# Both sides are timed externally, in wall clock around the command. The C tool's
# own `encode N frames time T ms` line would be the natural source, but this
# vintage's mpi_enc_test prints nothing at all - not even its usage - so its
# internal timer is unreachable; timing one side internally and the other
# externally would quietly charge process startup to only one of them. The
# overhead is the same few tens of milliseconds in both, which is why the
# comparison stands and why the absolute fps is quoted as throughput of the
# command rather than of the codec loop.
#
# Usage: scripts/board_bench_encode_vs_mpp.sh          (all rows)
#        SAMPLES=9 scripts/board_bench_encode_vs_mpp.sh
set -uo pipefail

rust_bin="$HOME/rk-mpp-rs/target/release/rk-mpp-rs"
mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
samples="${SAMPLES:-5}"
frames=12

# width x height pairs, all multiples of 16
sizes=(320x240 640x480 1280x720 1920x1088)

if ! ssh "$BOARD" "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.0"; then
  printf 'SKIP encode bench (reference encoder missing at %s)\n' "$mpp_ref"
  exit 0
fi

median() {
  local values
  values="$(printf '%s\n' "$@" | sort -n)"
  local n; n=$(printf '%s\n' "$values" | wc -l)
  printf '%s\n' "$values" | sed -n "$((n / 2 + 1))p"
}

# Both commands run inside the same timer, so the measurement is symmetric.
timed() { # command... -> "frames ms"
  local frames="$1"; shift
  # The command's own report is discarded: it is text, and letting it through
  # into the same line as the elapsed time is what made the first pass of this
  # script print the encoder's summary where the millisecond figure belonged.
  ssh "$BOARD" "start=\$(date +%s%N); $1 >/dev/null 2>&1; end=\$(date +%s%N); \
          echo $frames \$(( (end - start) / 1000000 ))" 2>/dev/null
}

run_mpp() { # width height file -> "frames ms"
  timed "$frames" "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 300 $mpp_bin -i $3 \
    -o /dev/null -w $1 -h $2 -t 7 -f 0 -n $frames -rc 2 -qc 26:26:26:26:26 -fps 30:1"
}

run_rust() { # width height file -> "frames ms"
  # The C harness uses its default GOP of 60; pass the same GOP so the
  # bitstreams differ only in the build-identity SEI text.
  timed "$frames" "cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264-enc $3 \
    /dev/null $1 $2 26 baseline $frames 60"
}

printf '%-12s %3s pics | %-22s | %-22s | rust\n' 'size' 'pics' 'C MPP' 'this crate'
for entry in "${sizes[@]}"; do
  width="${entry%%x*}"; height="${height#*}"; height="${entry##*x}"
  file="/tmp/enc_${width}x${height}_${frames}.nv12"
  if ! ssh "$BOARD" "test -s $file"; then
    ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc2=size=${width}x${height}:rate=30 \
        -frames:v $frames -pix_fmt nv12 -f rawvideo $file" >/dev/null 2>&1
  fi
  if ! ssh "$BOARD" "test -s $file"; then
    printf 'SKIP %-12s (no input frames)\n' "$entry"
    continue
  fi

  rust_ms=() mpp_ms=()
  for _ in $(seq 1 "$samples"); do
    read -r _ t <<<"$(run_mpp "$width" "$height" "$file")"
    [[ -n "${t:-}" ]] && mpp_ms+=("$t")
    read -r _ t <<<"$(run_rust "$width" "$height" "$file")"
    [[ -n "${t:-}" ]] && rust_ms+=("$t")
  done
  if [[ ${#rust_ms[@]} -eq 0 || ${#mpp_ms[@]} -eq 0 ]]; then
    printf 'SKIP %-12s (rust=%s mpp=%s samples)\n' "$entry" "${#rust_ms[@]}" "${#mpp_ms[@]}"
    continue
  fi
  r=$(median "${rust_ms[@]}"); m=$(median "${mpp_ms[@]}")
  printf '%-12s %3s pics | C MPP %8s ms (%6.1f fps) | rust %8s ms (%6.1f fps) | %+.1f%%\n' \
    "$entry" "$frames" "$m" "$(awk -v f=$frames -v t="$m" 'BEGIN{print f*1000/t}')" \
    "$r" "$(awk -v f=$frames -v t="$r" 'BEGIN{print f*1000/t}')" \
    "$(awk -v r="$r" -v m="$m" 'BEGIN{print (m-r)/r*100}')"
done
