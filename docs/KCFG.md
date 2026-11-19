# Kernel configuration compatibility

Audit baseline: Rockchip MPP `9b56348` (`2026-11-03`), including
`test/mpi_enc_test.c`, `kmpp/base/kmpp_venc_cfg.c` and
`kmpp/kmpp_venc_obj.c`.

## `MPP_SET_VENC_INIT_KCFG`

The compatibility layer consumes this command before `mpp_init`. It copies
the typed store into a small Rust config, so the caller may deinit the kcfg
handle after `mpp_init`.

| key | Rust behavior |
| --- | --- |
| `type` | Must be `MPP_CTX_ENC` |
| `coding` | Must be H.264, HEVC or MJPEG encoder coding |
| `max_width`, `max_height` | Must be non-zero; later `MPP_ENC_SET_CFG` dimensions cannot exceed them |
| `chan_id`, `online`, `buf_size`, `max_strm_cnt` | Validated key, kernel-only, storage-only |
| `shared_buf_en`, `smart_en`, `max_lt_cnt`, `qpmap_en` | Validated key, kernel-only, storage-only |
| `chan_dup`, `tmvp_enable`, `only_smartp`, `ntfy_mode` | Validated key, kernel-only, storage-only |
| `input_timeout` | Validated key; synchronous Rust encoder does not need a kernel input timeout |
| any other key | `MPP_SET_VENC_INIT_KCFG` returns `MPP_NOK` |

A type/coding mismatch between init-kcfg and `mpp_init` returns
`MPP_ERR_VALUE`; the pending config is retained so a corrected `mpp_init`
can still use it.

The board gate is:

```bash
BOARD=t6 scripts/board_mpp_kcfg.sh
```

It compiles `tools/mpp_kcfg_probe.c` against the upstream vendor headers,
loads it with `librk_mpp_rs.so` as `librockchip_mpp.so.1`, and checks mapping,
mismatch and unknown-key rejection.

## Other kcfg objects

`mpp_vdec_kcfg_*`, the remaining `mpp_venc_kcfg_*` modes and
`mpp_venc_ctrl_*` retain vendor-compatible typed storage and flex-buffer
semantics. They do not issue a kernel kcfg ioctl because this crate programs
the HAL directly.

The complete upstream `mpi_enc_test -kmpp_mode=1` still requires the broader
KMPP object ABI (`kmpp_obj_get`, `kmpp_obj_to_shm`, object definitions, shared
memory/ioctl helpers). Those symbols are deliberately not faked: an init-kcfg
probe validates the implemented surface without claiming full KMPP-object
compatibility.
