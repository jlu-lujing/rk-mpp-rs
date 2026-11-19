# Encoder throughput against C MPP

Board: NanoPC-T6-LTS (RK3588), CPU and DDR governors pinned to `performance`.

## Method

- 60 frames of `testsrc2` NV12, 320x240 / 640x480 / 1280x720 / 1920x1088.
- Fixed QP 26 (`-rc 2 -qc 26:...` on the C harness, `26` on the CLI), GOP 60
  on both sides, Baseline H.264 / Main H.265.
- Five interleaved samples, medians; both write to `/dev/null`, so the measured
  region is submit + poll + parameter sets + bitstream assembly.
- **Same work first:** the 60-frame 1080p H.264 and H.265 streams were compared
  byte for byte. The only difference is the build-identity SEI text (C writes
  its own git line, this crate writes `rk-mpp-rs 0.1.0 ...`);
  `scripts/strip_version_sei.py` removes exactly that NAL and the remaining
  streams are identical.

## 2026-11-19: H.265 file-input pipeline

`h265-gop` previously read each frame into a temporary `Vec` and then copied
that frame into the encoder's DMA surface on the encoder thread. H.264 already
used two reusable `system-uncached` surfaces: a reader thread filled the next
surface while the encoder drove VEPU580 on the current one. H.265 now uses the
same bounded two-surface pipeline and submits the surface fd directly.

On t6 at the persistent 80% OPP cap, 120 frames of 720p H.265 gave:

| metric | before | after | change |
| --- | ---: | ---: | ---: |
| wall time | 415 ms | 302 ms | -27.2% |
| task-clock | 182.4 ms | 99.8 ms | -45.3% |
| system time | 137.7 ms | 58.5 ms | -57.5% |

The final binary's separate five-sample wall-only check had a 279 ms median.
The before/after streams were byte-identical:
`a2865a07…88d20a`. A seven-sample interleaved heap check kept
`system-uncached` (275 ms median) over `system` (285 ms median), so the faster
result is not an uncached-heap accident.

The current five-sample, 60-frame interleaved medians are:

| codec | size | C MPP | this crate | Rust advantage |
| --- | --- | ---: | ---: | ---: |
| H.264 | 320x240 | 66 ms (909.1 fps) | **50 ms (1200.0 fps)** | **+32.0%** |
| H.264 | 640x480 | 113 ms (531.0 fps) | **71 ms (845.1 fps)** | **+59.2%** |
| H.264 | 1280x720 | 213 ms (281.7 fps) | **146 ms (411.0 fps)** | **+45.9%** |
| H.264 | 1920x1088 | 385 ms (155.8 fps) | **306 ms (196.1 fps)** | **+25.8%** |
| H.265 | 320x240 | 64 ms (937.5 fps) | **46 ms (1304.3 fps)** | **+39.1%** |
| H.265 | 640x480 | 133 ms (451.1 fps) | **75 ms (800.0 fps)** | **+77.3%** |
| H.265 | 1280x720 | 216 ms (277.8 fps) | **170 ms (352.9 fps)** | **+27.1%** |
| H.265 | 1920x1088 | 407 ms (147.4 fps) | **300 ms (200.0 fps)** | **+35.7%** |

For a steadier work region, one 500-frame 720p run measured H.264 at 1.022 s
against C's 2.503 s, and H.265 at 1.047 s against C's 1.842 s. Those two
figures include process setup and teardown, like the scripts above.

## Earlier 60-frame single-stream baseline

| codec | size | C MPP | this crate | Rust advantage |
| --- | --- | ---: | ---: | ---: |
| H.264 | 320x240 | 64 ms (938 fps) | **52 ms (1154 fps)** | **+23.1 %** |
| H.264 | 1280x720 | 308 ms (195 fps) | **165 ms (364 fps)** | **+86.7 %** |
| H.264 | 1920x1088 | 562 ms (107 fps) | **276 ms (217 fps)** | **+103.6 %** |
| H.265 | 320x240 | 66 ms (909 fps) | **47 ms (1277 fps)** | **+40.4 %** |
| H.265 | 1280x720 | 258 ms (233 fps) | **181 ms (332 fps)** | **+42.5 %** |
| H.265 | 1920x1088 | 586 ms (102 fps) | **345 ms (174 fps)** | **+69.9 %** |

## Earlier concurrent sessions (1920x1088, 60 frames each)

| codec | sessions | C aggregate | Rust aggregate | Rust |
| --- | ---: | ---: | ---: | ---: |
| H.264 | 2 | 227 FPS | **392 FPS** | +73 % |
| H.264 | 4 | 442 FPS | **470 FPS** | +6 % |
| H.264 | 8 | **517 FPS** | 511 FPS | -1 % |
| H.265 | 2 | 197 FPS | **260 FPS** | +32 % |
| H.265 | 4 | 480 FPS | 460 FPS | -4 % |
| H.265 | 8 | 493 FPS | **496 FPS** | **+1 %** |

The first sweep had H.265 at 405 FPS at 8 sessions (18 % behind) with 2.54 s of
system time against C's 1.07 s. The cause was the CLI reading the whole input
file per session (`fs::read`): eight sessions each pulled a private 82 MB copy
through the page cache, and `read` dominated the syscall profile. `h265-gop`,
`h265-gop-rc` and `h265-rate` now open the file and decode one NV12 frame at a
time, matching the H.264 path; the 8-session gap is gone and the Rust run uses
less system time than C (0.499 s vs 0.548 s at 720p/8 sessions).

## JPEG

On 12x 1080p frames the Rust JPEG encoder takes about 71 ms against 115 ms for
`ffmpeg -c:v mjpeg_rkmpp`, but the two were driven with different quality
knobs; only the direction is claimed.

## Reproduce

```bash
SAMPLES=7 scripts/board_bench_encode_vs_mpp.sh
SAMPLES=7 scripts/board_bench_h265_encode_vs_mpp.sh
```
