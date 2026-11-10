# rk-mpp-rs：RK3588 全硬件路径重写设计

日期：2026-10-30
状态：已确认（用户选择「按顺序全部做完」）；实施进度见文末与 `ROADMAP.md`

## 1. 目标

用纯 Rust 直接驱动 Rockchip `mpp_service`，覆盖 NanoPC-T6（RK3588）上**实际存在**的全部硬件块，
不链接 `librockchip_mpp` / `librga`：

| 硬件块 | client type | 覆盖能力 |
|---|---:|---|
| jpegd | 13 | JPEG 解码（已完成） |
| rkvdec | 9 | H.264、H.265、VP9、AVS2 解码 |
| av1d | 4 | AV1 解码 |
| vdpu2 | 1 | H.264/MPEG2/MPEG4/H.263/VP8 解码 |
| vdpu2-pp | 3 | 反交错、缩放、格式转换 |
| rkv-enc | 16 | H.264 / H.265 编码 |
| vepu-jpeg | 19 | JPEG 编码 |

### 非目标

- 板上没有硬件的 vepu1/vepu2/vepu22（bit 17/18/24）与旧 vdpu1/avs-plus，不做软件回退。
- 不追求与官方 C API 源码级一致；只追求**行为**与寄存器级结果一致。

## 2. 架构

保持现有四层，新增模块按 codec 纵向切片：

```
service/     内核接口：mpp_service client、dma_heap、IOMMU、BufferPool、异步完成
hal/         寄存器层：每个硬件块一个模块，纯函数 builder：Job -> Vec<u32>
codec/       语法层：parser / DPB / 命令缓冲，每个 codec 一个目录
reference/   软件参考解码器，作为硬件输出的 ground truth
frame.rs     公共 Picture / Frame / PixelFormat / BufferPool 句柄
```

### 关键约束

1. `hal::*::build()` 是**纯函数**，不做 ioctl、不持有 fd，只接收 `Job`（fd/iova、尺寸、语法参数）
   并返回寄存器字。这让寄存器可以完全离线做黄金测试。
2. `codec` 层不知道寄存器布局，只产出 `Job`。
3. `service` 层是唯一碰 `ioctl` 的地方，提供同步 submit/poll 与异步完成两种用法。
4. 新硬件块的寄存器映射由 `tools/gen_*_regs.py` 从
   `refs/mpp/mpp/hal/**/xxx_reg.h` 生成，人工只写 builder。

### 新增公共设施（随子项目逐步引入）

- `service::BufferPool`：可复用的 dma-buf 池，带引用计数，替代目前的逐帧 `DmaBuf::alloc`
- `service::Completion`：`MPP_CMD_POLL_HW_IRQ` 的 slice 表封装，支持一次提交多 slice
- `frame::Frame`：`{ width, height, format, stride, buf: DmaBuf }`
- `codec::Decoder` 枚举：统一 `push_stream()` / `next_frame()` 接口

## 3. 验证策略

三级，前两级必须每帧比对，第三级只在性能回归时跑。

### L1 寄存器黄金测试（离线，`cargo test`）

参考 MPP 在板上已有**打好 dump 补丁**的源码 `/tmp/mpp-develop`，并已产出：

| 文件 | 内容 |
|---|---|
| `/tmp/mpp_regs.txt` | 参考解码器每帧 `SET_REG_WRITE` 寄存器字 |
| `/tmp/mpp_fields.txt` | 寄存器字段级 dump |
| `/tmp/mpp_tables.txt` | 硬件表缓冲 dump |
| `/tmp/mpp_streams.bin` / `/tmp/our_streams.bin` | 双方送入硬件的码流逐字节对照 |

流程：

1. 用 `/tmp/mpp-develop/build/test/mpi_dec_test`（H.264 可用）对新码流重新 dump，
   产物先 `sudo rm` 旧文件再生成（旧文件是 root 所有）。
2. 把 dump 收敛成 `tests/golden/<codec>/<stream>.json` 提交进仓库。
3. Rust 单测读入黄金文件，要求 `hal` builder 产出逐字相同。

理由：官方 HAL 是 143k 行 C，逐行通读不可靠；寄存器黄金对比把移植变成可验证的机械工作，
也是后续 VP9/AV1/编码器唯一现实的做法。`mpi_dec_test` 目前只有 H.264 路径可用，
H.265 及以后的 codec 需要用 `mpp_hal` 直接驱动的方式补一个 dump 入口。

### L2 码流正确性（上板）

解码真实码流，与 `reference/` 软件解码器逐帧比较：

- 无损/整数路径：逐字节 bit-exact
- 有损路径：YUV PSNR ≥ 60 dB（B 帧、10-bit 单独设阈值并记录）

测试流已在板上就绪（本地生成后 scp）：

| 文件 | 内容 |
|---|---|
| `/tmp/test720.264` | 1280x720 main / B 帧 / 30 帧（H.264 主力） |
| `/tmp/test1080.264` | 1920x1080 baseline / 10 帧 |
| `/tmp/test_hard.264` | 1920x1080 high / 5 参考 / B-pyramid / 90 帧 |
| `/tmp/one_idr.264` | 640x480 单 IDR |
| `/tmp/one.hevc` | 640x480 单 IDR（x265 默认） |
| `/tmp/t720.hevc` | 1280x720 / 10 帧 / 含 B 帧 |
| `/tmp/t720_nob.hevc` | 同上，`bframes=0` |
| `/tmp/ip2.hevc` | 320x240 I+P 2 帧 |
| `/tmp/e_*.hevc` `/tmp/b_*.hevc` `/tmp/d_*.hevc` | 320x240 特征二分变体 |

源图基准：`/tmp/src_one.yuv`、`/tmp/src720.yuv`、`/tmp/src_simple.yuv`。
新增码流时用 `scripts/gen_streams.sh` 固定参数生成，保证可复现。

### L3 性能（上板）

同流对比官方 C MPP 耗时，不得更慢；每次 milestone 记录到 `ROADMAP.md`。

## 4. 子项目与验收

| # | 分支 | 范围 | 验收 |
|---|---|---|---|
| A | `feat/a-h264-closeout` | `ref_pic_list_modification` 重排、colmv 有效性、multi-slice、long-term ref、field/MBAFF | x264 B-pyramid / multi-ref / multi-slice 与 reference 逐帧 bit-exact |
| B | `feat/b-h265-decode` | rkvdec HEVC 表 + 寄存器 + CTU/SAO 主路径；tiles/WPP；Main10 | x265 720p/1080p/Main10 bit-exact 或 PSNR ≥ 60 |
| C | `feat/c-vp9-decode` | VP9 parser、分割、loop filter、超帧、rkvdec VP9 HAL | 同上 |
| D | `feat/d-av1-decode` | `hal/vpu/av1d` + `hal/rkdec/av1d` 寄存器，film grain 关 | 同上 |
| E | `feat/e-encoders` | rkv-enc H.264 → H.265；vepu JPEG | 编码后回解的 PSNR 对齐官方 MPP 同码率 |
| F | `feat/f-postproc` | vdpu-pp 反交错 / 缩放 / 格式转换 / crop | 与 RGA 输出逐像素比对 |
| G | `feat/g-framework` | 异步任务队列、dma-fence、BufferPool 引用计数、多核 CCU、secure | 长跑无泄漏、无丢帧 |

顺序理由：A 用已有 reference + 测试流跑通闭环模板；B/C/D 复用模板；E/F/G 依赖前面建立的
BufferPool 与 fence 抽象。

每个子项目完成时更新 `ROADMAP.md` 的 feature checklist，并记录 L2/L3 数据。

## 5. 工作流

### 机器布局

本机（macOS）：

```
/Users/user/workspace/rknn-test/rk-mpp-rs           纯 Rust 实现（主战场，独立 git 仓库）
/Users/user/workspace/rknn-test/rknn-fastinfer-rs   纯 Rust 推理
/Users/user/workspace/rknn-test/rknn-fastinfer      C 参考实现（build-rust / build-mpp）
/Users/user/workspace/rknn-test/refs/mpp            官方 MPP 源码（只读对照）
```

板子（NanoPC-T6，`ssh t6`）：

```
~/rk-mpp-rs                    同步后的工作副本
~/rknn-fastinfer-rs / ~/projects/rknn-fastinfer
/tmp/mpp-develop               官方 MPP（已打 dump 补丁，自编译）
/tmp/*.264 /tmp/*.hevc         测试码流
/tmp/src*.yuv                  源图基准
```

### 循环

```
rsync -a --exclude target rk-mpp-rs/ t6:~/rk-mpp-rs/
ssh t6 'cd ~/rk-mpp-rs && cargo build --release'
ssh t6 'cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264dec /tmp/test720.264 /tmp/o_rust.yuv'
```

封装为 `scripts/board.sh {sync|build|run|test}`。

注意事项：

- 所有跑硬件的命令需要 `sudo`（`/dev/mpp_service`）。
- `/tmp/mpp_*.txt` 是 root 所有，重新生成前必须 `sudo rm`。
- 本机只做 `cargo check` + `cargo test`（无 `/dev/mpp_service`，运行时测试全部上板）。
- 参考源码 `refs/mpp` 只读，不修改、不提交进 crate。
- 每个子项目一个分支（`feat/a-h264-closeout` 等），合回主线前必须通过 L1 + L2。

## 6. 风险

| 风险 | 应对 |
|---|---|
| HAL 移植量巨大（VP9 8k、AV1 14k、编码器 27k） | 寄存器黄金测试 + 生成器，按帧逐步推进 |
| 硬件文档缺失，只能对照 C 代码 | 以 `refs/mpp` 的 `hal/` 与内核驱动头文件为准 |
| 本机无硬件，所有运行验证依赖板子 | `scripts/board.sh` 标准化；L1 尽量离线覆盖 |
| 与官方 MPP 行为差异（缓冲对齐、colmv、tile 边界） | L2 逐帧比对，差异必须定位到具体宏块/CTU 并记录 |
| 许可证 | 官方 MPP 为 Apache-2.0，本 crate 为 MIT OR Apache-2.0，兼容；移植代码保留来源注释 |

## 7. 交付节奏

A→G 合计约 9 万行量级，属数月工程。按上表顺序推进，每个子项目独立交付、独立验证；
不把未验证的代码合入主线。

## 实施进度（2026-11-01）

| 硬件块 | 状态 |
|---|---|
| jpegd（13） | 完成，含 1/2–1/8 缩放与 RGB888 |
| rkvdec（9）H.264 | 完成，逐字节（frame/MBAFF/PAFF/LTR/错误路径） |
| rkvdec（9）H.265 | 完成，逐字节（Main/Main10/tiles/WPP/DSLICE/LTR/scaling list） |
| rkvdec（9）VP9 | profile 0/2、lossless、tiles、error-resilient 逐字节；边界矩阵未完 |
| rkvdec（9）AVS2 | 该板上 rkvdec 声明支持，但无 conformance 流，未做 |
| av1d（4） | Main 8/10-bit、film grain 逐字节；更宽 conformance 未完 |
| vdpu2（1） | H.264/MPEG2/MPEG4/H.263/VP8 解码全部逐字节；AVS+ 未有流 |
| vdpu2-pp（3） | 本源码中无解码器使用；该板的通用后处理是 IEP（28），未移植 |
| rkv-enc（16）H.264 | 固定 QP / CBR / VBR 逐 NAL 相同；AVBR、ROI/GDR、slice split、>4096 未完 |
| rkv-enc（16）H.265 | 未开始 |
| vepu-jpeg（19） | baseline 4:2:0 完成 |
| framework | 同步/异步、错误恢复、缓冲池和 CCU 批量提交已完成；fence/secure 受内核/TA 限制 |

整体约五成（按 `ROADMAP.md` 的里程碑工作量估加权）；后续顺序与注意事项见
`docs/HANDOFF.md`。
