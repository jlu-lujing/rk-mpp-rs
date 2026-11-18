# Encoder throughput against C MPP

Board: NanoPC-T6-LTS (RK3588), CPU and DDR governors pinned to `performance`.

## Method

- 60 frames of `testsrc2` NV12, 320x240 / 1280x720 / 1920x1088.
- Fixed QP 26 (`-rc 2 -qc 26:...` on the C harness, `26` on the CLI), GOP 60
  on both sides, Baseline H.264 / Main H.265.
- Five interleaved samples, medians; both write to `/dev/null`, so the measured
  region is submit + poll + parameter sets + bitstream assembly.
- **Same work first:** the 60-frame 1080p H.264 and H.265 streams were compared
  byte for byte. The only difference is the build-identity SEI text (C writes
  its own git line, this crate writes `rk-mpp-rs 0.1.0 ...`);
  `scripts/strip_version_sei.py` removes exactly that NAL and the remaining
  streams are identical.

## Single stream

| codec | size | C MPP | this crate | Rust advantage |
| --- | --- | ---: | ---: | ---: |
| H.264 | 320x240 | 61 ms (984 fps) | **44 ms (1364 fps)** | **+38.6 %** |
| H.264 | 1280x720 | 278 ms (216 fps) | **150 ms (400 fps)** | **+85.3 %** |
| H.264 | 1920x1088 | 570 ms (105 fps) | **306 ms (196 fps)** | **+86.3 %** |
| H.265 | 320x240 | 72 ms (833 fps) | **52 ms (1154 fps)** | **+38.5 %** |
| H.265 | 1280x720 | 226 ms (266 fps) | **214 ms (280 fps)** | **+5.6 %** |
| H.265 | 1920x1088 | 570 ms (105 fps) | **421 ms (143 fps)** | **+35.4 %** |

## Concurrent sessions (1920x1088, 60 frames each)

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
