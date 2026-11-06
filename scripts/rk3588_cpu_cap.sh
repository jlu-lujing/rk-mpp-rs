#!/bin/sh
set -eu

if [ "$(id -u)" -ne 0 ]; then
  echo "rk3588-cpu-cap: must run as root" >&2
  exit 1
fi

cap_policy() {
  policy="$1"
  governor="$2"
  base="/sys/devices/system/cpu/cpufreq/policy$policy"

  [ -n "$(cat "$base/related_cpus")" ] || {
    echo "rk3588-cpu-cap: policy$policy is missing" >&2
    return 1
  }

  target=$(( $(cat "$base/cpuinfo_max_freq") * limit_percent / 100 ))
  chosen="$(
    tr ' ' '\n' < "$base/scaling_available_frequencies" |
      awk -v target="$target" '$1 <= target && $1 > best { best = $1 } END { print best }'
  )"
  if [ -z "$chosen" ]; then
    echo "rk3588-cpu-cap: no OPP <= $target kHz for policy$policy" >&2
    return 1
  fi

  echo "$governor" > "$base/scaling_governor"
  echo "$chosen" > "$base/scaling_max_freq"
  printf 'policy%s cpus=%s governor=%s limit=%s%% cap=%s kHz\n' \
    "$policy" "$(cat "$base/related_cpus")" \
    "$(cat "$base/scaling_governor")" "$limit_percent" \
    "$(cat "$base/scaling_max_freq")"
}

limit_percent="${RK3588_CPU_LIMIT_PERCENT:-80}"

cap_policy 0 performance
cap_policy 4 performance
cap_policy 6 performance
