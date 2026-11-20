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
| H.264 | C ffmpeg | 0.558 s | 1075.3 | 1.429 s (256%) | baseline |
| H.264 | ffmpeg+rkmpp-rs | 0.513 s | 1169.6 | 0.939 s (183%) | **+8.8%** |
| H.264 | native rkmpp-rs | **0.233 s** | **2575.1** | **0.202 s (87%)** | **+139.5%** |
| H.265 | C ffmpeg | 0.540 s | 1111.1 | 1.446 s (268%) | baseline |
| H.265 | ffmpeg+rkmpp-rs | 0.517 s | 1160.5 | 0.995 s (192%) | **+4.4%** |
| H.265 | native rkmpp-rs | **0.227 s** | **2643.2** | **0.204 s (90%)** | **+137.9%** |

## Encode

| codec | path | median wall | FPS | CPU time | vs C |
|---|---|---:|---:|---:|---:|
| H.264 | C ffmpeg | **0.341 s** | **879.8** | 0.409 s (120%) | baseline |
| H.264 | ffmpeg+rkmpp-rs | 0.711 s | 421.9 | 0.377 s (53%) | **-52.0%** |
| H.264 | native rkmpp-rs | 0.625 s | 480.0 | **0.168 s (27%)** | **-45.4%** |
| H.265 | C ffmpeg | **0.443 s** | **677.2** | 0.402 s (91%) | baseline |
| H.265 | ffmpeg+rkmpp-rs | 0.738 s | 406.5 | 0.371 s (50%) | **-40.0%** |
| H.265 | native rkmpp-rs | 0.656 s | 457.3 | **0.167 s (25%)** | **-32.5%** |

## 2026-11-21 direct DMA-BUF improvement

Two compatibility-copy paths were removed:

- Imported encode input now uses `MppBuffer.fd`; it no longer falls back to CPU
  staging merely because `buffer.dma` is empty.
- Canonical decoded NV12 DMA-BUF storage is moved into the compatibility frame
  instead of being packed into a new `Vec`, allocated from dma-heap, mapped and
  copied for every picture.

Before/after, same seven-sample method:

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
