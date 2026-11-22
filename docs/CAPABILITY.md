# RK3588 MPP 能力矩阵（以 RK3588 上的 C MPP 为 oracle）

这一页回答三个问题：**哪些路径已经重写并对齐、每条结论用什么证据支撑、哪些在 RK3588
上根本无法验证**。所有“完成”都指与板上源码编译的 C MPP 逐字节/逐字段一致；没有 oracle
的能力单独标注，不混进完成度。

验证口径（所有行共用）：

- 参考实现是板上的 C MPP 构建（`/tmp/mpp-old` 2024-04 编码器，`/tmp/mpp-develop` 解码器），
  不是发行版库。
- 解码比较 planar 输出字节；编码比较整条 Annex-B/NAL 或逐 packet；IEP 比较原始 dst slot。
- 性能只在 CPU policy 与 `dmc` 固定、双方交错采样、取中位数时报告；百分比不稳时只写
  “不慢于 C”。
- 回归脚本在 `scripts/board_*.sh`，每条结论后的脚本名即复现入口。

## 解码

| 能力 | 状态 | 证据 / 边界 |
|---|---|---|
| JPEG（jpegd，RGB888/NV12，1/2–1/8 缩放） | 完成 | 与 `mpi_dec_test` 逐字节；比 C 快约 30% |
| H.264（rkvdec） | 完成 | frame/MBAFF/PAFF/长参考/错误路径逐字节；IDR 丢弃 DPB 引用但保留空闲帧池复用；Annex-B start-code 扫描用 `memchr` 跳零候选 |
| H.264 流式 CLI | 完成 | `h264_stream` 以 4 MiB 块增量扫描 Annex-B/AU，不再整文件读入；50 MiB/742 帧 RK3588 流热态 wall median 1.576 -> 1.595 s，峰值 RSS 55.9 -> 17.0 MiB。主 `h264dec` 复用 scanner 并保留 depth-3 ring，wall median 0.665 -> 0.665 s、RSS 59.5 -> 20.4 MiB。720p planar/NV12 哈希不变。SDK 同步在 SPS 到达后刷新 display reorder depth，并暴露 `Decoder::max_num_reorder_frames()` |
| H.265（rkvdec） | 完成 | Main/Main10/tiles/WPP/DSLICE/LTR/scaling list 逐字节；RK3576 的 16 格损坏矩阵中 14 格输出计数和 YUV 逐字节一致，损坏 SPS 与首个 IDR slice 起点两格由 Rust 及时拒绝而 C harness 超时；4:2:2/4:4:4 被硬件拒收（平台边界）；多 job ring 在 job 回收后复用 bitstream dma-buf；共享 H.264 的 start-code/EPB 扫描，整段 AU 定位使用 header-only NAL 扫描并提供 `decode_parsed_access_unit()` 直接消费 |
| VP9（rkvdec） | 完成 | profile 0/2、10-bit、lossless、tiles、errres、resize、show-existing、hidden alt-ref；边界两格由 libvpx 判定为 C bug |
| AV1（av1d） | 完成 | Main 8/10-bit、film grain、2x2 tile、96×96–1920×1080、长 GOP、多 sequence、320↔640 切换；C/Rust/libaom 三方逐字节；64×64 为 C bug |
| MPEG-2 / MPEG-4 / H.263 / VP8 | 完成 | 逐字节；隔行 MPEG-2 用 FFmpeg 作 oracle（C 会错误回绕输入）；VP8 profile 1/3 为派生流验证 |
| AVS+ / AVS2 | AVS+ 公开流完成，扩展进行中；AVS2 不可达 | RK3588 client 12 的公开 352×288 流已由 Rust sequence/PB parser、60-word HAL、两锚 DPB 和 `avsdec` 完成 7 帧 I/P/B 显示周期；关闭 C 默认 IEP2 vproc 后与 C 原始 I420 逐字节一致，SHA `47d05886…5fdb20`。C 默认后处理单帧为 `9b9a6dc9…39088b`。AVS+ profile 0x48、field picture 和 vproc 仍待完成。AVS2 在 RK3588 无设备 |
| H.265 SCC | 不存在 | MPP 源码没有 palette/SCC 路径 |

## 编码

编码吞吐与 C 的对比（60 帧、固定 QP、GOP 对齐、输出验证一致）见
[docs/ENCODE_BENCH.md](ENCODE_BENCH.md)：单路 H.264 快 39–86 %、H.265 快 6–39 %；
并发 8 路时 H.264 持平、H.265 也已追平（流式读入修复后 496 vs 493 FPS）。


| 能力 | 状态 | 证据 / 边界 |
|---|---|---|
| H.264（VEPU580） | 完成 | 固定 QP 24 格、P 帧 11 尺寸（含 FBC 阈值两侧）、CBR/VBR/AVBR、hier QP、SMART_P、two-pass deflicker、level、deblock、ROI/QP map、GDR 4/4、slice split 4/4、逐 slice 输出 5/5，全部逐字节或逐 job STATUS。GDR 用确定性 ROI 表对照：stock C 的未初始化栈字段会导致跨进程输出不同，修表后 C/Rust 35 NAL 全一致。输出 packet 使用 uncached DMA heap，省掉逐帧 cache-sync ioctl；500 帧 720p A/B 快 2.53%，码流不变 |
| H.265（VEPU580） | 完成 | IDR/P GOP、CBR/VBR/AVBR、GDR 4/4、ROI 4/4、deblock/SAO 4/4、BY_CTU/BY_BYTE 3/3、low-delay 4/4、水平 tiles 2/2。输出 packet 和 session CPU 输入都使用 uncached DMA heap；输入路径不再发每帧两次无效 sync。aggregate feedback 只重置 14 个实际 word，不再清 180-word STATUS。500 帧 720p 的 wall median 2.379s→1.318s，user+sys 1.287s→0.366s，码流哈希不变；输出 packet 的既有 A/B 仍快 2.41% |
| H.265 scene-tune（VEPU580 object path） | 2026-11-22 完成 | 这一行取代上一行的 14-word 表述。状态聚合保留 0、3、64–84 共 23 个 word，其中 77–84 是 `md_sad_b16num[4]`/`madi_b16num[4]`；H.265 使用 vendor 的 896/640/384 CTU MADP 门和完整 row-3 WGT/RDO 覆盖。320x240 300 帧跨 5 个 GOP 与 C **315 NAL、整文件 SHA-256 `d577adc9…32edea`** 完全一致；300 个 job 仅 13 个 dma-buf 地址字不同，非地址寄存器差异为 0。复跑：`BOARD=my-rk3588-host ./scripts/board_h265_scene_tune_300.sh` |
| H.264/H.265 IPC scene mode（VEPU580） | 2026-11-22 完成 | `MppEncCfg` 的 `tune:scene_mode` 0/1/2 已进入 typed encoder config；vendor 的 IPC 模式（1）会切到 row 0/1，IPC_PTZ（2）按参考实现仍走 default 分支。H.265 row 0/1 的 WGT/RDO/RC_KLUT 值由同一噪声 fixture 的 C trace 差分得出：160 个 job 中 WGT/RDO/OSD 与 C 逐字一致，row 0 在第 0 帧、row 1 在第 5 帧切入。H.264 的 IPC 模式另以 60 帧验证，64 NAL 一致。复跑：`board_h265_scene_mode_ipc.sh` / `board_h264_scene_mode_ipc.sh` |
| Encoder quantisation bias | 2026-11-22 完成（VEPU580） | `MppEncCfg` 的 `hw:qbias_en/i/p` 已进入 typed config。H.264 使用 C 的 fallback 683/341，H.265 使用 171/85；显式关闭时保留 scene row 的默认 bias。H.264 60 帧、64 NAL 与 C 一致；H.265 `en=1` 和 `en=0` 的 2 帧 trace 中 WGT/RDO 逐字一致，word1484 分别为 `0x190c8` 和 `0x15480`。复跑：`board_h264_qbias_vs_mpp.sh` / `board_h265_qbias_vs_mpp.sh` |
| H.265 VMAF tune | 2026-11-22 完成（VEPU580） | `tune:vmaf_opt` 只覆盖 pre-intra/ATF row 3 与 RDO segment/smear：WGT 1479–1482 置 `0x10101010`，RDO 2048 清 0，2065/2095 置 `0x10101010`，其余 13 个 RDO word 置 `0x00101010`；row3 的 2167/2169 保持默认。2 帧 C/R trace 的 WGT/RDO 逐字一致。复跑：`board_h265_vmaf_opt_vs_mpp.sh` |
| H.265 native CLI tune | 2026-11-22 完成（VEPU580） | 原生 `h265-gop` 支持 `H265_SCENE_MODE`、`H265_QBIAS_EN/I/P`、`H265_VMAF_OPT`。scene/qbias/vmaf 三格各 2 帧，与 C probe 的 WGT/RDO 逐字一致；scene 格的 OSD 与 `RC_KLUT[1056]` 也一致。复跑：`board_h265_cli_tune_trace.sh` |
| H.265 object throughput | 2026-11-22 完成（RK3588 80% OPP） | 新版 `mpi_enc_test` 的 300 帧、多 GOP、fixed-QP26 路径在 CPU6 交错 5 次：320x240 C/Rust wall 164/164 ms、user+sys 69/67 ms；1280x720 wall 868/802 ms、user+sys 291/226 ms。320x240 为 wall 持平，720p Rust 快 7.6%，CPU 少 22.3%。脚本把 wall 和 user+sys 都限制在 C 的 105% 内：`BOARD=my-rk3588-host SAMPLES=5 ./scripts/board_bench_h265_object_vs_mpp.sh` |
| H.264 object throughput | 2026-11-22 完成（RK3588 80% OPP） | 同一通用 object benchmark 的 300 帧、CPU6、交错 5 次结果：320x240 C/Rust wall 152/157 ms（Rust 慢 3.3%，在 5% 门内），user+sys 67/67 ms；1280x720 wall 822/769 ms（Rust 快 6.4%），user+sys 278/232 ms（少 16.5%）。`board_bench_object_encode_vs_mpp.sh` 对 H.264/H.265 共用 wall 和 CPU 双重门槛 |
| 320x240 steady-state encode | 2026-11-22 完成（RK3588 80% OPP） | 1000 帧、7 次交错：H.264 Rust/C wall 448/493 ms、user+sys 138/202 ms；H.265 457/484 ms、140/185 ms。Rust 分别快 9.1%/5.6%，CPU 少 31.7%/24.3%。300 帧 H.264 的小幅落后属于进程启动占比；通用脚本支持 `SIZES`/`FRAMES` |
| JPEG（vepu） | 完成 | baseline 4:2:0、restart、90/270 旋转、RGB/YUV 输入、低延迟 slices、RK3588 四 band 多核，逐字节/逐 packet |
| WPP / B 帧 / 多参考 | 不存在 | C 参考没有实现（标志恒 0、无 entry point、无 B_FRAME、`num_ref = 1`），不是本 crate 的缺口 |
| harness `-g 60` | BY-DESIGN | harness 解析成 `gop_len=0` 的病态 smart-GOP；本 crate 拒绝 `gop=0`，有效形式逐字节 |
| 同进程多会话 | 完成 | 1/2/3 线程在 64×64/320×240/640×480 产出一致且与 C 相同；旧的“每进程单会话”说法已撤回 |

## 后处理与框架

| 能力 | 状态 | 证据 / 边界 |
|---|---|---|
| RGB 缩放 / letterbox | 完成 | 自研 RGB 实现比 RGA 快 2.9–3.8×；`Frame::letterbox_nv12()` 直接读取 CPU/DMA-BUF 的 strided NV12 plane，在缩放中转换 BT.601 RGB，省掉整帧 NV12→RGB 中间副本。真实 H.264 720p CPU/DMA frame 输出逐字节一致；1080p→640 在 RK3588/RK3576 为 4.93/5.95 ms |
| IEP2 模式 | 完成 | I5O2/I5O1T/B、I2O2、I1O1T/B、DISABLE、BYPASS 9/9，含 TFF/BFF 与 1280×720，原始双 dst slot |
| IEP2 格式 | 完成 | 420/422 × SP/Planar × UV/VU 6/6 |
| IEP2 PD/DECT 逆 telecine | 完成 | 场序 + pulldown 状态机；telecine/逐行 + 322×242/722×482/1366×768 共 5/5，逐字节且逐帧状态相同 |
| 解码 Frame → IEP2 零拷贝 | 完成 | `IepDeinterlace::import_src_frame()` dup 解码 NV12 dma-buf，校验单 fd、Y/UV offset、horizontal/vertical stride 后直接作为三张源图；RK3588 的 rkvdec H.264 DMA frame→I5O2 板级测试通过。新增 50×34 解码流（H stride 64、V stride 48）将零拷贝导入与显式 padded 源对比，两路 dst 输出逐字节一致 |
| VDPP2 client 29 | 完成 | RK3576 `common`、ZME scale/dering/blend、四组缩放系数、可替换的自定义 11×17×8 tap8/tap6 bank、DCI histogram、默认 DMSR/ES/sharp register bank、全部 DMSR/ES/SHARP 输入参数，以及 YUV420/YUV444 分离 Y/UV 输出已移植。官方 Luckfox SDK 250301 的精确内核 `b16c5d1a69947ec66726165407a8aa75368def53` 用同版 GCC 10.3.1 重建，板 config 只改动 `CONFIG_ROCKCHIP_MPP_VDPP=y`，并保留原 FIT 的外部数据布局和 `sha256,rsa2048:dev` 签名声明。RK3576 已成功启动新内核，`mpp_vdpp 27961000.vdpp` 绑定 client 29，`probe_hw_support` bit 29 为真。真机 C/Rust 对比 5/5 逐字节通过：scale-only、默认 enhancement、YUV420 split、YUV444 split，以及 10,240-byte packed histogram（4096 local + 256 global，mean luma U10 513）。 |
| 8/10-bit 后处理转换 | 完成 | `Frame::convert_format()` 支持 8-bit NV12/I420 与 planar I420P10 双向转换、NV12 chroma 解交织/回打包，保留 PTS/DTS/POC/error info；8↔10 在 8-bit 输入域无损往返 |
| VDPP（client 29） | 不存在 | RK3588 无该硬件；硬件 scale/rotate/CSC/histogram/HDR 路径不可验证 |
| HDR tone mapping | 平台不可用 | RK3588 无 VDPP/tone-mapped CSC；没有 board oracle，不提供伪装成硬件等价的软件路径 |
| legacy `/dev/iep` scale | 不可验证 | RK3588 没有该节点；IEP2 只有去隔行，通用缩放使用自研 NV12/RGB scaler |
| 异步任务队列 / 缓冲池 | 完成 | 回调、等待、事件、逐任务 queue/run/turnaround timing、累计成功/失败/取消与队列高水位统计、引用计数 dma-heap 池；VP8 与 H.264/HEVC/AV1/VP9 DPB surface 均持有 lease；RK3588 async demo、H.264/HEVC/AV1/VP9 池化边界测试全过 |
| 硬件错误恢复 | 完成（W5-8） | submit/poll 故障全部走 close-before-open 会话替换，不调用会污染 RK3588 的 `RESET_SESSION`。`board_framework_recovery.sh` 4/4：16 次替换、H.264 重开后逐字节复用、VP8 截断 packet 的真实 STATUS 错误，以及 H.265 tile 恢复。H.265 项用无效 input fd 触发真实 `int_sta=0`，重开后清空 tile line buffer、重新执行丢弃预跑，并强制下一帧为带参数集 IDR；恢复码流与全新会话逐字节一致，RC reencode callback 不能把该 IDR 降回 P |
| 多核 CCU 批量提交 | 完成（W5-6） | `CcuTopology` 读取设备树的 vdec/venc/jpeg CCU 节点；`BatchServer` 以 `MPP_CMD_SET_SESSION_FD` 分隔最多 8 个 session。RK3588 的两条独立 VEPU2 JPEG session、RK3576 的两条独立 VEPU510 H.264 session 都在一个批次中触发，之后按真实 session poll，输出均与各自单会话逐字节一致；两板各 3/3。当前 6.1 驱动的批量 wait 不写每条 `mpp_bat_msg.ret`，因此只批量提交、单独回收；无 CCU/命令时自动回退 |
| 对外解码 SDK | H.264/H.265/VP9/AV1/MJPEG V1.3 完成 | `include/rk_mpp_rs.h` 与 `rk_mpp_rs::sdk`；共享/静态库、C/C++ 链接、分块或 packet 输入、flush/reset/recover/status/EOS、显式 frame release。RK3576 与 RK3588 上 H.264/MJPEG NV12/I420、H.265/VP9 8-bit NV12/I420 及 10-bit I420P10、AV1 8-bit 与 VDPU383 10-bit 均与对应 oracle 逐字节；RK3588 AV1 10-bit 按已登记 film-grain C bug 与 Rust/libaom 对齐。新 V1 暂不含 RGB888，旧 `rk_jpeg_*` 保留 |
| VP8 解码公共接口 | V1.7 完成 | 同一 decoder handle、一 packet 一 coded picture、NV12/I420、EOS/reset/release；RK3588 12 帧 alt-ref 输出与 C 逐字节。RK3576 无 VP8 decoder，RK3576 明确 SKIP |
| MPEG-2 / MPEG-4 / H.263 / AVS+ 公共接口 | 完成 | 同一 decoder handle、任意分块 elementary stream、NV12/I420、EOS/reset/release；AVS+ 跨 packet 保留 sequence/version、两锚 DPB 和 `vec_flag`，只消费完整 access unit；MPEG-2/MPEG-4/H.263 普通 SDK 在 EOS/flush 统一产出。MPP object 兼容路径另启用增量解析：MPEG-2 按 picture group、MPEG-4 持久 VOL/timing 状态、H.263 持久 I-frame gate。RK3588 AVS+ CLI 与 SDK 的 7 帧 I/P/B cycle 均和 C 原始输出逐字节；RK3576 无 VDPU2，明确 SKIP |
| 对外编码 SDK | H.264 V1.4 完成 | 同步 submit/receive、NV12/I420 输入复制与 coded geometry padding、owned packet/release、flush/reset。RK3588 VEPU580 支持 baseline/main/high 与通用 level/GOP/QP/fps；RK3576 VEPU510 当前限定 320×240 Baseline QP26 GOP6 @30fps。RK3576/RK3588 六帧输出与现有 CLI oracle 逐字节 |
| H.265 编码公共接口 | V1.5 完成 | 同一 encoder handle 支持 NV12/I420、owned packet/release、flush/reset；RK3588 VEPU580 开放 width/height、QP、GOP，RK3576 VEPU510 限定板验 320×240 Main QP26 GOP6 @30fps。RK3576/RK3588 六帧输出与对应 CLI oracle 逐字节 |
| 原生 VEPU580 文件编码队列 | 完成（H.265 默认 / H.264 opt-in） | H.265 固定 QP 在无 refresh/tiles/low-delay split 时默认 depth 2，300 帧 720p 为 0.430 s / 698 FPS，320x240/1280x720 与同步路径逐字节。H.264 默认同步；显式 depth 2 为 0.331 s / 906 FPS，码流可完整解码，但 scene-tune 统计晚一帧，不承诺与同步码流逐字节 |
| 编码器 DMA-BUF 输入 | V1.12 完成 | 保留 copied-input API，新增 `DmaBufInput<'a>` 和 `rk_mpp_encoder_submit_frame_dmabuf()`：NV12 fd、allocation 长度与 coded Y/UV stride 直接提交到 VEPU580/VEPU510/VPU720。H.264/H.265 要求 16 对齐，JPEG 要求 8×2；提交同步完成后 surface 可立即复用。`DmaBufInput` 借用原始 `BorrowedFd`，生命周期覆盖同步提交；不 dup、不 mmap。真实 VEPU580 测试发现 dup fd 会 `int_sta=0`，借用原 fd 正常，C ABI 亦然。RK3588 6 帧 `strace` 的 `F_DUPFD_CLOEXEC` 为 0。Rust API 可用 `with_osd()` 附单帧 OSD；硬件门对 H.264/H.265 的 copied/dma-buf、无 OSD/有 OSD 四路逐字节一致，VEPU510/JPEG 明确拒绝。compat 对象无 custom RC 且几何匹配时直接借用 fd；I420/NV21 或需要对象层处理时自动回退。VEPU580 OSD 的 `board_mpp_osd.sh` 三路 H.264 6 NAL、H.265 7 NAL 与 YUV 一致；cached DRM 的 500 帧 direct 呈双峰且 median 1.795s 慢于 fallback 1.428s，因此默认 fallback，`RKMPP_COMPAT_OSD_DMABUF=1` opt-in。RK3588 80% OPP、120 帧 720p 的 H.264/H.265 普通直交与 CPU fallback 分别保持 126/125 NAL 一致；H.264 9 次交错采样 wall median 273 vs 311 ms，user+sys median 59 vs 95 ms |
| JPEG 编码公共接口 | V1.6 完成 | 同一 encoder handle 支持 NV12/I420、0–10 量化档、可选 restart MCU；VEPU2/VPU720 自动选择。RK3576/RK3588 单帧 JPEG 与 `jpeg-enc` 逐字节 |
| SDK 安装与 ABI | 完成 | Linux SONAME `librk_mpp_rs.so.1`、版本化 `.so.1.14.0`、static/rlib、pkg-config、relocatable CMake shared/static target、ABI 符号和 C11 layout 快照；RK3576/RK3588 临时 prefix 安装及外部 consumer 编译运行 |
| 高级编码配置 ABI | V1.9 完成（VEPU580 / VEPU510） | 原 68-byte encoder config 和旧 create 路径不变；独立 advanced config 暴露 Fixed-QP/CBR/VBR/AVBR、目标/最小/最大码率与 QP 范围。RK3588 VEPU580 与 RK3576 VEPU510 的 H.264/H.265 三种 RC 均逐帧驱动会话；两板各 6 组 C API 输出与 CLI 一致，RK3576 的 2 帧和 6 帧矩阵还与 vendor 逐 stream/YUV/metadata 一致 |
| ROI / GDR / slice ABI | V1.10 完成（VEPU580） | 独立 V2 advanced config 保留 V1.9 布局，最多 8 个 ROI，支持 GDR row/column、BY_CTU/BY_BYTE slice、完整 AU 或 low-delay packet 输出。RK3588 H.264/H.265 的 ROI/GDR/slice 六组 C API 输出与既有 CLI 逐字节；V1.9 advanced consumer 可链接 V1.10。RK3576 VEPU510 streaming 控制暂明确 SKIP |
| H.264 syntax/deblock ABI | V1.13 完成 | 独立 V3 advanced config 保留 V1.10 的 ROI/GDR/slice 布局，并新增 H.264 deblock、CABAC、transform-8x8、`cabac_init_idc`、constraint-set force/value 与 `base_layer_pid`。syntax flags 逐字段生效；未选择项保持 profile 默认。C、Rust FFI 和 `rk-mpp-rs-sys` 均覆盖两个 initializer 与 V3 create 入口。RK3588 C V3 与已验证 CLI 逐字节：CABAC idc 2 为 18,046 bytes、constraint `0x003f0025` 为 17,977、base PID 5 为 17,977、Main 强制 CAVLC 为 20,263、High 关闭 8x8 为 17,979、deblock `1:-2:3` 为 18,098；旧 V1.9 consumer 与 V1.10 六格 ROI/GDR/slice 继续通过 |
| H.265 syntax ABI | V1.14 完成 | V3 末尾 32-byte reserve 升级为字段选择性 H.265 syntax descriptor，不改变 552-byte layout。C/Rust 可覆盖 SAO luma/chroma、deblock disable/beta/tc、loop-filter across slices/tiles 与 auto-tile，未选择项保留默认。RK3588 C V3 与 CLI 逐字节：SAO disable 15,618 bytes、deblock beta/tc 15,878、loop-filter 15,829，4608x64 auto-tile 为 8,876 |
| Rust raw FFI | 完成 | workspace crate `rk-mpp-rs-sys` 手写覆盖 V1.14 全部 41 个导出、enum、struct 和不透明句柄；ABI size/offset 测试、dynamic/static/pkg-config smoke 在 RK3588/RK3576 通过 |
| H.264/H.265/VP9/AV1/MJPEG DMA-BUF frame | V1.11 完成 | 默认 frame 仍为连续 CPU 存储；设置 `RK_MPP_DECODER_FLAG_DMABUF_OUTPUT` 后 H.264、8-bit 4:2:0 H.265/VP9/AV1 与 MJPEG NV12 frame 保留硬件 dma-buf。H.264 在 display reorder 中额外持有 DPB lease；H.265/VP9 将池化 video surface 租约随 `Picture` 保存；AV1 在 VDPU34x 上复制 display lease，在 VDPU383 上把 output buffer 改为引用计数池并跳过 DMA opt-in 时的 CPU readback。VP9/AV1 的 `show_existing_frame` 同样保持 surface 生命周期。Rust 暴露 borrowed plane metadata 与 owned fd duplicate，C 暴露 `rk_mpp_decoder_export_frame_dmabuf()`。Y/UV 的 fd/offset/size/stride 由 C consumer 和 `fstat` 校验；H.265 Main10/422/444、VP9/AV1 10-bit 及 CPU-frame codec 明确返回 unsupported |
| 官方 MPP drop-in 基础层 | 进行中 | 官方 `rk_mpi.h` consumer 已覆盖生命周期、184-byte `MppApi`、packet/frame/buffer、H.264/H.265/MJPEG CPU NV12 解码，以及 vendor `MppEncCfg`→`encode_put_frame`→`encode_get_packet` 基础闭环。本地 `MppMeta` 支持 s32/s64/ptr、frame/packet/buffer、`KEY_USER_DATA(S)` 深拷贝、duplicate/ref/lifetime 和 frame/packet 惰性持有；RK3588/RK3576 专用 C consumer 均通过，RK3588 原生 `mpp_meta_test` 2 线程门也通过。advanced decode 已把 `KEY_OUTPUT_FRAME` 与 `KEY_INPUT_PACKET` 接通；H.264/H.265/VP9/AV1/MJPEG 使用 DMA-BUF decoder frame。H.264/H.265/VP9/AV1/MJPEG 支持把调用方 dma-buf 注入 output surface，五者的 advanced C 输出均已逐字节验证；VP9 consumer 按 IVF frame header 拆 packet、保留真实 256-odd stride/9/5 容量，并避免隐藏参考帧占用调用方 `show_frame` surface；AV1 在 VDPU34x display pool 与 VDPU383 output/reference pool 上注入 coded show frame，`show_existing` 从历史 display surface 复制。internal `MppBuffer` 现在由真实 dma-heap fd 支撑；外部 `MppBuffer` 的 pointer/fd import、external group commit/get、fd-only mmap、unused recycle 和 usage 统计已接入，调用池 import 会复用 `clear()` 产生的空 slot。外部 fd-only `MppBuffer` 采用按需 mmap：pointer/info/read/write 或 CPU fallback 首次访问时才建立映射，DMA 直通不消耗 CPU 映射；standalone 解码替换已闭合：未修改的 `mpi_dec_test` 只需把 `librk_mpp_rs.so` 作为 `librockchip_mpp.so.1` 加载，不再需要 vendor log/cfg/OSAL 库；RK3588/RK3576 的 H.264/H.265/VP9/AV1/MJPEG 输出全部逐字节一致。同一 standalone 库也提供 key/value `MppEncCfg`，未修改的 `mpi_enc_test` 可完成单帧 H.264/H.265/MJPEG：H.264/H.265 重建与 vendor 逐字节一致，MJPEG 码流逐字节一致。`MPP_SET_VENC_INIT_KCFG` 已消费上游 init keys，映射 type/coding/max dimensions，拒绝 unknown key；完整 KMPP object ABI 仍是边界。`MppEncRefCfg` 的 create/reset/LT/ST add/check 与 `MPP_ENC_SET_REF_CFG` 已接通；LOG/JSON/TOML 三种 cfg-string apply/apply_file/extract 已实现，LOG/JSON 与 vendor 双向通过，TOML 与 vendor 提取逐字节相同且 Rust 可回读，而 vendor 连自己的 TOML 也无法解析（C-BUG）。VEPU580 和 VEPU510 都会解析旧路径藏在 `MppEncCfg` 的 `"rc:ref_cfg"` 指针，把 `-g 1:60`、`-g 2:60`、`-g 3:60` 的 TSVC2/TSVC3/TSVC4 映射到逐帧槽位、`nal_ref_idc`、frame_num、POC、type-14 prefix、ST/LT reorder 和 MMCO 标记。RK3588 VEPU580 的 TSVC2 六帧 16 NAL、TSVC2/TSVC3/TSVC4 12 帧 28 NAL 均逐字节一致（YUV `fec69971…64f0a4`、`d9a3ed55…ecad7e63`、`7794eb88…488f8e2`、`33923dd2…d1186a`）；TSVC4 64 帧跨 `gop=60` 的 136 NAL 也一致（YUV `a3ad1bc6…35a0593`）。RK3576 VEPU510 的 TSVC2 12 帧为 26 NAL/YUV `77a92442…fe35ab` 逐字节；TSVC2/TSVC3/TSVC4 的 C packet assembly 冷启动均不稳定，可能出现漏 prefix，TSVC3 还出现过 18 NAL、零帧解码，稳定输出才与 Rust 一致；板级门重试到稳定 oracle。H.264 `MPP_ENC_SET_IDR_FRAME`、per-IDR header mode 和 SEI disable 已接通；帧 2/4 强制 IDR 在 RK3588/RK3576 均为 12 NAL/YUV 逐字节一致。H.265 VEPU580 的同一控制也已接通，强制 IDR 为 15 NAL/YUV `b2765277…3c56607`，并在每次 IDR 后重置 POC。VEPU510 的 MPP object 路径也达到完整 15 NAL/YUV 逐字节一致（`7f3d4664…841231`）；`MPP_ENC_GET_HDR_SYNC` 现在按调用方是否请求 header sync 选择 direct 或 mpi_enc_test template，RK3576 kmpp_objdef_get/register ABI 也已修正。日志入口用 Rust naked ABI trampoline 保留 variadic 寄存器并尾跳到本地 C `vfprintf` helper，官方工具日志已实际展开参数。RC API 的查询、注册/替换、类型过滤和当前策略选择已与原厂逐行一致，并导出同注册表的 rc_api_add/rc_brief_get_all/rc_brief_get_by_type；非 default 策略的逐帧 callbacks 已在 RK3588 VEPU580 和 RK3576 VEPU510 的 H.264/H.265 fixed-QP/CBR/VBR/AVBR 中真实驱动硬件；两块板的 8/8 两帧 stream/YUV、完整 callback 字段、init 配置、硬件 bits 和 real QP 均与兼容 ABI 的 vendor 逐行一致；callback 主动请求的同帧型和 P→IDR hardware reencode 也已在两板 H.264/H.265 共 8/8 通过：首轮 packet 被丢弃，QP30 重跑同一输入与参考槽；H.265 复刻 vendor 的 P syntax + INTRA config + IDR NAL 混合模板；该 vendor 流是 C-BUG，FFmpeg 只能解出前两帧而 Rust 与完整流逐字节一致；H.264 保持 vendor 实际 P slice；组合 drop/PSKIP 仍未实现。OSD palette 与 KEY_OSD_DATA 已接入 VEPU580：H.264 和 H.265 的 OSD stream/YUV 均逐字节；H.265 修复了 OSD multi-offset 被后续 FBC/output offset 覆盖的问题。encoder packet meta 已接通；其余高级编码控制仍待完成。RK3588/RK3576 的 vendor `mpi_enc_test` 也都跑通 H.264、H.265 和 MJPEG。H.264 单帧、默认 6 帧、RK3588 TSVC2/TSVC3/TSVC4 和 RK3576 TSVC2 的 NAL/重建一致（仅 version-SEI 文本不同），H.265 单帧和 6 帧、MJPEG stream 逐字节一致。RK3576 的 High CABAC/8x8 模板、MEIW buffer、GOP60 和 `mpi_enc_test` 专用 H.265 寄存器/参数集已接入；`KEY_MOTION_INFO` 的 1200/1280-byte 调用方 buffer 也逐字节回传为 C 结果 |
| 官方 object-path 默认值与 H.265 SEI 边界 | 2026-11-21 复验完成 | 这一行补充并修正上一行末尾的 H.265 “整条 stream 逐字节”旧表述。FFmpeg 显式写 `h265:level=0`，参考按尺寸得到 2.0 并启用 temporal MVP；未修改的 `mpi_enc_test` 不写该 key，保留 `h265e_init` 的 4.0 和 TMVP off。VEPU580 缺省为 120，VEPU510 缺省为 30。RK3588 的 H.264/H.265 单帧和 6 帧参数集、rc-SEI、VCL 与解码重建一致；version-SEI 只比较 framing。standalone 解码 5/5、standalone 编码矩阵（含 TSVC2/3/4 与 64 帧 IGOP 重启）、官方编码门和 FFmpeg 8/8 均通过 |
| Legacy standalone decoder matrix | 2026-11-22 完成（RK3588） | 这一行取代上一行的 standalone 解码 5/5 计数。未修改的 `mpi_dec_test` 默认以 4 KiB packet 喂普通 elementary stream；MPP object 兼容路径使用持久 parser state 增量消费完整 picture/VOP，普通 SDK 仍按 whole-stream/EOS 解码。RK3588 为 13/13：H.264、H.265、VP8 IP/altref、VP9、MPEG-2、MPEG-4 IP/IPBB、H.263 CIF/QCIF、AV1、MJPEG，以及一个不带 `-n` 的 MPEG-2 EOS 格，均通过精确 NV12 字节数和 vendor 逐字节比较。100/96 帧相对基准中，MPEG-2/MPEG-4/H.263 比上一版 replay 快约 18–19×/19–23×/53–56×，当前与 vendor C 的差距为 0.02–0.04 s（DMC performance、CPU 4–7；非 80% OPP 绝对性能声明）。RK3576 无 VDPU2/VP8，本矩阵仅适用于 RK3588 |
| Legacy object CPU output | 2026-11-22 完成（RK3588） | compat legacy 帧先保留 planar I420，object 层直接写入普通 CPU `MppBuffer` 并在同一 pass 交织 UV，不再先 `i420_to_nv12` 再复制到新 DMA heap。100 帧 MPEG-2 的 Rust task-clock 50.1 ms、instructions 75.4M；vendor C 为 66.5 ms、80.0M。优化后的 perf 样本已无 Rust 用户态热点，主要落在文件写出和进程启动；wall 0.13 s 对 vendor 0.09–0.11 s 的剩余差异属于 I/O、启动和同步噪声 |
| MPP option trie / KMPP userspace object | 2026-11-22 部分完成 | option trie 同时支持 `0x47` 旧四参数 setup/二参数 add 的直接 `MppOptInfo` 上下文，以及 `0x20` 三参数 add 的 24-byte `MppTrieInfo v0` `{name, ctx, index, str_len}`；通用 object trie 保留当前 packed 4-byte `MppTrieInfo`。上游 `mpp-develop/build/test/mpi_enc_test` 现可用 Rust 库启动并运行默认 MPP 路径：兼容层实现 dual-ABI `kmpp_objdef_get/register`、trie VLA/叶子、用户态对象分配、entry 偏移读写、flag 与 init/deinit callback。`-kmpp_mode 0` H.264 单帧 3 NAL 与新版 vendor 完全一致（`e18fe707…14bdbd`）；H.265 补齐 full-range、CU-QP-delta、AQ/ROI/MD 和 QP-bias 默认值后，4 NAL 也完全一致（`119bbf79…caa76d5`）。60 帧单 GOP 的 H.264 62 NAL（`6a8cb407…3e3544`）和 H.265 63 NAL（`7eed6267…138b42`）均哈希一致，寄存器 trace 只剩 dma-buf 地址差异。OSD、ROI 和 `user_data_enable` 已通过 MppMeta 接入，2 帧 user-data 为 H.264 10 NAL、H.265 11 NAL，均完全一致。userspace `KmppBuffer` 已实现分配、setup、`KmppShmPtr` 导出和释放。KMPP mode 1/2 因内核无 `/dev/vcodec`、`/proc/kmpp` ctrl-cfg 能力被工具拒绝；kernel buffer-group ioctl 仍未实现 |
| FFmpeg RKMPP drop-in | 编码完成（H.264/H.265/MJPEG） | 未修改的 FFmpeg `c5e745e3c1` 可通过 `LD_LIBRARY_PATH` 把 `librockchip_mpp.so.1` 解析到 Rust 库，H.264/H.265/VP9 解码逐字节一致。MPP 默认 DRM group 按 vendor 语义分配 `system-uncached`；FFmpeg 首帧第二次 `MPP_ENC_SET_CFG` 保留 SEI/per-IDR header 控制。H.264/H.265 在 320x240 和 1280x720 的 12 帧及 60 帧单 GOP 序列均与 vendor C 整条码流逐字节一致，MJPEG 也逐字节；120 帧/GOP60 跨第二个 IDR 后，C 跨进程会产生多个码流并解码出不同 YUV，不能作为稳定 oracle，Rust 则保持确定性。H.265 补通配置 level、slice temporal MVP、collocated/colmv 寄存器及 GOP 边界 IDR；H.264 compat 路径按缺少 `KEY_MOTION_INFO` 的 FFmpeg 调用关闭 `mei_stor/meiw_addr` 并将 scene-tune 反馈延迟一帧，匹配 vendor async encoder。VEPU580 H.264/H.265 固定 QP path 在 session 内默认使用 depth-2 deferred queue：pending batch/STATUS/output 独立持有，三槽 recon 分别按 C 的轮转规则使用，blocking `POLL_HW_FINISH` 保证队列 readback 完成。H.265 另复刻 C `task_cnt == 2` 的 `reg0193_dual_core` 跨任务握手，修复此前长 GOP 第 41 帧附近的数据竞争。300 帧 720p 中，H.264 为 0.334 s / 898 FPS（C 0.349 s / 860 FPS）；H.265 为 0.444 s / 676 FPS（C 0.444 s / 676 FPS），CPU user+sys 0.372 s 对 C 0.393 s。`board_ffmpeg_rkmpp.sh` 同时要求 H.264/H.265/MJPEG 的 DMA-BUF 直通与 CPU fallback 逐字节一致 |
| `MppEncCfg` 字符串配置 | 完成（标量 + 已知数组） | LOG/JSON/TOML `extract/apply/apply_file` 已真实读写本层数值配置，不再返回空串或假成功；普通 EncCfg 与 `MPP_VENC_KCFG_TYPE_ST_CFG` 共用同一访问器。`rc:hier_qp_delta`、`rc:hier_frame_num`、`hw:aq_step_i`、`hw:qbias_arr` 可经 `set_st/get_st` 往返；该数组路径也覆盖 ST_CFG 句柄。`mpp_enc_cfg_deinit()` 同时释放普通 EncCfg 和 ST_CFG。H.264 的 `hier_qp_en/delta/frame_num` 已进入 RC 模型，`hw:aq_step_i` 已进入 VEPU580 I 帧 AQ 表；`rc:refresh_en/mode/num` 已映射到 H.264/H.265 typed GDR，`h264:dblk_*`/`cabac_en`/`trans8x8`/`cabac_idc`/`constraint_set`/`base_layer_pid` 已映射到 typed deblock/syntax，H.265 deblock/SAO/loop-filter/auto-tile 已映射到 typed syntax，`prep:format=YUV420P/NV21` 已分别接通 H.264/H.265 的 I420 planar 与 NV21 VU 交换；max-reencode、super-frame、priority、drop 与 debreath 标量也已进入运行时。层级 QP、自定义 AQ、debreath、super-reencode、drop/PSKIP、GDR、H.264 deblock/syntax/cabac_idc/constraint_set/base_layer_pid、H.265 syntax、I420/NV21 input 的 object-path 码流均与对应 CLI/vendor 路径一致。未修改的 vendor `mpp_enc_cfg_test` 在 RK3588 为 9/9 success，另有一组 ST_CFG LOG/JSON/TOML scalar+array 板级往返探针、H.264/H.265 GDR、H.264 deblock/syntax/cabac_idc/constraint_set/base_layer_pid、H.265 syntax 与双编码器 NV12/I420/NV21 对象映射探针通过 |
| vdec/venc kcfg 句柄兼容 | 完成（drop-in 存储面） | 非法 type 返回 `MPP_ERR_VALUE`；`init_by_name` 只接受 vendor canonical 名称；`CTRL_CFG` 初始化 `cmd/val/arg/ret`；flex base/set 只允许 CTRL_CFG，并记录 `flags=KMPP_CTRL_FLAG_FLEX` 与 size。`MPP_VDEC_KCFG_TYPE_INIT` 属于 KMPP object API，当前 MPI 没有对应 control command，因此解码键不伪装为 `DecoderConfig` 映射。RK3588 扩展 probe 与官方头文件 C migration consumer 均通过 |
| Encoder packet meta | 完成（VEPU580 / VEPU510 / JPEG HAL） | 按 `set_enc_info_to_packet()` 回填 H.264/H.265 的 8 类 block count、SSE、PSKIP、intra、start/average QP 与 H.265 `KEY_MOTION_INFO`。RK3588 VEPU580 fixed-QP/CBR 静止/运动 8 组；RK3576 VEPU510 fixed-QP 静止/运动 4 组；RK3588/RK3576 JPEG 各 1 组，均逐 packet 一致。JPEG 回填 q-factor quality，VEPU2 保持无 intra 标志，VPU720 写 `intra=0`；VEPU510 CBR 仍明确 SKIP。 |
| fence / secure | 受内核/TA 限制 | 该内核协议没有 fence；secure 无 TA/能力查询接口，未伪造 |
| RK3576 平台适配 | 进行中 | VDPU383 H.264 IPPP + 5/5 边界；H.265 12/12（含 500 帧 LTR SPS 与删 AU 缺失参考）并新增 16 格损坏矩阵：14 格 C/Rust 输出一致，损坏 SPS/首个 IDR slice 起点两格 Rust 及时结束而 C 超时；VP9 初始 7/7、VPU720 JPEG 19/19、VEPU510 H.264/H.265 功能、RC、ROI、slice、GDR 矩阵和 VDPP2 真机 scale/split/histogram 均板上通过。VDPU383 AV1 已完成 8-bit 10 帧引用序列和 10-bit skip-grain 序列：key/inter、`show_existing_frame`、八槽 output/CDF/colmv、primary-reference coefficient-CDF、704-byte 10-bit stride 与 packed-NV12 → planar u16 均接通；输出 SHA-256 分别为 `9d339ae3c5b270ac9059548da0d3ff5ef6cb0c94a9bcb11f5bfc93783e868138` 和 `f3b69b61a06d3967a89b91f11f56883588df53974d4a1b5efb8fbd9e004d49b8`。VDPU383 FGS enable/base 会以 STATUS 0 超时并复位，C HAL 也未启用 film grain。AVS2 与 VPU720 tile input 待完成 |

## 软件参考解码器（M11，自研 ground truth）

| 部分 | 状态 |
|---|---|
| H.264 CAVLC I-slice（预测/变换/DC Hadamard/deblock） | 完成，与原始合成源逐像素 |
| H.264 CABAC（I16/I4x4/I8x8/I_PCM/mb_qp_delta/混合宏块） | 完成；P-slice `P_Skip`、零运动/整数/分数 `P_L0_16x16`、`P_L0_L0_16x8` / `P_L0_L0_8x16`、`P_8x8`、explicit luma/双 chroma weighted `P_L0_16x16`、P deblocking、双参考 P16、B16 `L0/L1/BI`、spatial/temporal `B_Direct_16x16`/`B_Skip`、`B_L0_L0_16x8`/`B_L0_L0_8x16`、L0/L1/BI `B_8x8` 8x8 子分区和 mixed L0 8x4/4x8/4x4 已与 FFmpeg 逐像素闭合。P16 覆盖 mb_type、有符号 MVD 与绝对幅值邻居缓存、MVP 传播、CBP、QP、inter 4×4 residual、quarter-pel luma 六抽头（含 `(2,2)` 二维路径）、1/8-sample chroma 插值和边界扩展；分区门覆盖 scan-order A/B/C MVP、双 MVD context、上下/左右独立 MC 与共享宏块 residual；P_8x8 门覆盖四种 sub_mb_type、4×4 MVD cache/context 和逐子分区 luma/chroma MC；weighted 门覆盖 scale/round/clip、三平面 offset 和 CABAC 字节对齐边界；P deblock 覆盖 coeff/MV/ref 边界强度、QP average 与三平面滤波；多参考门覆盖 ref_idx unary、逐 4×4 reference cache、参考相关 MC 与 MVP fallback；B16 门覆盖 L0/L1 独立 cache、双 MVD 顺序、BI temporary prediction 与规范平均；direct/skip 门覆盖 skip 与 type 的独立邻域上下文、spatial A/B/C 参考与 MV 选择、POC distance scaling、colocated P motion map、逐 list direct MC、编码 direct residual 和零 residual skip；B 16x8/8x16 门覆盖 CABAC mb_type 前缀、顺序双 MVD、逐分区 MC 和共享 residual；B_8x8 门覆盖 mb_type 15、完整 sub_mb_type tree、L0/L1/BI 8x8 与 mixed L0 8x4/4x8/4x4 的逐列表 MVD、临时双预测、逐叶 MC 和共享 residual。过程中修正 inter CBF 不可用邻块规则、MVD bypass 符号、MVD 幅值缓存及跨字节 header 对齐；weighted P_Skip、col-zero B reference、mixed-list/BI B 宏块分区、direct 与 L1/BI 非 8x8 B_8x8、weighted/deblock B 未做 |
| HEVC 变换 | 完成 |
| AV1 符号解码器（规范 §8.2） | 完成：init_symbol/read_symbol/CDF 自适应/read_bool/read_literal/exit_symbol |
| AV1 默认 CDF 表 | 完成：97 张、19,697 个值，生成器 + FNV 校验（含 3D 表与 `128 * 125` 乘积更正；当前 FNV `0x7e85769cd6cb2bdc`） |
| AV1 tile CDF 状态 | 完成：`cdf_state.rs` 复制全部 97 张表（19,697 值），按静态表身份选择可变 row，`read_symbol` 跨调用保留自适应，支持 `disable_cdf_update` 与单表 reset；partition、intra mode、skip、tx_depth 已有 stateful 入口。另修正 Tx16/32/64 depth 行宽为 4（Tx8 为 3） |
| AV1 系数语法 stateful 路径 | 完成：`coeff.rs` 提供 all_zero/TxbSkip、EOB、coeff_base/eob/br、level loop 和 DC sign 的 `CdfState` 入口；stateful 与默认路径在同一码流上得到一致的 `Quant[]` |
| AV1 transform-block 系数消费 | 完成：`coeff_block.rs` 串联 all_zero、EOB、levels、signs、transform scan 和 above/left context 更新，返回完整 `Quant[]` |
| AV1 block mode 状态 | 完成：`segment.rs` 的 segment prediction/neg_deinterleave，以及方向模式的 signed `angle_delta_y/uv` |
| AV1 block delta | 完成：`delta.rs` 的 delta-q、single-channel delta-LF 与 four-row `delta_lf_multi`，包含小值扩展、signed shift 与 qindex clamp。彩色流消费 luma/U/V 四路独立 CDF，单色流只消费两路；真实彩色与单色 multi-LF keyframe 均与未修改的 libaom 对齐，彩色三平面、单色 Y 平面逐字节一致。单色门同时修正 UV 语法门控和单平面 deblock 网格 |
| AV1 intra transform-block 重建 | 完成：`reconstruct_intra_tx` 构造邻域、预测、选择 TX row/column kind、逆变换、加残差并写回平面 |
| AV1 syntax Quant 到像素重建桥接 | 完成首层：`reconstruct_intra_block_plane` 将熵解码的 column-major `Quant[]` 转置为逆变换布局，按 plane qindex/delta 选择 DC/AC 量化器，并支持普通 intra、palette、filter-intra 与 CFL 单 plane 重建；32 上限系数对 64 采样变换做零填充 |
| AV1 intra 帧级 YUV 重建 | 完成滤波前帧：`intra_frame.rs` 按 tile block 顺序先重建 Y、再用该 Y 做 U/V 的 CFL，使用 superblock 对齐 padding 容纳边缘 transform；真实 320×240 keyframe 已生成三平面并锁定 FNV 哈希 |
| AV1 intra 帧级 loop filter | 完成：从 block/tx/skip/delta-LF 按 plane 构造垂直与水平 `DeblockGrid`，luma 使用实际 transform 网格，色度按子采样后的 max-UV transform 和奇偶 MI 映射；filter size 同时读取边界两侧 transform extent。真实 restoration-enabled keyframe 的去块前后 Y/U/V 均与 libaom 逐像素一致 |
| AV1 intra 帧级 CDEF | 完成：使用 tile 的 `cdef_idx`，在去块 Y 平面做 8×8 方向搜索、variance 调强和主/次方向滤波，并由 luma 方向推导 U/V；真实 keyframe 的 CDEF 后三平面哈希已锁定 |
| AV1 intra 帧级 loop restoration | 完成：每个 superblock 在分区语法前消费 `read_lr`，保存每 plane 的 Wiener/SGRPROJ 网格；以 CDEF 输出和去块帧分别作为 `UpscaledCdefFrame` / `UpscaledCurrFrame` 驱动 §7.17 三平面恢复。新增 restoration-enabled 320×240 keyframe fixture，锁定重建前、去块后、CDEF 后、LR 后 12 个 Y/U/V 哈希 |
| AV1 superresolution（§7.16） | 完成：完整 64-phase × 8-tap normative upscaler，按 plane 处理水平子采样和 frame/coded width 差异；CDEF 与 deblocked frame 分别上采样后进入 LR。新增 213×240 → 320×240、denominator 12 的真实 fixture，最终三平面与 libaom 逐像素一致 |
| AV1 单参考 / compound 语法与 MV stack | 完成：`inter.rs` 实现 `read_is_inter`、单/双参考 `read_ref_frames`、uni/bidirectional compound 参考对、全部 8 种 compound mode、compound DRL、`new_mv/zero_mv/ref_mv`、compound group/type/mask/wedge/distance 语法、8-entry 空间 MV stack、双列表 MV 赋值、`lower_mv_precision` 和 MV joint/class/sign/frac/hp。OBMC 会按 above-first/left-second 检测可重叠邻块并读取 `use_obmc_cdf`；local warped motion 会收集同参考邻块中心 MV、选择 least-squares 样本并推导 affine；单参考与 compound global warp 会从 `ROTZOOM/AFFINE` 矩阵计算块中心 `GLOBALMV`，两个 reference 可分别选择 warp 或平移；reference temporal MV 会保存、投影并合并 8×8 motion field candidates。真实 key+P fixture 的 20 个单参考 block，以及 compound/temporal fixture 的参考帧/模式/MV 均与 libaom trace 一致 |
| AV1 参考帧 / 跨帧 CDF / 整数运动补偿 | 完成首批：八槽参考帧同时保存最终像素与 Backward CDF；保存时按规范清零 CDF counter。真实两帧 320×240 流从 keyframe 恢复到 P frame，单参考整数平移逐平面复制后，P frame Y/U/V 与 libaom 逐字节一致（FNV `dd5a7af1044fd871` / `930bef4f94807f25` / `930bef4f94807f25`）|
| AV1 film grain synthesis | 完成 8/10-bit 显示路径：`film_grain.rs` 移植 libaom 的 seed-derived Gaussian 64×64 luma / 32×32 chroma AR 模板、每 32×32 subblock 随机偏移、2/1 像素 overlap、Y/Cb/Cr scaling LUT、restricted-range clipping 与 8/10/12-bit 整数加噪。参考槽保存 post-filter/pre-grain 像素和该帧参数，`show_existing_frame` 从历史引用槽取参数，普通显示与历史显示都只修改输出副本。真实 8-bit 流 10/10 grain-applied 帧、10-bit 流 10/10 clean 帧和 10/10 grain-applied 帧均与 stock `aomdec` 逐样本一致 |
| AV1 多尺寸 / 4:4:4 conformance | 完成：96×96、128×128、192×192 各 10 帧 inter 流与 libaom 逐样本一致；新增 320×240 profile-1 I444 key+P+P，覆盖 one-sided compound、inter-intra、OBMC、warped motion 和相邻 NEWMV 候选。该门修正 `interintra_allowed()` 对 `8×32/32×8` 的错误尺寸门，以及单参考 MV-stack 扫描 compound 邻块时未计入 NEWMV 的 `newmv_count`/`mode_context` 偏差 |
| AV1 单列多行 tile | 完成：320×320 key+P+P 均为 `tile_cols_log2=0 / tile_rows_log2=1`；两个全宽 row tile 的熵状态都从 frame CDF 重新开始，帧级 inter 邻居状态跨 tile 延续，三帧 Y/U/V 与 `aomdec` 逐样本一致。AV1 没有独立 WPP 语法或 HEVC 式 entry-point 链，此前的 WPP 项按该 tile-row shape 验收 |
| AV1 screen-content IntraBC | 完成：`CdfState` 为 intra-block DV 保留独立 `ndvc`；解析 `use_intrabc`、整数 DV、tile 内源矩形校验、inter variable-transform tree/`TXFM_SPLIT`、inter tx type 和 IntraBC 残差，并从当前帧已解码区域复制 Y/U/V。默认 320×320 key+P+P 流的 20 个 IntraBC block、4/4 色 Y/UV palette 和三帧最终 YUV 与 `aomdec` 逐样本一致 |
| AV1 B-pyramid / reference reorder | 完成：20 帧 320×240 `lag-in-frames=19` 流包含多组隐藏 showable frame、显式 slot refresh、order-hint 重排和重复 `show_existing_frame`；八槽 reference store 同步轮换 pixel、SavedCdf 与 temporal motion state。20 个最终显示 YUV 与 `aomdec` 逐样本一致 |
| AV1 advanced B-pyramid | 完成：同一 20 帧重排序列保留 global/warped motion、OBMC、inter-intra、masked/distance-weighted compound 和 reference-frame MVs，多个帧使用至少三个非零 order-hint reference。20 个最终显示 YUV 与 `aomdec` 逐样本一致 |
| AV1 10-bit B-pyramid | 完成：320×240、12 帧 10-bit B 流的 coded frame 全部 showable，order-hint history 最多包含 7 个非零 slot，预测仍为单参考，覆盖 10-bit 运动补偿、残差与八槽参考轮换。12 个显示 I420P10 帧共 2,764,800 bytes，Y/U/V 所有 16-bit 样本与 stock `aomdec` 一致 |
| AV1 advanced 10-bit B-pyramid | 完成：同一 20 帧高级 B/多参考组合在 10-bit 下保留 hidden showable、`show_existing_frame`、global/warped motion、OBMC、inter-intra、masked/distance compound 和 reference-frame MVs。20 个显示 I420P10 帧共 4,608,000 bytes，Y/U/V 所有 16-bit 样本与 stock `aomdec` 一致 |
| AV1 10-bit B-pyramid deblock / CDEF | 完成：同一 20 帧高级 10-bit B 流的每个非 intra coded frame 都启用非零 loop filter，sequence 启用 CDEF；loop restoration 保持关闭。20 个显示 I420P10 帧共 4,608,000 bytes，Y/U/V 所有 16-bit 样本与 stock `aomdec` 一致 |
| AV1 10-bit 4:4:4 advanced B-pyramid | 完成：20 帧 I444P10 高级 B/多参考流在无 chroma 子采样条件下保留 hidden showable、`show_existing_frame`、global/warped motion、OBMC、inter-intra、masked/distance compound 和 reference-frame MVs。20 个显示帧共 9,216,000 bytes，Y/U/V 全样本与 stock `aomdec` 一致 |
| AV1 10-bit advanced B + loop restoration | 完成：20 帧 advanced B 流实际包含 `uses_lr=true` 的 inter frame，并同时启用 CDEF。该门修复 `NEW_NEARMV`/`NEAR_NEWMV` 的 NEW 分量参考索引（必须为 `1 + ref_mv_idx`），避免错误 temporal MV 扩散为后续 tile 熵失步；20 个显示 I420P10 帧共 4,608,000 bytes 全样本与 stock `aomdec` 一致 |
| AV1 fractional inter / transform tree / 后处理 | 完成：16-phase 8-tap normative fractional MV 双阶段插值，≤4 样本维度使用 four-tap regular filter；支持 inter 残差、all-zero、inter transform type、`TX_MODE_SELECT` 的递归 transform tree、switchable interpolation filter、inter frame 内 intra block 的完整 mode/palette/filter-intra/CFL/residual、coefficient CDF q-context 跨帧 collapse。修复 skip 块 above/left level/DC-sign context 清零、NEARMV DRL 终值、NEWMV `RefMvIdx` 选择、sub-8x8 chroma 四象限 MV 预测和 FLIPADST 的空间翻转语义。真实 high-QP 320×240 key+P 流 327 个 P block 与 libaom trace 一致；LF+CDEF 与 restoration-enabled key+P fixture 的 P frame（242 个分区叶块）三平面均逐字节一致。`UseWienerCdf` / `UseSgrprojCdf` / `RestorationTypeCdf` 现在随 SavedCdf 跨帧继承，inter LR 已完成 |
| AV1 average / distance- / difference-weighted / wedge / inter-intra prediction | 完成：compound CDF 行宽按 `8 symbols + terminal + counter` 的 9 个 u16 读取，参考对、mode、DRL 和双 MV 全部接入空间 stack。average 复刻 libaom 两阶段 16-bit 路径（`round_0=3`、`round_1=7`、floor average、round offset 4096+2048、4-bit 最终舍入）。distance 路径按 order hint 选 `quant_dist_weight`、分配 `fwd_offset`/`bck_offset`，使用截断 4-bit 加权后再减 round offset 并按 4-bit 舍入。masked 路径读取 `comp_group_idx` 的 above/left 上下文、compound type、mask type 或 wedge index/sign；difference mask 来自 luma 高精度差值，wedge 使用六张 64×64 master mask、9 种 block codebook 和 sign flip，二者都按色度采样规则下采样后执行同一 16-bit blend。inter-intra 在单参考 MV 后读取 DC/V/H/Smooth 模式与可选 wedge index，按邻域 intra 预测、16-line smooth mask 和固定 sign-0 wedge mask 与 inter 预测混合。average/distance/difference/wedge 及 smooth/wedge inter-intra fixture 的全部可见 Y/U/V 均逐字节一致 |
| AV1 switchable interpolation filters | 完成：regular/smooth/sharp/bilinear 全部 16-phase kernel 接入二维插值；宽高 ≤4 时按规范切换 regular/smooth 四抽头版本，sharp 回退 regular 四抽头，bilinear 保持八抽头零填充核 |
| AV1 inter 跨帧 CDF 基础 | 部分完成：coefficient CDF 按帧头 `base_q_idx` 固定选择，块级 delta-q 只改变反量化；4-high luma block 的 chroma coefficient context 使用子采样后的 residual size 清理；MV joint/class/sign/fraction CDF 并入 `CdfState`，随 SavedCdf 跨帧保存和恢复。新增三帧 delta-q fixture，三帧 Y/U/V 与 libaom 逐字节一致 |
| AV1 块几何 / partition / intra 模式 | 完成：22 种 BLOCK_SIZES、上下文、`y_mode`/`intra_frame_y_mode`/`uv_mode`+CFL、`tx_depth`、块级 `read_tx_size`（lossless / largest / select 与 split chain）、32 张扫描表；128×128 分区按规范的 8 符号 CDF 读取，窄 16×4/4×16 仍按 `MiSize >= BLOCK_8X8` 消费 palette/angle-delta |
| AV1 residual transform 布局 | 完成：`residual.rs` — 规范 `Subsampled_Size[22][2][2]`、chroma `get_tx_size`、>64 block 的 64x64 chunk、plane/raster transform block 坐标；4 个测试 |
| AV1 系数语法 | 完成：EOB 类别与 extra bits、`coeff_base`/`eob`/`br`、signs/Golomb、`Quant[]`、tile 级 above/left 上下文 |
| AV1 帧内预测 | 完成统一 `predict_intra` 入口：DC/V/H/SMOOTH/SMOOTH_V/H/PAETH、8 个方向模式及 angle delta、邻近样本不可用替换、边缘强度/滤波/上采样、CFL；带非零 angle delta 的 V/H 模式走方向预测而不是退化复制，色度 `filterType` 邻域按 4:2:0 奇偶规则定位；帧级重建维护每 plane 的 4×4 `BlockDecoded` 网格，above-right/below-left 只读取已重建样本；19 个预测测试 |
| AV1 filter-intra | 完成：`use_filter_intra`/5 种 mode 的持久 CDF、完整 `Intra_Filter_Taps` 和递归 4×2 预测；PAETH 的 transform 方向按 `Fimode_To_Intradir` 映射到 DC |
| AV1 palette mode / token map | 完成：screen-content/尺寸门控、邻域 size 上下文、Y/UV/V 颜色（含 10-bit）、cache merge、color-index wavefront 和全部 2–8 色 CDF；真实 keyframe 中的 palette 与 libaom 逐语法一致 |
| AV1 CFL alpha | 完成：`read_cfl_alphas()` 的 joint sign、U/V 16 级 magnitude、按 joint sign 选择 6 个持久 CDF 上下文，并保留 alpha index；真实 tile 的 CFL 块已闭合 |
| AV1 intra skip 语义 | 完成修正：skip 只跳过残差；segment 之后的 y/uv mode 仍按语法读取，避免真实 tile 失步 |
| AV1 逆变换 / 反量化 | 完成蝶形 DCT、ADST4/8/16 与 FLIPADST、2D 变换、量化与残差重建；行列移位按规范使用 `Round2`，矩形输入执行 2896/4096 预缩放，4/8/16/32 点 inverse identity 使用尺寸相关缩放；反量化补齐 24-bit 截断与 `dqDenom`（32/16×32 类除 2，64 点组合除 4）。libaom 的 4×4 DCT_DCT、4×4 IDTX、4×4 H_DCT、8×4 H_DCT、8×4 DCT_ADST、4×8 DCT_DCT 与 16×32 DC 向量均逐样本一致 |
| AV1 transform type / set | 完成：`tx_type.rs` — `get_tx_set`、intra set1/set2 inverse tables、intra `tx_type` CDF、row/column `TxKind`、scan direction 与 tx class |
| AV1 CDEF | 完成：方向搜索、主/次抽头、约束差值、全 skip 8×8 跳过、8/10-bit strength/damping 缩放、色度 damping 减一和主/次强度为零时的 no-clip 语义；使用 libaom 去块帧作为输入时 Y/U/V 三平面逐像素一致。10 个测试（含 8/10-bit 与色度几何） |
| AV1 CDEF index 语法 | 完成：64×64 luma 对齐网格、`L(cdef_bits)`、skip/lossless/intrabc gating 与同区复用；2 个测试 |
| AV1 loop restoration 滤波 | 完成：Wiener、self-guided（两遍 box + 投影）、stripe 边界源切换，8 个测试含两个独立规范向量 |
| AV1 LR 参数解码（§8.3 subexp） | 完成：`NS(n)` / `decode_subexp_bool` / `inverse_recenter` / `decode_signed_subexp_with_ref_bool`，7 个测试 |
| AV1 LR 帧/条带驱动（§7.17） | 完成：unit 网格几何、`stripeNum`、4x4 prelude、写回，11 个测试；网格可由码流层直接填充 |
| AV1 `read_lr` / `read_lr_unit` 码流层 | 完成：tile 级可变 CDF 与 `RefLrWiener`/`RefSgrXqd` 复位、`use_wiener`/`use_sgrproj`/`restoration_type` 符号、Wiener 两遍抽头与 SGRPROJ set/xqd（含 radius=0 的 `Clip3`）、superres 与 intrabc、unit 网格遍历，15 个测试。此前记为「缺 `TileWienerCdf`/`TileSgrprojCdf` 默认表」是误判：三张 `Default_*` 表本就在 `cdf_defaults.rs` 841–856 行，`Tile*` 是它们的每 tile 可变副本 |
| AV1 去块 / loop filter（§7.14） | 完成：帧级遍历（先竖后横）、自适应强度（sharpness / ref-mode deltas / segment feature / delta_lf）、`hev`/`filter`/`flat`/`flat2` 掩膜、`filter4` 窄滤波与 log2Size 3/4 宽滤波，17 个测试含独立计算的规范向量与 8/10-bit。每 4x4 的 `MiSizes`/`LoopfilterTxSizes`/`Skips`/`RefFrames`/`DeltaLFs` 由调用方经 `DeblockGrid` 注入 |
| AV1 位流描述符（规范 §8.1） | 完成：`bitstream.rs` — MSB-first `f(n)`、`le(n)`、`uvlc()`、`leb128()`（含 `Leb128Bytes` 与 ≤2³²−1 校验）、`su(n)`、`ns(n)`、`byte_alignment`、`trailing_bits`、`skip_bits`、`get_position`，9 个测试含 `n=5` 的 `ns(n)` 编码表与错误路径 |
| AV1 OBU / IVF 解复用（规范 §5.2） | 完成：`obu.rs` — `obu_type` 表、`obu_header`/extension、有/无 size field 双路径、temporal/spatial 丢弃判定、`trailing_bits_needed(consumed_bits)`，以及 DKIF IVF 容器 demux，9 个测试含畸形码流。规范表里的 N/Y 列是 **Layer-specific**，不是 size field 约束 |
| AV1 序列头（规范 §5.5） | 完成：`seq_header.rs` — reduced 快路径、timing/decoder model/operating parameters、全部 operating points（tier/model/显示延迟分支）、最大帧尺寸、frame id 长度、全部 feature flags、`color_config()` 三分支、film grain；`choose_operating_point()` 可由调用方指定；`parse_sequence_header_obu()` 校验 trailing bits。9 个测试 + 5 个真实 IVF fixture（50 帧）端到端解析，宽高/位深/4:2:0 与容器及 `ffprobe` 交叉一致 |
| AV1 帧头（规范 §5.9） | 完成：`frame_header.rs` — 完整 `uncompressed_header()`（show_existing、帧类型/id/order hint、帧尺寸与 superres、`tile_info()`、量化/segmentation/delta LF、lossless 默认值、`loop_filter_params()`、CDEF、`lr_params()`、`tx_mode`、`skip_mode_params()`、global motion 与 `read_global_param()`、`film_grain_params()`）。`FrameHeaderState` 注入依赖解码状态的量，默认为 `setup_past_independence()`；`parse_frame_header_obu()` 校验 trailing bits，`parse_frame_obu()` 做 `byte_alignment()` 并返回 tile-group payload；`loop_filter_params()` / `lr_geometry()` 直连 `deblock.rs` / `lr_frame.rs`。12 个测试，含 8 个真实 IVF 共 112 个帧头的独立 Python 位位置比对、8 组首帧字段快照、ffmpeg `trace_headers` 的 film grain 逐语法元素快照、负向用例，以及不注入 `skip_mode` 的 8 个 IVF 串行参考状态更新。`frame_header_copy()` 属 OBU 调度器，未实现 |
| AV1 tile-group 拆包（§5.11） | 完成：`tile_group.rs` — `tile_start_and_end_present_flag`、`tg_start`/`tg_end`、byte alignment、1–4 byte little-endian tile size、最后 tile 隐式延伸到 group 末尾、多 group 去重/完整性聚合，7 个测试；真实 320×240 单 tile 与 2×2 tile fixture 均得到完整且无重叠的 tile slice |
| AV1 stream 调度（IVF/OBU→frame→tile） | 完成：`stream.rs` — 按 packet/OBU 顺序产出 sequence/show-existing/coded-frame 事件，事件携带真实 tile ranges，并把帧头参考状态更新接在事件推进上；单 tile 与 2×2 fixture 均覆盖 |
| AV1 intra tile block sink | 完成语法闭环：按规范的 Y mode→Y angle→UV mode→CFL alpha→UV angle 顺序消费，并处理 palette、filter-intra、tx depth 与逐 plane 系数；真实 320×240 keyframe 的整 tile（17944 bits）通过分区覆盖、全部残差上下文和 `exit_symbol()` 尾位校验，记录的 `Quant[]` 已接入帧级像素重建 |
| AV1 tile 熵入口 / 分区驱动 | 完成：`tile_entropy.rs` — tile id→MI 边界、叶块回调、stateful partition CDF、图像边缘的折叠二符号/隐式 SPLIT、`HORZ_4`/`VERT_4` 越界保护；真实 2×2 fixture 与 320×240 单 tile keyframe 均通过 |
| AV1 全帧码流解码 / libaom 像素比对 | 部分：真实 320×240、无 film-grain keyframe 已从 OBU/序列头/帧头/tile 熵层生成 Y/U/V 三平面，并完成帧级去块、CDEF、§7.16 superres 与 LR；普通及 restoration-enabled fixture 的四个阶段均已锁定，213×240→320×240 superres fixture 的最终 Y/U/V 三平面也已与 libaom 逐像素一致。十七条单 tile 的真实 320×240 inter fixture 分别闭合整数 inter、fractional/residual inter、inter LF+CDEF、inter LR、delta-q、average/distance-weighted/difference-weighted/wedge compound、smooth/wedge inter-intra、OBMC、local warp、单参考与 compound global warp、temporal MV、inter segmentation 与混合 intra/inter selected tx context；所有可见帧 Y/U/V 均与 libaom 逐字节一致。额外的 2×2 tile fixture 覆盖 keyframe 与后续 10 个 inter coded frame：每个 tile 都从帧初始 CDF 重新开始，帧末 CDF 取 `context_update_tile_id`，帧级 mi/segment 状态跨 tile 携带而邻块上下文、MV 单点候选、OBMC/projection 与 CDEF 单元按 tile 裁剪；temporal motion-field 还按 half-res cell 清除 intra 覆盖、保留 base-invalid 时的 extension probes 并按 order hint 应用参考符号，difference-weighted chroma mask 则在 luma 域重算。全部显示帧 Y/U/V 与 `aomdec` 逐字节一致。仍缺更宽的 AV1 conformance/性能矩阵 |

## 已知 C bug（本 crate 保留独立结果）

| case | 依据 |
|---|---|
| VP9 `vp9_p0_720p_errres_seg` 帧 4/5 | Rust == libvpx；C 差 106+107 字节，V 平面，最大差 3 |
| VP9 `vp9_p0_lossless_321x241_2` 帧数 | libvpx/Rust 2 帧，C 1 帧 |
| AV1 film grain（8/10-bit） | Rust == libaom（带 grain），C 未置 `sw_apply_grain` |
| AV1 64×64 | 硬件 job 不完（status `0x1`，正常 `0x1100`），C 输出十帧全黑；libaom 有真实像素，Rust 报错不写黑帧 |
| MPEG-2 隔行 | C 回绕输入并多出帧，Rust 与 FFmpeg 一致 |
| VP8 损坏流 | C 直接中止，Rust 输出已解码帧（BY-DESIGN） |

## 复现入口

```bash
# 解码全量（AV1/VP9/... 各自子集可用 CORPUS 指向一个目录）
CORPUS=~/rk-mpp-oracles/corpus ./scripts/board_corpus_sweep.sh

# 编码
ONLY=gop ./scripts/board_h264_qp_vs_mpp.sh
./scripts/board_h265_tile_vs_mpp.sh
./scripts/board_h265_lowdelay_vs_mpp.sh

# 后处理
./scripts/board_iep_modes_vs_probe.sh
./scripts/board_iep_formats_vs_probe.sh
./scripts/board_iep_dect_vs_probe.sh

# 性能（固定 governor 后）
BENCH_FILTER=vp9 SAMPLES=5 ./scripts/board_bench_vs_mpp.sh
```

当前 ROADMAP：**128 完成 / 4 未完成**（含缩进子项为 168 / 7）。未完成项均属于上表的
平台/不存在边界；M11 的软件 AV1 tile entropy 项已关闭。M11 已落地 115 个 slice（含真实 2×2 与 1×2 tile tile-group 的逐 tile 熵闭合与像素门；inter 的 tile-group 驱动、帧级邻居携带、temporal/MV/chroma-mask 修正和 10 帧三平面门均已落地；8/10-bit film-grain 显示合成、多尺寸/4:4:4、screen-content IntraBC、simple/advanced 20 帧 8-bit B-pyramid、12 帧单参考 10-bit B、20 帧 advanced 10-bit B、deblock/CDEF、10-bit I444P10 和 inter-LR 组合也已落地）：符号解码、默认
CDF、partition/块模式、系数语法、Round2 逆变换与 libaom 矩形向量、量化、重建、帧内预测、CDEF、loop
restoration 滤波与 subexp/tile-CDF 参数解码、帧/条带驱动、`read_lr`/`read_lr_unit` 码流层、
§7.14 去块滤波、§8.1/§5.2/§5.5 位流层（描述符、OBU+IVF 解复用、完整序列头，已在
5 个真实 IVF fixture 的 50 帧上验证），以及 §5.9 完整帧头（8 个真实 IVF 共 112 个帧头逐位
对齐，并经 ffmpeg `trace_headers` 交叉核对）、`SavedFilmGrain` 跨参考槽继承与 seed 替换，真实 tile-group 的字节范围拆包、不依赖
`skip_mode` 注入的参考帧状态串行更新、统一帧内预测入口，以及把 OBU/帧头/tile 串起来的
stream 事件驱动、residual transform 的 chunk/plane/raster 布局、tile 级可变 CDF 状态，以及
partition/mode/skip/tx_depth 的持久 CDF 接线、系数 EOB/level/sign 的 stateful 路径，以及
tile MI 边界与叶块回调驱动、transform-type 选择、residual 的 `all_zero` 入口、单
transform block 的完整系数消费、segment/angle-delta block 状态、block delta 语法、
intra transform-block 重建、CDEF index 语法、filter-intra、palette/color-index token map、
CFL alpha、正确的 intra skip 语义，以及真实 320×240 keyframe 的整 tile 熵闭合。
预测、重建、去块、CDEF、superres 与 LR 的帧级像素写回现已接通，普通、restoration-enabled
和 213×240→320×240 superres keyframe 均已与 libaom 三平面逐像素一致；four-row
`delta_lf_multi` keyframe 亦在三平面逐字节一致；单参考整数与
fractional inter（含 residual 和 frame 内混合 intra/inter）均已用真实 key+P 流闭合到
P frame 像素级，SavedCdf 与八槽参考帧管理开始工作。inter 去块、CDEF 与 LR 已逐字节闭合；
average、distance-weighted、difference-weighted、wedge compound、inter-intra、OBMC、
local warped motion 与单参考 global warped motion 的回归也已三平面逐字节一致。local warp fixture 含
10 个 `WARPED_CAUSAL` block，覆盖零 MV 投影、least-squares affine、193-phase
8-tap filter 和 4:2:0 sub-8×8 chroma 回退；global warp fixture 含 756 个
`GLOBALMV + ROTZOOM` block；compound fixture 还覆盖 3 个同时含 global-warp 与
identity 参考的 `GLOBAL_GLOBALMV` block，包括 distance-weighted 和 wedge blend。
temporal fixture 还覆盖 reference motion-field 保存、order-hint projection 与 spatial
stack 合并，128 个 inter block 的三平面与 libaom 逐字节一致。segmentation fixture 另覆盖
key/inter `ALT_Q`、temporal segment-map prediction 与 1050 个非零 segment block，三帧
Y/U/V 也与 libaom 逐字节一致。完整 `av1_lr` key+P 流还覆盖非 skip inter 邻居的 selected
tx-size context，458 个 footprint block 与两帧 Y/U/V 均逐字节一致。
RK3588 `cargo test --release --all-targets` 为 **1303 个测试枚举 / 1262 常规通过 / 0 失败 / 41 个板级 ignored**；
RK3588 `cargo test --release --lib` 为 **1115 passed / 0 failed / 0 ignored**。41 个板级 ignored
均在 integration/example targets；其逐项通过记录
完整 RK3588 回归与性能闸门见 `docs/RK3588_VALIDATION.md`。
