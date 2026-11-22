#!/usr/bin/env bash
# Steady-state AVS+ comparison: 700 display frames, raw NV12 on both sides.
set -euo pipefail

BOARD="${BOARD:?set BOARD to the RK3588 SSH alias}"
remote_repo="${REMOTE_REPO:-\$HOME/rk-mpp-rs}"
mpp_ref="${MPP_REF:-\$HOME/rk-mpp-oracles/mpp-develop}"
fixture="${FIXTURE:-/tmp/avs_stream01.avs}"
fixture_sha=d296affc27c5977565a18d85897be82a3b1c77c9165b8928e6e5eaad698a736e
runs="${RUNS:-3}"

if [ ! -s "$fixture" ]; then
  curl -L --fail --silent \
    https://samples.ffmpeg.org/AVS/issue767/stream01.avs \
    -o "$fixture"
fi
test "$(sha256sum "$fixture" | cut -d' ' -f1)" = "$fixture_sha"
scp -q "$fixture" "$BOARD":/tmp/avs_stream01.avs

COPYFILE_DISABLE=1 tar --no-xattrs \
  --exclude .git --exclude target --exclude .worktrees --exclude refs \
  -cf - . | ssh "$BOARD" "tar -xf - -C \"$remote_repo\""
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd "$HOME/rk-mpp-rs" && \
  cargo build --release --bin rk-mpp-rs'

ssh "$BOARD" 'for _ in $(seq 1 100); do cat /tmp/avs_stream01.avs; done \
  >/tmp/avs_stream01_x100.avs'

c_times=/tmp/avs_perf_c.txt
r_times=/tmp/avs_perf_r.txt
ssh "$BOARD" "rm -f $c_times $r_times"
for _ in $(seq 1 "$runs"); do
  c_ms=$(ssh "$BOARD" "start=\$(date +%s%N); \
    sudo env enable_deinterlace=0 LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" timeout 180 \
    \"$mpp_ref/build/test/mpi_dec_test\" -i /tmp/avs_stream01.avs -o /dev/null \
    -t 16777221 -n 700 </dev/null >/tmp/avs_perf_c.log 2>&1; \
    end=\$(date +%s%N); echo \$(( (end-start)/1000000 ))")
  r_ms=$(ssh "$BOARD" "start=\$(date +%s%N); cd \"$remote_repo\"; \
    sudo timeout 180 ./target/release/rk-mpp-rs avsdec \
    /tmp/avs_stream01_x100.avs /dev/null 700 nv12 >/tmp/avs_perf_r.log 2>&1; \
    end=\$(date +%s%N); echo \$(( (end-start)/1000000 ))")
  ssh "$BOARD" "echo $c_ms >>$c_times; echo $r_ms >>$r_times"
  printf 'run: C=%sms Rust=%sms\n' "$c_ms" "$r_ms"
done

c_median=$(ssh "$BOARD" "sort -n $c_times | sed -n '$(((runs + 1) / 2))p'")
r_median=$(ssh "$BOARD" "sort -n $r_times | sed -n '$(((runs + 1) / 2))p'")
printf 'MEDIAN 700 frames: C=%sms Rust=%sms (raw NV12, vproc disabled)\n' \
  "$c_median" "$r_median"

c_vproc_times=/tmp/avs_vproc_perf_c.txt
r_vproc_times=/tmp/avs_vproc_perf_r.txt
ssh "$BOARD" "rm -f $c_vproc_times $r_vproc_times"
for _ in $(seq 1 "$runs"); do
  c_ms=$(ssh "$BOARD" "start=\$(date +%s%N); \
    sudo env LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" timeout 180 \
    \"$mpp_ref/build/test/mpi_dec_test\" -i /tmp/avs_stream01.avs -o /dev/null \
    -t 16777221 -n 700 </dev/null >/tmp/avs_vproc_perf_c.log 2>&1; \
    end=\$(date +%s%N); echo \$(( (end-start)/1000000 ))")
  r_ms=$(ssh "$BOARD" "start=\$(date +%s%N); cd \"$remote_repo\"; \
    sudo timeout 180 ./target/release/rk-mpp-rs avsdec \
    /tmp/avs_stream01_x100.avs /dev/null 700 vproc >/tmp/avs_vproc_perf_r.log 2>&1; \
    end=\$(date +%s%N); echo \$(( (end-start)/1000000 ))")
  ssh "$BOARD" "echo $c_ms >>$c_vproc_times; echo $r_ms >>$r_vproc_times"
  printf 'vproc run: C=%sms Rust=%sms\n' "$c_ms" "$r_ms"
done

c_vproc_median=$(ssh "$BOARD" "sort -n $c_vproc_times | sed -n '$(((runs + 1) / 2))p'")
r_vproc_median=$(ssh "$BOARD" "sort -n $r_vproc_times | sed -n '$(((runs + 1) / 2))p'")
printf 'MEDIAN 700 vproc frames: C=%sms Rust=%sms\n' \
  "$c_vproc_median" "$r_vproc_median"
