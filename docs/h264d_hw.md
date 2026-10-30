# rkvdec H.264 (vdpu34x) hardware contract — ported spec

Extracted from the official MPP sources so the Rust HAL can be written without
re-reading 1600 lines of C. Every constant below is used verbatim.

Sources: `hal/rkdec/h264d/hal_h264d_vdpu34x.c`,
`hal/rkdec/inc/vdpu34x_com.h`, `hal/rkdec/common/vdpu34x_com.c`.

## Register file

276 words. Six sections written at these word offsets:

| section | base word | byte offset | content |
|---|---:|---:|---|
| (reserved) | 0 | 0 | not written |
| comm_gen | 8 | 32 | dec_e, strides, error ctrl, gating |
| comm_paras | 64 | 256 | POC list, ref info bits |
| comm_addr | 128 | 512 | all base addresses |
| h2645_highpoc | 200 | 800 | high-bit POC for refs 0..31 |
| irq_status | 224 | 896 | written back only |
| statistic | 256 | 1024 | perf counters |

`codec::h264::regs` (old rkvdec) is **not** for RK3588; use
`hal::vdpu34x_regs` (generated from `vdpu34x_com.h`).

## Buffer sizes (bytes)

```
CABAC_TAB       = 928*4 + 128 = 3840        (file cabac table, copied once)
SPSPPS_UNIT     = 48                        (one 32-byte block + pad)
SPSPPS          = 256*48 + 128 = 12416      (unit replicated 256x)
RPS             = 128 + 128 + 128 = 384
SCALING_LIST    = 6*16 + 2*64 + 128 = 352
ERROR_INFO      = 256*144*4 = 147456
STREAM_INFO_SET = align4k(SPSPPS) + align4k(RPS) + align4k(SCALING_LIST)
```

Each spspps unit is `build_sps_pps()` (32 B) padded to 48 B; MPP copies the
block 256 times and writes `reg161_pps_head_base = fd + spspps_offset`.

## RCB (external command buffer) sizing

```
MPP_RCB_BYTES(bits) = align(ceil(bits/8), 256)      // RCB_ALLINE_SIZE = 256
width  = align(frame_width_mbs*16, 64)
height = align(frame_height_mbs*16, 64)

RCB_DBLK_ROW   bits = width * (2 + 6*bit_depth)
RCB_INTRA_ROW  bits = width * 44
RCB_TRANSD_ROW bits = (width > 8192) ? ((width-8192+3)/4)*2 : 0
RCB_STRMD_ROW  bits = (width > 4096) ? ((width+15)/16)*154 : 0
RCB_INTER_ROW  bits = width * 42
RCB_SAO_ROW    = 0
RCB_FBC_ROW    = 0                          (fbc disabled)
RCB_TRANSD_COL bits = (height > 8192) ? ((height-8192+3)/4)*2 : 0
RCB_INTER_COL  = 0
RCB_FILT_COL   = 0
```

Layout = concatenation in this order, each aligned to 256:
139 DBLK, 133 INTRA, 134 TRANSD_ROW, 136 STRMD_ROW, 137 INTER_ROW,
140 SAO, 141 FBC, 135 TRANSD_COL, 138 INTER_COL, 142 FILT_COL.
All ten registers get the same fd; each gets its region offset via
`SET_REG_ADDR_OFFSET` (reg_idx, byte offset).

## COLVM (motion vector) buffer

`frame_mbs_only` ⇒ compressed layout:
```
segment_w = 64*4*4/16 = 64 ; segment_h = 16
seg_cnt_w = align(width, 64)/64 ; seg_cnt_h = align(height, 16)/16
head      = align(seg_cnt_w, 16) * seg_cnt_h
payload   = seg_cnt_w * seg_cnt_h * 64 * 16
size      = head + payload
```
Field (non frame_mbs_only) doubles the size and disables compression.

## Common registers (init_common_regs)

```
reg9.dec_mode = 1                 (H.264)
reg15.rlc_mode = 0
reg11: buf_empty_en=1, dec_timeout_e=1, dec_clkgate_e=1, dec_e_strmd_clkgate_dis=0
reg10.dec_e = 1
reg17.slice_num = 0x3fff
reg12.wait_reset_en = 1
reg13: h26x_error_mode=1, colmv_error_mode=1, h26x_streamd_error_mode=1, timeout_mode=1
reg21: error_deb_en=1, inter_error_prc_mode=0, error_intra_mode=1
RK3588: reg24.cabac_err_en_lowbits=0, reg25.highbits=0, reg26.swreg_block_gating_e=0xfffef
reg26.reg_cfg_gating_en = 1
reg32.timeout_threshold = 0x3ffff
```

## Per-frame registers (set_registers)

```
reg16_str_len        = packet byte length
reg13.cur_pic_is_idr = idr_flag
reg12.colmv_compress_en = frame_mbs_only
reg28.sw_poc_arb_flag = 0
reg18.y_hor_virstride  = hor_stride/16
reg19.uv_hor_virstride = hor_stride/16
reg20.y_virstride      = (hor_stride*ver_stride)/16
reg65_cur_top_poc = POC(top)          reg66_cur_bot_poc = POC(bottom)
reg130_decout_base   = current frame fd
reg131_colmv_cur_base = current colmv fd
reg132_error_ref_base = nearest ref fd (or current)
reg67_98_ref_poc[2i]   = ref[i] top POC
reg67_98_ref_poc[2i+1] = ref[i] bottom POC
ref bits (reg99 refs0-3, reg100 refs4-7, reg101 refs8-11, reg102 refs12-15):
  h264_refN_field / _topfield_used / _botfield_used / _colmv_use_flag
reg164_179_ref_base[i]       = ref frame fd
reg181_196_ref_colmv_base[i] = ref colmv fd
reg128_rlc_base      = stream fd
reg129_rlcwrite_base = same as reg128
reg197_cabactbl_base = cabac fd
reg161_pps_head_base = spspps fd (+ offset)
reg163_rps_base      = rps fd
reg180_scanlist_base = scanlist fd, reg12.scanlist_addr_valid_en = 1
reg100.. high-bit POC (h2645_highpoc) for field pictures / error refs
```

## Submission

1. write tables (spspps ×256, rps, sclst) into their dma-bufs
2. write registers at byte offset 32 (word 8) for the whole file
3. `SET_REG_ADDR_OFFSET` for RCB regions and table offsets
4. `MPP_CMD_SET_REG_WRITE` → `POLL_HW_FINISH`
5. error check: `reg1.dec_error_sta || !dec_rdy_sta || dec_empty_sta ||
   reg45.strmd_error_status || reg45.colmv_error_ref_picidx ||
   reg76.strmd_detect_error_flag`

## Not implemented yet (explicitly rejected)

- FBC output (reg12.fbc_e) and tile formats
- field pictures / MBAFF (`frame_mbs_only == 0`)
- multi-core CCU distribution
- 10-bit and 4:2:2 output

## Debugging the err 0x23 (in progress)

The reference decoder can be instrumented to dump what it actually programs:

```
# on the board, in /tmp/mpp-develop
# hal_h264d_vdpu34x.c: add an MPP_REG_DUMP block at the top of
# vdpu34x_h264d_start() printing the named comm_gen / comm_paras /
# comm_addr fields, and after prepare_spspps/prepare_framerps dump the
# 48-byte spspps unit and the 128-byte rps block.
make -j8
sudo env MPP_REG_DUMP=1 LD_LIBRARY_PATH=build/mpp:build/osal:build/utils \
     build/test/mpi_dec_test -i stream.264 -t 7 -n 3
```

Our side dumps the same things with `RFI_REG_DUMP=1` (register words) and
`rk-mpp-rs spspps <file>` (SPS/PPS block).

Findings so far:

* bit order: MPP's `mpp_put_bits` is **LSB-first inside little-endian u64
  words**. An MSB-first writer produces bit-reversed tables; the fixed writer
  reproduces MPP's SPS/PPS byte for byte except
  `second_chroma_qp_index_offset` (MPP 15, ours 0 for the 720p test stream).
* register semantics confirmed equal: y_hor 80, y_vir 57600, slice_num
  0x3fff, colmv_compress 1, scanlist_addr_valid 1, table bases are fds
  (MPP shares one buffer for cabac/spspps/rps/sclst, we use one each — the
  offsets are 0 either way).
* difference still to check: `reg66` (current bottom-field POC) — MPP writes
  the same value as the top-field POC, we write 0.
* the RPS block for an IDR is all zeros on both sides, so the remaining fault
  is likely in the slice payload framing or the ref list content of P frames.

## Reference list reordering experiments (do not repeat)

`ref_pic_list_modification` is parsed correctly (verified: x264 emits
`Sub(1), Sub(15), Add(0), Sub(1), End` on one P slice), but applying it made
things worse in every configuration tried:

| initial L0 order | reordering | t_a min | 720p min | 1080p min |
|---|---|---:|---:|---:|
| frame_num desc | no  | **55.4 dB** | **34.5 dB** | 37.0 dB |
| frame_num desc | yes | 41.8 | 33.7 | 37.0 |
| frame_num asc  | yes | 38.5 | 32.6 | 36.2 |

The 1080p stream prefers ascending order while the 640x480 P stream prefers
descending, which means the *initial* list is still not what MPP produces.
Next step is to dump MPP's `RefPicList[0][i]` for a specific frame and match
it exactly, rather than deriving it from the spec alone.

## HEVC cross-check status

`hal_h265d_slice_hw_rps` in the board's MPP build was instrumented to dump
`sw_rps_buf`, but `mpi_dec_test -t 16777227` (HEVC) fails with
`mpp_init failed`, so the reference decoder is not usable for HEVC on this
build (the H.264 path works). The RPS table layout in `codec::hevc::hw` is
therefore ported from the source and covered by unit tests, but not yet
verified byte-for-byte against MPP. Re-check once the HEVC HAL lands: the
table is fixed-size (3200 bytes) and starts with 32 long-term entries.

## HEVC bisection (which encoder features break the hardware)

320x240 single-IDR streams, PSNR against the source picture we generated:

| configuration | Y PSNR |
|---|---:|
| ctu16, all tools off | 48.7 dB |
| + SAO / + deblock / + WPP / + strong-intra-smoothing (one at a time) | 48.2-48.7 |
| SAO+WPP, SAO+DB, WPP+DB, SAO+WPP+DB, +SIS | 49.1-49.4 |
| CTU 32 / CTU 64 | 50.5 |
| 640x480, all tools off | 54.2 |
| **x265 defaults** | **7.6** |
| defaults with ctu16 / deblock=0 / sao=0 | 10.2 / 7.6 / 5.9 |

So resolution, CTU size, SAO, WPP, deblocking and strong intra smoothing all
decode correctly on their own and in pairs. Comparing the 112-byte packets of a
good and a bad stream, only bit 192 (lf_across_slices), bit 204 (lists_mod) and
bit 205 (merge level) differ; forcing lists_modification to 0 in the packet does
not fix the bad stream, so the PPS summary is not the whole story — the next
suspect is a slice-header syntax element that only the default encoder emits.

### PPS alignment, concrete evidence

The good and bad PPS NALs differ by exactly one byte:

```
good (decodes, 49 dB): c1 73 c1 c4 89
bad  (7.6 dB)        : c1 73 c1 89
```

The bad payload is 32 bits (31 usable after the stop bit) and underflows under
every interpretation we tried, while the good payload has 8 extra bits. The
only 8-bit group that can appear or disappear between these two streams is the
deblocking pair `pps_beta_offset_div2` + `pps_tc_offset_div2`, which is present
only when `deblocking_filter_control_present_flag == 1` and the filter is not
disabled. That means our PPS parser is misaligned by the time it reaches the
deblocking flags: it reports `db_control = 0` for the good stream even though
the bit budget only closes if that group is present.

Next: walk the PPS syntax against both payloads bit by bit, find the field
where our reader diverges (candidates: the sign-data-hiding/cabac pair order,
`diff_cu_qp_delta_depth`, or the entropy-coding-sync substream flag), fix the
alignment, and re-check that the good stream still lands on the stop bit.

## HEVC: non-IDR frames never decode (measurement correction)

Comparing MD5s of the decoded luma planes showed frames 0 and 3 are **byte
identical**, while frames 1-3 differ from each other but all have mean 125.
That is stale heap memory, not decoded pictures: every non-IDR job raises
`err 0x23` (18 events for 9 such frames, two cores per event) and writes
nothing, so the output buffer keeps whatever the dma-heap page contained from
an earlier run.

Consequence: the "38 dB / 40 dB / 26 dB" figures reported for multi-frame HEVC
were comparisons against recycled memory, not decode quality. Only the IDR
(56-57 dB) is a real measurement.

Register experiments that produced byte-identical output (rps_mode, the slice
RPS table, error_intra_mode, DPB trimming, colmv sizing) were therefore all
inconclusive: the frames they would have affected never decoded.

Next: find why a P/B job is rejected. The reference list and POCs printed by
RFI_HEVC_LISTS look correct (poc order 0,4,2,1,3,9,7,5 and matching s0/s1), so
the rejection is more likely a register or table precondition than the lists.
