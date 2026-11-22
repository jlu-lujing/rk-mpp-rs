#!/usr/bin/env bash
set -euo pipefail
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
CODEC=h265 exec "$here/board_bench_object_encode_vs_mpp.sh" "$@"
