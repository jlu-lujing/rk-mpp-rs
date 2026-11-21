# FFmpeg RKMPP three-way benchmark

Date: 2026-11-21  
Board: RK3588  
CPU governors: `performance` on policies 0/4/6

## Paths

| label | implementation |
|---|---|
| C ffmpeg | installed Rockchip FFmpeg `c5e745e3c1` with vendor `librockchip_mpp.so.1` |
| ffmpeg+rkmpp-rs | the same FFmpeg binary and unchanged `*_rkmpp` wrappers, with `librockchip_mpp.so.1` resolved to `librk_mpp_rs.so` |
| native rkmpp-rs | `rk-mpp-rs` directly using its Rust SDK/CLI, without the FFmpeg C-ABI wrapper |

The FFmpeg rows therefore measure the production FFmpeg frontend. The native
row measures the lower-overhead Rust path and is not an identical application
stack.

## Method

- 1280x720, seven interleaved samples, median.
- Decode: 600-frame H.264 and H.265 streams generated from the same 720p
  `testsrc2` source with fixed QP 26 and GOP 60. Both FFmpeg paths and native
  Rust write NV12 to `/dev/null`.
- Encode: the same 300-frame NV12 file. H.264 High and H.265 Main, fixed QP 26,
  GOP 60, 30 fps, output muxed to `/dev/null`.
- Path order alternates each sample. A warmup runs each path first.
- `vs_c` is `(C wall / path wall - 1) * 100`; positive means faster than
  FFmpeg+C.

## Decode

| codec | path | median wall | FPS | CPU time | vs C |
|---|---|---:|---:|---:|---:|
| H.264 | C ffmpeg | 0.562 s | 1067.6 | 1.437 s (256%) | baseline |
| H.264 | ffmpeg+rkmpp-rs | 0.505 s | 1188.1 | 0.963 s (191%) | **+11.3%** |
| H.264 | native rkmpp-rs | **0.234 s** | **2564.1** | **0.202 s (86%)** | **+140.2%** |
| H.265 | C ffmpeg | 0.545 s | 1100.9 | 1.431 s (263%) | baseline |
| H.265 | ffmpeg+rkmpp-rs | 0.526 s | 1140.7 | 1.007 s (191%) | **+3.6%** |
| H.265 | native rkmpp-rs | **0.210 s** | **2857.1** | **0.187 s (89%)** | **+159.5%** |

## Encode

| codec | path | median wall | FPS | CPU time | vs C |
|---|---|---:|---:|---:|---:|
| H.264 | C ffmpeg | 0.326 s | 920.2 | 0.377 s (116%) | baseline |
| H.264 | ffmpeg+rkmpp-rs | **0.310 s** | **967.7** | 0.330 s (106%) | **+5.2%** |
| H.264 | native rkmpp-rs | 0.577 s | 519.9 | **0.127 s (22%)** | **-43.5%** |
| H.265 | C ffmpeg | **0.426 s** | **704.2** | 0.418 s (98%) | baseline |
| H.265 | ffmpeg+rkmpp-rs | 0.606 s | 495.0 | 0.390 s (64%) | **-29.7%** |
| H.265 | native rkmpp-rs | 0.614 s | 488.6 | **0.128 s (21%)** | **-30.6%** |

## 2026-11-21 direct DMA-BUF improvement

Two compatibility-copy paths were removed:

- Imported encode input now uses `MppBuffer.fd`; it no longer falls back to CPU
  staging merely because `buffer.dma` is empty.
- Canonical decoded NV12 DMA-BUF storage is moved into the compatibility frame
  instead of being packed into a new `Vec`, allocated from dma-heap, mapped and
  copied for every picture.

Before/after, same seven-sample method:
(The "after" column is the direct-copy milestone; the following cache-correctness
section supersedes it with slightly slower but byte-correct numbers.)

| case | before | after | change |
|---|---:|---:|---:|
| H.264 FFmpeg+rkmpp-rs decode | 0.901 s | **0.513 s** | **-43.1% wall** |
| H.265 FFmpeg+rkmpp-rs decode | 0.927 s | **0.517 s** | **-44.2% wall** |
| H.264 FFmpeg+rkmpp-rs encode | 0.806 s | **0.711 s** | **-11.8% wall** |
| H.265 FFmpeg+rkmpp-rs encode | 0.838 s | **0.738 s** | **-11.9% wall** |

The H.264 decode compatibility path also dropped:

| counter | before | after |
|---|---:|---:|
| page faults | 211,580 | **10,173** |
| `mmap` | 1,512 | **915** |
| `munmap` | 1,296 | **699** |
| retired instructions | 783.0 M | **453.9 M** |

H.264 encode retired instructions fell from 223.7 M to **111.0 M**, essentially
the same instruction count as vendor C. Its remaining wall-time gap is
scheduling: the Rust compatibility encoder still performs synchronous
`submit -> poll(enc_done)` per picture, while the FFmpeg C wrapper keeps several
encoder frames in flight. MJPEG intentionally remains on CPU staging because
its current backend does not accept DMA-BUF input.

## 2026-11-21 buffer-cache and session-control correctness

FFmpeg's RKMPP hwcontext requests an uncached MPP DRM buffer by default. The
compatibility allocator had mapped every imported mpp buffer through the
`system` dma-heap even without `MPP_BUFFER_FLAGS_CACHABLE`, so the second and
later CPU-written frames could reach VEPU580 with dirty cache lines. The first
I-frame matched vendor C, while the P-frame diverged.

Allocating the MPP default and DRM32 groups from `system-uncached` restored
exact external-buffer behavior:

- H.264 2-frame H.264 High/QP26 is byte-identical to vendor C and native Rust;
- H.264 12-frame FFmpeg output is byte-identical to vendor C;
- H.264 and H.265 are byte-identical to vendor C for the 12-frame gate and
  60-frame single-GOP sequences at 320x240 and 1280x720;
- FFmpeg's second `MPP_ENC_SET_CFG` at the first frame now preserves the
  previously set SEI and per-IDR header modes instead of reconstructing an
  encoder with default controls. This removes the extra version/RC SEI NALs.
- H.265 now carries the configured level into VPS/SPS and enables temporal MVP
  on the slice, collocated-reference and colmv load/store registers when the
  FFmpeg object path requests it. The H.265 long-term path also emits a fresh
  IDR at each configured GOP boundary.
- The FFmpeg object path has no `KEY_MOTION_INFO`; H.264 therefore leaves
  `mei_stor`/`meiw_addr` clear and delays scene-tune feedback by one picture,
  matching vendor MPP's asynchronous encoder state at the frame where the
  detector first changes rows.

The board gate compares DMA-BUF input with `RKMPP_COMPAT_CPU_INPUT=1` for
H.264, H.265 and MJPEG. It requires 12-frame and 60-frame/single-GOP H.264 and
H.265 outputs to match vendor C byte for byte. The correct uncached mapping
costs about 2-4% in the FFmpeg frontend rows versus the previous incorrect run;
native Rust throughput is unchanged.

Further repetition invalidated the earlier 120-frame/GOP60 exactness claim.
After a second IDR, C FFmpeg produces several different streams across fresh
processes, and those streams decode to different YUV. A cold RK3588 test of
eight 120-frame H.264 runs produced four hashes (`0c9e7106…`,
`d7aba365…`, `cc460bf5…`, `38c89798…`), while eight 60-frame runs all produced
`77ec189d…`. H.265 behaved the same way: eight 60-frame runs were stable at
`6e634163…`. Rust stayed on its deterministic 60-frame-compatible mode. The
exact gate therefore uses one complete 60-frame GOP rather than treating an
unstable second-GOP C result as an oracle.

## 2026-11-21 VEPU580 H.264 deferred queue

The FFmpeg compatibility H.264 fixed-QP path now keeps two hardware jobs below
the encoder session. Each pending job owns its register/batch/STATUS data and
one of two output buffers; packet timing and `KEY_INPUT_FRAME` metadata remain
in submission order. A third frame completes the oldest job before preparing
its replacement.

The queue uses all three reconstruction surfaces in the same `0 → 1 → 2 → 0`
rotation as vendor C. Completion uses the kernel's blocking
`POLL_HW_FINISH`, which writes all queued readbacks before returning; the
non-blocking poll is only safe for the single-job synchronous path.

H.264 60-frame single-GOP output remains byte-identical at 320x240 and 1280x720.
Eleven interleaved 300-frame 720p encode samples:

The dedicated encode benchmarks pin every path to CPUs 4-7 and alternate the
path order on every sample (`C -> sync -> depth1 -> depth2`, then reverse).

| path | wall | FPS | CPU | vs C |
|---|---:|---:|---:|---:|
| C FFmpeg | 0.349 s | 859.6 | 0.388 s (111%) | baseline |
| Rust synchronous | 0.859 s | 349.2 | 0.501 s (58%) | -59.4% |
| Rust depth 1 | 0.617 s | 486.2 | 0.470 s (76%) | -43.4% |
| Rust depth 2 | **0.334 s** | **898.2** | 0.388 s (116%) | **+4.5%** |

## 2026-11-21 VEPU580 H.265 deferred queue

The same compatibility queue now covers VEPU580 H.265 fixed-QP short-term GOPs.
H.265 is different from H.264 in two places:

- every P reconstruction rotates through three surfaces while referring to the
  immediately preceding picture;
- FFmpeg's H.265 object path uses a short-term reference, whereas the standalone
  harness uses the long-term IDR variant.

Eleven interleaved 300-frame 720p samples:

| path | wall | FPS | result |
|---|---:|---:|---|
| C FFmpeg | 0.444 s | 676 FPS | baseline |
| Rust synchronous | 0.893 s | 336 FPS | 60f exact |
| Rust depth 1 | 0.666 s | 451 FPS | 60f exact |
| Rust depth 2 | **0.444 s** | **676 FPS** | **60f exact, default** |

The earlier depth-2 result (0.329 s / 912 FPS) omitted one half of C's
frame-parallel HAL setup and diverged around frame 41. `reg0193_dual_core` is a
persistent cross-task handshake, not picture-local syntax. For C's
`task_cnt == 2` path an intra picture restarts both IDs at zero with `rxe=0`;
later jobs write `txid=curr`, `rxid=prev`, `txe=1`, `rxe=1`, offset 9, then
advance `prev=curr` and `curr=(curr+1)&3`. The first five C words are
`00090010`, `00090031`, `00090036`, `0009003b`, and `0009003c`; Rust now emits
the same sequence.

With the handshake modelled, the 60-frame 320x240 and 1280x720 single-GOP
streams are byte-identical to C. Depth 2 is now the compatibility default. The
median gain over synchronous H.265 is 101%, and Rust matches C wall time while
using less CPU (0.372 s user+sys versus 0.393 s).

## 2026-11-21 lazy imported-buffer mapping

FFmpeg's imported MPP buffers need their DMA-BUF fd for the hardware path, not
their CPU mapping. Previously `mpp_buffer_import()` eagerly mapped every
1280x720 NV12 frame and unmapped it after encode, adding roughly one
`mmap/munmap` pair per picture. Mapping is now deferred until a C caller asks
for the pointer or buffer info, performs read/write, or the encoder falls back
from DMA-BUF to CPU input.

On a three-run `perf stat`, pinned to CPUs 4-7:

| H.265 path | task-clock | instructions | cycles |
|---|---:|---:|---:|
| C FFmpeg | 402.6 ms | 327.3 M | 468.4 M |
| Rust FFmpeg | **362.9 ms** | 327.4 M | 449.9 M |

The Rust instruction count now matches C instead of exceeding it by about 19%.
Over 300 frames, `strace -c` mmap calls fell from 544 to 244. The C object gate
still verifies that `mpp_buffer_get_ptr()`, `mpp_buffer_info_get()` and
read/write lazily establish the mapping and preserve byte contents.

## Interpretation

- Loading `rk-mpp-rs` through FFmpeg is functionally compatible, but this
  compatibility path now decodes slightly faster than vendor C MPP while using
  substantially less CPU.
- Native Rust has no FFmpeg demux/mux/frontend overhead. Decode is 2.4-2.6x
  faster than C FFmpeg while using roughly one-third of the CPU.
- H.264 encode is about 4.5% faster than vendor C FFmpeg through the depth-2
  session queue. H.265 depth 2 raises synchronous throughput by about 101% to
  676 FPS, matching C wall time while using less CPU and remaining byte-exact.
- Encoded streams were not treated as byte-identical across all three APIs.
  The established exactness gates remain the dedicated standalone and SDK
  tests.

## Reproduce

Run on the board:

```bash
SAMPLES=7 scripts/board_ffmpeg_rkmpp_bench.sh
# H.264 sync/depth-1/depth-2 comparison:
SAMPLES=11 scripts/board_ffmpeg_h264_pipeline_bench.sh
# H.265 sync/depth-1/depth-2 comparison:
SAMPLES=11 scripts/board_ffmpeg_h265_pipeline_bench.sh
```
