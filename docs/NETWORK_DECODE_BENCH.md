# Network H.264 decode benchmark

Date: 2026-11-15  
Board: NanoPC-T6-LTS / RK3588  
CPU policy: persistent 80% OPP cap (`1.416 / 1.8 / 1.8 GHz`)

## Input

- `s2` hosts a 30-second HLS stream (`192.0.2.10:18081`).
- t6 pulls the HLS stream over its normal LAN.
- The stream is demuxed once to H.264 Annex-B, then both decoders consume the
  exact same file.
- H.264: 1920x800, 742 frames, 50 MiB.
- Annex-B SHA-256:
  `8eb7e9a95c667db1a4d6085a9e89382752ec9d1cc6b69ac88f8cfabde7c56185`.

No YOLO, RGB conversion or model work is included.

## Production C decoder vs Rust MPP

Both CLIs output the same planar `yuv420p` representation to `/dev/null`:

```bash
# C vendor MPP
ffmpeg -c:v h264_rkmpp -i stream.264 -fps_mode passthrough \
  -pix_fmt yuv420p -f rawvideo -y /dev/null

# Rust MPP
sudo rk-mpp-rs h264dec stream.264 /dev/null
```

### Before optimization

Seven samples, alternating C/Rust order:

| Implementation | Wall time samples | Median | FPS |
| --- | --- | ---: | ---: |
| C `h264_rkmpp` | 1247, 1282, 1294, 1268, 1289, 1253, 1288 ms | **1282 ms** | **578.78** |
| Rust MPP | 2272, 2094, 2239, 2227, 2276, 2205, 2161 ms | **2227 ms** | **333.18** |

Rust takes **73.71% longer** than the production C decoder, or about **1.74x**
C wall time. A following three-sample smoke run measured C 580.6 FPS and Rust
342.3 FPS (+69.6% Rust wall time), consistent with the seven-sample result.

The Rust CLI also reports an internal decode loop of roughly 1.9-2.0 seconds
without output writes, so replacing `/dev/null` is not the main explanation.

### After AU single-scan and native output

The stream path now scans Annex-B start codes once and stores NAL header ranges
per access unit. `decode_parsed_access_unit` only performs RBSP de-escaping for
the AU being decoded. The CLI also exposes native NV12 and DMA-BUF output, and
the non-blocking poll interval defaults to 20 us.

Planar YUV, seven alternating samples:

| Implementation | Median | FPS |
| --- | ---: | ---: |
| C `h264_rkmpp` | **1278 ms** | **580.59** |
| Rust MPP | **2040 ms** | **363.73** |

Rust improves from 333.18 to 363.73 FPS (**+9.15%**). The C-relative wall-time
overhead falls from 73.71% to **59.62%**.

Native-surface path, seven alternating samples:

| Implementation | Median | FPS |
| --- | ---: | ---: |
| C `h264_rkmpp` -> NV12 | **1075 ms** | **690.23** |
| Rust MPP -> DMA-BUF surface | **1662 ms** | **446.45** |

Rust's native path is **54.60%** slower than the C native path. Its internal
decode-loop median is about 1.49 seconds, or roughly 498 FPS. This is the
preferred Rust output mode for an in-process consumer: it avoids planarisation
and makes the hardware surface available directly.

## `mpi_dec_test` harness caveat

The stock vendor `mpi_dec_test` defaults to 4 KiB reader packets. On this
1080p-class stream it repeatedly sleeps while draining decoder output:

| C feeding mode | Median | FPS |
| --- | ---: | ---: |
| 4 KiB `mpi_dec_test` reader | ~8.0 s | 92.75 |
| 256 KiB `mpi_dec_test` reader | **2784 ms** | **266.52** |
| Rust MPP planar internal loop after optimization | **1893.5 ms** | **392.0** |

With the same enlarged reader, Rust appears about 47% faster than `mpi_dec_test`,
but the production C FFmpeg path is substantially faster than both. The result
shows that `mpi_dec_test` with its default reader is a harness-limited
measurement and must not be used as the C hardware ceiling.

`scripts/board_bench_vs_mpp.sh` now defaults `reader_buf_size=262144` for the
vendor tool.

## Correctness check

The first 20 compressed frames were decoded by both implementations to planar
YUV. The first 18 frames are byte-identical. The final two frames differ only
at the truncated-stream flush boundary, where the extracted fixture ends
before the decoder has the same future-reference context. The full-stream
correctness gates in the normal t6/t62 suites remain authoritative.

## Interpretation

The optimized Rust H.264 command path is closer, but still slower than the
production C FFmpeg/RKMPP frontend on a sustained 1080p network stream.
Per-frame profiling after the parser change leaves roughly **1.9 ms in hardware
submit+poll**, versus about 0.3 ms for parsing and DPB preparation. The remaining
gap is therefore not another obvious CPU hot loop:

1. submission and polling remain synchronous per access unit;
2. the input queue is not kept full while the previous picture runs;
3. eliminating that gap needs a multi-job resource ring rather than more parser
   micro-optimisation.

For a YOLO/display consumer, the practical path is already clear: use
`RKMPP_DECODE_DMABUF=1` and keep the decoded surface on the device.

## Reproduce

```bash
VIDEO_URL=http://192.0.2.10:18081/demo30.m3u8 \
STREAM=/tmp/rfi-network-decode.264 \
SAMPLES=7 OUTPUT=dmabuf \
./scripts/board_network_h264_decode_bench.sh
```

The script stores per-sample wall times, Rust logs and `summary.json` under
`target/network-decode-<timestamp>/`.
