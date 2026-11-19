# 从 Rockchip C MPP 迁移到 rk-mpp-rs

本仓库提供三层迁移路径，按改动量从小到大排列。三层使用同一套硬件会话、
寄存器编程与验证结果；差别只在调用面。

## 0. 先跑覆盖检查

```bash
# 在板子上构建出 librk_mpp_rs.so 之后：
nm -D --defined-only target/release/librk_mpp_rs.so | awk '{print $3}' > /tmp/nm.txt
python3 tools/check_mpp_api_coverage.py --headers ../refs/mpp/inc --symbols /tmp/nm.txt
```

当前结果：**297 个声明符号全部覆盖（297/297）**。其中：

- 核心 API 100 %（`mpp_buffer` 29/29、`mpp_frame` 71/71、`mpp_packet` 34/34、
  `mpp_meta` 22/22、`mpp_dec_cfg` 13/13、`mpp_enc_cfg` 28/28、
  `mpp_compat` 4/4）；
- `mpp_sys_cfg_*`（19）：句柄 + 按名称的类型化存储/回显；
- `mpp_task_meta_*`（12）：映射到 `mpp_meta_*`，保留 default_value 语义；
- `mpp_vdec_kcfg_*`（16）与 `mpp_venc_kcfg_*`/`mpp_venc_ctrl_*`（26）：句柄 +
  类型化存储 + 4 KiB flex 区。`MPP_SET_VENC_INIT_KCFG` 已消费 type/coding、
  max dimensions 和上游 init keys；其余 kernel-only 键仍是 storage-only。
  unknown init key 会明确拒绝。非法句柄 type、未知 canonical name 和非
  CTRL_CFG flex 操作也会按 vendor 语义报错。完整审计见 [`KCFG.md`](KCFG.md)。
- `MppEncRefCfg` 的 `check` 会执行实际层级校验：最多 16 个配置项、LT index
  唯一且小于 16、temporal id 为 0..3、LT/ST mode 分离、首尾 ST 帧必须是
  temporal-zero reference；负 repeat 会按 vendor 行为归零，非法配置返回
  `MPP_NOK`。`set_cfg_cnt` 设置容量并清零 live entry，`add_*` 逐项追加并
  检查剩余容量，`mpp_enc_ref_cfg_copy` 复制完整层级；
  `mpp_enc_ref_cfg_get_cpb_info` 的 DPB/LT/ST/temporal/GOP 字段由虚拟 CPB
  dryrun 计算，并与 vendor 的 IPPP、TSVC2/3/4 probe 一致。`init/reset` 的
  默认值也恢复成 vendor 的单帧 forward-reference IPPP。LOG、JSON、TOML
  三种 `apply/apply_file/extract` 均已实现，可读取 vendor 的 ref-cfg schema；
  LOG/JSON 与 vendor 双向通过，TOML 提取逐字节相同且 Rust 可回读。当前 vendor
  连自己导出的 TOML 也无法再次解析，该 C bug 记录在
  [`DIVERGENCES.md`](DIVERGENCES.md)。
  调度器已支持上一 ST、指定 LT index 和倒数第 N 个 ST 引用模式。

## 1. 二进制 drop-in（不改代码）

适用于已经链接 `librockchip_mpp.so.1` 的应用（`mpi_dec_test`、`mpi_enc_test`、
自研播放器等）。

```bash
BOARD=t6 scripts/board_mpp_standalone.sh          # 构建并验证只加载 librk_mpp_rs.so
BOARD=t6 scripts/board_mpp_official_dec_test.sh   # 未修改的官方解码测试
BOARD=t6 scripts/board_mpp_official_enc_test.sh   # 未修改的官方编码测试
```

部署时让动态链接器优先找到本库：

```bash
# 方案 A：把 SONAME 换成 rockchip 的名字（官方工具无需改动）
ln -sf librk_mpp_rs.so.1 /opt/rk-mpp-rs/lib/librockchip_mpp.so.1
LD_LIBRARY_PATH=/opt/rk-mpp-rs/lib ./mpi_dec_test -i in.h264 -t 7 -o out.yuv

# 方案 B：LD_PRELOAD，保留系统库作为后备
LD_PRELOAD=/opt/rk-mpp-rs/lib/librk_mpp_rs.so.1 ./mpi_dec_test ...
```

已验证：t6/lf1 上 H.264/H.265/VP9/AV1/MJPEG 的官方 `mpi_dec_test` 输出与
vendor 库逐字节一致；`mpi_enc_test` 可完成 H.264/H.265/MJPEG 单帧编码。

安装 SDK（含 SONAME、pkg-config、CMake config）：

```bash
scripts/install_sdk.sh --prefix /usr/local
pkg-config --cflags --libs rk-mpp-rs
```

## 2. 源码级替换（官方头文件 + 本库）

不改应用逻辑，只改链接目标：

```diff
- find_library(MPP rockchip_mpp)
+ find_library(MPP rk_mpp_rs)
  target_include_directories(app PRIVATE ${MPP_INCLUDE_DIR})
  target_link_libraries(app ${MPP})
```

编译仍使用官方 `inc/`（`rk_mpi.h`、`mpp_frame.h`…）。`MppCtx/MppFrame/
MppPacket/MppBuffer/MppMeta/MppApi` 的布局与调用序列按官方 ABI 实现，
`MppApi` 表为 184 字节（aarch64）。

**已覆盖的核心调用**：

- `mpp_create/mpp_init/mpp_destroy`、`mpp_check_support_format`；
- `decode_put_packet/decode_get_frame`、`encode_put_frame/encode_get_packet`；
- `MppPacket/MppFrame/MppBuffer/MppMeta` 的 get/set 与生命周期；
- `MPP_DEC_SET_CFG/MPP_ENC_SET_CFG`、`MppEncCfg` 键值读写
  （`mpp_enc_cfg_get_*/set_*` 等），包括 `rc:ref_cfg`、`MPP_ENC_SET_IDR_FRAME`、
  per-IDR header mode、SEI disable；
- `mpp_frame` 的 SAR、mastering display、content light、HDR dynamic meta；
- `mpp_set_log_level/get_log_level/set_log_callback`（默认 `MPP_LOG_INFO`，
  可用 `mpp_log_level` 环境变量覆盖，与 vendor 一致）；
- `mpp_compat_query/query_by_id/update/show` 三个运行时开关。

这些符号在板上有一个独立的 C consumer 冒烟门（只包含官方 `inc/` 头文件）：

```bash
BOARD=t6 scripts/board_mpp_migration.sh
```

覆盖日志级别与回调、`mpp_frame` 的 SAR/内容亮度/母版显示、compat 查询与更新。

## 3. 新 C API / Rust crate（推荐新代码）

`include/rk_mpp_rs.h` 是本仓库自己的稳定 API，语义比官方 MPI 简单：

```c
rk_mpp_decoder_config_t cfg;
rk_mpp_decoder_config_init(&cfg, RK_MPP_CODEC_H264, RK_MPP_PIXEL_FORMAT_NV12);
rk_mpp_decoder_t *dec;
rk_mpp_decoder_create(&cfg, &dec);
rk_mpp_decoder_submit_packet(dec, data, size, pts, dts, flags);
while (rk_mpp_decoder_receive_frame(dec, &frame) == RK_MPP_OK) {
    /* frame.plane_data[...] */
    rk_mpp_frame_release(dec, &frame);
}
rk_mpp_decoder_flush(dec);
rk_mpp_decoder_close(dec);
```

Rust 侧：

```toml
[dependencies]
rk-mpp-rs = { git = "https://github.com/jlu-lujing/rk-mpp-rs" }
```

```rust
let mut dec = rk_mpp_rs::codec::h264::Decoder::new()?;
dec.set_pipeline_depth(true, 3);           // 提交环，默认 CLI 已开启
for au in rk_mpp_rs::codec::h264::decoder::split_parsed_access_units(&bytes) {
    if let Some(picture) = dec.decode_parsed_access_unit(&au)? { use_picture(picture); }
}
for picture in dec.flush()? { use_picture(picture); }
```

## 4. 已知差异与建议

| 差异 | 影响 | 建议 |
| --- | --- | --- |
| `mpp_sys_cfg_*` 只有存储/回显 | `mpp_sys_cfg_ioctl` 与 `st_get_*` 返回 `MPP_NOK`，不计算 dec_buf_chk 的 stride/size | 需要真实 buffer 检查时保留厂商库，或直接用本库的 frame layout |
| `mpp_task_meta_*` 接受 meta 句柄 | 没有公开 task 对象；传入别的 runtime 的 task 指针会返回 `MPP_NOK` | 用 `mpp_meta_get_with_tag()` 创建句柄后直接传给它 |
| venc init kcfg 已映射；其余 kcfg storage-only | init type/coding/max dimensions 生效，kernel-only 高级 cfg 不生效 | 完整 KMPP object 调用面仍需厂商库；普通 `mpi_enc_test` 可迁移 |
| 不调用 `MPP_CMD_RESET_SESSION` | 板上会导致后续解码卡死 | 本库统一 close-before-open 恢复；不要手工 reset |
| 不包含厂商二进制/固件 | 需要 `/dev/mpp_service` 与内核驱动 | 依赖系统内核，不需要 `librockchip_mpp` |
| 板级脚本假设 `t6`/`lf1` 主机别名 | 仅影响 `scripts/board_*.sh` | 使用前设置 `BOARD=<ssh alias>` |

## 5. 迁移后验证

```bash
cargo test --workspace --all-targets --offline     # 主机侧
BOARD=t6 scripts/board_regress_all.sh              # 板级逐字节回归
BOARD=t6 scripts/board_mpp_official_dec_test.sh    # 官方工具 drop-in
BOARD=t6 scripts/board_mpp_compat_data.sh          # 官方头文件 C consumer
```
