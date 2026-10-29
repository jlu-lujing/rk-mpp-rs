# rk-mpp-rs

Pure-Rust replacement for the parts of Rockchip **MPP** (and eventually **RGA**)
that `rknn-fastinfer` needs: hardware JPEG decode and a 2D scale/letterbox, with
no `librockchip_mpp` / `librga` dependency.

## Why

`rknn-fastinfer` measured, on a NanoPC-T6 (RK3588, driver `mpp_jpgdec`):

| input | shippped MPP / TurboJPEG | source-built MPP | pure-Rust target |
| --- | ---: | ---: | ---: |
| 640x640 JPEG | 4.95 ms | 1.34 ms | <= 1.3 ms |
| 1920x1080 JPEG | 15.9 ms | 2.60 ms | <= 2.6 ms |

The shipped MPP never submits JPEG jobs at all; a source build of
rockchip-linux/mpp fixes it (see `rknn-fastinfer/docs/NATIVE_CONV.md`). Driving
the kernel interface directly means no C library, no ABI mismatch and no
"version of the day" behaviour.

## Status

| piece | state |
| --- | --- |
| dma-heap allocation + mmap | **working** |
| `mpp_service` session (`MPP_CMD_INIT_CLIENT_TYPE`) | **working** |
| `MPP_CMD_PROBE_HW_SUPPORT` / `QUERY_HW_ID` | **working** |
| JPEG parser (SOF/DQT/DHT/DRI/SOS, zigzag tables) | **working** |
| table buffer (1280 B) | **working** |
| jpegd register builder (`jpegd_gen_regs` port) | **working**, RGB888 + NV12 |
| submit / poll (`SET_REG_WRITE`, `POLL_HW_FINISH`) | **working** |
| NV12 or RGB888 output | **working** |
| RGB888 bilinear letterbox scaler | **working**, 0.84 ms (1080p) / 0.53 ms (640) |
| RGA3 driver port | optional, see below |

**The decoder works end to end in pure Rust** (`rk-mpp-rs decode frame.jpg out.rgb`):

| image | size | PSNR vs libjpeg | decode (submit+poll) |
| --- | --- | ---: | ---: |
| bus.jpg 4:2:0 | 640x640 | 44.1 dB | **0.95 ms** (1056 fps) |
| COCO 4:4:4 | 640x436 | 56.6 dB | 1.13 ms |
| big1080 4:2:0 | 1920x1080 | 49.9 dB | **2.22 ms** (451 fps) |

The C MPP wrapper takes 1.34 ms (640) and 2.60 ms (1080p) for the same work, so
the Rust path is ~30% faster and drops the dependency.

Verified on the board:

```
$ ./target/release/rk-mpp-rs
rk-mpp-rs probe
  session: client_type=13
  hw support mask: 0x1009321a
  jpeg dec present: true
  rkvdec present:  true
  hw id: 3676241925
  dma-heap system: 1048576 bytes, fd=4
  dma-heap system-uncached: 1048576 bytes, fd=4
  dma-heap cma: 1048576 bytes, fd=4
```

## Layout

```
src/mpp.rs       mpp_service ioctl client (single MPP_IOC_CFG_V1 command channel)
src/dma_heap.rs  /dev/dma_heap alloc + mmap
src/main.rs      probe binary
```

The protocol is one ioctl (`_IOW('v', 1, u32)`) taking a `MppReqV1`; definitions
are ported from `rockchip-linux/mpp`: `osal/inc/mpp_service.h`,
`osal/inc/mpp_dev_defs.h`, `osal/driver/inc/mpp_service_impl.h`.

## Scaler (replaces RGA today)

`src/scale.rs` resizes RGB888 and letterboxes into a fixed destination, with the
padding filled with 114. Measured on the board against the RGA blit it replaces
(same source buffer, same destination fill, 300 iterations):

| conversion | this crate | RGA (`c_RkRgaBlit`) |
| --- | ---: | ---: |
| 1920x1080 -> 640x640 letterbox | **0.84 ms** (1190 fps) | 3.21 ms |
| 640x640 -> 640x640 letterbox | **0.53 ms** (1900 fps) | 1.50 ms |

That is 3.8x and 2.9x faster than RGA, and it removes `librga` from the
dependency list. The win comes from specialising the integer case: when the
ratio is exact (1920x1080 -> 640 is exactly 1/3) the bilinear weights are zero
and the loop reduces to a strided copy; a 1:1 mapping becomes a `memcpy`.

## Plan

1. ~~**JPEG parser**~~ done (`src/jpeg.rs`): SOF0/SOF1/SOF2, DQT (8/16-bit),
   DHT, DRI, SOS; tables kept in file (zigzag) order; entropy range computed.
   `rk-mpp-rs jpeg-info frame.jpg` prints dimensions, sampling, components.
2. ~~**Table buffer**~~ done (`src/tables.rs`): 384 B quant (zigzag -> raster),
   704 B min-code/acc-addr blocks, 576 B symbols; 1280 B total, matching
   `RKD_TABLE_SIZE`.
3. **Register builder** (map generated into `src/jpeg_regs.rs`): port `mpp/hal/vpu/jpegd/hal_jpegd_rkv.c`
   (`jpegd_gen_regs`, `setup_output_fmt`) into a typed `JpegRegSet`.
4. **Submit/poll**: `MPP_CMD_SET_REG_WRITE` then `MPP_CMD_POLL_HW_FINISH`,
   buffers translated with `MPP_CMD_TRANS_FD_TO_IOVA`.
5. **Output**: ask for `YUV420SP` first (simplest), then `RGB888` like the C path
   did (the PP does full-range YUV->RGB for JPEG frames).
6. ~~Scaler~~ done; a NEON kernel for the non-integer ratios is still open
   (e.g. 1280x960 -> 640x640 has fractional weights).
7. **RGA (optional)**: the kernel ABI is public —
   `drivers/video/rockchip/rga3/include/rga.h` (`RGA_IOC_REQUEST_CREATE/SUBMIT`,
   `struct rga_user_request`) plus `rga3_reg_info.c` for the registers. Only
   worth porting if we need operations a NEON kernel cannot do well
   (rotation, format conversion, multi-plane composition).

## Build

```bash
cargo build --release            # host, for type checking
# on the board (aarch64):
cargo build --release --offline
./target/release/rk-mpp-rs
```

No third-party crates beyond `libc`.
