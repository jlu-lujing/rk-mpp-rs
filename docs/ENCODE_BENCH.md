# Encoder throughput against C MPP

Board: RK3588, CPU and DDR governors pinned to `performance`.

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

## 2026-11-22: upstream H.265 object path after scene-tune closure

The unchanged upstream `mpp_enc_test` was run once with vendor C and once with
`librk_mpp_rs.so` preloaded. Both used the same 300-frame moving `testsrc2`
input, fixed QP 26 and `-g 0:60`. RK3588 was at the persistent 80% OPP cap
(little 1.416 GHz, big 1.8 GHz), DDR was `performance` at 2.112 GHz, and both
commands were pinned to CPU 6. Five C/Rust samples were interleaved.

| size | C wall | Rust wall | Rust wall gain | C user+sys | Rust user+sys | Rust CPU reduction |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 320x240 | 164 ms | 164 ms | 0.0% | 69 ms | **67 ms** | **2.9%** |
| 1280x720 | 868 ms | **802 ms** | **7.6%** | 291 ms | **226 ms** | **22.3%** |

This is the current multi-GOP scene-tune path, not a single-GOP benchmark.
The same 320x240 run is byte-identical to C at 315 NALs and SHA-256
`d577adc9…32edea`; see `scripts/board_h265_scene_tune_300.sh`. The benchmark
gate fails if Rust exceeds C by more than 5% in either wall or user+sys:

```bash
BOARD=my-rk3588-host SAMPLES=5 ./scripts/board_bench_h265_object_vs_mpp.sh
```

The same harness now covers H.264. Five interleaved 300-frame samples at the
same 80% OPP/CPU6 conditions give:

| size | C wall | Rust wall | Rust wall delta | C user+sys | Rust user+sys | Rust CPU delta |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 320x240 | 152 ms | 157 ms | -3.3% | 67 ms | 67 ms | 0.0% |
| 1280x720 | 822 ms | **769 ms** | **+6.4%** | 278 ms | **232 ms** | **16.5% less** |

The 320x240 cell is inside the existing 5% parity gate; 720p is faster with
lower CPU. H.264 and H.265 now share one implementation:

```bash
BOARD=my-rk3588-host SAMPLES=5 ./scripts/board_bench_h264_object_vs_mpp.sh
BOARD=my-rk3588-host SAMPLES=5 ./scripts/board_bench_h265_object_vs_mpp.sh
# or: CODEC=h264|h265 BOARD=... ./scripts/board_bench_object_encode_vs_mpp.sh
```

A 1000-frame steady-state run removes the 300-frame process-startup share.
Seven interleaved 320x240 samples give:

| codec | C wall | Rust wall | Rust wall gain | C user+sys | Rust user+sys | Rust CPU reduction |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| H.264 | 493 ms | **448 ms** | **9.1%** | 202 ms | **138 ms** | **31.7%** |
| H.265 | 484 ms | **457 ms** | **5.6%** | 185 ms | **140 ms** | **24.3%** |

The benchmark accepts `SIZES=320x240,1280x720` and `FRAMES=N`, so a focused
steady-state cell can be rerun without changing the script.

## 2026-11-19: H.265 file-input pipeline

`h265-gop` previously read each frame into a temporary `Vec` and then copied
that frame into the encoder's DMA surface on the encoder thread. H.264 already
used two reusable `system-uncached` surfaces: a reader thread filled the next
surface while the encoder drove VEPU580 on the current one. H.265 now uses the
same bounded two-surface pipeline and submits the surface fd directly.

On RK3588 at the persistent 80% OPP cap, 120 frames of 720p H.265 gave:

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

## 2026-11-20: H.265 session CPU fallback drops cached-input syncs

The object/compat fallback still used the session's cached `system` input and
two `DMA_BUF_SYNC` ioctls per picture. It now uses `system-uncached`, matching
the file pipeline, and omits those syncs. On RK3588 at 80% OPP, seven interleaved
500-frame 720p samples gave:

| change | wall median | user+sys median |
| --- | ---: | ---: |
| cached + sync | 2.379 s | 1.287 s |
| uncached + sync | 2.006 s | 1.011 s |
| uncached, no sync | **1.412 s** | **0.455 s** |

The stream stayed byte-identical
(`6fea6748…da157dc`). Applying the same sync removal to H.264 was not kept:
the interleaved A/B did not show a stable improvement.

H.265 aggregate feedback also cleared all 180 STATUS words per frame even
though only 14 are consumed. Resetting and summing just words 0/3 and 64-75
moved the same seven-sample wall median from 1.412 to **1.318 s** and
user+sys from 0.455 to **0.366 s**, with the same stream hash.

`h265-gop` now writes each completed access unit through a 1 MiB `BufWriter`
instead of retaining the whole encoded sequence in one `Vec`. The 120-frame
720p SHA remained identical, and `/dev/null` keeps its no-file-write benchmark
path.

The two rate-control paths now share the same input/output treatment.
`h265-gop-rc` and `h265-rate` reuse the two-surface DMA input pipeline and
stream each access unit through a 1 MiB writer. Five interleaved 120-frame
720p samples moved GOP-RC from 387 ms forced-single-core to 279 ms pipelined,
and model CBR from 384 to 273 ms. Their pipelined and single-core streams were
byte-identical:
`7c4e93db…c11f65` for GOP-RC and `bb99af68…d7a6ad03` for CBR.

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

H.264 now keeps its STATUS vector and `Batch` in the session instead of
allocating both per picture. Seven interleaved 500-frame 720p samples at 80%
OPP reduced the median from 1140 to 1055 ms. The 7,031,305-byte output stayed
byte-identical (`68715bd4…e2d5617c05`).

H.265 similarly reuses aggregate/per-tile STATUS vectors and Batch storage.
Seven interleaved 500-frame 720p samples moved from 1111 to 1069 ms with an
identical stream (`ae613590…c93b469783`).

The kernel overwrites the complete STATUS range through `SET_REG_READ`, so the
per-frame 16 KiB clear was redundant. Removing it moved H.264 from 1098 to
1052 ms and H.265 from 1129 to 1075 ms in seven interleaved 500-frame 720p
samples, with unchanged stream hashes.

The VEPU580 output surface now uses the `system-uncached` heap, so the encoder
no longer issues `DMA_BUF_SYNC` START/END ioctls before reading each completed
packet. Seven interleaved 500-frame 720p A/B samples at 80% OPP moved H.264
from 993.284 to **968.188 ms** (-2.53%) and H.265 from 1043.576 to
**1018.445 ms** (-2.41%). The new outputs remained byte-identical to the
recorded streams: `0a039e02…a2bc6a2e` for H.264 and
`ae613590…c93b469783` for H.265. H.264/H.265 low-delay output also stayed
5/5 and 4/4 exact.

## 2026-11-21: native CLI fixed-QP hardware queue

The native file encoders now feed the same VEPU580 deferred queue used by the
FFmpeg compatibility path. The input reader owns one extra DMA surface, two
completed jobs may remain below the session, and packet output is released
only after the corresponding input surface has completed.

H.264 uses a third packet output surface so it can submit the next hardware job
after waiting for the oldest one but before copying that completed packet. The
same experiment regressed H.265 because its remaining limit is not output
assembly, so H.265 stays on two packet buffers.

H.265 fixed-QP file encoding enables depth 2 by default when the session has no
refresh, low-delay split or tiles. H.264 remains synchronous by default because
depth 1 adds no useful overlap while depth 2 legitimately exposes scene-tune
state one picture later than the blocking path. The H.264 high-throughput mode
is explicit and applies only to fixed-QP sessions without GDR, debreath,
hierarchical QP, custom rate control or low-delay slice output:

```bash
RKMPP_ENCODER_PIPELINE=1 RKMPP_ENCODER_PIPELINE_DEPTH=2 \
    rk-mpp-rs h264-enc input.nv12 output.264 1280 720 26 high 300 60
```

Eleven interleaved 300-frame 1280x720 samples, pinned to CPUs 4-7:

| path | wall | FPS | CPU user+sys |
|---|---:|---:|---:|
| H.264 C FFmpeg | 0.362 s | 829 | 0.412 s |
| H.264 Rust sync/default | 0.644 s | 466 | 0.276 s |
| H.264 Rust depth 1 | 0.616 s | 487 | 0.288 s |
| H.264 Rust depth 2, explicit | **0.331 s** | **906** | **0.209 s** |
| H.265 C FFmpeg | 0.454 s | 661 | 0.419 s |
| H.265 Rust sync | 0.683 s | 439 | 0.285 s |
| H.265 Rust depth 1 | 0.677 s | 443 | 0.293 s |
| H.265 Rust depth 2/default | **0.430 s** | **698** | **0.214 s** |

The script `scripts/board_native_encoder_pipeline_bench.sh` also checks streams:
H.264 depth 1 and H.265 depth 1/2 are byte-identical to sync for all 300
frames at 320x240 and 1280x720. H.264 depth 2 decodes all frames but is not
asserted byte-identical to sync because one observed statistics window is
necessarily delayed by the deeper queue.

```bash
SAMPLES=11 scripts/board_native_encoder_pipeline_bench.sh
```

## 2026-11-21: concurrent encoder sessions

`scripts/board_multistream_encode_bench.sh` starts 1/2/4/6/8 independent
FFmpeg processes, pins them round-robin across CPUs 4-7, and compares vendor C
with the Rust drop-in. Three interleaved samples at 300 frames each still show
Rust ahead at the practical 1-2 stream operating points; aggregate throughput
then saturates in the encoder hardware rather than the host CPU.

| streams | H.264 C / Rust aggregate | H.265 C / Rust aggregate |
|---:|---:|---:|
| 1 | 838 / **898 FPS** | 638 / **671 FPS** |
| 2 | 1003 / **1007 FPS** | 925 / **957 FPS** |
| 4 | 1042 / **1047 FPS** | 984 / 979 FPS |
| 6 | 1071 / 1059 FPS | 959 / **1009 FPS** |
| 8 | **1077 FPS** / 1068 FPS | 971 / **1010 FPS** |

The small negative deltas at high stream counts are within sampling noise and
occur after the aggregate has plateaued. The useful conclusion is that adding
more host-side queues no longer raises throughput; further gains must reduce
hardware work or frame count, not add CPU concurrency.

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
BOARD=my-rk3588-host SAMPLES=7 ./scripts/board_bench_h265_object_vs_mpp.sh
BOARD=my-rk3588-host SAMPLES=7 ./scripts/board_bench_h264_object_vs_mpp.sh
```
