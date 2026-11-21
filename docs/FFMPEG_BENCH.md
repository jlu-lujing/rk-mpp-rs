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
| H.264 | C ffmpeg | 0.568 s | 1056.3 | 1.433 s (252%) | baseline |
| H.264 | ffmpeg+rkmpp-rs | 0.522 s | 1149.4 | 0.978 s (187%) | **+8.8%** |
| H.264 | native rkmpp-rs | **0.249 s** | **2409.6** | **0.216 s (87%)** | **+128.1%** |
| H.265 | C ffmpeg | 0.528 s | 1136.4 | 1.416 s (268%) | baseline |
| H.265 | ffmpeg+rkmpp-rs | 0.505 s | 1188.1 | 0.993 s (197%) | **+4.6%** |
| H.265 | native rkmpp-rs | **0.215 s** | **2790.7** | **0.188 s (87%)** | **+145.6%** |

## Encode

| codec | path | median wall | FPS | CPU time | vs C |
|---|---|---:|---:|---:|---:|
| H.264 | C ffmpeg | 0.339 s | 885.0 | 0.382 s (113%) | baseline |
| H.264 | ffmpeg+rkmpp-rs | **0.325 s** | **923.1** | 0.341 s (105%) | **+4.3%** |
| H.264 | native rkmpp-rs | 0.582 s | 515.5 | **0.126 s (22%)** | **-41.8%** |
| H.265 | C ffmpeg | **0.442 s** | **678.7** | 0.462 s (105%) | baseline |
| H.265 | ffmpeg+rkmpp-rs | 0.740 s | 405.4 | 0.435 s (59%) | **-40.3%** |
| H.265 | native rkmpp-rs | 0.667 s | 449.8 | **0.178 s (27%)** | **-33.7%** |

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
- H.264 and H.265 are byte-identical to vendor C for both the 12-frame gate
  and 120-frame GOP60 sequences at 320x240 and 1280x720;
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
H.264, H.265 and MJPEG. It requires 12-frame and 120-frame/GOP60 H.264 and H.265
outputs to match vendor C byte for byte. The correct uncached mapping costs
about 2-4% in the FFmpeg frontend rows versus the previous incorrect run; native
Rust throughput is unchanged.

RK3588 retains encoder state across processes. A two-frame warmup was not
enough after a preceding long H.264 run; the long-GOP gate therefore performs a
discarded 120-frame encode with the same codec immediately before each measured
path. This makes the C and Rust processes start from the same hardware state and
removes alternating false failures between the two long cases.

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

H.264 120-frame GOP60 output remains byte-identical at 320x240 and 1280x720.
Seven interleaved 300-frame 720p encode samples:

| path | wall | FPS | CPU | vs C |
|---|---:|---:|---:|---:|
| C FFmpeg | 0.321 s | 934.6 | 0.372 s (116%) | baseline |
| Rust synchronous | 0.711 s | 421.9 | 0.337 s (47%) | -54.9% |
| Rust depth 1 | 0.571 s | 525.4 | 0.328 s (57%) | -43.8% |
| Rust depth 2 | **0.306 s** | **980.4** | 0.334 s (109%) | **+4.9%** |

## Interpretation

- Loading `rk-mpp-rs` through FFmpeg is functionally compatible, but this
  compatibility path now decodes slightly faster than vendor C MPP while using
  substantially less CPU.
- Native Rust has no FFmpeg demux/mux/frontend overhead. Decode is 2.4-2.6x
  faster than C FFmpeg while using roughly one-third of the CPU.
- H.264 encode now beats vendor C FFmpeg by about 4-5% through the depth-2
  session queue. H.265 still uses the synchronous path and remains about 40%
  slower than C in the FFmpeg frontend.
- The next encoder step is the same deferred queue below the VEPU580 H.265
  session.
- Encoded streams were not treated as byte-identical across all three APIs.
  The established exactness gates remain the dedicated standalone and SDK
  tests.

## Reproduce

Run on the board:

```bash
SAMPLES=7 scripts/board_ffmpeg_rkmpp_bench.sh
# H.264 sync/depth-1/depth-2 comparison:
SAMPLES=7 scripts/board_ffmpeg_h264_pipeline_bench.sh
```
