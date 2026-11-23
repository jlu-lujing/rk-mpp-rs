#!/usr/bin/env bash
# Compare every AV1 tile fixture input image with C MPP before submitting a
# Rust hardware job. This script only runs the known-good C decode and the
# CPU-only Rust image exporter; it never runs `rk-mpp-rs av1dec`.
#
# Usage:
#   BOARD=my-rk3588-host scripts/board_av1_presubmit_compare.sh
#   FIXTURE=tests/data/av1_inter_global_warped_320x240.ivf JOBS=2 FRAMES=2 \
#     BOARD=my-rk3588-host scripts/board_av1_presubmit_compare.sh
#
# The C side is built in a separate shared clone at
# ~/rk-mpp-oracles/mpp-av1-dump so the normal mpp-develop oracle is left
# untouched. A non-git MPP source snapshot is copied before patching. Both dump
# directories remain on the board for inspection.
set -euo pipefail

board="${BOARD:?set BOARD to a Rockchip SSH alias}"
jobs="${JOBS:-11}"
frames="${FRAMES:-10}"
fixture="${FIXTURE:-tests/data/av1_tiles_320x240_10.ivf}"
oracle_src="${ORACLE_SRC:-}"
dump_oracle="${DUMP_ORACLE:-}"
here="$(cd "$(dirname "$0")" && pwd)"
archive="$(mktemp /tmp/rk-mpp-rs-av1.XXXXXX.tar)"

cleanup() {
  rm -f "$archive"
}
trap cleanup EXIT

printf 'probing %s\n' "$board"
ssh -o BatchMode=yes -o ConnectTimeout=6 "$board" \
  'printf "target: "; hostname; uptime'

printf 'archiving committed source\n'
git archive --format=tar HEAD >"$archive"
remote_root="$(ssh "$board" 'mktemp -d /tmp/av1-presubmit.XXXXXX')"
printf 'remote workspace: %s\n' "$remote_root"

scp -q "$archive" "$board:$remote_root/source.tar"
scp -q "$here/av1_reg_dump.patch" "$board:$remote_root/av1_reg_dump.patch"
scp -q "$here/../tools/av1/compare_input_dumps.py" \
  "$board:$remote_root/compare_input_dumps.py"

ssh "$board" \
  env REMOTE_ROOT="$remote_root" JOBS="$jobs" FRAMES="$frames" FIXTURE="$fixture" \
  ORACLE_SRC="$oracle_src" DUMP_ORACLE="$dump_oracle" \
  bash -s <<'REMOTE'
set -euo pipefail

repo="$REMOTE_ROOT/rk-mpp-rs"
oracle_src="${ORACLE_SRC:-$HOME/rk-mpp-oracles/mpp-develop}"
dump_oracle="${DUMP_ORACLE:-$HOME/rk-mpp-oracles/mpp-av1-dump-v2}"
case "$FIXTURE" in
  /*) fixture="$FIXTURE" ;;
  *) fixture="$repo/$FIXTURE" ;;
esac

mkdir "$repo"
tar -xf "$REMOTE_ROOT/source.tar" -C "$repo"

if [ ! -d "$oracle_src/.git" ] && [ ! -d "$oracle_src/mpp" ]; then
    printf 'missing C oracle source: %s\n' "$oracle_src" >&2
  exit 1
fi
mkdir -p "$(dirname "$dump_oracle")"
if [ -d "$oracle_src/.git" ] && [ ! -e "$dump_oracle" ]; then
  git clone --shared "$oracle_src" "$dump_oracle"
elif [ -d "$oracle_src/.git" ] && [ ! -d "$dump_oracle/.git" ]; then
  printf 'refusing non-git dump oracle path: %s\n' "$dump_oracle" >&2
  exit 1
elif [ ! -e "$dump_oracle" ]; then
    mkdir "$dump_oracle"
    tar -C "$oracle_src" --exclude build-dump -cf - . |
        tar -C "$dump_oracle" -xf -
fi

source_file="$dump_oracle/mpp/hal/vpu/av1d/hal_av1d_vdpu.c"
if grep -q '^#define DUMP_AV1_DATAS 0$' "$source_file"; then
    if [ -d "$dump_oracle/.git" ]; then
        git -C "$dump_oracle" apply --unidiff-zero \
            --check "$REMOTE_ROOT/av1_reg_dump.patch"
        git -C "$dump_oracle" apply --unidiff-zero \
            "$REMOTE_ROOT/av1_reg_dump.patch"
    else
        patch -d "$dump_oracle" -p1 --dry-run \
            <"$REMOTE_ROOT/av1_reg_dump.patch"
        patch -d "$dump_oracle" -p1 <"$REMOTE_ROOT/av1_reg_dump.patch"
    fi
elif ! grep -q '^#define DUMP_AV1_DATAS 1$' "$source_file"; then
  printf 'unexpected DUMP_AV1_DATAS setting in %s\n' "$source_file" >&2
  exit 1
fi
if ! grep -q 'getenv("AV1_DATA_DUMP_DIR")' "$source_file"; then
  printf 'dump oracle predates the configurable output path; remove %s once\n' \
    "$dump_oracle" >&2
  exit 1
fi

if [ -d "$dump_oracle/.git" ]; then
    printf 'building C dump oracle at %s\n' \
        "$(git -C "$dump_oracle" rev-parse --short HEAD)"
else
    printf 'building C dump oracle from a source snapshot\n'
fi
cmake -S "$dump_oracle" -B "$dump_oracle/build-dump" \
  -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build "$dump_oracle/build-dump" -j"$(nproc)" >/dev/null

c_dir="$(mktemp -d /tmp/av1-c-dump.XXXXXX)"
c_out="$REMOTE_ROOT/c.nv12"
c_log="$REMOTE_ROOT/c.log"
printf 'running the known-good C decode; dump=%s\n' "$c_dir"
sudo timeout 60 env \
  LD_LIBRARY_PATH="$dump_oracle/build-dump/mpp" \
  AV1_DATA_DUMP_DIR="$c_dir" \
  "$dump_oracle/build-dump/test/mpi_dec_test" \
  -i "$fixture" -t 16777224 -n "$FRAMES" -o "$c_out" -v q \
  >"$c_log" 2>&1

c_jobs="$(find "$c_dir" -maxdepth 1 -name 'reg_*_in.txt' | wc -l)"
if [ "$c_jobs" -lt "$JOBS" ]; then
  printf 'C produced %s register images, fewer than the %s compared jobs; see %s\n' \
    "$c_jobs" "$JOBS" "$c_log" >&2
  exit 1
fi
if [ "$c_jobs" -gt "$JOBS" ]; then
  printf 'C produced %s register images; comparing the first %s and leaving %s extra job(s) for audit\n' \
    "$c_jobs" "$JOBS" "$((c_jobs - JOBS))"
fi

r_dir="$(mktemp -d /tmp/av1-rust-dump.XXXXXX)"
printf 'generating CPU-only Rust input images; dump=%s\n' "$r_dir"
if ! (
  cd "$repo"
  export PATH="$HOME/.cargo/bin:$PATH"
  cargo run --offline --quiet --example av1_dump_regs -- \
    "$fixture" "$JOBS" "$r_dir"
) >"$REMOTE_ROOT/rust.log" 2>&1; then
  tail -80 "$REMOTE_ROOT/rust.log" >&2
  exit 1
fi

python3 "$REMOTE_ROOT/compare_input_dumps.py" "$c_dir" "$r_dir" \
  --jobs "$JOBS" --allow-stateful-probability
printf '\nPRE-SUBMIT DETERMINISTIC INPUTS MATCH\nC dump:    %s\nRust dump: %s\nC log:     %s\nRust log:  %s\n' \
  "$c_dir" "$r_dir" "$c_log" "$REMOTE_ROOT/rust.log"
REMOTE
