# Reimplementing the full MPP surface in Rust

The official Rockchip MPP is ~296k lines of C/C++ (decoders 43k, encoders 10k,
HAL 143k, framework 29k, osal/driver 18k). This file tracks every feature
point, what it costs in the original, and where we stand.

Scope is the whole library, not only what YOLO needs: all decoders, all
encoders, post-processing, and the framework features (async tasks, fences,
multi-core, secure mode).

## Board capability map (NanoPC-T6-LTS, mask `0x1009321a`)

| block | present | drives |
|---|---|---|
| vdpu2 | yes | H.264, MPEG2, MPEG4, H.263, VP8 |
| vdpu2-pp | yes | deinterlace, scale, format conversion |
| rkvdec | yes | H.264, H.265, VP9, AVS2/AVS+ |
| av1d | yes | AV1 |
| jpegd | yes | baseline/progressive-free JPEG decode |
| rkv-enc | yes | H.264, H.265 encode |
| vepu-jpeg | yes | JPEG encode |
| vepu1/2/22 | no | — |

## Milestones

| # | deliverable | official LOC | status |
|---|---|---:|---|
| M0 | `mpp_service` client: all `MPP_CMD_*`, MULTI_MSG batches, RCB, IOVA, poll, probe | 1.0k | **done** |
| M0 | JPEG decode (`jpegd`): parser, hw tables, registers, RGB888/NV12, scaler | 2.9k | **done** |
| M0 | JPEG decode beats MPP by ~30 % (no buf_slot/meta layer); 0.96 ms @ 640p | — | **done** |
| M1 | H.264 syntax: Annex-B, RBSP, Exp-Golomb, SPS/PPS/slice header, scaling lists | 3.5k | **done** |
| M1 | H.264 register map generated from `hal_h264d_rkv_reg.h` (80 words, 122 fields) | 0.25k | **done** |
| M2 | H.264 decode on rkvdec: reg writer, DPB, reference lists, MMCO, error concealment | 6.0k | **working** |
| M3 | H.265 decode: syntax (VPS/SPS/PPS, RPS, tiles, WPP) + rkvdec HAL | 12k | todo |
| M4 | VP9 decode (segmentation, loop filter, superframes) | 8k | todo |
| M5 | AV1 decode (frame headers, OBU, film grain off) | 14k | todo |
| M6 | MPEG2 / MPEG4 / H.263 / VP8 / AVS+ / AVS2 decode | 14k | todo |
| M7 | H.264 encode (rkvenc): RC (CBR/VBR), intra/inter, deblock, ROI, GDR | 15k | todo |
| M8 | H.265 encode (rkvenc) + JPEG encode (vepu) | 12k | todo |
| M9 | Post-processor: vdpu-pp deinterlace/scale/format, RGA interop | 8k | todo |
| M10 | Framework: task queue, async completion, dma fences, multi-core CCU, secure mode | 29k | todo |

## Feature checklist (per official capability)

### Decoders
- [x] JPEG baseline, 4:2:0/4:2:2/4:4:4/gray, NV12 + RGB888 output
- [ ] JPEG 1/2, 1/4 hardware downscale
- [x] H.264 syntax (SPS/PPS/slice, CABAC-aware header, B-frames, weighted pred)
- [x] H.264 decode on rkvdec: IDR bit-exact, 1080p/720p streams decode with
      0-8 hardware errors; inter frames 22-57 dB (reference marking/colmv still
      to be made exact)
- [ ] H.264 multi-slice, field/MBAFF, long-term refs, error concealment
- [ ] H.265 Main/Main10, tiles, WPP, RPS, SCC
- [ ] VP9 profile 0/2, 10-bit, superframes
- [ ] AV1 Main, film grain
- [ ] MPEG2, MPEG4 ASP, H.263, VP8, AVS+, AVS2

### Encoders
- [ ] H.264 (rkvenc): IDR/P, CBR/VBR, QP map, ROI, GDR, deblock, slice split
- [ ] H.265 (rkvenc): same plus tiles/WPP, SAO
- [ ] JPEG encoder (vepu): quality table, 4:2:0/4:2:2

### Post-processing
- [x] RGB888 letterbox scaler (own, faster than RGA for our sizes)
- [ ] vdpu-pp: deinterlace, YUV scale, NV12↔RGB, crop
- [ ] 10-bit ↔ 8-bit conversion, HDR tone mapping

### Framework
- [x] synchronous submit + poll
- [ ] async task queue with completion callbacks
- [ ] dma-fence in/out, buffer import/export
- [ ] multi-core (CCU) distribution
- [ ] secure/TA mode
- [ ] frame buffer pool with reference counting (DPB)

## Verification

- Unit tests: `cargo test` (25 today: parser, registers, tables, scaler, NEON).
- Real streams: `rk-mpp-rs h264 <file.264>` prints SPS/PPS/slice fields; validated
  against x264 output (720p main with B-frames, 1080p baseline).
- Bit-exactness target for every decoder: MD5/PSNR against `ffmpeg -c:v <codec>`.
- Performance target: never slower than the C MPP on the same stream.
