# rk-mpp-rs

`rk-mpp-rs` 是 Rockchip MPP 的纯 Rust 实现。它直接驱动
`/dev/mpp_service`、DMA heap 和 Rockchip 硬件块，不链接
`librockchip_mpp`，也不依赖 `librga`。

项目目标不是做一个“接口相似”的封装，而是提供可验证的替代实现：

- 与板上 C MPP 逐字节或逐 packet 比较；
- 提供二进制 drop-in、官方 MPI 源码兼容层、Rust SDK 和 C SDK；
- 在相同硬件条件下不少于 C 的吞吐，并尽量减少 CPU 拷贝。

当前主验证平台是 **RK3588**，第二平台是 **RK3576**。验证结论只适用于
[能力矩阵](docs/CAPABILITY.md) 中列出的硬件和调用路径，不外推到其他 SoC、
内核或产品板。

## 快速导航

- [从 C MPP 迁移](docs/MIGRATION.md)
- [能力与证据矩阵](docs/CAPABILITY.md)
- [路线图](ROADMAP.md)
- [开发与板端交接](docs/HANDOFF.md)
- [已知差异](docs/DIVERGENCES.md)
- [C API 头文件](include/rk_mpp_rs.h)
- [Rust SDK 示例](examples/)

## 特性

| 领域 | 当前状态 |
|---|---|
| 硬件访问 | 直接实现 MPP CMD、IOVA、RCB、poll、DMA heap、batch/CCU 和 SOC probe |
| 解码 | H.264、H.265、VP9、AV1、VP8、MPEG-2、MPEG-4、H.263、AVS+、JPEG/MJPEG |
| 编码 | H.264/H.265、JPEG；支持固定 QP、CBR/VBR/AVBR、ROI、GDR、slice、OSD、packet meta |
| 后处理 | 自研 NV12/RGB 缩放与 letterbox、IEP2 去隔行、RK3576 VDPP2 scale/split/histogram |
| SDK | Rust SDK、C API V1.14、`rk-mpp-rs-sys`、pkg-config、CMake、动态/静态库 |
| MPP 兼容 | 官方 `MppApi`、MppFrame/MppPacket/MppBuffer/MppMeta、对象路径和 `mpi_*_test` drop-in |
| 运行时 | 异步任务队列、DMA-BUF 池、错误恢复、多会话 CCU、显式 EOS/flush/reset |
| 验证 | C/Rust 逐字节、软件 decoder 独立 oracle、4K/长 GOP/多路/Soak 和板端 ABI 门 |

当前 API 和已验证硬件路径已经可用于实际集成；未列在能力矩阵中的 SoC、
内核版本和厂商 HAL 路径不提供兼容承诺。

## 平台覆盖

| 平台 | 主要硬件块 | 已验证范围 |
|---|---|---|
| RK3588 | VDPU2、AV1D、rkvdec、JPEGD、VEPU580、VEPU2 JPEG、IEP2 | 主验证平台；完整 4K 基础矩阵 26/26，扩展功能矩阵 25/25 |
| RK3576 | VDPU383、VEPU510、VPU720、VDPP2 | 4K 可执行矩阵 17/17，5 个平台/未实现项明确 SKIP；VPU720 JPEG 4K 专项 21/21 |

能力边界示例：

- RK3588 没有 VDPP/client 29，也没有 legacy `/dev/iep` scale；
- RK3576 没有 VDPU2、VP8 decoder 或 AVS2；
- IEP2 的最大可用尺寸是 1920×1088，4K 会在提交前明确拒绝；
- H.265 4:2:2/4:4:4、部分 10-bit DMA-BUF 和部分 RK3576 4K 编码路径仍是平台边界。

完整状态、样本和 C bug 记录见 [CAPABILITY.md](docs/CAPABILITY.md) 和
[DIVERGENCES.md](docs/DIVERGENCES.md)。

## 选择迁移方式

| 场景 | 推荐路径 | 入口 |
|---|---|---|
| 已有程序直接链接 `librockchip_mpp.so.1` | 二进制 drop-in | [迁移指南 §1](docs/MIGRATION.md) |
| 保留官方 `rk_mpi.h`，只替换链接库 | 源码兼容层 | [迁移指南 §2](docs/MIGRATION.md) |
| 新项目使用 Rust | `rk_mpp_rs::sdk` | [Rust 示例](examples/sdk_decode.rs) |
| 新项目使用 C/C++ | `include/rk_mpp_rs.h` | [C API](#c-api) |
| 已有 `ffmpeg --enable-rkmpp` | `LD_LIBRARY_PATH` overlay | [FFmpeg drop-in](#ffmpeg-drop-in) |

官方头文件共有 **297 个声明符号，覆盖 297/297**。其中核心对象 API、配置、
meta、buffer 和测试工具辅助符号均有实现；`sys_cfg`、`vdec_kcfg`、
`venc_kcfg` 的部分 kernel-only 键保留类型化存储语义。详细覆盖审计见
[docs/MIGRATION.md](docs/MIGRATION.md)。

## 快速开始

### 前置条件

- Linux aarch64 板卡；
- 内核提供 `/dev/mpp_service` 和对应 DMA heap 节点；
- Rust stable 和 Cargo；
- 运行时除 `libc` 外没有第三方 crate。

### 构建与主机测试

```bash
git clone https://github.com/jlu-lujing/rk-mpp-rs.git
cd rk-mpp-rs

cargo build --release
cargo test --workspace --all-targets
```

需要硬件设备的测试均标记为 `#[ignore]`，不会在普通 CI 主机上运行。

### 同步到板卡并构建

```bash
BOARD=my-rk3588-host

ssh "$BOARD" 'mkdir -p "$HOME/rk-mpp-rs"'
COPYFILE_DISABLE=1 tar \
  --no-xattrs \
  --exclude .git \
  --exclude target \
  --exclude .worktrees \
  --exclude refs \
  -cf - . |
  ssh "$BOARD" 'tar -xf - -C "$HOME/rk-mpp-rs"'

ssh "$BOARD" 'cd "$HOME/rk-mpp-rs" && cargo build --release'
```

### 第一次硬件检查

```bash
# 无参数会 probe 当前 SoC 和硬件能力
./target/release/rk-mpp-rs

# H.264 整段解码
./target/release/rk-mpp-rs h264dec input.264 output.yuv

# 输出 NV12
RKMPP_DECODE_NV12=1 ./target/release/rk-mpp-rs h264dec input.264 output.nv12

# JPEG 解码到 RGB；最后一个参数是 1/2/4/8 下采样
./target/release/rk-mpp-rs decode input.jpg output.rgb 1
```

### 常用 CLI

| 命令 | 用途 |
|---|---|
| `rk-mpp-rs h264dec <in.264> <out.yuv>` | H.264 解码 |
| `rk-mpp-rs hevcdec <in.265> <out.yuv>` | H.265 解码 |
| `rk-mpp-rs vp9dec <in.ivf> <out.yuv>` | VP9 解码 |
| `rk-mpp-rs av1dec <in.ivf> <out.yuv>` | AV1 解码 |
| `rk-mpp-rs decode <in.jpg> <out.rgb> [1\|2\|4\|8]` | JPEG 解码 |
| `rk-mpp-rs h264-enc <in.nv12> <out.264> <w> <h> ...` | H.264 编码 |
| `rk-mpp-rs h265-gop <in.nv12> <out.265> ...` | H.265 GOP 编码 |
| `rk-mpp-rs iep-dei <in.yuv> <out.yuv> <w> <h>` | IEP2 去隔行 |
| `rk-mpp-rs vdpp-scale ...` | RK3576 VDPP2 缩放/格式转换 |

完整参数、RK3576 VEPU510 专用命令和高级编码选项见
[docs/HANDOFF.md](docs/HANDOFF.md)。

## Rust SDK

```rust
use rk_mpp_rs::sdk::{
    Codec, Decoder, DecoderConfig, Packet, PacketFlags, PixelFormat,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut decoder = Decoder::new(
        DecoderConfig::new(Codec::H264).with_output_format(PixelFormat::I420),
    )?;

    let input = std::fs::read("input.264")?;
    decoder.submit_packet(
        Packet::new(&input).with_flags(PacketFlags::EOS),
    )?;

    while let Some(frame) = decoder.receive_frame() {
        println!(
            "{}x{} pts={}",
            frame.width(),
            frame.height(),
            frame.pts()
        );
    }
    decoder.flush()?;
    Ok(())
}
```

可以直接运行完整示例：

```bash
cargo run --release --example sdk_decode -- h264 input.264 output.yuv
cargo run --release --example sdk_encode -- h264 input.nv12 output.264 320 240 26 6
```

SDK 支持任意分块输入、显式 EOS、flush/reset/recover、frame release 和
可选 DMA-BUF 输出。H.264/H.265 有有界显示重排；VP9/VP8 每次提交一个 coded
packet，AV1 每次提交一个 temporal unit。IVF/WebM 解复用仍由调用方负责。

## C API

安装库、头文件、pkg-config 和 CMake config：

```bash
./scripts/install_sdk.sh --prefix /usr/local
gcc app.c $(pkg-config --cflags --libs rk-mpp-rs)
```

```c
#include <rk_mpp_rs.h>

rk_mpp_decoder_config_t config;
rk_mpp_decoder_config_init(
    &config,
    RK_MPP_CODEC_H264,
    RK_MPP_PIXEL_FORMAT_I420);

rk_mpp_decoder_t *decoder = NULL;
rk_mpp_decoder_create(&config, &decoder);
```

头文件中的 lifetime、线程安全、平面布局和错误码说明是 C API 的契约。

## 二进制 drop-in

已有程序仍按 `librockchip_mpp.so.1` 链接时，可以让动态链接器优先加载
本库：

```bash
overlay=$(mktemp -d)
ln -s "$PWD/target/release/librk_mpp_rs.so" \
  "$overlay/librockchip_mpp.so.1"

LD_LIBRARY_PATH="$overlay" ./mpi_dec_test \
  -i input.264 -t 7 -o output.yuv
```

RK3588/RK3576 上已验证未修改的 `mpi_dec_test`、`mpi_enc_test` 和 FFmpeg
RKMPP 前端。当前 MPP 兼容层对 H.264/H.265/VP9/AV1/MJPEG advanced decode
支持 caller DMA-BUF output；4K MJPEG 的 `poll/dequeue/enqueue` 路径也已闭合。

### FFmpeg drop-in

```bash
overlay=$(mktemp -d)
ln -s /opt/rk-mpp-rs/lib/librk_mpp_rs.so.1 \
  "$overlay/librockchip_mpp.so.1"

LD_LIBRARY_PATH="$overlay" ffmpeg \
  -c:v h264_rkmpp -i input.264 \
  -fps_mode passthrough -pix_fmt nv12 -f rawvideo output.yuv
```

FFmpeg 4K 解码必须使用 `-fps_mode passthrough`；否则 `-frames:v` 可能因
EOF/时间基在两侧都写出 0 字节。重新编译带 `--enable-rkmpp` 的 FFmpeg 时，
使用：

```bash
./scripts/install_sdk.sh --prefix /opt/rk-mpp-ffmpeg --mpp-compat
```

## 验证

### 当前 4K 回归快照

| 门禁 | 结果 |
|---|---|
| RK3588 基础 4K | **26/26 PASS** |
| RK3588 扩展 4K | **25/25 PASS** |
| RK3576 适用 4K | **17 PASS / 0 FAIL / 5 SKIP** |
| RK3576 VPU720 JPEG 4K | **21/21 PASS** |
| 主机全量测试 | **1427 passed / 0 failed / 47 ignored** |

RK3588 4K 覆盖 H.264/H.265/VP9/AV1 的 8/10-bit、三帧状态、film grain、
Main10、H.264/H.265/MJPEG 编码、FFmpeg drop-in、JPEG 缩放、1–16 路并发、
长 GOP/DPB soak、SDK DMA-BUF、packet meta、IEP2 上限和 VDPP2 边界。

RK3576 的 5 个 SKIP 是未实现或平台边界，不计作 PASS：FFmpeg 4K drop-in、
Rust 4K H.264/H.265 编码、MJPEG 对象编码，以及由 VPU720 JPEG 专项门覆盖的
MJPEG 4K 解码。

### 主机 CI

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace --all-targets
```

### 板端常用门

```bash
# RK3588 扩展 4K 功能；默认使用板上的 /tmp/mpp-old 编码 oracle
BOARD=my-rk3588-host ./scripts/board_4k_features.sh

# 在板卡本机运行基础 4K 矩阵
ssh "$BOARD" 'cd ~/rk-mpp-rs && ./scripts/board_4k_matrix.sh'

# 官方工具 drop-in
ssh "$BOARD" 'cd ~/rk-mpp-rs && ./scripts/board_mpp_standalone.sh'
ssh "$BOARD" 'cd ~/rk-mpp-rs && ./scripts/board_mpp_official_dec_test.sh'
ssh "$BOARD" 'cd ~/rk-mpp-rs && ./scripts/board_mpp_official_enc_test.sh'
```

完整回归命令、参考 C 构建和已知坑见
[docs/HANDOFF.md](docs/HANDOFF.md)。

## 性能

性能结论只在固定 CPU/DDR governor、双方交错采样并先校验输出一致的前提下
报告。代表性结果：

| 路径 | 当前结果 |
|---|---|
| H.264 720p 300 帧编码 | Rust 0.334 s / 898 FPS，C FFmpeg 0.349 s / 860 FPS |
| H.265 720p 300 帧编码 | Rust 与 C 同为 676 FPS；Rust user+sys 0.372 s，C 0.393 s |
| 320×240 1000 帧稳定态 | H.264 Rust 9.1% faster，CPU -31.7%；H.265 Rust 5.6% faster，CPU -24.3% |
| H.264 流式解码 | 50 MiB / 742 帧 wall median 1.60 s，峰值 RSS 从约 55.9 MiB 降到约 17 MiB |
| NV12/RGB 缩放 | 1080p→640 0.84 ms，RGA 3.21 ms |

这些数字是板卡、内核、输入和 governor 相关的工程结果，不是跨设备 SLA。
完整方法和样本见：

- [ENCODE_BENCH.md](docs/ENCODE_BENCH.md)
- [FFMPEG_BENCH.md](docs/FFMPEG_BENCH.md)
- [RK3588_VALIDATION.md](docs/RK3588_VALIDATION.md)

## 项目结构

```text
src/service/    mpp_service、dma-heap、batch/CCU、poll、probe
src/codec/      H.264/H.265/VP9/AV1/MPEG/H.263/VP8/JPEG 语法与码控
src/hal/        各 Rockchip 硬件块寄存器生成
src/reference/  自研软件参考解码器
src/scale.rs    NV12/RGB 缩放与 letterbox
include/        C API 头文件
crates/         rk-mpp-rs-sys raw FFI
examples/       Rust SDK 示例
scripts/        板端验证、性能和取证脚本
tools/          寄存器字段生成器、C probe、diff 工具
tests/data/     真实 C 输出和固定 fixture
```

## 文档

| 文档 | 内容 |
|---|---|
| [ROADMAP.md](ROADMAP.md) | 逐 capability 的完成项、证据和未完成项 |
| [CAPABILITY.md](docs/CAPABILITY.md) | 能力矩阵、板级边界和回归入口 |
| [MIGRATION.md](docs/MIGRATION.md) | drop-in、源码替换、SDK 和 FFmpeg 迁移 |
| [HANDOFF.md](docs/HANDOFF.md) | 板卡工具链、完整命令和开发交接 |
| [DIVERGENCES.md](docs/DIVERGENCES.md) | 与 C 的差异、已确认 C bug 和无 oracle 项 |
| [KCFG.md](docs/KCFG.md) | 官方 kcfg 兼容面和存储语义 |
| [rk3576.md](docs/rk3576.md) | RK3576 硬件代际与适配进度 |
| [SECURITY.md](SECURITY.md) | 安全边界和漏洞报告方式 |
| [CONTRIBUTING.md](CONTRIBUTING.md) | 开发规则、证据要求和 PR 流程 |

## 已知限制

- 不宣称尚未在能力矩阵中列出的 SoC、内核、固件和厂商 HAL 路径可用。
- RK3576 仍然只保证有板级证据的 block；4K H.264/H.265 编码尚未实现。
- AVS+ 的 AEC 和独立 field-pair 仍待验证；AVS2 在 RK3588 无设备。
- IEP2 上限为 1920×1088；4K 由 C 侧和 Rust 侧一起明确拒绝。
- 官方 MPP 的 kernel-only storage 键不伪装成硬件能力；KMPP kernel object
  路径受内核 `/dev/vcodec`、`/proc/kmpp` 和 buffer-group ioctl 支持限制。
- FFmpeg 验证基于特定的 `--enable-rkmpp` 版本；不同 wrapper 版本可能需要
  单独对齐调用序列。
- 自研 AV1 软件参考解码器仍在推进；它不是硬件路径的前置依赖。
- 性能百分比需要 governor、核绑定和交错采样，不能把单次运行当作稳定结论。

## 贡献与许可

提交代码前请运行：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace --all-targets
```

需要真实硬件的测试应保持 `#[ignore]`，并把板卡、内核、命令和输出证据写入
`scripts/board_*.sh`、`ROADMAP.md` 和 `docs/CAPABILITY.md`。

安全问题请按 [SECURITY.md](SECURITY.md) 私下报告。贡献流程见
[CONTRIBUTING.md](CONTRIBUTING.md)。

本项目使用 **MIT OR Apache-2.0** 双许可。
