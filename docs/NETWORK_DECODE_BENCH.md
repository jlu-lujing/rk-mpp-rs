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

### Lazy RBSP and word-width de-escape probe

`perf` on the optimised tree still attributed 28.2 % of cycles to
`split_annex_b_headers` and 23.8 % to `ebsp_to_rbsp`. The second one was wasted
work: only the SPS, the PPS and the first slice header of an access unit are
parsed, while every slice payload is handed to the hardware as raw EBSP. The
start-code walk itself is inherently one pass over the stream, so it only got a
cheaper inner loop. Two changes followed.

- `decode_nals` now takes `NalHeader` and de-escapes lazily. SPS/PPS and the
  first VCL slice are expanded on demand; the slice payloads are copied raw, so
  the per-AU `Vec<NalUnit>` with one `Cow` per NAL is gone entirely.
- The `00 00 03` probe behind `ebsp_to_rbsp` scans eight bytes at a time with a
  SWAR zero-pair test and only falls back to byte comparisons on a hit. A unit
  test compares it against `windows(3).any()` on hand-picked shapes and
  exhaustively over 3^12 payloads from the `{0, 1, 3}` alphabet.

Seven alternating samples after both changes:

| Path | Median | FPS |
| --- | ---: | ---: |
| C `h264_rkmpp` -> planar | **1284 ms** | **577.88** |
| Rust MPP -> planar | **2013 ms** | **368.60** |
| C `h264_rkmpp` -> NV12 | **1092 ms** | **679.49** |
| Rust MPP -> DMA-BUF surface | **1585 ms** | **468.14** |

That is another **+1.36 %** planar and **+4.86 %** native-surface over the
previous step, and the C-relative wall-time overhead is now 56.78 % planar and
45.15 % native. Raw samples and the phase profile live in
`bench/network-decode/2026-11-15/phase-profile.json`. `cargo test --release --lib` parses the whole 51 MB stream in
88 ms, so the parser is no longer the lever.

### Queue depth: what the driver actually does

An `LD_PRELOAD` shim around `ioctl` captured every `MppReqV1` batch from both
the vendor path and this crate (742 frames each). The register files are
identical except the stream-length padding (±8 bytes) and one error-concealment
bit, and both submit nine to ten requests per frame, so the submission shape is
not the difference.

Measured with the DDR governor pinned to `performance` (the default
`dmc_ondemand` moves between 528 MHz and 2112 MHz, which alone swings this
benchmark by 1.8x) and the decode thread pinned to a big core:

| | vendor | this crate (1 job) |
| --- | ---: | ---: |
| poll wait per frame | **0.87 ms** | **1.58 ms** |
| submits per poll | 1-4 | 1 |

Queueing deeper does work when the pictures are independent. On an all-intra
240-frame 1920x800 stream (`/tmp/ai.264`):

| depth | wall | per frame |
| --- | ---: | ---: |
| 1 (blocking) | 700-740 ms | 2.9-3.1 ms |
| 2 | 370-384 ms | 1.55 ms |
| 3 | 264-282 ms | **1.15 ms** |
| vendor | 358-435 ms | 1.5-1.8 ms |

Depth 3 is 1.4x faster than the vendor path, which shows the hardware does
have the headroom. It also shows the blocking path carries roughly 1.7 ms per
frame of round-trip overhead that queueing hides.

On streams with real inter prediction the same queueing produces deterministic
but wrong pictures (40/120 frames at depth 2, 119/120 at depth 3), and the
driver logs `mpp_rkvdec2 ... resetting for err 0x23 / 0x107`. The register
stream is identical to the vendor's, so the remaining work is the DPB/frame
lifetime under deferred output rather than the register file: the deferred
picture holds a lease, which changes which buffer the pool hands out next.

Queueing therefore stays opt-in (`RKMPP_H264_PIPELINE=<depth>`), and correctness
gates run with it off.

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
production C FFmpeg/RKMPP frontend on a sustained 1080p network stream. The
remaining gap is measured, not guessed:

| Measurement | Result |
| --- | --- |
| `perf stat` task-clock over 1629 ms wall | **283 ms**, 0.174 CPUs |
| submit ioctl, per picture | **18 us** |
| poll (hardware), per picture | **1761 us** |
| whole-stream Annex-B split, 51 MB | **88 ms** |

So the process is idle about 83 % of the wall time and the parser is no longer
the lever. The driver already round-robins a single stream across both
`rkvdec-core` instances — `/proc/interrupts` shows equal counts — but a score
that saturates the silicon needs more than one picture in flight:

| Concurrent decodes | Rust wall | C wall |
| --- | ---: | ---: |
| 1 | 1827 ms | 809 ms |
| 2 | 1966 ms | 1390 ms |
| 3 | 1983 ms | 1937 ms |
| 4 | 2612 ms | 2476 ms |

Rust barely slows down from one to three concurrent decoders, which is the
signature of a submission pipe that is never full; C's single-stream 809 ms is
already close to two-job overlap. The remaining work is therefore a multi-job
resource ring (per-job register batch, global/RPS tables, RCB, stream buffer and
output surface), not another parser micro-optimisation.

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
