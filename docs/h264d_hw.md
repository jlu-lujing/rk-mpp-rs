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

## HEVC validation status

The board's C MPP HEVC decoder is usable with device type `16777220`. The
current Rust path passes 14/14 board regressions, including P/B frames, 1080p,
Main10, default scaling lists plus explicit SPS and PPS list data, long-term
references, multi-slice, dependent slice segments, CRA/open-GOP, missing
references, four IDR epochs and tiles. Output is compared to MPP after
conversion to the same planar format and must be byte-identical.

### Dependent and independent slice segments

`DSLICE_A_HHI_5.bit` from the JCT-VC HEVC conformance set contains 50
pictures in 903 NAL units, with 850 VCL NAL units. A non-first slice first
reads `dependent_slice_segment_flag` when the PPS enables dependent slices,
then `slice_segment_address` at `ceil(log2(ctb_width * ctb_height))` bits.
Dependent segments reuse the first segment's header; independent segments
continue through the full slice header.

The Rust decoder now validates every segment's CTB address, NAL type and PPS
id, and concatenates all VCL NAL units from the picture into one hardware job.
All 50 pictures report `err=0x0` and hardware status `00000107/00000000/00000001`.
The 155,520,000-byte planar output has the same SHA-256 as C MPP
`0f28bd89c091b103b3fe3722fa9e8435d96eee414a6c33c29658b4728ec62bc7`.

### Chroma formats and the board capability boundary

The source has separate 4:2:0, 4:2:2 and 4:4:4 paths:

* `h265d_sps.c` maps 8/10-bit 4:2:0, 4:2:2 and 4:4:4 to the corresponding
  `MPP_FMT_YUV*SP` formats, rejects other depths, and rejects mismatched
  luma/chroma depths.
* `h265d_dpb.c` sets `hor_stride` from the coded width and bit depth.
* `mpp_common.c` gives the allocation factors 4:2:0=`3/2`, 4:2:2=`2`,
  4:4:4=`3`.
* `hal_h265d_vdpu34x.c` programs `reg18.y_hor_virstride` and
  `reg19.uv_hor_virstride` from the frame stride and
  `reg20.y_virstride` from `y_stride * ver_stride`.

Rust now carries `chroma_format_idc` in the decoded `Frame`; allocation,
plane slicing, register stride setup and semi-planar-to-planar conversion all
use that value instead of assuming NV12. Parser tests cover 8/10-bit 4:2:2 and
4:4:4, mismatched depths, 12-bit rejection and separate colour planes.

The NanoPC-T6 `rkvdec` does not decode 4:2:2 or 4:4:4 HEVC, even though the C
MPP source contains those branches. Ten-frame x265 fixtures at 256x256 were
tested in 8-bit and 10-bit forms:

| fixture | C MPP requested buffer | C MPP output | Rust output/status |
|---|---:|---:|---|
| 4:2:2 8-bit | 131,072 B | 0 frames | 0 frames, `reg224=0x123` |
| 4:4:4 8-bit | 196,608 B | 0 frames | 0 frames, `reg224=0x123` |
| 4:2:2 10-bit | 163,840 B | 0 frames | 0 frames, `reg224=0x123` |
| 4:4:4 10-bit | 245,760 B | 0 frames | 0 frames, `reg224=0x123` |

C MPP reports every frame with error flag 1, and `dmesg` records
`mpp_rkvdec2 ... resetting for err 0x123`. Therefore 4:2:2/4:4:4 cannot be
declared end-to-end supported on this board. Running the same Rust path on a
SoC which accepts those formats is still required for a byte comparison.

A generated 12-bit 4:2:0 stream is rejected before submission with
`unsupported: bit depth other than 8 or 10`, matching MPP's source-level
`MPP_ERR_PROTOL` boundary.

### Scaling lists and the separate vdpu34x table

`sps_scaling_list_enabled_flag` is not enough by itself. MPP distributes the
matrix state over several places:

* the 112-byte SPS/PPS packet records that scaling lists are enabled;
* `h265d_syntax.c::fill_scaling_lists` expands inherited/default matrices;
* `hal_h265d_com.c::hal_record_scaling_list` packs them into a separate
  1360-byte `scalingFactor_t` table;
* `hal_h265d_vdpu34x.c` programs `reg180_scanlist_base`, and
  `reg12.scanlist_addr_valid_en` must also be set.

Omitting the third and fourth steps is therefore invisible in a successful
SPS/PPS packet comparison. The pre-fix Rust decoder accepted the packet but
programmed no scan-list address, so the MPP-encoded 1920x1080 scaling-list
fixture produced zero frames: the first job reported `err=0x100` and hardware
status `0x23`.

The Rust path now parses SPS and PPS scaling-list data, applies the MPP
inheritance rules, emits the default HEVC matrices when no explicit data is
present, transposes them into MPP's 1360-byte hardware layout, and enables the
scan-list address for each job. On the 10-frame board fixture both decoders
emit 31,104,000 bytes with SHA-256
`8ace9ea6d9dfbdf520b28651d68166b464c019c2fdb0f1afbbc6c1eae49e73a0`.
All ten Rust jobs report `err=0x0` and status `0x107`.

`RFI_HEVC_SCALING_DUMP=1` writes the generated table to
`/tmp/our_hevc_scaling_table.bin`. It is byte-identical to MPP's captured
`/tmp/mpp_scaling_table.bin`; both are 1360 bytes with SHA-256
`9b2224ca91f8f25e8f5f5931ce4f479faeb018f51cdccf2fa2f80a485847a67a`.
`codec::hevc::hw::default_scaling_list_matches_mpp_hardware_table` locks the
same output in the host test suite.

The explicit path uses x265's HM-format
`scripts/hevc_explicit_scalinglist.txt`. The first 32x32 matrix exposed two
independent bugs that default matrices cannot catch. First, Rust used
`1 << (4 + (size_id << 1))` scan positions for the 32x32 class, which requests
1024 entries from the 64-entry diagonal table; MPP clamps this with
`MPP_MIN(64, ...)`. Second, Rust hard-coded the two 4x4 hardware regions to
16 even when the SPS carried custom matrices. MPP's `SCALE_4X4_COPY` starts
flat but receives explicit SPS coefficients, so the values must flow into both
the `scalingfactor0` matrix and the rotated `scalingfactor1` copy.

On the 256x256, 10-frame explicit-SPS fixture, the Rust and MPP 1360-byte
tables are byte-identical with SHA-256
`91b3404324dd0f3f7c7a8f61651d7c7688a9a0cbae4844e4da1ea75ecc2e9edd`.
Both decoders emit 983,040 bytes with SHA-256
`ecdaee4973148a40faa39e2dafc322507c5cc2d53e33a02fb08537e7a53668ec`;
all ten Rust jobs report `err=0x0` and status `0x107`.

`scripts/make_hevc_pps_scaling.py` inserts
`pps_scaling_list_data_present_flag=1` and an explicit 4x4 matrix into the PPS
without changing any slice payload. C MPP and Rust then select the PPS list
over the SPS list. Their 1360-byte tables are byte-identical with SHA-256
`2b84b39796f92b9c777db8168900428d8bcec847a129b7ac90f044aa9f836e82`.
Both decoders emit 983,040 bytes with SHA-256
`49a240f2097c85dae58aec09a8ece9478a02b17ffb9df2fb63c2623be55fa007`;
all ten Rust jobs again report `err=0x0` and status `0x107`.
`scripts/board_regress_hevc.sh` checks both explicit fixtures automatically.

### Main10 buffer and output layout

MPP does not allocate Main10 decoder output as P010. `h265d_frame_create`
sets the horizontal byte stride to
`(MPP_ALIGN(coded_width, 64) * bit_depth) >> 3`; every sample remains packed
at 10 bits. For a 256-pixel-wide picture the stride is therefore 320 bytes
and the 256-row Y plane occupies 81,920 bytes, rather than the 131,072 bytes
of an expanded 16-bit plane.

`utils.c::rearrange_pix` performs the inverse packing used by
`dump_mpp_frame_to_file`: it expands groups of four packed 10-bit samples into
five 16-bit samples. Chroma remains half-width and interleaved within each
row, so the dump conversion must extract U and V separately before writing a
planar `yuv420p10le` frame.

The board fixture is a 256x256, 30-frame x265 Main10 stream. Rust emits
30/30 frames, every job reports `reg224=0x107` with `err=0`, and the converted
Rust and MPP outputs have the same SHA-256
`0bfb5b36a8758daa59a7d1e7ff4e46d9f6b3eb8da4198b68500251a1404ca818`.
The raw hardware dump can be enabled with `RFI_HEVC_FRAME_DUMP=1`;
`scripts/board_regress_hevc.sh` also checks it automatically.

`scripts/gen_hevc_tiles.sh` uses MPP's encoder because the board's libx265
does not expose tile options. Its 1280x720 two-column stream decodes 30/30
frames and matches MPP. Explicit non-uniform tile geometry is also preserved:
the parser retains `column_width_minus1[]` and `row_height_minus1[]`, and the
packet test verifies widths `[7, 9, 4]` and heights `[5, 7]` after the final
tile is filled from the CTB dimensions. A non-uniform board fixture is still
pending.

RPS and PPS packets have been cross-checked at the field/byte level against the
board MPP source and runtime output. The source paths that matter are
`codec/dec/h265/h265d_pps.c`, `codec/dec/h265/h265d_syntax.c` and
`hal/rkdec/h265d/hal_h265d_vdpu34x.c`.

### Short- and long-term RPS

The SPS RPS table must contain both `sps_lt_rps` and all
`sps_st_rps[64]`. Parsing and discarding the SPS short-term sets leaves the
hardware table empty even when the frame-level DPB logic is otherwise correct.
Each short-term slot is fixed at 48 bytes: the first 8 bits are the negative
and positive counts, followed by 15 `(16-bit delta POC, 1-bit used)` pairs,
alignment to 64 bits, and 64 trailing zero bits
(`hal_h265d_slice_hw_rps`).

MPP also has two easy-to-miss prediction rules. SPS sets predict from the
immediately preceding set without a `delta_idx_minus1`; predicted inline slice
sets do carry it. The `used_by_curr_pic_flag` of the predicted set must not
inherit the source set's used flag. Both rules are covered by parser tests, and
the selected SPS short-term set is now cloned into the slice header for DPB
error checking and software reference-list construction.

`scripts/board_regress_hevc.sh` generates a 320x320, 30-frame H.265 stream with
MPP's `ref_cfg_gop3_tsvc4.json`, marks frame 4 as LTR and uses it at frame 6.
The parser observes `lt_present=true`, SPS POC LSB 0 and used long-term slice
entries from POC 4 onward. Rust and MPP both emit 4,608,000 bytes with
SHA-256 `cc9994ca371f999bf0fe175deb1dbf58529a3df312feecb42a9e4b88f4864ab5`;
all 30 jobs report `err=0` and status `0x107`.

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

### Resolved: MPP does not consume the WPP substream loop-filter flag

The divergence was the entropy-coding-sync substream flag. HEVC syntax places
`pps_loop_filter_across_substreams_enabled_flag` after the tile fields when WPP
is enabled, but `h265d_nal_pps` in MPP does not read it. Reading the standard
bit shifts the remaining PPS fields by one. For the 720p regression stream,
Rust then emitted packet bytes 24/25 as `02 b0`; MPP emits `03 40`.

The IDR happened to decode despite the corrupt tail. The first P picture was
rejected with `reg224=0x23` (`dec_irq | dec_irq_raw | buf_empty_sta`). Mirroring
MPP's post-parse state fixes the packet: all 10 pictures decode with POC
`0..9`, every job reports `reg224=0x107`, and the 13,824,000-byte output is
byte-identical to MPP. The byte-level regression test temporarily reproduced
the old `02 b0` packet before passing with `03 40`.

### Resolved: IDR starts a new reference epoch, not only a new POC

The original short streams hid a second state-lifetime bug. After a later IDR
reset POC to zero, the DPB still contained pictures from the previous epoch.
RPS lookup matches by POC, so those stale entries made later references
ambiguous. On a four-IDR, 120-frame stream, the first GOP emitted 30 pictures,
the second emitted only the IDR and POC 3, and the rest were marked
`FRAME_ERR_DEC_INVALID` (`0x10`); final output was 36 pictures instead of 120.

IDR/BLA POC reset now clears the DPB before the new picture is inserted. The
same stream emits four complete `0..29` POC epochs, `120/120` pictures, and is
byte-identical to MPP. The board regression now generates and checks this
multi-IDR case.

### Resolved: CRA retains pre-CRA pictures for following RASL

An open-GOP CRA can carry a short-term RPS whose entries are all unused by the
CRA itself. This does not make the pictures decoded immediately before it
invalid: the following RASL pictures can still reference them. C MPP clears
the DPB reference marks at `h265d_build_rps()` but leaves the stored pictures
available for the first RASL RPS
(`mpp/codec/dec/h265/h265d_dpb.c:277`).

The Rust decoder previously released every entry not named by the CRA RPS.
That removed POC 4 and 6 before RASL POC 8 arrived, so POC 7..9 and 17..19 were
marked `FRAME_ERR_DEC_INVALID` and only 24/30 pictures were written. A CRA now
clears reference marks without releasing the DPB. The next RASL picture
selects the pre-CRA entries it needs, after which normal RPS garbage collection
resumes. The regression emits all POC `0..29`, every job reports `err=0x0`
with status `00000107/00000000/00000001`, and the output is byte-identical to
MPP.

## Historical HEVC failure: non-IDR frames never decode

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
