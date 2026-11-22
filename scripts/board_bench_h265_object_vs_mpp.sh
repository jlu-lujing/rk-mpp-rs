#!/usr/bin/env bash
# Benchmark the upstream H.265 object path through unchanged mpi_enc_test.
#
# C and Rust run interleaved on one big core. Each sample records wall, user
# and system time. The gate requires Rust to be within 5% of C for both wall
# and user+sys, so a throughput win cannot hide a CPU-cost regression.
set -euo pipefail

BOARD="${BOARD:?set BOARD to the RK3588 SSH alias}"
samples="${SAMPLES:-7}"
frames="${FRAMES:-300}"
remote_repo="${REMOTE_REPO:-\$HOME/rk-mpp-rs}"
mpp_ref="${MPP_REF:-\$HOME/rk-mpp-oracles/mpp-develop}"

ssh "$BOARD" \
  "REMOTE_REPO=\"$remote_repo\" MPP_REF=\"$mpp_ref\" SAMPLES=$samples FRAMES=$frames bash -s" \
  <<'REMOTE'
set -euo pipefail
repo="$REMOTE_REPO"
ref="$MPP_REF"
enc="$ref/build/test/mpi_enc_test"
lib="$ref/build/mpp"
rust_lib="$repo/target/release/librk_mpp_rs.so"

little_cap=$(cat /sys/devices/system/cpu/cpufreq/policy0/scaling_max_freq)
big_cap=$(cat /sys/devices/system/cpu/cpufreq/policy6/scaling_max_freq)
if [ "$little_cap" -gt 1416000 ] || [ "$big_cap" -gt 1800000 ]; then
  echo "80% OPP cap is not active (little=$little_cap big=$big_cap)" >&2
  exit 2
fi
echo performance | sudo tee /sys/class/devfreq/dmc/governor >/dev/null

for path in "$enc" "$lib/librockchip_mpp.so.0" "$rust_lib"; do
  if [ ! -e "$path" ]; then
    echo "missing $path" >&2
    exit 2
  fi
done

median() {
  printf '%s\n' "$@" | sort -n | sed -n "$(( $# / 2 + 1 ))p"
}

measure_c() {
  local file="$1" width="$2" height="$3"
  TIMEFORMAT='%R %U %S'
  {
    time sudo taskset -c 6 env LD_LIBRARY_PATH="$lib" timeout 120 "$enc" \
      -i "$file" -o /dev/null -w "$width" -h "$height" -n "$FRAMES" \
      -rc 2 -fps 30 -g 0:60 -t 16777220 -qc 26 </dev/null >/dev/null 2>&1
  } 2>&1
}

measure_rust() {
  local file="$1" width="$2" height="$3"
  TIMEFORMAT='%R %U %S'
  {
    time sudo taskset -c 6 env LD_PRELOAD="$rust_lib" LD_LIBRARY_PATH="$lib" \
      timeout 120 "$enc" -i "$file" -o /dev/null -w "$width" -h "$height" \
      -n "$FRAMES" -rc 2 -fps 30 -g 0:60 -t 16777220 -qc 26 \
      </dev/null >/dev/null 2>&1
  } 2>&1
}

fail=0
for spec in "320 240 /tmp/rk_mpp_h265_object_320x240_${FRAMES}.nv12" \
            "1280 720 /tmp/rk_mpp_h265_object_1280x720_${FRAMES}.nv12"; do
  set -- $spec
  width=$1
  height=$2
  fixture=$3
  if [ ! -s "$fixture" ]; then
    ffmpeg -nostdin -y -hide_banner -loglevel error \
      -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
      -frames:v "$FRAMES" -pix_fmt nv12 -f rawvideo "$fixture"
  fi

  c_wall=()
  c_cpu=()
  r_wall=()
  r_cpu=()
  for sample in $(seq 1 "$SAMPLES"); do
    read -r cw cu cs <<<"$(measure_c "$fixture" "$width" "$height")"
    read -r rw ru rs <<<"$(measure_rust "$fixture" "$width" "$height")"
    c_wall+=("$cw")
    r_wall+=("$rw")
    c_cpu+=("$(awk -v u="$cu" -v s="$cs" 'BEGIN { printf "%.3f", u + s }')")
    r_cpu+=("$(awk -v u="$ru" -v s="$rs" 'BEGIN { printf "%.3f", u + s }')")
    echo "sample ${width}x${height} $sample C=${cw}s/${cu}+${cs}s Rust=${rw}s/${ru}+${rs}s"
  done

  cm=$(median "${c_wall[@]}")
  rm=$(median "${r_wall[@]}")
  cc=$(median "${c_cpu[@]}")
  rc=$(median "${r_cpu[@]}")
  printf '%sx%s C_MEDIAN wall=%ss cpu=%ss Rust_MEDIAN wall=%ss cpu=%ss\n' \
    "$width" "$height" "$cm" "$cc" "$rm" "$rc"
  printf '%sx%s wall_delta=%+.1f%% cpu_delta=%+.1f%%\n' "$width" "$height" \
    "$(awk -v c="$cm" -v r="$rm" 'BEGIN { print (c-r)/c*100 }')" \
    "$(awk -v c="$cc" -v r="$rc" 'BEGIN { print (c-r)/c*100 }')"

  if ! awk -v c="$cm" -v r="$rm" 'BEGIN { exit !(r <= c * 1.05) }'; then
    echo "FAIL h265-object-wall-${width}x${height}" >&2
    fail=1
  fi
  if ! awk -v c="$cc" -v r="$rc" 'BEGIN { exit !(r <= c * 1.05) }'; then
    echo "FAIL h265-object-cpu-${width}x${height}" >&2
    fail=1
  fi
done

exit "$fail"
REMOTE
