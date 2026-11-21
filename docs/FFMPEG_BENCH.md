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
| H.264 | C ffmpeg | 0.554 s | 1083.0 | 1.438 s (260%) | baseline |
| H.264 | ffmpeg+rkmpp-rs | 0.533 s | 1125.7 | 0.986 s (185%) | **+3.9%** |
| H.264 | native rkmpp-rs | **0.238 s** | **2521.0** | **0.202 s (85%)** | **+132.8%** |
| H.265 | C ffmpeg | 0.533 s | 1125.7 | 1.389 s (261%) | baseline |
| H.265 | ffmpeg+rkmpp-rs | 0.529 s | 1134.2 | 1.058 s (200%) | **+0.8%** |
| H.265 | native rkmpp-rs | **0.214 s** | **2803.7** | **0.188 s (88%)** | **+149.1%** |

## Encode

| codec | path | median wall | FPS | CPU time | vs C |
|---|---|---:|---:|---:|---:|
| H.264 | C ffmpeg | **0.327 s** | **917.4** | 0.385 s (118%) | baseline |
| H.264 | ffmpeg+rkmpp-rs | 0.727 s | 412.7 | 0.358 s (49%) | **-55.0%** |
| H.264 | native rkmpp-rs | 0.622 s | 482.3 | **0.177 s (28%)** | **-47.4%** |
| H.265 | C ffmpeg | **0.445 s** | **674.2** | 0.472 s (106%) | baseline |
| H.265 | ffmpeg+rkmpp-rs | 0.744 s | 403.2 | 0.434 s (58%) | **-40.2%** |
| H.265 | native rkmpp-rs | 0.665 s | 451.1 | **0.180 s (27%)** | **-33.1%** |

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
- H.265 2-frame visible NV12 is identical, although its parameter sets and
  packet bytes still differ from vendor C;
- FFmpeg's second `MPP_ENC_SET_CFG` at the first frame now preserves the
  previously set SEI and per-IDR header modes instead of reconstructing an
  encoder with default controls. This removes the extra version/RC SEI NALs.

The board gate compares DMA-BUF input with `RKMPP_COMPAT_CPU_INPUT=1` for
H.264, H.265 and MJPEG, and requires the H.264 12-frame output to match vendor C
byte for byte. The correct uncached mapping costs about 2-4% in the FFmpeg
frontend rows versus the previous incorrect run; native Rust throughput is
unchanged.

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
