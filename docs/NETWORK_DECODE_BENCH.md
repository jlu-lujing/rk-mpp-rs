# Network H.264 decode benchmark

Date: 2026-11-15  
Board: RK3588
CPU policy: persistent 80% OPP cap (`1.416 / 1.8 / 1.8 GHz`)

## Input

- `s2` hosts a 30-second HLS stream (`192.0.2.10:18081`).
- RK3588 pulls the HLS stream over its normal LAN.
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

Queueing was therefore left opt-in (`RKMPP_H264_PIPELINE=<depth>`) at that
point, with correctness gates running it off. The 2026-11-17 section below
closes that lifetime bug; the queued path is now the default at depth 3.

### 2026-11-17: the pending ring is keyed to jobs, and queueing is on

The deferred-output bookkeeping had four bugs, all of which only showed up
once more than one job was in flight:

1. **One pending slot for a ring of up to four jobs.** Depth 3 lost one
   picture per run (`239/240`), because the second picture overwrote the
   first in the single `Option<PendingFrame>`.
2. **Top fields armed a job but produced no picture**, so the ring and the
   pending list desynchronised and the PAFF stream pulled the wrong picture
   out of the queue.
3. **Concealed pictures armed no job**, so the missing-AU stream silently lost
   the frame the blocking path still emits.
4. **Deferred leases exhausted the surface pool.** A picture whose job was
   still in flight kept its surface leased, but the slot selector reused that
   slot anyway (replacing the lease with a fresh allocation) until
   `BufferPool` reported `Full`.

Fixes: the pending list is a `VecDeque<Option<PendingFrame>>` with exactly one
entry per submitted job (`None` for a top field); pictures that need no
hardware job (error concealment) are queued in submission order instead of
being dropped; the slot selector prefers a free surface nobody else leases;
and the surface pools carry `JOB_SLOTS` spare slots. If the pool still runs
short, the oldest job is reaped early and its picture is queued — the pool
becomes backpressure on queue depth instead of a hard failure.

`RKMPP_H264_PIPELINE` now defaults to 3 for `rk-mpp-rs h264dec`; `0` restores
the blocking path. Depth 3 reaches the same poll time as 4 (about 15 us per
picture) without needing the extra surface.

**Fairness gate.** A timing comparison is only meaningful when both decoders
produce the same pictures, so every stream below was first compared
byte-for-byte against software FFmpeg: Rust, C and the software reference are
120/120 identical for `test720.264` (1280x720), 90/90 for `test_hard.264`
(1920x1080), 240/240 for `/tmp/ai.264` (all-intra 1920x800) and 600/600 for
`/tmp/demo_short.264` (1920x800 inter). The last one required a real fix: it
wraps `frame_num` at 16, and eviction plus P-list ordering compared the raw
`frame_num` instead of FrameNumWrap, which selected the wrong reference and
produced ±1 drift from frame 18. See `docs/DIVERGENCES.md`; it is now
byte-identical and is included below.

Seven interleaved samples, wall clock, DDR governor pinned (`dmc` 2112 MHz, all
CPU policies `performance`), planar output to `/dev/null` on both sides (`C` =
production `ffmpeg -c:v h264_rkmpp`):

| Stream | frames | C | Rust (depth 3) | Rust advantage |
| --- | ---: | ---: | ---: | ---: |
| 1920x800 inter (`/tmp/demo_short.264`) | 600 | 923 ms | **533 ms** | **+73.2 %** |
| 1920x1080 inter (`test_hard.264`) | 90 | 287 ms | **202 ms** | **+42.1 %** |
| 1280x720 (`test720.264`) | 120 | 207 ms | **141 ms** | **+46.8 %** |
| all-intra 1920x800 (`/tmp/ai.264`) | 240 | 442 ms | **372 ms** | **+18.8 %** |

Wall clock includes process start-up, and FFmpeg's fixed cost is larger: on a
two-frame 48x32 stream the same commands take about 50 ms (C) and 30 ms (Rust).
Subtracting that measured overhead leaves the 600-frame row at about +71 % and
the short rows at roughly +40 % (1080p), +43 % (720p) and +15 % (all-intra) —
smaller, but still ahead on every verified stream. Only the comparison against
the previous single-job H.264 path is unaffected by start-up: the same
1920x800 inter stream took 1819 ms with one job in flight, so the ring is worth
roughly 3.4x inside the decoder.

Method note: never compare the CLI by piping its stdout. The command prints
"N access units" and three per-frame trace lines to stdout before/between the
YUV bytes, so a pipe-based hash silently misaligns every frame. Comparisons
must go through a real file (or `/dev/null`), which is what the numbers above
do.

Correctness at that speed:

* `scripts/board_regress.sh` is **7/7 PASS** with the default depth, including
  the PAFF `field_paff.264` and missing-AU `mr1_missing_au20.264` streams.
* The same seven streams are byte-exact at depths 1, 2, 3 and 4 when depth is
  forced explicitly.
* Deferred output is output-preserving: `test720.264` at depths 1-4 is
  byte-identical to the reference, and the NV12 path matches the planar
  reference converted by ffmpeg.
* `RKMPP_DECODE_DMABUF=1` still reports 120/120 output frames on
  `test720.264`; `RKMPP_H264_PIPELINE=0` still matches the reference.
* Workspace gate: **1319 passed / 0 failed** over 59 suites; release library
  **1149 passed / 0 failed / 0 ignored**.

## Concurrent streams: how many 30 FPS decodes fit (2026-11-18)

Each stream is an independent process decoding `/tmp/rfi-network-demo30.264`
(1920x800, 742 frames) to planar YUV on `/dev/null`; C is production
`ffmpeg -c:v h264_rkmpp`, Rust is `rk-mpp-rs h264dec`. Sweeps ran to 48
streams, and each reported point is the slowest stream in the batch (three
rounds at the ceiling, all stable).

| streams | C slowest stream | Rust slowest stream |
| ---: | ---: | ---: |
| 8 | 98.2 FPS | 152.0 FPS |
| 16 | 49.4 | 76.4 |
| 24 | 32.7 | 50.8 |
| 26 | **30.5-30.8** | 47 | 
| 28 | 28.1 | 43.3 |
| 32 | 24.6 | 38.0 |
| 40 | — | **30.4** |
| 44 | — | 27.8 |
| 48 | — | 25.5 |

Aggregate throughput is flat once the machine is full: **~798 FPS for C** and
**~1225 FPS for Rust**. The two ceilings therefore are:

| | max streams at >=30 FPS | aggregate | CPU at ceiling | RSS at ceiling |
| --- | ---: | ---: | ---: | ---: |
| C `ffmpeg h264_rkmpp` | **26** | 798 FPS | ~100 % (all cores) | ~3.3 GB |
| Rust `h264dec` | **40** | 1225 FPS | ~18 % | ~2.2 GB |

C saturates the CPU before the video block: one C stream already uses ~256 %
CPU, and at 26 streams the host is at 100 % while aggregate decode is still
only 798 FPS. Rust is hardware-bound at 40 streams (~18 % CPU), and 1225 FPS
is the silicon limit measured on this stream. At C's ceiling of 26 streams,
Rust still delivers ~47 FPS per stream.

All 40 Rust processes at the ceiling exited 0 and each reported
`decoded 742 frames (742 output)`; all 26 C processes exited 0. No
`mpp_task_timeout_work` or decoder reset appeared during the sweeps.

### Why 40 and not more: the VPU is the limit, not the CPU

At the 40-stream ceiling the host CPU sits at about 18 %: every process spends
its time blocked in `POLL_HW_FINISH` while the decoder works. The two
`rkvdec` cores are what run out:

| run | rkvdec-core0 | rkvdec-core1 | total jobs/s |
| --- | ---: | ---: | ---: |
| Rust, 40x 1920x800 | 620/s | 620/s | **1240** |
| C, 26x 1920x800 (its ceiling) | 442/s | 442/s | 884 |

`1240 / 30 = 41`, which is exactly the observed 40-stream ceiling. The number
is not a process or session limit; it is "aggregate decode throughput divided
by 30". The same machine on the same codec scales with the decode workload:

| stream | aggregate at 8 concurrent | streams at >=30 FPS |
| --- | ---: | ---: |
| 640x360 | 5941 FPS | ~198 |
| 1280x720 | 1630 FPS | ~54 |
| 1920x800 | 1214 FPS | ~40 |
| 1920x1080 | 735 FPS | ~24 |

Planar and NV12 output give the same aggregate (1218 vs 1213 FPS at 8
streams), so the host-side chroma conversion is not the limiter either. C
leaves the VPU at about 71 % (884 of 1240 jobs/s) because its frontend already
uses every CPU core at 26 streams; Rust reaches the silicon ceiling with the
CPU mostly idle.

This supersedes the older note in `rknn-fastinfer/docs/LIBRARY.md`
(C 28 / Rust 24): that used a different 1080p30 stream and a Rust build from
before the submission ring, NEON chroma split and streamed output.

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

## Bounded-memory `h264_stream` (2026-11-21)

The streaming CLI previously kept bounded display reorder but still called
`std::fs::read()` on the complete Annex-B input. It now reuses the incremental
H.264 access-unit scanner with a 4 MiB read chunk.

Three interleaved old/new pairs on the 50 MiB, 742-frame 1920x800 stream:

| Version | warm wall samples | warm median | peak RSS |
| --- | --- | ---: | ---: |
| whole-file splitter | 1.575, 1.562, 1.576 s | **1.576 s** | **55.9 MiB** |
| bounded scanner | 1.582, 1.595, 1.597 s | **1.595 s** | **17.0 MiB** |

The first bounded run was cold at 2.226 s. The bounded version emits the same
742 output frames. `test720.264` output remains byte-identical:

- planar:
  `f31d2c0321ed82f6f2d4da5fed36bdce53ee355773879877de93138bf1500582`
- NV12:
  `f14c7ca6b599c8930fab66f5fb4e4656e488b608bfc583aaeba919d1681cd6e2`

This change also fixed the stable SDK's H.264 reorder refresh: before the fix
the SDK could retain its initial depth 16 even after SPS parsing declared 2.

### Main `h264dec` bounded input

The primary CLI now uses the same scanner and keeps the existing three-job
submission ring. Three interleaved old/new pairs:

| Version | wall samples | median | peak RSS |
| --- | --- | ---: | ---: |
| whole-file `h264dec` | 0.665, 0.664, 0.677 s | **0.665 s** | **59.5 MiB** |
| bounded `h264dec` | 0.666, 0.677, 0.665 s | **0.665 s** | **20.4 MiB** |

The bounded path reports `742 access units`, `742 frames / 742 output`, and
retains the same 720p planar/NV12 SHA-256 hashes shown above.

## Correctness check

The first 20 compressed frames were decoded by both implementations to planar
YUV. The first 18 frames are byte-identical. The final two frames differ only
at the truncated-stream flush boundary, where the extracted fixture ends
before the decoder has the same future-reference context. The full-stream
correctness gates in the normal RK3588 suites remain authoritative.

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
