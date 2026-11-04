# Known divergences from the C reference

Scope rule (2026-11-02): pixel/byte-exact imitation plus output-preserving
performance. A difference is a defect **unless the C behaviour has been proven
wrong**. This file is the register of every difference that is currently known,
with its disposition and the evidence behind it. New work should close an entry
here before adding capability.

Dispositions:

- **FIX** - ours is wrong or unproven; the C result is the target.
- **C-BUG** - the C result is proven wrong by an independent decoder/spec check;
  ours is kept and the evidence is recorded.
- **NO-ORACLE** - the C vintage cannot produce the comparison at all; not claimed.
- **BY-DESIGN** - a deliberate, documented difference in *behaviour* (exit code,
  recovery) that does not change decoded pixels for valid streams.

## Decoders

| Case | Disposition | Evidence |
|---|---|---|
| VP9 `vp9_p0_720p_errres_seg`, frames 4/5 | **C-BUG** | Rust == libvpx (SHA-256 `0f0ff0da…deb51`) on all 6 frames; C differs by 106+107 V-plane bytes, max delta 3 (`5b737017…cac1`). Registers match word for word. |
| VP9 `vp9_p0_lossless_321x241_2`, frame count | **C-BUG** | libvpx and Rust produce 2 frames; C emits 1. Rust hash is the libvpx hash `8ac48f11…beca`. |
| VP9 damaged streams (`half-last`, `mid-flip`) | **BY-DESIGN** | C aborts the run (rc 255, 0 frames); Rust emits the pictures that did decode. Matching C would discard valid output. |
| AV1 2x2 tiles, first hardware job on t62 | **FIXED** | Rust omitted C's CDEF and loop-restoration filter bases in `reg[85]`/`reg[91]`; STATUS reported `sw_dec_bus_int` (`0x2109`), and the kernel reset `mpp_av1dec`. Programming both bases plus C's DMA32/uncached heap and display-slot sizing makes the one-frame prefix succeed with STATUS `0x1100`. The full fixture now emits 10 frames/1,152,000 bytes byte-identical to current C planar output (`9d339ae3…8138`), and the 8/10-bit film-grain hardware regression passes. |
| AV1 8-bit film grain (`av1_film_grain_320x240_10`) | **C-BUG** | Rust applies grain and matches libaom byte-for-byte (`bdd74ace…f632afb`). C's `vdpu_av1d_set_fgs()` never sets `sw_apply_grain`, so C emits the skip-grain image. |
| AV1 10-bit film grain (`film_grain_10bit`) | **C-BUG** | Rust matches libaom's grain-applied `yuv420p10le` output (`b75e6172…fe904d`); C emits libaom's skip-grain image (`f3b69b61…49b8`). |
| VP8 key-frame scale bits | **FIXED** | The two bits are display-only: libvpx (FFmpeg) and C both decode at the coded size. The parser no longer refuses them; `board_vp8_profile_vs_mpp.sh` derives a scaled key frame and compares 12 frames, byte-identical to C. |
| AV1 damaged container / half-last OBU | **BY-DESIGN** | C emits eight pictures and then blocks until the 60-second timeout. Rust rejects the truncated container before decode or names the oversized OBU after nine pictures, keeping both operations bounded. |
| AV1 mid-packet bit flip | **FIXED** | C and Rust both complete ten pictures and the planar outputs are byte-identical (`af41ab70…01bc`). |
| VP9 profile edge matrix | **NO-ORACLE** | The remaining VP9 profile edges have no differential fixture in the corpus. |
| VP9 profile 1/3 hardware | **NO-ORACLE** | A vpxenc 1.9 profile-1 I422 stream is accepted by ffmpeg, but t6's C MPP reports `Invalid frame marker`, emits 0 frames and loops until timeout. The installed vpxenc build cannot emit profile 3. Rust still rejects profile 1/3 explicitly instead of inventing an unverified layout. |
| H.263 field-coded PAFF | **NO-ORACLE** | No stream in the corpus. |
| AVS+ / AVS2 | **NO-ORACLE** | No conformance stream and no hardware on t6. |

## Encoders

| Case | Disposition | Evidence |
|---|---|---|
| H.264 slice split, output packets (`MPP_ENC_SPLIT_OUT_LOWDELAY`) | **FIXED** | `split_out=1`, `slen_fifo` and one-entry `POLL_HW_IRQ` drain now emit per-slice packets. `board_h264_lowdelay_vs_mpp.sh` passes BY_CTU 96/150/8, BY_BYTE 256 and six IDR+P pictures (5/5): concatenated streams and every slice boundary match. The 38-slice cell crosses the 32-entry FIFO. C may represent the final header/EOI as a trailing zero-length entry; both layouts end at the same offset. |
| JPEG low-delay slices | **FIXED** | `jpeg-enc ... rst<mcus> splitout:lowdelay` exposes byte ranges ending after each RST marker. `board_jpege_lowdelay_vs_mpp.sh` is 2/2: 320x240/5 packets and 640x480/10 packets match C in complete bytes and every packet length. Rotation and >720p are explicitly rejected because C selects different paths there. |
| JPEG default multi-core partitioning | **FIXED** | Rust reproduces C's streaming `MULTI_MSG` sequence, `SET_SESSION_FD` delimiters, per-band scratch buffers and reg53 concatenation. `board_jpege_multicore_vs_mpp.sh` is 3/3 (1920x1080 qf50/qf90 and uneven 1288x722); `board_jpege_stride_vs_mpp.sh` is 2/2 (322x242 and 1366x768). |
| JPEG explicit split with rows beyond the picture | **C-BUG** | At 1920x1080 `split_arg=1200`, C returns success but emits a 173,668-byte JPEG with no EOI; the marker audit and FFmpeg reject it. Rust uses the same registers/fixture and emits a valid EOI-terminated JPEG. Default/equal-band C paths remain byte-identical. |
| JPEG rotated multi-core | **FIXED** | Rust now uses the post-rotation MCU grid, C's reg103 `mcu_hor` field, x-axis input offsets and 270-degree reverse band order. `board_jpege_multicore_vs_mpp.sh` adds 1920x1080 rot90 and rot270; both are byte-identical. |
| H.264 P pictures above 4096 width | **FIXED** | `FRAMES=2 board_h264_wide_vs_mpp.sh` encodes IDR+P at 4160x64 and 4608x256: both byte-identical to the C probe, CIME extension lines included. |
| H.265 GDR / intra refresh | **FIXED** | `h265e_probe` now supplies an explicit short-term `MPP_ENC_SET_REF_CFG`, activating C's refresh scheduler. Rust matches IGOP scheduling, dual-DPB alternation, refresh tables, search-axis override and recovery-point prefixes. `board_h265_gdr_vs_mpp.sh` passes 320x240/640x480 × row/column, 4/4 byte-identical. |
| H.265 slice split and low-delay packets | **FIXED** | `vepu580_setup_split` is matched for BY_BYTE and BY_CTU, including `slen_fifo`, `slc_done_en` and the 32-entry FIFO rule. Ordinary one-AU output is 3/3 (`board_h265_slice_vs_mpp.sh`), including a 40-slice 640x480 picture. `board_h265_lowdelay_vs_mpp.sh` passes 4/4: 3 CTU slices, a 9-packet byte split, the same FIFO crossing and six short-term IDR+P pictures. Complete streams and every packet length are identical; the C probe drains through EOI on a worker thread. |
| H.265 ROI / QP map | **FIXED** | Rust reproduces the reference ROI helper's 16x16 region pass, HEVC CTU map, CU64/CU32/CU16/CU8 records, force-split and both base/QP tables. `board_h265_roi_vs_mpp.sh` is 4/4 over relative QP, absolute QP+force-intra, multiple regions and 640x480; all six-picture GOP streams are byte-identical. |
| H.265 encode: WPP, B frames, multiple references | **NO-ORACLE** | Not implemented by the C reference in either vintage. WPP: `m_entropyCodingSyncEnabledFlag` is assigned 0 in both `h265e_ps.c` init paths of both vintages (2024-04 421/450, master 422/451) and that zero is copied into the HAL syntax word, master writes `num_entry_point_offsets = 0` unconditionally and 2024-04 has no entry-point writer at all. B frames: no `B_FRAME`/`MPP_FRAME_TYPE_B` in `codec/enc*` or `hal/rkenc` in either tree, although the slice-header writer has dormant `B_SLICE` code. Multiple references: `h265e_api.c` sets `h265->num_ref = 1` and `m_numRefIdxL0DefaultActive = 1`. Implementing them would not be a rewrite of MPP. |
| H.265 tile + `MPP_ENC_SPLIT_OUT_LOWDELAY` | **BY-DESIGN** | Refused with an explicit error. The slice FIFO reports lengths per job and its behaviour across tile jobs has no C evidence; inventing a packet layout would be a false claim. WPP, B frames and multi-reference remain open. |
| H.265 first tiled picture of a session | **NO-ORACLE** | The reference has no session-independent result. The same C probe, same fixture and settings, emits `9724d029..` right after one of our sessions and `b27d68dc..` right after its own, and the two decode to **different pixels** (`7dc42fe5..` against `c9fcd4f7..`): the VEPU580 keeps dual-core state between processes and the first tiled job reads it. Returning 512 MiB of zeroed pages between runs changes neither value, so the state is inside the block, and `MPP_CMD_RESET_SESSION` only drains a queue. Both sides therefore encode one discarded picture: ours pins its output to `9724d029..` whatever ran before, and `board_h265_tile_vs_mpp.sh` runs one of our encodes immediately before the C side to put it in the same state, so the comparison is reproducible instead of depending on which script ran last. Inside that state the streams are byte-identical and the decoded YUV is identical; the claim stops there. |

## Post-processing

| Case | Disposition | Evidence |
|---|---|---|
| IEP2 deinterlace modes and formats | **FIXED** | The original 720x480 I5O2 job is byte-identical to `iep2_test`. A parameterized C client-28 probe covers I5O2/I5O1T/I5O1B/I2O2/I1O1T/I1O1B/DISABLE/BYPASS: `board_iep_modes_vs_probe.sh` is 9/9 over both raw dst slots, including TFF/BFF and 1280x720. The 420/422 x SP/planar x UV/VU matrix is 6/6. T modes write dst0, B modes write dst1, and the unused slot remains zero. |
| IEP2 PD/DECT inverse telecine | **FIXED** | `iep2_test` hard-codes I5O2 and never reaches the pulldown machines, so the reference is `tools/iep2_dect_probe.c` (mode DECT, per-frame `iep2_api_info` printed). `board_iep_dect_vs_probe.sh` is 2/2: 3:2 telecine at 720x480 (`pd_types` 0, `pd_flag` NC/NON/CC cycling) and progressive content (`pd_types` 4) both match the probe's YUV byte for byte and every state line. |
| IEP legacy YUV scale/crop | **NO-ORACLE** | The scale API belongs to the legacy `/dev/iep` backend. t6 has no `/dev/iep` node and the legacy client/driver does not bind; IEP2 client 28 has no scale/crop fields. No C hardware output exists here to compare against. |
| VDPP (client 29) | **NO-ORACLE** | The engine does not exist on RK3588; M9's original engine cannot be imitated on this board. |

## Framework

| Case | Disposition | Evidence |
|---|---|---|
| fences, multi-core CCU, secure mode | NOT IMPLEMENTED | Surveyed in `docs/framework-m10.md`; blocked on this kernel/board or on a TA. |
| rkvenc multi-instance | NOT IMPLEMENTED | The reference itself supports one live session per process here. |
