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
| H.264 | C ffmpeg | 0.565 s | 1061.9 | 1.443 s (255%) | baseline |
| H.264 | ffmpeg+rkmpp-rs | 0.901 s | 665.9 | 1.355 s (150%) | **-37.3%** |
| H.264 | native rkmpp-rs | **0.235 s** | **2553.2** | **0.201 s (86%)** | **+140.4%** |
| H.265 | C ffmpeg | 0.546 s | 1098.9 | 1.434 s (263%) | baseline |
| H.265 | ffmpeg+rkmpp-rs | 0.927 s | 647.2 | 1.462 s (158%) | **-41.1%** |
| H.265 | native rkmpp-rs | **0.211 s** | **2843.6** | **0.187 s (89%)** | **+158.8%** |

## Encode

| codec | path | median wall | FPS | CPU time | vs C |
|---|---|---:|---:|---:|---:|
| H.264 | C ffmpeg | **0.326 s** | **920.2** | 0.392 s (120%) | baseline |
| H.264 | ffmpeg+rkmpp-rs | 0.806 s | 372.2 | 0.533 s (66%) | **-59.6%** |
| H.264 | native rkmpp-rs | 0.617 s | 486.2 | **0.176 s (29%)** | **-47.2%** |
| H.265 | C ffmpeg | **0.445 s** | **674.2** | 0.445 s (100%) | baseline |
| H.265 | ffmpeg+rkmpp-rs | 0.838 s | 358.0 | 0.536 s (64%) | **-46.9%** |
| H.265 | native rkmpp-rs | 0.615 s | 487.8 | **0.125 s (20%)** | **-27.6%** |

## Interpretation

- Loading `rk-mpp-rs` through FFmpeg is functionally compatible, but this
  compatibility path is not currently a throughput win over vendor C MPP.
- FFmpeg+rkmpp-rs decode uses less CPU than C FFmpeg but has lower wall-clock
  throughput. That points to serialized work or waits in the C-ABI bridge, not
  extra total CPU load.
- Native Rust has no FFmpeg demux/mux/frontend overhead. Decode is 2.4-2.6x
  faster than C FFmpeg while using roughly one-third of the CPU.
- For encode, vendor C FFmpeg is the fastest wall-clock path in this setup.
  Native Rust is slower than C FFmpeg but uses only 20-29% CPU, versus 100-120%
  for C.
- Encoded streams were not treated as byte-identical across all three APIs.
  The established exactness gates remain the dedicated standalone and SDK
  tests.

## Reproduce

Run on the board:

```bash
SAMPLES=7 scripts/board_ffmpeg_rkmpp_bench.sh
```
