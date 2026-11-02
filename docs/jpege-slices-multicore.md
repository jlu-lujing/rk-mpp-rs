# JPEG slice and multi-core partitioning on VEPU2 (research)

Status: low-delay packet output and the default multi-core partition path,
including 90/270-degree rotation, are implemented and board-verified. Both mechanisms live in
`hal_jpege_vepu2_v2.c` for RK3588's client-19 (`VPU_CLIENT_VEPU2_JPEG`),
the same client `rk-mpp-rs` opens. Line numbers refer to the develop
reference under `refs/mpp`. Everything the *single-job* restart path needs
is already implemented in `src/hal/jpege.rs` (`EncoderConfig::split_plan`,
`RESTART_MARKER_FIRST`, the DRI writer); this document covers the two
multi-job paths and the blockers.

## 1. Low-delay slices (sequential band encoding)

### How C arms it

`mpp_enc_impl.c:241-257` — `low_delay_part_mode` requires all three of
`cfg->base.low_delay`, `cfg->split.split_mode`, and a HAL exposing
`part_start`/`part_wait` (VEPU2 JPEG does). When armed, the encode loop at
`mpp_enc_impl.c:2585-2661` replaces the single start/wait with a
`do { part_start; part_wait } while (!part_last)` loop and emits one
output packet per band (soi/eoi flags). The JPEG syntax fields come from
the `MPP_ENC_SPLIT_BY_CTU` branch of `jpege_api_v2.c:257-287`:

```
part_rows  = ceil(split_arg / mcu_hor)          (0 if >= mcu_ver or arg > mcu_cnt)
restart_ri = mcu_hor * part_rows                (RK3588 branch)
low_delay  = base.low_delay && part_rows
```

The pinned 2024-04 `mpi_enc_test` does not expose these keys: its env names
cover `split_mode` / `split_arg` / `split_out`, but `base:low_delay` is only a
`MppEncCfg` key. `tools/jpege_probe.c` therefore drives the reference library
directly and drains low-delay packets with EOI, using a worker thread so MPP's
fixed output-task pool cannot deadlock above eight bands.

### Register derivation per band

`hal_jpege_vepu2_gen_regs()` runs once and leaves the picture-sized image
with `reg107 |= 1 << 24` ("slice encode end by RST") and `reg109 |= 1 << 16`
(slice interrupt) when low_delay (`:546-551`); exposed as `SLICE_END_BY_RST`
/ `SLICE_IRQ_ENABLE` in our HAL. Each band then overwrites (`part_start`,
`:895-992`):

| reg | value |
|---|---|
| 51/52 | `get_msb_lsb_at_pos(out_buf, part_bytepos)` |
| 53 | `size - part_bytepos` |
| 60 | `((part_bytepos & 7) * 8) << 16 \| part_x_fill << 4 \| part_y_fill` — `part_y_fill` only on the last band; `part_x_fill` from gen_regs |
| 77 | output fd, reg-offset = `part_bytepos` |
| 103 | `mcu_hor << 8 \| part_enc_mcu_h << 20 \| 0x61` |
| 107 | `part_not_end << 24 \| jpege_restart_marker[rst_marker_idx & 7]` — note: unlike gen_regs, the `part_rows << 16` field is NOT written here; then `rst_marker_idx++` |
| 48/49/50 offsets | NV12 band rows: `offset_byte[0] += mcu_y * 16 * hor_stride`, `offset_byte[1] += (mcu_y * 16 / 2) * hor_stride` |

Software bookkeeping after each poll (`part_wait`, `:994-1031`):
`part_bytepos = (part_bytepos & ~7) + hw_bits / 8` with `hw_bits = reg53`
readback; the next band continues the stream at that byte.

`part_start` never applies the 90/270 offset swap (only `multi_core_start`
does, `:661-679`), so C's part path is effectively rotation-free.

### Rust implementation

The full stream a part loop produces is byte-equivalent to the ported single
job with the same split, because `restart_ri` is *defined* as one band of
MCUs: the hardware inserts the RST marker at every band boundary either way,
and the part marker index (`rst_marker_idx++` per band) coincides with the DRI
restart cycle modulo 8.

Rust therefore keeps one hardware job and exposes `EncodedJpeg::packets` as
byte ranges over the completed stream. Packet 0 contains the JPEG header and
first band; each following range ends immediately after the next RST marker.
This produces the same packet bytes and boundaries without resubmitting the
picture once per band.

`scripts/board_jpege_lowdelay_vs_mpp.sh` verifies 320x240 / five packets and
640x480 / ten packets: both complete streams and every packet length are
identical to C. The old restart matrix remains 12/12 byte-identical. Rotated
low-delay output is rejected because C's part path does not apply the rotation
offset rules, and pictures above the 1280x720 area gate are rejected because C
enters the separate multi-core path below.

## 2. Multi-core partitioning (RK3588 default above 720p)

`hal_jpege_vepu2_get_task()` (`:179-322`) auto-splits for
`VPU_CLIENT_VEPU2_JPEG` whenever `width * height > 1280 * 720` and the
frame is not small in one dimension, unless `task_cnt > 1` (frame-parallel
mode, which disables splitting). Bands:

- default mode: `part_rows = ceil(mb_h / 4) rounded to a multiple of 4
  rows per core` (`:249-251`), then filled core by core with the whole
  picture's remainder going to the last core that still has rows; core 0
  additionally gets `restart_ri = mb_w * part_rows` if none is set
  (`:296-313`);
- `MPP_ENC_SPLIT_BY_CTU` mode: `part_rows = syntax->part_rows` from the
  split argument, ecs counts distributed by the balance table at
  `:255-295` (8-row first, then 4, then halves; remainder to core 3).

`multi_core_start()` (`:577-733`) submits up to four register sets to the
same client: band 0 writes to the real output buffer at `part_bytepos`
with the header prefix; bands i > 0 each write to a scratch buffer of
`width * height / 2` bytes from offset 0. Between register sets C issues
`MPP_DEV_DELIMIT`, then one `MPP_DEV_CMD_SEND`; `multi_core_wait()`
(`:735-796`) polls once per band, reads each band's `reg53` for its byte
length, and concatenates the scratch contents behind band 0's stream.
`rst_marker_idx += ecs_cnt[i]` keeps the DRI cycle consistent across
bands, and for 90/270 rotation the bands run along x with the 270
direction reversed (`:661-679`).

### Rust implementation

An ioctl trace showed that C's vcodec path submits each `MULTI_MSG` request in
a separate ioctl, uses `MPP_CMD_SET_SESSION_FD` as the delimiter payload, and
marks only the final register-offset request with `LAST_MSG`. `MppService` now
has a streaming batch submit that reproduces that sequence.

`Vepu2Jpeg` follows `multi_core_start` for up to four bands:

- band 0 writes into the real output buffer with the JPEG header prefix;
- later bands use reusable scratch dma-bufs of `width * height / 2`;
- each band rewrites reg51/52/53/60/77/103/107 and submits the exact NV12
  input offsets, including C's zero-valued V-plane entries;
- hardware lengths are read from each reg53 and concatenated in order.

The default plan is C's four-way split (`align(mcu_rows, 4) / 4` rows per
band). Explicit BY_CTU uses the same ECS balance table. Input allocation keeps
the C 64-row DMA tail while logical NV12 data uses `ver_stride = align(height,
2)`, which matters for 1080p.

`scripts/board_jpege_multicore_vs_mpp.sh` is 5/5: 1920x1080 at qfactor 50 and
90, 1288x722 with a 12+12+12+10 band split, and 1920x1080 rotated 90/270.
All complete JPEGs are byte-identical. The 1280x720 boundary is also byte-identical, and
`scripts/board_jpege_stride_vs_mpp.sh` covers 322x242 and 1366x768.
Seven alternating whole-process 1080p/qf50 samples had medians of 64.7 ms for
C and 60.7 ms for Rust; this is a regression guard, not a stable microbenchmark.

Rotated band plans use the post-rotation MCU grid while reg103's high field
keeps C's `mcu_hor` value; input offsets run along x and 270-degree bands use
the reverse order. An explicit split whose balance table makes the final band
read beyond the coded picture can make the
C harness emit a JPEG without EOI while returning success; Rust emits a valid
EOI-terminated stream. That case is recorded as a C bug rather than imitated.
