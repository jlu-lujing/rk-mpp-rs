#!/usr/bin/env bash
# Decode throughput: this crate vs the source-built C MPP, on the same stream.
#
# Runs on the board (`bash scripts/board_bench_vs_mpp.sh`, or from the host with
# `ssh t6 bash ~/rk-mpp-rs/scripts/board_bench_vs_mpp.sh`). The two binaries are
# interleaved sample by sample because the clock and thermal state drift; only
# medians over interleaved samples are comparable. A single unsplit run is not
# evidence: one lone mpi_dec_test pass printed 1037 fps where the interleaved
# median is 177 fps.
#
# Rust timing covers decode only (submit + poll + DPB + NV12->planar), not the
# output file write. C MPP covers its decode loop with -o /dev/null and native
# NV12 output, so the planar conversion counts against Rust and not against MPP.
set -uo pipefail

rust_bin="$HOME/rk-mpp-rs/target/release/rk-mpp-rs"
mpp_bin="/tmp/mpp-develop/build/test/mpi_dec_test"
samples="${SAMPLES:-5}"

# stream : rust subcommand : MppCodingType : width : height : max frames (0 = all)
streams=(
  "/tmp/test720.264:h264dec:7:1280:720:0"
  "/tmp/test_hard.264:h264dec:7:1920:1080:0"
  "/tmp/t720.hevc:hevcdec:16777220:1280:720:0"
  "/tmp/hevc_multigop_120.hevc:hevcdec:16777220:1280:720:0"
  "/tmp/hevc_main10_256.hevc:hevcdec:16777220:320:256:0"
  "/tmp/vp90_720.ivf:vp9dec:10:1280:720:0"
  "/tmp/p0_320x240_30.ivf:vp9dec:10:320:240:0"
  "/tmp/av1_320x240_10.ivf:av1dec:16777224:320:240:0"
  "/tmp/m2v_320x240_12.m2v:m2vdec:2:320:240:0"
  "/tmp/m2v_320x240_ilaced.m2v:m2vdec:2:320:240:0"
  "/tmp/m2v_64x64_3.m2v:m2vdec:2:64:64:0"
  "/tmp/mp4_320x240_12_ip.m4v:mp4dec:4:320:240:0"
  "/tmp/mp4_320x240_12_ibpb.m4v:mp4dec:4:320:240:0"
  "/tmp/mp4_150x100_9_ip.m4v:mp4dec:4:150:100:0"
)

median() {
  local n values
  values="$(printf '%s\n' "$@" | sort -n)"
  n=$(printf '%s\n' "$values" | wc -l)
  printf '%s\n' "$values" | sed -n "$((n / 2 + 1))p"
}

# Both sides report "N pictures in T ms"; print "pictures milliseconds".
run_rust() {
  local cmd="$1" stream="$2"
  sudo "$rust_bin" "$cmd" "$stream" /dev/null 2>&1 |
    sed -nE 's/.*decoded [0-9]+ (frames|packets) \(([0-9]+) output\) in ([0-9.]+) ms.*/\2 \3/p' |
    tail -1
}

run_mpp() {
  local type="$1" stream="$2" width="$3" height="$4" frames="$5"
  (cd "$(dirname "$mpp_bin")" &&
    sudo "$mpp_bin" -i "$stream" -o /dev/null -w "$width" -h "$height" \
      -t "$type" -n "$frames" -v q 2>&1) |
    sed -nE 's/.*decode ([0-9]+) frames time ([0-9]+) ms.*/\1 \2/p' | tail -1
}

for entry in "${streams[@]}"; do
  IFS=':' read -r stream cmd type width height frames <<<"$entry"
  if [[ ! -f "$stream" ]]; then
    printf 'SKIP %-34s (stream missing)\n' "$(basename "$stream")"
    continue
  fi

  rust_ms=() mpp_ms=() rust_n="" mpp_n=""
  for _ in $(seq 1 "$samples"); do
    read -r n t <<<"$(run_mpp "$type" "$stream" "$width" "$height" "$frames")"
    [[ -n "${t:-}" ]] && { mpp_ms+=("$t"); mpp_n="$n"; }
    read -r n t <<<"$(run_rust "$cmd" "$stream")"
    [[ -n "${t:-}" ]] && { rust_ms+=("$t"); rust_n="$n"; }
  done

  if [[ ${#rust_ms[@]} -eq 0 || ${#mpp_ms[@]} -eq 0 ]]; then
    printf 'SKIP %-34s (rust=%s mpp=%s samples, no measurement)\n' \
      "$(basename "$stream")" "${#rust_ms[@]}" "${#mpp_ms[@]}"
    continue
  fi

  r=$(median "${rust_ms[@]}")
  m=$(median "${mpp_ms[@]}")
  printf '%-34s %3s pics | C MPP %8s ms (%7.1f fps) | rust %8s ms (%7.1f fps) | rust %+.1f%%\n' \
    "$(basename "$stream")" "${rust_n:-?}" "$m" \
    "$(awk -v f="${mpp_n:-1}" -v t="$m" 'BEGIN{print f*1000/t}')" \
    "$r" "$(awk -v f="${rust_n:-1}" -v t="$r" 'BEGIN{print f*1000/t}')" \
    "$(awk -v r="$r" -v m="$m" 'BEGIN{print (m-r)/r*100}')"
done
