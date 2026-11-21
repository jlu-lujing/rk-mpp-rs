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
| H.264 | C ffmpeg | 0.557 s | 1077.2 | 1.413 s (254%) | baseline |
| H.264 | ffmpeg+rkmpp-rs | 0.494 s | 1214.6 | 0.978 s (198%) | **+12.8%** |
| H.264 | native rkmpp-rs | **0.231 s** | **2597.4** | **0.200 s (87%)** | **+141.1%** |
| H.265 | C ffmpeg | 0.531 s | 1129.9 | 1.427 s (269%) | baseline |
| H.265 | ffmpeg+rkmpp-rs | 0.513 s | 1169.6 | 0.976 s (190%) | **+3.5%** |
| H.265 | native rkmpp-rs | **0.215 s** | **2790.7** | **0.188 s (87%)** | **+147.0%** |

## Encode

| codec | path | median wall | FPS | CPU time | vs C |
|---|---|---:|---:|---:|---:|
| H.264 | C ffmpeg | **0.328 s** | **914.6** | 0.390 s (119%) | baseline |
| H.264 | ffmpeg+rkmpp-rs | 0.707 s | 424.3 | 0.438 s (62%) | **-53.6%** |
| H.264 | native rkmpp-rs | 0.630 s | 476.2 | **0.179 s (28%)** | **-47.9%** |
| H.265 | C ffmpeg | **0.434 s** | **691.2** | 0.447 s (103%) | baseline |
| H.265 | ffmpeg+rkmpp-rs | 0.746 s | 402.1 | 0.329 s (44%) | **-41.8%** |
| H.265 | native rkmpp-rs | 0.623 s | 481.5 | **0.134 s (22%)** | **-30.3%** |

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

## Interpretation

- Loading `rk-mpp-rs` through FFmpeg is functionally compatible, but this
  compatibility path now decodes slightly faster than vendor C MPP while using
  substantially less CPU.
- Native Rust has no FFmpeg demux/mux/frontend overhead. Decode is 2.4-2.6x
  faster than C FFmpeg while using roughly one-third of the CPU.
- For encode, vendor C FFmpeg is still the fastest wall-clock path. DMA input
  staging is gone from FFmpeg+rkmpp-rs, but synchronous encoder completion
  leaves hardware idle between pictures.
- The next encoder step is a 2-4-job submit/poll pipeline, not another copy
  optimization.
- Encoded streams were not treated as byte-identical across all three APIs.
  The established exactness gates remain the dedicated standalone and SDK
  tests.

## Reproduce

Run on the board:

```bash
SAMPLES=7 scripts/board_ffmpeg_rkmpp_bench.sh
```
