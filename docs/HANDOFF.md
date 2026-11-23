# rk-mpp-rs 交接文档

写给下一个接手实现的模型/人。目标是 `refs/mpp` → `rk-mpp-rs` 的完整 Rust 重写：
功能对齐、性能不劣于 C、每次验证通过就提交。

**当前阶段口径（用户 2026-11-02 决定）**：只做像素/字节级模仿，以及不改变输出的性能
提升；结果与 C 不一致即视为缺陷，除非另外证明 C 是 bug。功能前沿（tiles/WPP/SAO、B 帧、
refresh/GDR 分配等）冻结，先把已实现路径上的已知差异关掉。

**该口径之后的实际推进（截至 2026-11-11）**：deblock/SAO 细调、H.264 two-pass deflicker、
>4096 宽 P 帧与 **H.265 水平 tiles** 已按同一逐字节标准落地（tiles 见 ROADMAP 的
`H.265 (rkvenc): uniform horizontal tiles`）；AV1 软件参考解码器已完成 fractional
single-reference inter、inter residual、recursive transform tree、frame 内混合
intra/inter、inter deblock/CDEF、inter LR、average/distance-weighted/difference-weighted/wedge
compound、inter-intra、OBMC、local warp、单参考/compound global warped motion 与 temporal
MV fields，以及 inter segmentation 的 segment map、temporal prediction 与 segment
features；`delta_lf_multi` 的四路 CDF 路径已由真实彩色 keyframe 三平面与单色 Y 平面像素门
闭合，单色门同时修正 UV 语法门控和 deblock 单平面网格；混合 intra/inter selected tx
context 也已闭合完整 `av1_lr` key+P 流。第 104 slice 又补上
`SavedFilmGrain[refFrame]`：刷新参考槽时保存完整 grain 参数，`update_grain=0`
时按 `film_grain_params_ref_idx` 继承 scaling points 和 AR coefficients，仅替换
seed。真实 OBU 集成门从现有 grain IVF 的 inter header 构造短分支变体，验证
21-bit 继承语法、seed 保留和完整参数复制。第 105 slice 完成软件 film grain
synthesis：移植 libaom 的 seeded AR 模板、32×32 subblock 随机偏移、2/1 像素
overlap、Y/Cb/Cr scaling LUT、restricted-range clipping 与高位深加噪路径。
参考槽保留 post-filter/pre-grain 像素和对应参数，普通显示与
`show_existing_frame` 只在输出副本上施加 grain。真实的 8-bit grain-applied 10 帧、
10-bit clean 10 帧和 10-bit grain-applied 10 帧全部与 stock `aomdec` 逐样本一致。
第 106 slice 把 M11 的真实 inter conformance 扩到 96×96、128×128、192×192 各 10 帧，
并加入 320×240 4:4:4 key+P+P。I444 门定位到两个状态错误：inter-intra 的尺寸门
误收 `8×32/32×8`；单参考 MV stack 扫描 compound 邻块时漏计其中的 NEWMV，使
`newmv_count`/`mode_context` 少一档并最终偏移 MV 和系数语法。两处均已用单测锁定，
四条新流全部与 libaom 逐样本一致。
第 107 slice 锁定 320×320 `tile_cols_log2=0 / tile_rows_log2=1` 的 key+P+P，
两个全宽 row tile 都从 frame CDF 重启，三帧与 `aomdec` 逐样本一致。AV1 本身没有
独立 WPP flag 或 HEVC 式 entry-point context 链，因此 M11 的该项已经关闭。
第 108 slice 完成 screen-content IntraBC：独立 `ndvc`、tile 内 DV 校验、inter
variable-transform tree、IntraBC 残差和当前帧块复制均已接通。默认 320×320
key+P+P 的 20 个 IntraBC block、4/4 色 Y/UV palette 和三帧最终 YUV 逐样本一致。
第 109 slice 锁定 20 帧 B-pyramid：隐藏 showable picture、order-hint 重排、
参考槽刷新和 `show_existing_frame` 在八槽 pixel/CDF/temporal state 上轮换，
20 个最终显示 YUV 逐样本一致。剩余前沿收窄为高级 B/多参考组合和更宽 conformance。
第 110 slice 将该 B-pyramid 与 global/warped motion、OBMC、inter-intra、masked/
distance compound 和 reference-frame MVs 组合，20 个最终显示 YUV 仍逐样本一致。
第 111 slice 把同一参考调度 frontier 推到 10-bit：12 帧 B-pyramid 的 coded frame
全部 showable，order-hint history 最多包含 7 个非零 slot，预测仍为单参考并连续验证
10-bit 运动补偿、残差与参考槽轮换。12 个显示帧的完整 I420P10 共 2,764,800 bytes
与 stock `aomdec` 逐样本一致，不存在差异字节。
第 112 slice 把同一高级组合推进到 10-bit，同时保留 hidden showable、
`show_existing_frame`、双参考预测和 20 帧参考重排。20 个显示 I420P10 帧共
4,608,000 bytes，全部与 stock `aomdec` 逐样本一致，不存在差异字节。
第 113 slice 在同一 20 帧 10-bit 高级 B 流上打开非零 deblock 和 CDEF；每个非 intra
coded frame 都走 loop filter，20 个显示帧仍全部逐样本一致。该码流未启用 loop
restoration，本轮不把 LR 计入证据。
第 114 slice 把高级 B/多参考组合扩到 10-bit 4:4:4。20 个显示 I444P10 帧共
9,216,000 bytes，Y/U/V 全部与 stock `aomdec` 逐样本一致。
第 115 slice 增加真正启用 inter LR 的 20 帧 10-bit advanced B 流。首次出现
`NEW_NEARMV` 时，NEW 分量错误地使用 `ref_mv_idx`，而规范和 libaom 使用
`1 + ref_mv_idx`；错误 MV 随后污染 temporal motion field，并使第 5 个 coded frame
出现 `ExitSymbol/PaddingNotZero`。修复后 20 个显示帧共 4,608,000 bytes 全部与
stock `aomdec` 逐样本一致。当前剩余前沿主要是更多 LR/post-filter shape、
未覆盖的调度组合与更宽 conformance。
第 116 slice 把 screen-content IntraBC/palette 推到 RK3588 hardware gate。C 在
`coding.intrabc` 时把七个 reference 寄存器都指向 `CurrPicTextureIndex`；Rust 的
hardware reference map 现在包含当前 tile-output frame，消除了 `0x2109`
`sw_dec_bus_int`。随后定位到 intra CDF 更新错误：硬件 MV 区只应写入
`cdfs_ndvc`，regular MV CDF 必须保留。修复后 320x320 key+P+P 的 460,800 bytes
三帧与 C 逐字节一致（`5298dbfb…c22ad`），门为
`scripts/board_av1_intrabc_palette.sh`。
第 117 slice 关闭 AV1 monochrome 显示输出。C harness 对 `PIX_FMT_GRAY` 直接报
不支持并循环到超时；Rust hardware 的 Y 平面已经与 libaom 一致，但未使用 U/V
表面为 0。现在单色输出填充 neutral 128（10-bit 为 512），320x240 I420
115,200 bytes 与 `aomdec` 逐字节一致（`df3a2fe1…e909b`），门为
`scripts/board_av1_monochrome.sh`。
第 118 slice 完成 H.264 CABAC B-frame deblocking。`DeblockMap` 现在同时携带
L0/L1 的 reference 和 4×4 MV；B boundary strength 按双列表规则计算，未使用列表
用 sentinel 表示。新的 48x32 IDR+P+B fixture 混合 direct/skip 并开启 loop filter，
三帧与 FFmpeg 逐像素一致。该门还修正 chroma AC-only residual：AC 存在而 DC
缺省时，DC 变换块应按全零处理，而不是拒绝宏块。
第 119 slice 接通 implicit weighted B：`weighted_bipred_idc=2` 用 clipped
POC 距离计算 `w0`，`w1 = 64 - w0`，BI 以 6-bit 分母混合。B_BI fixture
只改写 PPS 两位进入该路径，显示帧与 FFmpeg 逐像素一致；等距权重和
非等距权重公式均有单测。Explicit `weighted_bipred_idc=1` 仍待做。
第 120 slice 补上 weighted `P_Skip`：静态 IDR+P 流打开
`weighted_pred_flag=1` 并保持 P 帧 100% skip，默认 luma/chroma weight
flags 走 identity 缩放。header 解析、权重应用和两帧输出均与 FFmpeg
逐像素一致。
AV1 4:4:4 硬件边界也已收口：C `mpi_dec_test` 在 20 帧 10-bit 4:4:4 fixture
上会在 AV1 parser 内 segfault，而 Rust hardware 之前会静默降成 4:2:0。现在
hardware CLI 显式拒绝非 4:2:0，软件参考解码器仍以 9,216,000-byte 输出对齐
stock `aomdec`。该边界记入 DIVERGENCES，不能把 4:4:4 的软件门当成硬件支持。
同日 M11 也开始闭合 H.264 P-slice CABAC。首个严格门是 48×32 Main-profile
IDR+P：P 帧六个宏块全部为 `P_Skip`，验证 P 的 `cabac_init_idc` 上下文、
left/top“已解码且非 skip”条件都为 0 时选择 ctxIdx 11、逐宏块终止位和
reference picture 0 复制。fixture 固定 `weightp=0:no-deblock=1`，两帧
4,608 bytes 与 FFmpeg 逐样本一致。
当前实现只接受全 skip 且 `first_mb_in_slice=0` 的单个 4:2:0 slice；P_L0、
weighted P_Skip、带 deblocking 的 P 帧和 B-slice CABAC 必须继续实现，不能把
这个门当成 P/B 完成。
紧接着的 P16 门把 48×32 fixture 固定为 100% I16 IDR + 100% P16 P 帧：
每个 P 宏块消费 mb_type、零 MVD、CBP、QP、16 个 luma 4×4 residual 并执行
zero-motion 参考块复制与残差重建。x264 trace 与 Rust 上下文状态对比定位到
inter CBF 与 I-slice 的根本差异：不可用邻块按 0 处理，不能沿用 I-slice 的
非零约定。两帧 4,608 bytes 与 FFmpeg 全样本一致。当前仍显式拒绝非零 MV、
P_16x8/P_8x16/P_8x8、多个参考、weighted/deblock 和 B-slice，不能把 P16
零运动门当作一般 P 帧完成。
整数运动 P16 门进一步把同一 48×32 结构推进到真实 8 像素平移：临时 x264 trace
构建禁用 P_SKIP/子分区/intra 后，把六个 P16 宏块固定为 `(+32, 0)` quarter-pel。
Rust 现在实现 P16 MVP 中值与可用性规则、非零 MVD 符号、luma +8 px/chroma +4 px
整数 MC 和帧边扩展。该门发现 MVD bypass 符号被反向解释，修正后 P 帧 2,304 bytes
与 FFmpeg 全样本一致。分数运动门随后用合法 subpel 搜索流覆盖 `(-6,-9)`、
`(8,1)`、`(5,1)`、`(-5,-6)`、`(7,1)` 等最终 MV，实现 quarter-pel luma
水平/垂直/二维六抽头和 1/8-sample 4:2:0 chroma 双线性插值；同时把 MVD 邻居
缓存改为绝对幅值，避免后续宏块 MVD context 漂移。随后两组强制分区流把
`P_L0_L0_16x8` 和 `P_L0_L0_8x16` 的 mb_type 三 bin、两个 MVD、partition
scan-order A/B/C MVP/MVD context、独立上下/左右 MC 与共享 residual 接通；两帧
均与 FFmpeg 逐样本一致，并补齐 quarter-pel `(2,2)` 二维插值。P_8x8 混合门随后
在同一个 P 帧中覆盖 16x16、8x16 和 P_8x8 宏块，四种 `sub_mb_type`
（`8x8/8x4/4x8/4x4`）、4x4 MVD cache/context、逐子分区 luma/chroma MC 与共享
residual 均与 FFmpeg 逐样本一致。explicit weighted `P_L0_16x16` 门又接通 luma `3/8` 与
U/V `7/4` 权重、规范舍入/裁剪和三平面 offset；同时修正 weighted slice header
在 bit 71 结束时 CABAC 起始多跳 1 byte 的 bug。P16 deblock 门随后接通
coeff/4×4 MV 边界强度、QP average、alpha/beta offset 和垂直优先三平面滤波，
并与 FFmpeg 整帧逐样本一致。双参考门再覆盖 `ref_idx_l0` unary、逐 4×4
reference cache、参考相关 MC 和 A-only MVP fallback，P2 参考模式
`1,0,0,0,1,0` 与 FFmpeg 逐样本一致。B-slice 的首个 `B_L0_16x16` 门也已接通
B skip/type CABAC 上下文、list-0 ref/MVD 和 forward MC，显示帧逐样本一致。
随后 `B_L1_16x16` 与 `B_BI_16x16` 也接通独立 L0/L1 cache、双 MVD 顺序和
规范 BI 平均。`B_Direct_16x16`/`B_Skip` 又接通 spatial A/B/C 参考与 MV
选择、skip/type 独立上下文、逐 list direct MC 和零 residual skip。
temporal direct 再通过 POC 0/4/2 的 distance scale 与 colocated P 16×16
运动图生成 B 的两列表 MV；`B_L0_L0_16x8`/`8x16` 也接通 CABAC 形状前缀、
顺序双 MVD、逐分区 MC 和共享 residual；`B_8x8` 再接通四个 L0 sub_mb_type、
四个 MVD 和逐象限 MC；同一结构下的 L1/BI 8x8 子分区也已闭合双列表 MVD
和临时双向预测；mixed L0 8x4/4x8/4x4 又覆盖完整的 2/2/4/1 MVD 几何。
当前仍缺 col-zero B reference、mixed-list/BI B 宏块分区、
direct 与 L1/BI 非 8x8 B_8x8、explicit weighted B。
口径本身（不一致即缺陷）没有放松：tiles 的残留状态问题就是靠登记
`docs/DIVERGENCES.md` 而不是靠放宽判据处理的。

**对外 SDK 推进（2026-11-08）**：已新增版本化 `rk_mpp_*` V1.8 C ABI
(`include/rk_mpp_rs.h`) 和 Rust `sdk` 模块，优先覆盖 H.264、H.265、VP9、AV1
与 MJPEG 解码。
构建同时产出 `cdylib`、`staticlib`、`rlib`；packet 可任意分块，frame 由
decoder 持有到显式 release。RK3576 与 RK3588 的 C/C++ 动态/静态链接门、生命周期错误路径、
7/127/313-byte H.264 分块、单包整条 H.264、97/211-byte MJPEG 分块及四帧
单包 MJPEG 全部通过；H.265 8-bit NV12/I420 和 Main10 I420P10 也分别用
37/53-byte 分块、单包输入和 6 帧 B 结构流验证；VP9 profile 0 的 12 帧
NV12/I420 和 profile 2 10-bit 的 12 帧 I420P10 也通过。AV1 的 8-bit
key+P 两帧与 C 逐字节；RK3588 的 10-bit film-grain C API 输出与 Rust/libaom
对齐，而 C MPP 不应用 grain，属于 `docs/CAPABILITY.md` 已登记差异。VP9/AV1
的 submit 单位分别是 coded packet/temporal unit，不是任意字节块。复跑：

```bash
BOARD=my-rk3576-host ./scripts/board_c_api_h264_mjpeg.sh
BOARD=my-rk3588-host ./scripts/board_c_api_h264_mjpeg.sh
```

VP8 也已接入同一 decoder handle；RK3588 的 12 帧 alt-ref IVF 与 C 逐字节。
RK3576 没有 VP8 decoder，C MPP 与 Rust 都在 create 阶段明确拒绝，脚本
登记为平台 SKIP。

MPEG-2、MPEG-4、H.263 也接入同一 decoder handle；由于现有硬件前端只提供
整条 elementary stream 解码，接口支持任意分块输入但在 EOS/flush 时统一
产出。RK3588 的 10/12/12 帧输出全部分别与旧 `/tmp/mpp-develop` C oracle
逐字节；RK3576 无 VDPU2，登记平台 SKIP。

编码侧新增 H.264 VEPU580/VEPU510 的同步 submit/receive：输入 NV12/I420
在 `submit_frame()` 返回前完成复制与 coded-geometry padding，packet 由 encoder
持有到显式 release。RK3588 支持 baseline/main/high 与通用 level/GOP/QP/fps；
RK3576 目前只开放板验过的 320×240 Baseline QP26 GOP6 @30fps 捕获路径。
RK3576/RK3588 的 6 帧输出均与对应 CLI oracle 逐字节一致。

H.265 编码复用同一 encoder handle：RK3588 VEPU580 支持 width/height、QP、GOP；
RK3576 VEPU510 只开放已捕获并板验的 320×240 Main QP26 GOP6 @30fps。
RK3576/RK3588 的 6 帧 H.265 输出也分别与 `h265e510-gop6` / `h265-gop` 逐字节。

JPEG 编码复用同一 handle：NV12/I420 输入、0–10 内置量化档、可选 restart
MCU 间隔；VEPU2/VPU720 自动选择。RK3576/RK3588 单帧 JPEG 与 `jpeg-enc` 逐字节。

SDK 安装入口为 `scripts/install_sdk.sh --prefix ... [--destdir ...]`。
Linux cdylib 的 SONAME 为 `librk_mpp_rs.so.1`，实际文件按 API 版本命名为
`.so.1.14.0`。`scripts/check_sdk_abi.sh` 固定导出符号、C11 enum/struct
size/offset 与 SONAME；`scripts/board_sdk_package.sh` 会用临时 prefix 编译并
运行 pkg-config、CMake shared、CMake static consumer。

V1.9 的编码配置保持向后兼容：原 `rk_mpp_encoder_config_t` 仍是 68 bytes，
`rk_mpp_encoder_create()` 的表单和 offset 不变。新增独立
`rk_mpp_encoder_advanced_config_t`、`rk_mpp_encoder_rate_control_t` 与
`rk_mpp_encoder_create_advanced()`，暴露 CBR/VBR/AVBR、码率范围和 QP 范围。
RK3588 VEPU580 的 H.264/H.265 六组 C API RC 输出与既有 CLI 逐字节相同；用 V1.8
快照头文件编译的 consumer 也能直接运行在 V1.9 库上。RK3576 VEPU510 的
CBR/VBR/AVBR 逐帧 streaming RC 已接入；用户自定义 RC callback 也已接通
H.264/H.265，并在 RK3588/RK3576 的 fixed/CBR/VBR/AVBR 8/8 矩阵上通过。
当前 host workspace gate 为 `1366 passed / 0 failed / 47 ignored`。V1.14
切片的 RK3588 门包括 V1.9/V1.10 兼容 consumer、六组 H.264 与三组 H.265
C V3/CLI 逐字节、41-symbol ABI、SDK install/pkg-config/CMake 和
`rk-mpp-rs-sys` native smoke，均通过。此前的完整 RK3588 workspace gate 为
`1366 passed / 0 failed / 46 ignored`，严格 rustdoc 同时通过。RK3576 使用简化门
`1090 library tests / 0 failed` 加 packet-meta/JPEG 专项。rustfmt 与严格
workspace rustdoc 同时通过。

V1.10 用新的 `rk_mpp_encoder_advanced_config_v2_t` 扩展 ROI、GDR 与 slice，
不扩大 V1.9 的 104-byte advanced config。RK3588 H.264/H.265 的 ROI、
slice-lowdelay 和 GDR 共六组 C API 输出与各自 CLI 逐字节一致；V1.9 advanced
consumer 直接链接 V1.10。

`crates/rk-mpp-rs-sys` 是独立的 workspace crate，手写覆盖 V1.14 的全部 41 个
C 导出和固定布局。`scripts/board_sys_crate.sh` 验证显式路径 dynamic、
static 和安装后 pkg-config 三种链接方式；在 RK3588/RK3576 均通过。release static
consumer 需设置 `CARGO_PROFILE_RELEASE_LTO=false`。

V1.11 的 H.264、8-bit 4:2:0 H.265/VP9/AV1 与 MJPEG NV12 可通过
`RK_MPP_DECODER_FLAG_DMABUF_OUTPUT` opt-in 保留真实 dma-buf；Rust 和 C 都能
取得 plane fd/offset/size/stride，C 返回的 fd 由调用方负责 close。H.264
display reorder 与 H.265/VP9/AV1 picture reorder 都会额外持有池化 surface
lease；VDPU383 AV1 的 DMA 路径不再执行 CPU planar readback。默认行为仍是连续
CPU frame；H.265 Main10/422/444 与 VP9/AV1 10-bit 导出 DMA-BUF 会明确返回
unsupported。

V1.12 增加编码器 DMA-BUF 输入。`DmaBufInput` /
`rk_mpp_encoder_submit_frame_dmabuf()` 接受 NV12 fd、allocation 长度和 Y/UV
stride；H.264/H.265 要求 16 对齐，JPEG 要求 8×2 对齐。原 copied-input API
保持不变。RK3588 VEPU580、RK3576 VEPU510 与 RK3576 VPU720 的 fd/copied 输出逐字节相同。

V1.13 增加 `rk_mpp_encoder_advanced_config_v3_t`，保留 V1.10 V2 的
ROI/GDR/slice 布局和全部旧初始化函数。V3 增加 typed H.264 deblock 与
syntax：CABAC、transform-8x8、`cabac_init_idc`、constraint-set force/value
和 `base_layer_pid`。syntax 通过逐字段 flags 生效，未设置项继续使用
Main/High profile 推导值。`rk-mpp-rs-sys` 与 C11 ABI 快照已扩到 40 个导出。

V1.14 把 V3 末尾 32-byte reserve 改为同尺寸 H.265 syntax descriptor，因此
结构仍是 552 bytes，V1.13 字段 offset 不变。C 路径可覆盖 SAO luma/chroma、
deblock disable/beta/tc、跨 slice/tile loop-filter 和 auto-tile；布尔值由
`flags` + `values` 表达，未选字段继续使用默认。ABI 快照增至 41 个导出。

`h264_stream` 已改为 4 MiB 分块输入和增量 AU scanner。50 MiB/742 帧网络流
的峰值 RSS 从约 55.9 MiB 降到约 17 MiB，热态 wall median 1.576 -> 1.595 s，
输出仍为 742 帧；720p planar/NV12 哈希不变。同一改动让 SDK 在 SPS 到达后
刷新 H.264 reorder depth，并暴露 `Decoder::max_num_reorder_frames()`。
主 `h264dec` 也改用该 scanner，并保留 depth-3 ring：同一长流 wall median
0.665 -> 0.665 s，峰值 RSS 59.5 -> 20.4 MiB，742 帧输出不变。

官方 MPP drop-in 兼容层的第一个 slice 已落地：`mpp_create/init/destroy`、
格式查询和官方 `MppApi` 函数表布局已通过 vendor `rk_mpi.h` consumer。
第二个 slice 已接入 packet、frame、buffer 所有权和 H.264/H.265/MJPEG
CPU NV12 解码：`decode_put_packet` 把 packet 复制进现有 SDK session，
`decode_get_frame` 返回带 packed NV12 `MppBuffer` 的 `MppFrame`。
RK3588/RK3576 的 15-frame H.264、单帧 H.265 和单帧 MJPEG 输出均与 C MPP
逐字节一致（SHA-256 分别为 `22a8…23f7`、`fced…05e0`、`98c5…89f8`）。
第三个 slice 已把最小 packet meta（`KEY_OUTPUT_FRAME`）接到 advanced
MJPEG 路径：vendor `mpi_dec_test`
二进制无需重编译，通过 `LD_PRELOAD` 切到 rk-mpp-rs 后，H.264 640×480
6 帧、H.265 和 MJPEG 在 RK3588/RK3576 都成功，输出 SHA 与原生 MPP 相同，且
advanced MJPEG 不再报告 frame identity mismatch。vendor log/cfg/OSAL
辅助符号后来也已由本库提供；未修改的 `mpi_dec_test` 和 `mpi_enc_test`
现在都能只加载 standalone `librk_mpp_rs.so` 运行。
RK3588 的 standalone 解码门现已扩到 16/16：除 H.264/H.265/VP9/AV1/MJPEG
外，还覆盖 VP8 IP/altref、MPEG-2、MPEG-4 IP/IPBB、H.263 CIF/QCIF、两个
AVS+ vproc 完整周期和 AVS interlaced-frame 周期。
legacy frame 现在携带源布局；AVS vproc 以 NV12 直接进入官方工具，避免
NV12→I420→NV12 往返，MPEG/H.263 仍按需做 I420→NV12。
官方工具按 4 KiB packet 喂 elementary stream，MPP object 兼容路径用持久
parser state 增量消费完整 picture/VOP；MPEG-4 保留 VOL/timing，H.263 保留
I-picture gate，普通 SDK 仍保持 whole-stream/EOS 语义。每个 case 都校验精确
NV12 字节数，不依赖两个工具同时截断的错误输出；另有一格不带 `-n` 覆盖 EOS。
100/96 帧相对基准中，MPEG-2/MPEG-4/H.263 比上一版 replay 快约
18–19×/19–23×/53–56×，与 vendor C 的差距为 0.02–0.04 s（DMC performance、
CPU 4–7；非 80% OPP 绝对性能声明）。compat legacy 帧不再先生成 NV12 Vec
再复制进 DMA heap，而是保留 I420，object 层直接写入普通 CPU buffer 并在
同一 pass 交织 UV。100 帧 MPEG-2 的 Rust task-clock/instructions 已降到
50.1 ms/75.4M，vendor 为 66.5 ms/80.0M；perf 采样只剩文件写出和启动等
公共开销。

第四个 slice 已把本地 `MppMeta` 扩成真正的 key-value 容器：
s32/s64/ptr、frame/packet/buffer、`KEY_USER_DATA(S)` 深拷贝、
duplicate/ref/lifetime，以及 frame/packet 的惰性 meta 持有均已接入。
`scripts/board_mpp_compat_meta.sh` 在 RK3588/RK3576 通过；RK3588 还用未修改的原生
`mpp_meta_test` 跑了 2 线程 10 轮，返回值为 0。advanced decode 现在同时
处理 packet 的 `KEY_OUTPUT_FRAME` 和输出 frame 的 `KEY_INPUT_PACKET`。
advanced zero-copy frame 在后续 slice 已闭合；encoder packet kernel meta
也已在 2026-11-10 接通 VEPU580 H.264/H.265，详见下文。

第五个 slice 已补上外部 `MppBuffer` 所有权入口：
`mpp_buffer_import_with_tag` 接受 caller pointer 或 dma-buf fd，
`mpp_buffer_group_get_external`/`mpp_buffer_commit`/`mpp_buffer_get` 的
一次性 commit 流程已接通，fd-only 导入由兼容层独立 mmap 并在最终 put 时
解除映射。RK3576 C consumer 覆盖 pointer import、external group commit/get、
fd-only import、读写和 `MppBufferInfo` round-trip。

第六个 slice 把 internal `MppBuffer` 切到真实 dma-heap fd，并补 group
unused-buffer recycle：buffer 最终 put 后回到 group，下一次同尺寸 get 会复用
同一对象；`mpp_buffer_group_usage/unused` 和 limit size/count 现在有实际值。
RK3576 C consumer 校验 internal fd/info/sync、internal/external recycle、usage、
fd-only import 和 refcount。advanced zero-copy frame 继续保留为下一步工作。

第七个 slice 先把 advanced decode 的重复复制收敛掉：H.264/H.265/VP9/AV1/
MJPEG 兼容 decoder 请求 DMA-BUF `sdk::Frame`，`frame_fill_from_sdk` 直接从
decoder planes 按 stride 写入调用方 frame buffer，不再先分配临时 MppFrame
和临时 dma buffer 再复制。official `mpi_dec_test` 的三编码输出仍逐字节一致。

第八个 slice 进一步接入调用方 fd 注入：H.264/H.265/VP9 的 packed-output pool
和 MJPEG output override 接受调用方 dma-buf，`frame_fill_from_sdk` 通过
`fstat(dev, ino)` 识别同一 surface 并跳过最后一次 copy。RK3576 C consumer 的
H.265/MJPEG advanced frame identity 门通过；H.264 640×480 6 帧 advanced
输出 SHA `338214ea…f815` 与 C golden 逐字节一致。VP9 advanced injection
仍需板级矩阵，AV1 当前保留 plane-to-frame 写入。

第九个 slice 闭合 VP9 advanced injection。C consumer 现在按 IVF frame
header 逐 packet 提交，而不是把整个容器塞进一个 `MppPacket`；每次输出 buffer
只作一个硬件 surface，并至少存活到 DPB 释放，避免 reference surface 被下一帧
覆盖。VP9 decoder 只允许 `show_frame` 消耗 `KEY_OUTPUT_FRAME`，隐藏的
alt-ref/reference frame 使用内部 surface。同 dma-buf 快路径保留真实
`mpp_align_256_odd` stride 和 MPP 的 9/5 frame allocation size，避免 metadata
回退成 packed NV12。RK3588 与 RK3576 的 320×240 lossless 12 帧 advanced 输出均与
C MPP 逐字节一致，SHA `c5381bbf…3949a`；AV1 pool injection 继续保持未完成。

第十个 slice 闭合 AV1 advanced injection，并修复长流池复用。VDPU34x 把
调用方 dma-buf 导入 display pool，VDPU383 则替换 frame 的 output lease，
保留同一 frame 的 colmv/CDF。coded `show_frame` 直接写调用方 surface；
`show_existing` 展示的是已经解码的历史 reference，必须从它的历史 display
surface 复制，不能把 packet 的新 buffer 追认成旧帧。C consumer 同时接受
两个 SoC 的 AV1 stride：RK3588 为 320-byte，RK3576 VDPU383 为 448-byte，并按较大
容量分配。RK3588/RK3576 的 `av1_320x240_10.ivf` 10 帧 advanced 输出均与 C MPP
逐字节一致，SHA `88f34ea5…c5f8e`。`BufferPool::import` 现在复用 `clear()`
留下的 Empty slot，避免长码流超过池上限后错误返回 `Full`。

第十一个 slice 处理持续运动的多路回退。H.264 默认 planar 输出为了
I420 语义会逐帧做 NV12 chroma deinterleave；单路时仍比 C 快约 80%，但
4/6 个独立进程把 CPU copy/分配开销叠加后，聚合吞吐反而比 C 低约 10%。
DMA-BUF frame 模式不需要 CPU 生成 planar YUV，CLI 现在通过
`RKMPP_DECODE_DMABUF=1` 明确选择该测量路径（只允许 `/dev/null`，
避免把零字节文件伪装成 YUV）。同时删掉 DMA 路径里无意义的映射与
`DMA_BUF_SYNC`。4 路从中位数 1048 提升到 1240 aggregate FPS，
6/8 路与 C 基本持平，24 路每路仍有 65.7 FPS。固定 200 µs 非阻塞 poll
改成 `yield_now()` 的单次实验反而把单路降到 305 FPS，已回退。

多 job 提交环的稳态资源也已收紧。H.265 解码以前对每个画面重新
`dma_heap_alloc + mmap` 一块 padded bitstream buffer；现在 job 被 reap 后
把映射放回 decoder，下一次 submit 直接复用，仍由 pending frame 持有到驱动
报告完成。H.264 IDR 仍清空 DPB 和所有 slot，但不再调用 `BufferPool::clear()`
提前释放空闲 surface；“新鲜 IDR”由最后一个 lease 的引用计数保证，而不是
靠重新分配。两者都不改变输出语义，只移除 CPU/内核固定成本。

Annex-B 扫描也改成共享热路径。H.264/H.265 先用 libc `memchr` 跳到零字节
候选，再确认 `00 00 01` / `00 00 00 01`；emulation-prevention 检测复用
H.264 已按 64-bit 构造的 two-zero detector。RK3588、80% OPP cap 下，1200 帧
流量的 task-clock 从 H.264 `419.2 -> 379.6 ms`、H.265
`419.0 -> 379.1 ms`，各降约 9.5%；24 路 H.264 并发
`1764 -> 1733 ms`。单路的帧率仍主要受硬件节拍限制，因此这里只把 CPU
下降作为主要证据。

H.265 的整段 AU 定位进一步改为 header-only：`split_access_units()` 只读取
NAL type、两层 NAL header、offset 和原始 payload 范围，不再为了判断
`first_slice_segment_in_pic_flag` 对每个压缩 NAL 做完整 EPB 重写和分配。
具体 AU 进入 decoder 后仍走原来的完整 `split_annex_b()`，因此参数集和
slice header 的解转义语义不变。RK3588 80% OPP cap 下，1200 帧 task-clock
`381.8 -> 368.1 ms`（-3.6%），instructions `836.3M -> 784.8M`（-6.2%）；
并发墙钟持平，因此只按解析 CPU 成本记账。

H.265 现在也提供与 H.264 对称的 parsed-AU 接口：
`split_parsed_access_units()` 返回原流切片和相对 offset 的 NAL header，
`Decoder::decode_parsed_access_unit()` 直接消费该结果，不再重扫 start
code。CLI 已切到这个路径。输出与旧接口相同；1200 帧交错墙钟差异只有
约 0.1%，因此这是 API/集成能力改进，不宣传为吞吐优化。

Host CI 也已补齐。`.github/workflows/ci.yml` 在 Ubuntu 上执行
`cargo fmt --check`、workspace/all-targets clippy 和完整 host tests；硬件测试
继续通过 `#[ignore]` 与板级脚本分离。为此修复了 macOS 的 `c_char` cast、
Linux affinity API 的平台门，以及 `rk-mpp-rs-sys` 在只有 `.a` 时错误坚持
dylib 链接的问题。`native_smoke` 是 Linux 动态库门，在非 Linux host 上跳过。
同轮 clippy 还发现 AV1 4:4:0 的 `subsampling_y` 重复判断，已改为同时检查 x/y。

板级脚本的可移植性入口也已收紧。`setup_board_refs.sh --check` 只验证
SSH、`/dev/mpp_service`、dma-heap、passwordless sudo、两套 C oracle、
ioctl tracer 和 corpus，不再静默创建或复制；`board_regress_all.sh`
默认把 unexpected `SKIP` 当作失败，只有显式 `ALLOW_SKIPS=1` 才接受。
RK3588 的 `--check` 通过并看到 676 个 corpus stream；没有本地 oracle/corpus
的另一块板会明确退出非零。最低 Debian/kernel/device/sudo 要求和所有路径
override 已写入 `docs/board.md`。

性能脚本也遵循同一规则：decode/H.264 encode/H.265 encode benchmark 在
`RUST_BIN`、C oracle 或 fixture 缺失时默认非零退出，路径均可用环境变量
覆盖；只有显式 `ALLOW_SKIPS=1` 才允许部分测量。这样 `board_perf_gate.sh`
不会把缺文件误判成“没有该行”的软跳过。

H.265 文件输入也改成与 H.264 相同的双 surface DMA 管线：reader 线程填下一
幅，VEPU580 编码当前幅，`h265-gop` 通过 surface fd 直接提交，不再先读进
临时 `Vec` 再复制。RK3588、80% OPP 下，120 帧 720p 的 wall time 从 415 ms 降到
302 ms，task-clock 从 182.4 ms 降到 99.8 ms，system time 从 137.7 ms 降到
58.5 ms；前后 H.265 流的 SHA-256 都是
`a2865a07a64d266073072d75eea21433e9e5ef09a76acd4034a146f38188d20a`。
五次交错 60 帧矩阵中，H.265 在 320x240、640x480、720p、1080p 分别比 C 快
39.1%、77.3%、27.1%、35.7%。

H.265 session 的 CPU fallback 输入原先仍是 `system` 并在每帧 copy 前后发
两次 `DMA_BUF_SYNC`。现在与 CLI 一样使用 `system-uncached`，uncached mapping
上的两次 sync 也删除。RK3588 80% OPP、500 帧 720p 七次采样的 wall median
从 2.379 s 降到 1.412 s，user+sys 从 1.287 s 降到 0.455 s；只切 heap
时是 2.006 s，说明主要收益来自去掉 ioctl。旧/新码流均为
`6fea6748…da157dc`。H.264 做同类删除没有稳定收益，已撤回，不能机械套用。
H.265 aggregate feedback 又收紧了清零范围：`Statistics::from_words` 需要
word 0/1/2/3、64–84，原先每帧 `fill(0)` 整个 180-word STATUS。现在共享
索引表只清零并累加 0、3、64–84 共 23 words，word 1/2 随后由 SSE 覆盖。
64–84 必须保留 `md_sad_b16num[4]` 和 `madi_b16num[4]`，否则 scene-tune
每帧都看到空直方图；500 帧 wall median 仍为 1.318 s，user+sys 0.366 s，
码流哈希不变。

上游 `mpi_enc_test` 的 `MPP_SET_VENC_INIT_KCFG` 已接通。兼容层复制
type/coding/max dimensions 到 `mpp_init`/`MPP_ENC_SET_CFG`，保留所有已知
kernel-only key，遇到未知 key 返回 `MPP_NOK`；type/coding mismatch 返回
`MPP_ERR_VALUE` 且不丢弃待用配置。`scripts/board_mpp_kcfg.sh` 用上游头文件
编译 `tools/mpp_kcfg_probe.c`，在 RK3588 以 `librk_mpp_rs.so` 作
`librockchip_mpp.so.1` 完成 mapping/mismatch/unknown-key 三组验证。完整
`mpp-develop/mpi_enc_test` 的 KMPP object 模式仍受内核能力限制；当前
RK3588 没有 `/dev/vcodec` 或 `/proc/kmpp` 的 ctrl-cfg 能力，工具会在进入
Rust 编码路径前拒绝 mode 1/2。边界与 key 审计写在 `docs/KCFG.md`。
MPP option trie 现在兼容 `0x47` 旧四参数 `mpp_opt_setup` / 二参数
`mpp_trie_add_info` 的 direct `MppOptInfo` 上下文，以及 `0x20` 三参数 add
的 24-byte `MppTrieInfo v0` `{name, ctx, index, str_len}`；通用 object trie
继续使用当前 packed 4-byte 布局。旧官方 `mpi_enc_test` 的
H.264/H.265/MJPEG 门仍全通过。`/usr/local/bin/mpi_enc_test` 不含
`kmpp_mode`；真正导入完整 object ABI 的是
`mpp-develop/build/test/mpi_enc_test`。该工具现在可用 Rust 库完成默认
MPP 路径：`-kmpp_mode 0` H.264 单帧 3 NAL 与新版 vendor 完全一致
（`e18fe707…14bdbd`）。H.265 在补齐 full-range、CU-QP-delta、AQ/ROI/MD
和 QP-bias 默认值后，4 NAL 也与新版 vendor 完全一致
（`119bbf79…caa76d5`）；ioctl trace 仅剩 dma-buf 地址字不同。60 帧单 GOP
的 H.264 62 NAL（`6a8cb407…3e3544`）和 H.265 63 NAL
（`7eed6267…138b42`）也完全一致。OSD、ROI 和 `user_data_enable` 已通过
MppMeta 接入；2 帧 user-data 为 H.264 10 NAL、H.265 11 NAL，全部一致。
userspace `KmppBuffer` 已实现分配、setup、`KmppShmPtr` 导出和释放；kernel
buffer-group ioctl 仍是平台边界，完整 KMPP object ABI 尚未闭合。

VEPU580 H.265 的 scene-tune 状态机也已与新版 object 路径闭合。H.265 与
H.264 共用状态转移，但 mean-MADP 门是 `ctu_avg_madp_thd` 的
896/640/384，不是 H.264 的 192/128/64；`fb->st_mb_num` 则来自 STATUS
`st_bnum_b16.num_b16`。row 3 必须同时覆盖 WGT 1479–1482/1496/1498/
1499/1501/1503 和 RDO 2048/2065–2067/2075–2077/2095–2097/
2105–2107/2135–2137/2167/2169。320x240 运动 fixture 的 300 帧跨 5 个
GOP，在 107/227 帧进入 row 3 后回到 row 2；C/Rust 为 315 NAL 且整文件
SHA-256 都是 `d577adc9…32edea`。ioctl trace 的 300 个 job 只剩 13 个
dma-buf 地址字不同，所有非地址寄存器一致。复跑命令：

```bash
BOARD=my-rk3588-host ./scripts/board_h265_scene_tune_300.sh
```

`tune:scene_mode` 现在从 vendor `MppEncCfg` 进入公共 `EncoderConfig`，接受
0/1/2。H.265 IPC 模式（1）会从 default 的 row 2/3 切到 row 0/1；row 0
还包含 RC_KLUT `chrm_klut_ofst=3`，row 1 保留 default 的 0。row 0/1 的
WGT/RDO 值不是按源码猜的，而是从同一高噪声 fixture 的 C trace 差分得到。
RK3588 160-job 门禁中 WGT/RDO/OSD 逐字一致，row 0 在第 0 帧、row 1 在第
5 帧切换；H.264 的 `H264_SCENE_MODE=1` 也已用 60 帧验证，C/Rust 64 NAL
一致，整文件差异仅为 version-SEI 文本。`h264e_probe` / `h265e_probe`
新增 `SCENE_MODE` 环境变量用于复现：

```bash
BOARD=my-rk3588-host ./scripts/board_h265_scene_mode_ipc.sh
BOARD=my-rk3588-host ./scripts/board_h264_scene_mode_ipc.sh
```

`hw:qbias_en/i/p` 也已进入 typed config。H.264 的 fallback 是 683/341，
H.265 是 171/85；`en=0` 时不覆盖 scene row 的默认 bias。H.264 60 帧
64 NAL 与 C 一致；H.265 在 `en=1` 和 `en=0` 两格中 WGT/RDO 都与 C
逐字一致，word1484 分别为 `0x190c8` 和 `0x15480`：

```bash
BOARD=my-rk3588-host ./scripts/board_h264_qbias_vs_mpp.sh
BOARD=my-rk3588-host ./scripts/board_h265_qbias_vs_mpp.sh
```

`tune:vmaf_opt` 只覆盖 pre-intra/ATF row 3 和 RDO segment/smear，不能直接
当成完整 row 3：RDO 2167/2169 仍保持默认。2 帧 C/R trace 的 WGT/RDO 已逐字
一致，复跑：

```bash
BOARD=my-rk3588-host ./scripts/board_h265_vmaf_opt_vs_mpp.sh
```

原生 `h265-gop` 也暴露了同样的 tunable：

```bash
H265_SCENE_MODE=1 rk-mpp-rs h265-gop in.nv12 out.265 320 240 300 26
H265_QBIAS_EN=1 H265_QBIAS_I=200 H265_QBIAS_P=100 \
  rk-mpp-rs h265-gop in.nv12 out.265 320 240 300 26
H265_VMAF_OPT=1 rk-mpp-rs h265-gop in.nv12 out.265 320 240 300 26
BOARD=my-rk3588-host ./scripts/board_h265_cli_tune_trace.sh
```

同一路径的当前吞吐已在 RK3588 80% OPP、DDR `performance` @2.112 GHz、
CPU6 固定下交错采样。300 帧多 GOP fixed-QP26 的五次中位数为：320x240
C/Rust wall 164/164 ms、user+sys 69/67 ms；1280x720 wall 868/802 ms、
user+sys 291/226 ms。320x240 为 wall 持平，720p Rust 快 7.6%，CPU 少
22.3%。
`scripts/board_bench_h265_object_vs_mpp.sh` 对 wall 和 user+sys 都执行
“Rust 不超过 C 的 105%”门槛，防止吞吐优势来自忙等或额外 CPU。
H.264 新版 object 路径用同一脚本测得：320x240 Rust 157 ms 对 C 152 ms
（慢 3.3%，仍在 5% 门内），user+sys 都是 67 ms；720p Rust 769 ms 对 C
822 ms（快 6.4%），user+sys 232 对 278 ms。实现已收敛为
`board_bench_object_encode_vs_mpp.sh`，H.264/H.265 入口只是薄包装。
1000 帧稳定态消除了 300 帧启动占比：320x240 七次交错中 H.264 Rust/C
wall 448/493 ms、user+sys 138/202 ms；H.265 457/484 ms、140/185 ms。
脚本现在接受 `SIZES=320x240,1280x720` 和 `FRAMES=N`。

`vdec_kcfg`/`venc_kcfg` 的句柄生命周期也已按 vendor 收紧：非法 type 返回
`MPP_ERR_VALUE`，`init_by_name` 只接受 `KmppVdec*Cfg`/`KmppVenc*Cfg`
规范族名，CTRL_CFG 初始化 `cmd/val/arg/ret`，flex base/set 仅允许 CTRL_CFG；
`set_flex` 同步写入 `flags=KMPP_CTRL_FLAG_FLEX` 与 size。RK3588 的扩展
`mpp_kcfg_probe` 和只使用官方头文件的 `board_mpp_migration.sh` 均通过。

H.264/H.265/JPEG 的独立 `*_vs_mpp.sh` 矩阵也统一走
`scripts/board_dependency.sh`：缺 reference binary/library 或 ioctl tracer 时
默认 `exit 2`，只有显式 `ALLOW_SKIPS=1` 才输出 `SKIP` 并返回成功。已用错误
`MPP_REF` 验证失败路径，并验证显式允许后的跳过路径。

同一依赖门已扩到 H.264/HEVC/VP8/VP9/MPEG-2/MPEG-4/H.263 回归、损坏流、IEP2、
VDPP2 register/hardware、官方头文件 migration、compat data 和 RK3576
H.264 edge corpus。相关脚本都支持
`MPP_BIN`/`MPP_LIB`/`MPP_REF`/`MPP_SRC` 覆盖；只有硬件本身不存在等平台
边界才保留无条件 `SKIP`。

`mpp_enc_ref_cfg_check()` 也已从“只验句柄”改成真实层级校验：16 项上限、
LT index 唯一且小于 16、temporal id 0..3、LT/ST mode 分离、首尾 ST 必须为
temporal-zero reference；负 repeat 按 vendor 行为归零。`set_cfg_cnt` 现在
是容量设置并清空 live entry，`add_*` 从当前尾部追加，copy 复制完整层级。
`get_cpb_info` 通过移植 vendor 的虚拟 CPB dryrun 计算 DPB/LT/ST/temporal/GOP，
IPPP 与 TSVC2/3/4 的输出逐字段一致。`init/reset` 默认恢复为 vendor 的单帧
forward-reference IPPP。LOG、JSON、TOML `apply/apply_file/extract` 均已实现。
RK3588 上 LOG/JSON 与 vendor 双向解析一致；TOML 导出与 vendor 逐字节相同，Rust
可回读，但 vendor 对自己导出的 TOML 仍返回解析错误，已登记为 C bug。
reference scheduler 也补上了上一 ST、指定 LT index、倒数第 N 个 ST 三种合法
mode。standalone 的 TSVC2/3/4 与 64 帧 IGOP 重启流继续逐 NAL/重建一致。

`MppEncCfg` 的 LOG/JSON/TOML `extract/apply/apply_file` 也已从空字符串和
假成功改为真实实现。本层保存的数值标量全部可往返；`set_st/get_st` 现在保存
`rc:hier_qp_delta`、`rc:hier_frame_num`、`hw:aq_step_i` 和 `hw:qbias_arr`
四个已知长度数组。未知数组大小返回 `MPP_NOK`，解析失败不会部分写入。
RK3588 原生 vendor 的完整配置与 Rust scalar/array 配置已做三格式双向读取验证。
`rc:hier_qp_en/delta/frame_num` 也已沿
`MppEncCfg -> MPP_ENC_SET_CFG -> RateControlConfig -> Vepu580H264` 真正进入
RC 模型；`board_mpp_hier_qp.sh` 的 33 帧 CBR 流在剥离 version-SEI 后与
`/tmp/mpp-old` 逐字节一致。
同一路径也已接入 `hw:aq_step_i`：16 项自定义 I 帧 AQ 表会写入 VEPU580 的
`RCKLUT.aq_step`，P 帧继续使用参考的独立默认表。板级门增加自定义
`0..15` AQ 表，33 帧 CBR 码流同样与 old oracle 逐字节一致。

`rc:max_reenc_times`、`rc:super_*`、`rc:priority`、`rc:drop_*` 和
`rc:debreath_*` 也已通过 `H264RcTuning` 进入 H.264 运行时配置。
`board_mpp_rc_fields.sh` 用未修改的 `rc_probe` 验证两格：12 帧 debreath 与
6 帧 super-frame reencode；剥离 version-SEI 后均与 `/tmp/mpp-old` 逐字节一致。
同一门再加低码率 `DROP_MODE=2` 的六帧 PSKIP 格，也已逐字节通过。

普通 `MppEncCfg` 与 `MPP_VENC_KCFG_TYPE_ST_CFG` 现在共用同一套
`mpp_enc_cfg_set/get/extract/apply`。补齐 `mpp_enc_cfg_objdef` 和
`kmpp_obj_to_entry` 导出后，未修改的 vendor `mpp_enc_cfg_test` 在 Rust 库上
达到 **9/9 success**，包括 scalar/array、LOG/JSON extract/apply 与 kobj
roundtrip；LOG/JSON/TOML vendor/Rust 双向读取仍为 **3/3 PASS**。ST_CFG
句柄本身也支持四个已知数组的 `set_st/get_st/extract/apply`，并有独立 C
探针对 LOG/JSON/TOML 各做一次 scalar+array 往返；`mpp_enc_cfg_deinit()`
按 vendor 用法释放该句柄。

H.264 每帧的 4,160-word STATUS 数组和 Batch 请求链改为 session 内复用。
RK3588、80% OPP、7 次交错 500 帧 720p 中位数从 1140 ms 降到 1055 ms，输出
7,031,305 bytes 完全一致
（`68715bd450098e6559911a6e1fd9837fa142fd29df4ec8a3707932e2d5617c05`）。
H.264 输出布局现在用 `Cow` 复用无 temporal prefix 的 hardware payload，
省掉一整个码流的临时复制。同一 RK3588 80% OPP 条件、7 次交错 500 帧 720p
中位数从 1088 ms 降到 1073 ms（约 1.4%），输出 SHA-256 不变。

高层 `Encoder::submit_frame()` 只需要 packet view，因此 H.264/H.265 新增
crate 内部 queue 路径，不再为每帧同时构造公开 `EncodedFrame::annex_b`。
公开 `encode_frame*()` API 仍保留完整 Annex-B 字段。RK3588 上旧/新
`sdk_encode` 各编码 500 帧 720p，H.264 SHA-256
`68715bd4…617c05`、H.265 SHA-256 `ae613590…469783` 均完全一致。
`examples/sdk_encode_stream` 同时提供固定内存的流式示例。
尺寸已按编码器对齐要求的 NV12 输入现在直接借用调用方 slice；旧路径会先分配
完整 coded frame 再逐行复制，随后 HAL 又复制进 DMA-BUF。RK3588、80% OPP、11 次
交错 500 帧 720p H.264 高层 API 中，user CPU 中位数从 0.341 s 降到 0.201 s
（约 41%）。wall time 受硬件等待波动影响，不作为该优化主指标；H.264/H.265
输出 SHA-256 均保持不变。
非 16 对齐的 NV12 也不再创建 padding staging frame。H.264/H.265 直接把
tightly packed 可见行写入现有 DMA 输入，padding 只在首次调用清零并在后续帧
保持；H.265 reencode 期间同一 prepared input 会跨所有硬件尝试复用。
RK3588、80% OPP、11 次交错 1920x1080 60 帧测试中，H.264 user CPU 中位数从
0.101 s 降到 0.028 s（约 72%），H.265 从 0.116 s 降到 0.046 s（约 60%）。
两者旧/新码流均逐字节一致：H.264 为 3,212,736 bytes /
`81c37cd1ab6b6f750ff54476fbd6c523a3ce2dc45e91e32c8b0206c62167d7bb`，
H.265 为 2,517,365 bytes /
`0b1104b9a347653b7310f00b20cf7bef81652cdbe7adf8be4e844bdcc8ccaad5`。
`MppEncCfg` 的 `rc:refresh_en/mode/num` 现已映射为 H.264/H.265 typed
`GdrConfig`。`board_mpp_enc_cfg_gdr.sh` 用未修改的 vendor H.264/H.265 probe
只设置这些对象键，并与 Rust CLI 比较：H.264 35 NAL 一致
（version-SEI 仅 framing 不同，`09b7bc26…4367ca8`），H.265 stream 逐字节一致
（`bc22eb33…454f035`）。
H.264 `h264:dblk_disable/alpha/beta` 及兼容的 `deblock_*` 别名也已映射到
typed `H264DeblockConfig`，offset 限制为 `-6..=6`。同一 deblock 板级门用
Rust standalone 链接 vendor probe，alpha 与 disable 两个对象配置格均与 Rust
CLI 的 5 NAL 一致（version-SEI 仅 framing 不同，
`2730e887…10c697d4`）。
`h264:cabac_en/trans8x8/cabac_idc` 也已映射到 typed H.264 syntax，并修复 MPI 对象层
错误拒绝 Main profile 77 的旧检查。vendor/Rust 对象 probe 各通过两格：
Main-CAVLC/no8x8 `5c83b5e8…85e8c1ff`，High-CABAC/no8x8
`349ac658…d4d2a5`。两帧 Main-CABAC 的 idc1/idc2 也为 6 NAL 逐字节，
分别为 `47e28d89…3862a80` 和 `85087619…5c76b6c99`；软件 P_SKIP CABAC
上下文同步采用 C 的 `skip_init_state[3][2]`。
`h264:constraint_set` 也已按 C 的 `(force_mask << 16) | value` 语义实现：
只覆盖 force mask 指定的 SPS constraint 位，其他 profile 默认位保留。
set1=0 `0x020000` 和 set0=1 `0x010001` 两格的 vendor、对象和 typed CLI
均逐字节一致。
`h264:base_layer_pid` 已接入 VEPU580/VEPU510 的 type-14 prefix：
`priority_id = base_layer_pid + temporal_id`。RK3588 硬件门使用 base=5、
temporal_id=3，验证生成 priority_id=8；值域限制为 0..=56。
H.265 `h265:dblk_*`、SAO、`lpf_acs_*` 与 `auto_tile` 也已映射到
typed `SyntaxOptions`；VEPU510 对非默认 syntax 明确返回 unsupported。
独立对象路径门的两格均与 typed CLI 逐字节一致：
`1263cb1f…f647b4`（beta/tc + chroma SAO + loop filter）和
`1cef6ee0…b8171d`（SAO luma/chroma off）。
兼容对象现在也接受 `prep:format=MPP_FMT_YUV420P/NV21` 和 format-4/5
MppFrame。`board_mpp_enc_cfg_i420.sh` 用同一 `testsrc` 经 NV12/I420/NV21
三条对象路径分别编码 H.264 和 H.265：H.264 5 NAL 与
`073c6215…f00b435e` 一致，H.265 与 `df2395b1…be2f4b62` 一致。
compat context 现在持有可复用的 `encoder_input_scratch`，每帧 NV12/I420/NV21
整理只调整容量和内容，不再重新分配输入 Vec。25 帧 H.264/H.265 对象 GDR
连续会话仍分别保持 `09b7bc26…4367ca8` 与 `bc22eb33…454f035`。
在此基础上，internal dma-heap NV12 frame 不再进入 scratch：无 OSD/custom RC
且 coded stride/size 匹配时，`encode_put_frame` 直接借用 `MppBuffer` 的现有
fd 同步提交，既不复制整帧，也不逐帧 `dup/mmap`；I420/NV21、OSD、custom RC
和几何不匹配仍使用 CPU fallback。RK3588 80% OPP、120 帧 720p 的
`DmaBufInput<'a>` 现在保存 `BorrowedFd<'a>`，不 dup、不 mmap；板级测试证明
dup fd 在 VEPU580 上会 `int_sta=0`，原始 fd 才能正常提交。
`rk_mpp_encoder_submit_frame_dmabuf()` 同样直接借用调用方 fd；RK3588 6 帧 `strace -f -e
trace=fcntl` 中 `F_DUPFD_CLOEXEC` 为 0，`board_sdk_encoder_dmabuf.sh`
在 strace 可用时会强制此条件。
`board_mpp_enc_cfg_dma_input.sh` 中 H.264/H.265 分别保持 126/125 NAL 一致，
码流为 `2fdcd6ed…bcbb2c3` 与 `1865f014…f7af84b`。H.264 9 次交错采样的
wall median 为 273 ms 对 CPU fallback 311 ms（fallback 呈 306–449 ms 双峰），
user+sys median 从 95 ms 降到 59 ms。

VEPU580 H.264/H.265 OSD 也支持 direct fd：OSD table 与源画面独立，寄存器
会同时带上 OSD offset。`board_mpp_osd.sh` 现在跑 vendor、direct 和 forced
fallback 三路；H.264 6 NAL/YUV `1055ca00…73b22a`、H.265 7 NAL/YUV
`6f44da15…e5b176`。通用 `compare_streams.py` 已能识别 H.265 的 build-identity
PREFIX_SEI，避免把两路版本文本当数据差异。实测 cached DRM 输入的 500 帧
720p OSD direct 为双峰且 median 1.795 s，fallback 1.428 s，因此 OSD direct
只通过 `RKMPP_COMPAT_OSD_DMABUF=1` opt-in；默认仍是 CPU fallback。
`RKMPP_COMPAT_CPU_INPUT=1` 用于强制 fallback A/B。
Rust API 同步补齐 `DmaBufInput::with_osd(config)`：VEPU580 H.264/H.265
会把 OSD 表随 fd 帧提交，VEPU510/JPEG 返回 unsupported。板级 ignored gate
对 copied/dma-buf × 无 OSD/有 OSD 共四路做逐字节比较并通过。

H.265 也把 aggregate STATUS、per-tile STATUS 和 Batch 改为 session 内复用。
RK3588、80% OPP、7 次交错 500 帧 720p 中位数从 1111 ms 降到 1069 ms，输出完全
一致（`ae613590ee5b546f89d76dad1ecaa1528da292abfbd4e92303cc84c93b469783`）。

H.264/H.265 的 STATUS 全范围是内核 `SET_REG_READ` 目标，硬件会逐 word 覆盖，
因此去掉了每帧 16 KiB 的清零。7 次交错 500 帧 720p 中，H.264 从 1098 ms
降到 1052 ms，H.265 从 1129 ms 降到 1075 ms，码流哈希不变。

VEPU580 输出 packet 改用 `system-uncached` DMA heap，CPU 读取前不再对每帧
发 `DMA_BUF_SYNC` START/END。RK3588、80% OPP、7 次交错 500 帧 720p A/B 中，
H.264 从 993.284 ms 降到 968.188 ms（-2.53%），H.265 从 1043.576 ms 降到
1018.445 ms（-2.41%）。两路输出分别保持
`0a039e02…a2bc6a2e` 和 `ae613590…c93b469783`；H.264 low-delay 5/5 继续
逐 packet 一致。H.265 low-delay 也已恢复为 4/4：脚本先探测所选 vendor
头文件中是否存在 `EncRcTaskInfo.bit_target_fix`，再向 C probe 传入对应的
`CUSTOM_RC_HAS_BIT_TARGET_FIX`；码流只剥离 build-identity SEI 文本后逐字节
比较。BY_CTU、BY_BYTE、40-packet FIFO 与六帧 short-term 四格均通过。

`h265-gop` 的输出也改为逐 access unit 写 1 MiB `BufWriter`，不再在内存里
累计整条码流；`/dev/null` 基准仍跳过文件写入。120 帧 720p 输出 SHA-256 保持
`a2865a07a64d266073072d75eea21433e9e5ef09a76acd4034a146f38188d20a`，
60 帧 H.265 四尺寸吞吐门仍为 4/4 PASS。

`h265-gop-rc` 与 `h265-rate` 也接入双 surface DMA 输入和逐 AU 输出。RK3588、
80% OPP、120 帧 720p、五次交错采样中，GOP-RC 从单核 387 ms 降到 279 ms，
模型 CBR 从 384 ms 降到 273 ms；两路 pipeline 与强制单核输出分别保持
`7c4e93db…c11f65`、`bb99af68…d7a6ad03` 完全一致。

第十二个 slice 去掉未修改 `mpi_dec_test` 对 vendor MPP 库的依赖。
`librk_mpp_rs.so` 现在同时导出 `mpp_debug`、日志入口、环境/时间/OSAL、
SoC 识别、自旋锁、`MppDecCfg`、singleton、trie、kmpp objdef 和 cfg 兼容对象；
`mpp_opt` 注册的 option 元数据按 C 的 `MppTrieInfo + name + ctx` 布局返回，
所以官方测试命令行仍由原 binary 自己解析。脚本通过只含
`librockchip_mpp.so.1 -> librk_mpp_rs.so` 的目录运行并用 `ldd` 证明 vendor
库未加载。RK3588/RK3576 的 H.264 6 帧、H.265、VP9 12 帧、AV1 10 帧和 MJPEG
均与 vendor C 逐字节一致。日志入口随后改为 Rust naked ABI trampoline：
保留 x0-x7/stack 上的 variadic 参数并尾跳到本地 C `vfprintf` helper，
脚本断言官方 decoder 日志必须出现真实的 width/height/type。

第十三个 slice 把 standalone 从 decoder 扩到编码对象路径。未修改的
`mpi_enc_test` 现在能只加载 `librk_mpp_rs.so` 完成 H.264、H.265 和 MJPEG
单帧编码。补入的是一张真实的 key/value `MppEncCfg`，因此现有
`MPP_ENC_SET_CFG` 会通过 `mpp_enc_cfg_get_s32` 读取 codec、geometry、RC、
GOP、QP 等值，而不是依赖 vendor dlsym。另一个兼容点是旧版 `mpp_opt_add`
会把 `MppOptInfo *` 直接传给 `mpp_trie_add_info`、并从
`mpp_trie_get_info` 取回同一 `MppOptInfo *`；shim 按指针是否与参数地址
同区辨识这套旧 ABI，同时保持新版 `MppTrieInfo + name + ctx` 布局。
RK3588 的三编码 standalone 编码门通过：H.264/H.265 解码重建与 vendor
一致，MJPEG stream 逐字节一致。`MppEncRefCfg` 的
create/reset/set_cfg_cnt/add_lt/add_st/check 和旧 option trie ABI
也已接入。旧版 `mpi_enc_test` 实际把对象挂在
`mpp_enc_cfg_set_ptr(cfg, "rc:ref_cfg", ref)`，而不是另发
`MPP_ENC_SET_REF_CFG`；RK3588 现在会在 `MPP_ENC_SET_CFG` 前解析该指针。
`-g 1:60` 六帧 TSVC2 已映射到 VEPU580 的逐帧槽位、`nal_ref_idc`、
frame_num、POC 和 type-14 prefix：16 个 NAL 除 version-SEI 文本外逐字节
一致，六帧 YUV 的 SHA-256 为 `fec69971…64f0a4`；扩到 12 帧后 28 个
NAL 仍一致，YUV 为 `d9a3ed55…ecad7e63`。TSVC3 的 4-step 调度、temporal
layer 回指和 short-term reorder 也已闭合，12 帧 YUV 为
`7794eb88…488f8e2`。TSVC4 的 LT0 周期替换、LT reorder、IDR
`long_term_reference_flag` 和 P 帧 MMCO6 也已闭合，12 帧 YUV 为
`33923dd2…d1186a`；64 帧跨 `gop=60` 的 136 NAL 也一致，包含第二个
SPS/PPS/SEI IDR 前缀和参考状态重置，YUV 为 `a3ad1bc6…35a0593`。
RK3576 VEPU510 的 TSVC2 12 帧也达到 26 NAL 和 YUV
`77a92442…fe35ab` 一致。VEPU510 TSVC2/TSVC3/TSVC4 的 C packet assembly
冷启动不稳定：TSVC2 曾整段漏 prefix；TSVC3 的一次 18-NAL 冷启动没有
任何解码画面；TSVC4 的 5 次中 3 次与 Rust 的 26 NAL/YUV 完全一致，
另外两次漏 prefix，其中一次重建也不同。板级门会重试 C 直到取得稳定
oracle。相关项均已登记在 `docs/DIVERGENCES.md`。
官方 H.264 对象路径的 `MPP_ENC_SET_IDR_FRAME`、`HEADER_MODE` 和
`SEI_CFG` 已接通。帧 2/4 强制 IDR 在 RK3588/RK3576 均为 12 NAL 和 YUV
逐字节一致；`HEADER_MODE_EACH_IDR` 重发 SPS/PPS，`SEI_MODE_DISABLE`
不再静默吞掉。H.265 VEPU580 也已接通同一组控制：15 NAL 和 YUV 为
`b2765277…3c56607`，IDR 后的 POC 重置正确。RK3576 VEPU510 的 object
路径完整 15 NAL 码流/YUV 也与 C 逐字节一致，YUV 为
`7f3d4664…841231`。`MPP_ENC_GET_HDR_SYNC` 现在决定 direct 或
`mpi_enc_test` template；RK3576 `kmpp_objdef_get/register` 的当前 ABI 也已
修正，未修改工具通过 LD_PRELOAD 启动不再段错误。RC API 的查询、注册/替换、
按编码类型过滤和当前策略选择已在 RK3588/RK3576 与原厂逐行一致，并导出
`rc_api_add`/`rc_brief_get_all`/`rc_brief_get_by_type`；自定义策略的
逐帧回调已在 RK3588 VEPU580 H.264/H.265 中真实驱动硬件：fixed-QP 两帧
stream/YUV 逐字节；CBR 的 callbacks、RC 寄存器、RC SEI 和两帧
stream/YUV 也全部逐字节。
VEPU510 的内置 CBR/VBR/AVBR 已接通，用户自定义 streaming RC callback
也已在 H.264/H.265 上接通。RK3588 VEPU580 与 RK3576 VEPU510 的
fixed/CBR/VBR/AVBR 共 8/8 两帧码流和 YUV 均与 vendor 逐字节一致；
callback 字段、init 配置和 `bit_real/quality_real` 也逐行一致。
RK3576 使用 1.0.11 的 `RcCfg=260 / EncRcTaskInfo=136 / EncRcTask=336`
布局，RK3588 使用 `256 / 112 / 312` 的 2024 ABI 投影。`check_reenc` 主动
请求一次同帧型重编也已接通：H.264/H.265 在两板共 4/4，首轮 packet 被替换，
第二轮使用 callback 更新的 QP30 重跑同一输入/参考槽，最终 stream/YUV
和完整 callback trace 一致。第二帧 P→IDR 的帧型变更同样为 4/4；
H.265 精确复刻 vendor 的 P syntax + INTRA global config + IDR NAL
混合模板，H.264 则保持 vendor 实际输出的 P slice。H.265 该请求登记为
C-BUG：FFmpeg 只输出前两帧，第三帧因 POC 参考缺失终止；Rust stream
与 vendor 逐字节一致。重编与 drop/PSKIP 的组合仍未实现。
OSD palette 与 `KEY_OSD_DATA` 已接通 VEPU580：H.264/H.265 的 OSD
stream/YUV 均逐字节。H.265 原先在 OSD offsets 后重新赋值 FBC/output
offset 列表，导致 `reg3093..3099` 的 7 个 multi-offset 丢失；改为追加后，
stream SHA `625d464e…efc838` 与 YUV `6f44da15…3cbd310` 均与 vendor 相同。
encoder 输出 packet 的 kernel meta 已接通 VEPU580/VEPU510 H.264/H.265。
`scripts/board_mpp_packet_meta.sh` 在不改 vendor probe 的前提下对比
`set_enc_info_to_packet()` 的全部 board-visible 字段：8 类 block count、
SSE、PSKIP、intra、start/average QP 和 `KEY_MOTION_INFO`。RK3588 的 fixed-QP、
CBR 以及静止/持续运动两帧矩阵均逐 packet 一致；H.265 motion-info 的
640-byte caller view 也按 FNV-1a 哈希逐帧一致。该门禁同时抓出两个旧偏差：
模型把 H.265 I-QP delta 当成了 H.264 的 2，实际为 4；以及 SEI disable
分支没有把 rate-control 模式传入 PPS，导致 `cu_qp_delta_enabled=0`。两者
修正后 CBR stream/YUV 恢复逐字节。JPEG 的 start/average quality 都是
q-factor；VEPU2 不写 `KEY_OUTPUT_INTRA`，VPU720 写 `intra=0`，Rust 现按
实际 HAL 分支输出。
RK3576 VEPU510 由 STATUS 直接生成 block histogram、SSE、PSKIP、intra 和
start/average QP；H.264/H.265 的 moving/still fixed-QP 各 2/2 帧与 vendor
一致。1.0.11 的 H.265 fixed-QP 把 `quality_real` 直接回报目标 QP，而不是
`qp_sum / 1280`，这一固件差异也单独建模。VEPU510 streaming CBR 继续
明确 SKIP。
其余高级编码控制仍待接入。

```bash
# RK3588: focused C consumer + unchanged vendor mpp_meta_test
MPP_INCLUDE=/tmp/mpp-develop/inc \
MPP_META_TEST=/tmp/mpp-develop/build/mpp/base/test/mpp_meta_test \
./scripts/board_mpp_compat_meta.sh
# RK3576: focused C consumer against /usr/local/include/rockchip
./scripts/board_mpp_compat_meta.sh
```

encoder 对象流已开始接通：`MPP_ENC_SET_CFG` 通过 vendor `MppEncCfg`
读取 codec、size、profile、RC、GOP、fps 和 JPEG q-factor，再创建统一
`encoder::Encoder`；`encode_put_frame` 将 `MppFrame` 的 NV12 buffer 提交，
`encode_get_packet` 返回 owned `MppPacket`。在 RK3588，未修改的 vendor
`mpi_enc_test` 生成 H.265（SHA `1f0067…52e84a`）和 MJPEG（SHA
`08eb7c…88623`）与 C MPP 逐字节一致；H.264 单帧解码 YUV 为
`9d8c6f…24ac6fc`，与 C 相同。默认 MPP CPB 的 IDR-only reference 调度和
slice-header amend 已补齐：H.264 320×240 6 帧的 SPS/PPS、rc-SEI、IDR 和
P1–P5 VCL 与 C 完全一致，解码重建逐帧相同；只有 version-SEI 的构建文本
长度不同。RK3576 也已通过同一官方工具门：H.264 High 单帧和真实 6 帧的
NAL/解码重建一致，H.265 单帧和 6 帧的 NAL/解码重建一致，MJPEG 逐字节
一致。RK3576 使用
单独的 High CABAC/8x8 寄存器模板、MEIW `KEY_MOTION_INFO` buffer，以及
`mpi_enc_test` 的 H.265 参数集/无 SEI 对象流，不再复用直连 CLI 的 4710-byte
前缀格式。RK3588 当前默认 SEI 路径下，H.265 单帧和 6 帧的
VPS/SPS/PPS、rc-SEI、VCL NAL 与重建一致，version-SEI 仅比较 framing。
2026-11-21 的复验还补上了 object-path 默认值差异：FFmpeg 总会显式写
`h265:level`，所以 `0` 表示让 SPS 按尺寸抬到 2.0，并启用 temporal MVP；
未修改的 `mpi_enc_test` 不写这个 key，参考 `h265e_init` 保留 4.0 且关闭
temporal MVP。兼容层现在按 key 是否存在保留这一区别，VEPU580 缺省为 120，
VEPU510 缺省为 30。`CHECK_MOTION_INFO=1` 还会用 `tools/mpp_mdinfo_dump.c` 抓取调用方
buffer：H.264 1200-byte（SHA `655a3ef0…5955273`）和 H.265 1280-byte
（SHA `bfe492ba…09f462`）均与 C 逐字节一致。

新 V1 暂只承诺 NV12/I420；JPEG 硬件 RGB 输出存在不可重复的 8-byte writeback，
旧 `rk_jpeg_*` RGB 兼容接口保留。后续工作是把同一契约扩到其余解码器与编码器。

**当前主验证 SoC：RK3588。** 第二台 RK3588 验证机只保留为历史证据；后续同步、
构建和板级验证全部在主 RK3588 验证板（内核 6.1.57）执行。RK3588 重启后
`/tmp/mpp-old`、`/tmp/mpp-develop`、`/tmp/mpp_trace.so` 会被清理，权威副本仍在
`~/rk-mpp-oracles`，先运行 `scripts/setup_board_refs.sh` 回填。所有板级脚本默认
使用 `BOARD` 指定的 SSH alias；公开文档统一使用 SoC 名，不记录本机设备名。

RK3588 当前持久化使用硬件最高频率的 80% OPP：little 1.416 GHz、big 1.8 GHz，
governor 固定为 `performance`。pooled JPEG 必须保证 `MppService` 在任何共享
dma-buf lease 归还前关闭；旧顺序曾让驱动在 `mpp_dma_find_buffer_fd()` 自旋锁死。
当前顺序已通过 500 轮 async demo / 3000 jobs 压测。

本文以仓库现状为准；接手时先跑 `git rev-parse --short HEAD` 和
`git status --short`，不要相信本文记录的历史提交号。`README.md` 是最新状态速览，
`ROADMAP.md` 是逐项证据台账（权威），本文是"从哪继续、怎么不被坑"的操作手册。

---

## 0. 五分钟了解现状
一页能力矩阵（做了什么、怎么证明、哪些在 RK3588 不可达）：[CAPABILITY.md](CAPABILITY.md)。

**2026-11-21 FFmpeg drop-in 修复：** 未修改的 FFmpeg `c5e745e3c1` 现在可把
`librockchip_mpp.so.1` 解析到本仓库。FFmpeg 的首帧会再次发送
`MPP_ENC_SET_CFG` 补充 DRM stride；compat 层重建 encoder 后必须恢复
`MPP_ENC_SET_SEI_CFG` 和 `MPP_ENC_SET_HEADER_MODE`，否则会多出 version/RC
SEI。另一个问题是 FFmpeg 默认请求非 cacheable MPP DRM group，而兼容层原先
仍从 `system` heap 分配；CPU 可见数据正确，但 P 帧送硬件时会读到旧 cache。
非 `MPP_BUFFER_FLAGS_CACHABLE` 的 group 必须走 `system-uncached`。后续又补通
H.265 level/temporal-MVP/GOP-boundary 状态，以及 H.264 在 FFmpeg 未提供
`KEY_MOTION_INFO` 时的 `mei_stor=0` 与 scene-tune 单帧延迟。现在 H.264/H.265
的 12 帧门和 320x240/1280x720 的 60 帧单 GOP 序列都与 vendor C 整条码流
逐字节一致。VEPU580 H.264 固定 QP 兼容路径现在默认 depth-2 deferred queue；
每个 job 独立持有 batch/STATUS/output，三槽 recon 按 C 的 `0→1→2` 轮转，
pipeline 完成必须使用 blocking `POLL_HW_FINISH`。300 帧 720p 为
0.334 s / 898 FPS，C FFmpeg 为 0.349 s / 860 FPS，输出保持逐字节一致。
H.265 也已接入同一 queue contract，并补上 C `task_cnt == 2` 路径的
`reg0193_dual_core` 跨任务握手。此前 depth 2 的 912 FPS 来自未同步双核路径，
长 GOP 会在约第 41 帧分叉；现在 `txid/rxid/txe/rxe` 与 offset 9 按 C 的
`0→1→2→3→0` 序列写入，320x240/1280x720 的 60 帧单 GOP 均逐字节。300 帧
720p 为 0.444 s / 676 FPS，C 为 0.444 s / 676 FPS；Rust CPU user+sys
0.372 s，C 0.393 s。H.265 depth 2 现在是兼容路径默认。
外部 MPP DMA-BUF 的 CPU 映射也改为按需建立。300 帧 H.265 下 mmap 从 544
降到 244，Rust task-clock 从约 428 ms 降到 363 ms，instructions 为 327.4M，
与 C 的 327.3M 基本一致；pointer/info/read/write 与 CPU fallback 仍按需建立
映射并通过 C object gate。
原生 fixed-QP 文件编码也接入了 deferred queue。H.265 `h265-gop` 在普通
GOP 下默认 depth 2，300 帧 720p 为 0.430 s / 698 FPS，C FFmpeg 为
0.454 s / 661 FPS，320x240/1280x720 与同步路径逐字节。H.264 默认保持
同步；显式 depth 2 达到 0.331 s / 906 FPS，但 scene-tune 统计晚一帧，
输出未经多 GOP 同步逐字节断言，因此只作为性能 opt-in。
重复测试同时发现 120 帧/GOP60 跨第二个 IDR 后，C FFmpeg 会在不同进程产生
多个码流且解码像素不同；相同参数的 60 帧在 8 次冷启动重复中全部一致，因此
精确门使用一个完整 60 帧 GOP，不用不稳定的 C 结果定义 Rust 正确性。
回归入口：`scripts/board_ffmpeg_rkmpp.sh`（8/8）和
`scripts/board_ffmpeg_{h264,h265}_pipeline_bench.sh`。


> **2026-11-02 事件：** 第一次尝试 `scripts/board_corpus_sweep.sh`（corpus 全量
> 解码对照）时，RK3588 在跑完不到 10 条流后从网络消失，连续约 3 分钟 ssh 超时，疑似
> 内核/驱动被某条流挂住，需要物理冷启动。脚本已加固（`TIMEOUT_S` 默认 30 秒、
> `MAX_BYTES` 默认 256 MiB、逐条打印 verdict），但**尚未完成任何一次全量运行**；
> 重启后先用 `LIMIT=10` 逐段放量，并记录卡住的是哪一条流。

> **2026-11-02 第二台 RK3588 验证机事件：**
> `tests/data/av1_tiles_320x240_10.ivf`
> 会触发 `rk_vcodec: mpp_task_timeout_work ... processing time out!` 与
> `mpp_av1dec ... resetting`，并可导致整板重启（内核 6.1.141）。当时进一步做
> **只含第一个 2,351-byte coded frame 的 IVF 前缀**后，C 仍能输出一帧；Rust
> 报告一个输出但 YUV 为 0 字节，内核再次 reset/reboot。这不是 RK3588 全量结论的
> 自动延伸：主验证板是 6.1.57，第二台 RK3588 验证机是 6.1.141；同一 SoC
> 不等于同一驱动执行行为。
> 根因是 Rust 少了 C 的 CDEF/loop-restoration filter DMA base：
> `reg[85]`/`reg[91]` 为 0，STATUS `0x2109` 的 `sw_dec_bus_int` 正是总线错误。
> 同时对齐 C 的 uncached DMA32 表/帧、DMA32 stream 和 4:2:2 大小的 display
> allocation。修复后单帧 STATUS `0x1100`，完整 fixture 10 帧/1,152,000 bytes
> 与当前 C 逐字节一致，8/10-bit film-grain 硬件回归也通过。
>
> 修复后 tile fixture 已从 corpus quarantine 移回常规扫描。安全措施保留：
> 1. `POLL_HW_FINISH` 改为 5 秒上限（非阻塞轮询 + EINVAL 回退），避免进程永久挂住；
> 2. 修复合入前曾把该夹具在任何解码之前 `QUARANTINE`；stub 验证过带/不带
>    `td_` 前缀均不会启动解码器。根因修复后该 quarantine 已移除，fixture
>    重新作为 corpus sweep 的 AV1 tile 覆盖项。
> 复现时连带检查 `dmesg` 是否有 `mpp_task_timeout_work`；不要盲目重复跑。
>
> **安全诊断入口（2026-11-02）：** `examples/av1_dump_regs.rs` 现在可以在
> **不打开 `/dev/mpp_service`** 的情况下，为 tile fixture 的 11 个作业导出
> PicParams、输入 CDF、global model、tile-info、stream、film-grain 和 512-word
> 寄存器/地址偏移镜像。配套的 `tools/av1/compare_input_dumps.py` 与 C
> `DUMP_AV1_DATAS` 逐字节及逐字段比较，并归一化两边不同的 DMA fd：
>
> ```bash
> cargo run --example av1_dump_regs -- \
>   tests/data/av1_tiles_320x240_10.ivf 11 /tmp/av1-rust
> python3 tools/av1/compare_input_dumps.py /data/video /tmp/av1-rust
> ```
>
> 离线证据：导出器自行重放 Rust 的 DPB 槽位分配，不从 C snapshot 抄 current/
> reference slot；11/11 PicParams 仍与 C golden 相同，88 个镜像文件尺寸正确。
> 比较器在模拟一致输入上 11/11 PASS，并能在单字破坏时定位到具体寄存器。
> 第二台 RK3588 验证机的实际 C dump 已证明：作业 0-10 的 PicParams、global
> model、tile-info、
> split stream、film-grain、512-word 寄存器和地址偏移全部相同。作业 0-2 的输入
> CDF 也相同；作业 3+ 的 CDF 依赖上一硬件作业写回，普通 CPU 导出无法独立复现，
> 比较器将其记作 `DEFERRED`，不会掩盖其它镜像或寄存器差异。
>
> 第二台 RK3588 验证机恢复后先运行安全预检，不要直接跑 `av1dec`：
>
> ```bash
> BOARD=my-rk3588-host ./scripts/board_av1_presubmit_compare.sh
> ```
>
> 脚本会从当前提交临时打包源码，在独立的
> `~/rk-mpp-oracles/mpp-av1-dump-v2` 构建 C dump oracle，运行已知正常的 C 解码，
> 再运行 CPU-only Rust 导出器并比较全部输入镜像。它不会向 Rust AV1 提交硬件
> 作业；只有打印 `PRE-SUBMIT DETERMINISTIC INPUTS MATCH` 后才恢复板级 AV1 调试。

**2026-11-23 4K 编解码矩阵：** `scripts/board_4k_matrix.sh` 在 RK3588 上
**22/22 PASS**。矩阵覆盖 3840×2160 的 H.264/H.265/VP9 8-bit 单帧与三帧、
H.265/VP9/AV1 Main10、AV1 8-bit 单帧/三帧、AV1 10-bit、AV1 8/10-bit
film grain、H.264/H.265 编码、MJPEG 1080p/1440p/4K 编码和 4K MJPEG 解码；
普通解码逐字节对齐 C MPP 与软件 oracle，AV1 film grain 逐字节对齐
`aomdec` 的带 grain 输出，H.264/H.265/MJPEG 编码逐字节对齐 vendor FFmpeg。AV1 的软件 oracle
固定使用 `aomdec --rawvideo`，因为该板 FFmpeg 构建没有可用的 AV1 软件解码器。

这轮定位并修复了三个实际问题。第一，MPP object/FFmpeg 路径原先对所有解码器
请求 NV12 DMA-BUF，10-bit H.265/VP9/AV1 在解码器内被提前拒绝，FFmpeg 只能
看到 EOS；现在 10-bit 4:2:0 硬件 surface 暴露为 MPP `YUV420SP_10BIT`
（FFmpeg `NV15`），4K Main10 输出与 vendor 一致。第二，VP9 的帧分配器把
像素宽度当成了 10-bit 行字节数，4K 时 3,840-byte stride 不足以容纳
4,800-byte packed row；现在先换算字节再做 MPP 256-odd 对齐。第三，JPEG
object/FFmpeg 路径不应继承直接 `jpeg-enc` CLI 的自动多核 band 策略；
1080p/1440p/4K 未显式请求 split 时不再写 DRI，三档输出均与 vendor 逐字节。

复跑需要 high-bit-depth `vpxenc`；系统包没有高比特深度支持时，用
`VPXENC=/path/to/vpxenc` 指向启用 `--enable-vp9-highbitdepth` 的构建：

```bash
VPXENC=/path/to/vpxenc scripts/board_4k_matrix.sh
```

FFmpeg 4K 解码必须加 `-fps_mode passthrough`。`-frames:v 1` 会让 vendor 和
Rust 都可能因 EOF/时间基丢帧而写出 0 字节，这不是 Rust 独有回归。

扩展功能面由 `scripts/board_4k_features.sh` 统一回归，当前 **13/13** 门禁组
通过。它串起 H.264/H.265 的 RC、hier QP、ROI、slice、tiles、GDR、force-IDR、
OSD，以及 MJPEG 多核/旋转/RGB。4K H.264 BY_CTU/BY_BYTE 分别为 680/403 NAL，
H.265 为 260/205 NAL；H.265 4K auto-tile 为 9 NAL，GDR row/column 均为
17 NAL（1 IDR + 7 P + recovery SEI），OSD direct/fallback 与 vendor 的
6/7 NAL 和解码 YUV 一致.

这轮还修了 4K H.265 RC 的首帧策略：300 kbps 目标下 C 的 RC 会对首 IDR
re-encode，从 QP26 改为 QP29；Rust 旧路径只编码一次、把 QP32 留给下一帧，
导致首 IDR 20,615 bytes 对 C 12,150 bytes。现在 model-driven H.265 走同一
`reencode_quality` policy loop，并在 DMA-BUF 重试时保留 input fd。CBR/VBR/
AVBR 和 smart-GOP 在 320×240 与 4K 都逐字节一致。

## 第二台 RK3588 验证机（2026-11-02 起）

另一台 RK3588 验证板：内核 6.1.141、`/dev/mpp_service` 可用、`hw_support mask =
0x1009321a`，与主 RK3588 验证板相同。已在这台机器上复验：VP8 profile 1/3
与缩放关键帧、VP9 六格矩阵、H.265 三尺寸 13 格码控，结果与 RK3588 一致。

搭建要点（新板照做即可）：

1. **oracle 必须用 git checkout 构建，不能用源码 tarball。** 版本 SEI 由
   `CMakeLists.txt` 在 `${PROJECT_SOURCE_DIR}/.git` 存在时用
   `git log -1 --pretty=format:"%h author: %<|(30)%an %cd %s"` 生成；没有 `.git`
   时会写 `unknown mpp version for missing VCS info`，整条 H.265 码流就会在第 ~90
   字节差一个 SEI 文本。用 `git bundle` 传仓库再 `git checkout 80957d266` 即可。
2. **cargo 在这台板是 offline + Debian registry**（`~/.cargo/config.toml`），
   registry 里只有 `libc-0.2.169`。复制过来的 `Cargo.lock` 若锁定别的版本会解析
   失败；删掉锁文件让 cargo 就地解析即可。
3. 板级脚本通过 `BOARD` 选择目标 SSH alias。驱动脚本时从
   **另一个目录**（如 `~/rk-mpp-run`）运行，因为有些脚本会打包当前目录并在
   同一目标解压；在目标目录里运行会自己吞自己。
4. `ffmpeg`/`ffprobe` 需要 `apt-get install`，矩阵脚本的 NV12→planar 依赖它们。板上的 ffmpeg **没有 AV1 编码器**（无 libaom-av1），AV1 大尺寸 fixture 要在宿主生成后 scp，小尺寸已固化在 `tests/data/av1_{96x96,128x128,192x192}_10.ivf`。
  AV1 默认 CDF 表由 `tools/gen_av1_cdf.py` 从规范 HTML 生成：`curl -sL https://aomediacodec.github.io/av1-spec/ -o /tmp/av1spec.html && python3 tools/gen_av1_cdf.py /tmp/av1spec.html > src/reference/av1/cdf_defaults.rs`。
5. 第二台 RK3588 验证机上已有重新生成的覆盖语料
   `~/rk-mpp-oracles/corpus-gen`（59 个 payload；
   从当前提交重跑后会再写一个 `corpus-manifest.json`）。原 RK3588 的第三方 corpus
   无法原样复制；扫描时使用 `CORPUS=.../corpus-gen`，并确保从当前提交重新运行
   `scripts/gen_corpus.sh`，不要使用过期的 MPEG-4 容器副本。manifest 会在任何
   解码前校验文件集合、大小、SHA-256、IVF FourCC 和 raw elementary-stream magic；
   `.m4v` 若实际是 MP4 容器会直接失败。验证：`tests/data` 的 142 个文件写/读
   清单均通过，单字节修改和伪 `.m4v` 均被拒绝。
   当前 `corpus-gen4` 有 66 个 payload，AV1 扩展到 640x480 long-GOP、
   1280x720、1920x1080、Main10、多 sequence header 和 320↔640 动态分辨率；
   全量结果为 `pass=60 fail=0 named=6 skipped=0`。AV1 tile 已是普通 PASS。64×64 AV1
   在 C 上也会逐帧自恢复 reset，C/Rust 均输出十帧零图，不能单独归因于 Rust。
6. **H.264 QP 矩阵需要参考 harness 的 level 补丁。** `mpi_enc_test.c` 把
   `h264:level` 写死为 40，而 crate 的 Constrained Baseline 是 30；`board_h264_qp_vs_mpp.sh`
   现在会连同 profile/cabac/trans8x8 一起把 level sed 成 30，所以新板不需要预置打过补丁
   的 `/tmp/mpi_enc_test.c.orig`。在这之前，第二台 RK3588 验证机上 30 格全部因为
   SPS level 不同而失败。

```bash
cd ~/workspace/rknn-test/rk-mpp-rs
cargo test --workspace --all-targets --offline   # host：1364 passed / 0 failed / 46 ignored（59 suites）
cargo fmt --check
python3 scripts/check_corpus_manifest.py --write tests/data \
  --manifest /tmp/tests-data-manifest.json
python3 scripts/check_corpus_manifest.py --check tests/data \
  --manifest /tmp/tests-data-manifest.json
git log --oneline -10
grep -c '^- \[x\]' ROADMAP.md  # 已完成条目数
grep -c '^- \[ \]' ROADMAP.md  # 未完成条目数
```

板上验证（先设置 `BOARD=<ssh fixture>`，详见第 4 节）：

```bash
ONLY=gop ./scripts/board_h264_qp_vs_mpp.sh   # 固定 QP：6 格全 PASS 约 3 分钟
./scripts/board_h264_cbr_vs_mpp.sh           # CBR + VBR + AVBR 各 10 NAL 全同
./scripts/board_h264_level_vs_mpp.sh         # SPS level 矩阵 13 格
./scripts/board_h264_deblock_vs_mpp.sh       # deblock 3 格 + auto-QP 2 格
./scripts/board_h264_roi_vs_mpp.sh           # ROI/QP map 2 格
./scripts/board_h264_scene_vs_mpp.sh         # 硬切场景检测
./scripts/board_h264_slice_vs_mpp.sh         # 普通整 AU slice split 4/4
./scripts/board_h264_lowdelay_vs_mpp.sh      # 逐 slice packet 5/5
./scripts/board_jpege_restart_vs_mpp.sh      # JPEG restart（-t 8 + split env）
./scripts/board_jpege_lowdelay_vs_mpp.sh     # JPEG 逐 band packet 2/2
./scripts/board_jpege_multicore_vs_mpp.sh    # JPEG 默认多核 + 旋转 5/5
./scripts/board_jpege_stride_vs_mpp.sh       # JPEG stride 2/2
./scripts/board_h265_gdr_vs_mpp.sh           # H.265 GDR row/column 4/4
./scripts/board_h265_slice_vs_mpp.sh         # H.265 普通整 AU slice 3/3
./scripts/board_h265_lowdelay_vs_mpp.sh      # H.265 逐 slice packet 4/4
./scripts/board_h265_roi_vs_mpp.sh           # H.265 ROI/QP map 4/4
./scripts/board_h265_tile_vs_mpp.sh          # H.265 水平 tiles 2/2
./scripts/board_iep_modes_vs_probe.sh        # IEP2 全模式 + 原始 dst slots 9/9
./scripts/board_iep_formats_vs_probe.sh      # IEP2 420/422 SP/Planar 6/6
./scripts/board_iep_dect_vs_probe.sh         # IEP2 PD/DECT 逆 telecine + 几何边界 5/5
./scripts/board_vp_corrupt_vs_mpp.sh         # 损坏流 vs C 判决表
```

ROADMAP 顶层清单当前为 128 项完成、4 项未完成；未完成项包含硬件不存在、无
conformance 流和冻结的功能前沿，不能直接用勾选比例估算代码工作量。已完成路径里
H.264/H.265 编码、JPEG、IEP2 和主要解码矩阵都有板上逐字节证据。
AV1 tile 在第二台 RK3588 验证机上的超时已作为历史事件记录并完成根因修复；
当前工作板回到主 RK3588。
H.264/H.265 普通 slice split 与低延迟逐 slice packet 都已在 RK3588 上通过。其它已知差异和
下一步证据见 `docs/DIVERGENCES.md` 与 ROADMAP。

---

## 1. 目录与关键文件

| 路径 | 作用 |
|---|---|
| `src/service/` | `mpp_service` ioctl 客户端、dma-heap、batch、poll |
| `src/codec/<codec>/` | 各格式语法解析（h264/hevc/vp9/av1/m2v/mpg4/h263/vp8/jpeg） |
| `src/hal/` | 各硬件块寄存器生成（h264d, hevc, av1d, vp9d, vp8d, jpegd/jpege, h264e） |
| `src/hal/h264e.rs` | VEPU580 H.264 编码器：缓冲、寄存器、提交、STATUS 读取 |
| `src/codec/rc/` | 码率控制移植（`data.rs` = MppDataV2，`model.rs` = rc_model_v2 的 CBR/VBR） |
| `src/scale.rs` | 自研 RGB 缩放/letterbox（替代 RGA，比 RGA 快 2.9–3.8×），并支持 CPU/DMA Frame 的 strided NV12 直缩到 RGB model input |
| `src/sdk.rs`, `src/c_api.rs`, `include/rk_mpp_rs.h` | 稳定 Rust/C 解码 SDK、V1 ABI、frame/packet 生命周期 |
| `src/reference/` | 自研软件参考解码器（M11：CABAC、HEVC 反变换、H.264 CAVLC/4×4+8×8 扫描/反量化/残差/DC；AV1 key/intra 滤波链、SavedCdf、单参考 integer/fractional inter、average/distance-weighted/difference-weighted/wedge compound、inter-intra、OBMC、local warp、单参考/compound global warp 与 temporal MV fields、inter residual 与 frame 内混合 intra/inter、多 tile tile-group：逐 tile 从帧初始 CDF 重启、帧末 CDF 取 context_update_tile_id，帧级 mi/segment 状态跨 tile 携带而邻块上下文、MV 单点候选、OBMC/projection 与 CDEF 单元按 tile 裁剪。2×2 tile 的 10 个 inter coded frame 已全部与 `aomdec` Y/U/V 逐字节一致；temporal motion field 会按 half-res cell 清除 intra 覆盖、在 base-invalid 时继续 extension probes、应用 order-hint sign bias，difference-weighted chroma mask 使用 luma 域重算） |
| `scripts/board_*.sh` | 板上验证脚本（回归、吞吐、寄存器捕获） |
| `tools/` | 取证工具：ioctl tracer、C 探针、寄存器 diff、字段生成器 |
| `tests/data/` | 所有 golden/fixture（C 侧真实输出） |
| `ROADMAP.md` | 逐 capability 的证据台账，**提交前必须更新** |

生成器：`tools/gen_h264_regs.py`、`gen_vdpu34x_regs.py`、`gen_vepu580_regs.py`
（从 C 结构体定义生成寄存器字段名/位置，改 HAL 前先看它们能否直接给出字段）。

`gen_vepu580_regs.py` 的 H.265 路线读两个 vintage：共享的 CTL/BASE/RC_KLUT 取自
`refs/mpp`（master），H.265 专属的 WGT/RDO/OSD 取自 `rk-mpp-rs/refs/mpp-2024-04`
（与板上 oracle 同源的 2024-04 副本，见该目录 README）。这种混用只在两边布局一致时成立，
所以改完任一头部文件或生成器后要跑 `python3 tools/check_vepu580_vintage_drift.py`。
两份寄存器映射（`vepu580_regs.rs`、`h265e_regs.rs`）都必须能从生成器 + `rustfmt`
逐字节重现，H.265 那份额外加了 `windows_match_the_generated_section_table` 把
`RegisterFile` 的数组长度钉在分区表上（H.265 是六段、无 SCL，与 H.264 不同）。

---

## 2. 验证文化（不可降级）

1. **每个能力都要有 C 侧预言机级别的证据**：优先"与板上源码编译的 C MPP 逐字节相同"，
   其次"与参考实现自己打印的内部状态逐帧相同"，最后才是 PSNR/可解码。
2. **寄存器级差异要用命名字段解释**，不能只说"差 N 个字"。
   工具：`tools/mpp_ioctl_trace.c`（LD_PRELOAD 抓 ioctl）+
   `tools/vepu580_trace_diff.py` / `tools/rc_trace_report.py`（把字翻译成字段）。
3. **发现结论错了，写进 ROADMAP 并撤回**，不要悄悄改。仓库里已有两次这样的记录
   （STATUS 窗口错位一个字；把 tracer 双打印误当成 12 次作业），都保留了原文与更正。
4. **测试掉了一个也要能被发现**：加测试时若用脚本切片改文件，先 diff 模块对 HEAD
   （曾把 `mod tests` 切掉，`cargo test` 仍绿）。
5. 提交信息写"做了什么 + 为什么 + 证据"，像 `git log` 里现有的那些。
   一次提交一个可验证的结论，别把未验证功能混进已验证提交。

---

## 3. 已完成（编码器码率控制这条线）

H.264/VEPU580 编码已做到：

- **固定 QP**：4 尺寸 × 3 profile × 2 QP = 24 格 IDR，加 6 格 GOP（1 IDR + 1/3/5 P，
  静止/运动），**逐 NAL 与 C 编码器相同**，含两条 user-data SEI。脚本：
  `scripts/board_h264_qp_vs_mpp.sh`。
- **码率控制三件套**（全部有 fixture 或板上判据）：
  1. 寄存器半边 `RegisterFile::apply_rate_control`（`setup_vepu580_rc_base` 非固定 QP 分支），
     对 C 的 CBR 作业 BASE/RCKLUT/RDO **逐字相同**（仅 dma 地址不同）。
     fixture：`tests/data/vepu580_rc_c_move6_cbr_regs.txt`。
  2. 位分配 `codec::rc::model`（`bits_model_*`、`calc_next_i_ratio`），
     与 C 自己打印的 `i_scale` 两条跑法逐帧相同。
     fixture：`tests/data/rc_model_v2_c_state.txt`。
  3. 量化控制器 `calc_cbr_ratio` / `calc_vbr_ratio` + `cur_scale_qp` 记账，
     与 C 打印的六帧 × 六个数全同。fixture：`tests/data/rc_model_v2_c_cbr_qp.txt`。
- **CBR 与 VBR 已接线**：`EncoderConfig::rc_mode`，CLI `[cbr:|vbr:]target:min:max`；
  `scripts/board_h264_cbr_vs_mpp.sh` 两模式各 **10/10 NAL 相同**。

关键语义（重新实现时别弄错）：

- `bit_real = bs_lgth_l32 * 8`，`bs_lgth_l32` 就是 STATUS 第一字（硬件写的字节数，
  不含软件前缀）。`hal_h264e_vepu580.c:2389` / `:2440`。
- `quality_real = qp_sum / mbs`，`mbs = mb_w * mb_h`（16×16 块数）；
  `madi / num_b16`，`madp / num_ctu`（分母为 0 时该项取 0）。
- CBR 预算用 `bps_target`；**VBR/AVBR 预算用 `bps_max`**，但控制器朝 `bps_target` 调。
- CBR 的 `ins_ratio` 不满足"正在逼近目标"的两个条件时**归零**；
  VBR 只在高/降两种条件下计数；VBR 没有水位项。
- 首帧 intra 的 `cur_scale_qp = (quality_init + i_quality_delta) << 6`；
  `i_quality_delta` 不是逐帧偏移。
- 固定 QP 与 RC 的 `uuid_rc_cfg` SEI 文本不同（`fixqp` vs `cbr/vbr/avbr` 行），
  模式名经 `sei::RcCfgLine::with_mode`。

---

## 4. 板子与工具链（必读）

- 当前板：`BOARD=my-rk3588-host`（RK3588，内核 6.1.57）。`BOARD` 是 SSH
  alias，公开文档中的名字仅作示例。
  `/dev/mpp_service` 需要 `sudo`。
- 参考 C MPP：
  - `/tmp/mpp-old`（commit `80957d266`，2024-04-24）＝**编码器字节级预言机**；
    构建在 `/tmp/mpp-old/build`。SONAME 为 `librockchip_mpp.so.1`。
  - `/tmp/mpp-develop`（master）＝解码器基准。
  - `/tmp/mpp_trace.so`＝ioctl tracer（由 `tools/mpp_ioctl_trace.c` 构建）。
  - ⚠️ 它们是 `/tmp` 里的**工作副本**；权威副本在 `~/rk-mpp-oracles`
    （`mpp-old`、`mpp-develop`、`mpp_trace.so`、`corpus-gen/`）。第二台 RK3588
    验证机的 `/tmp`
    是 tmpfs；RK3588 即使不是 tmpfs 也会被 tmpfiles 清理，两者重启后都应先运行
    `scripts/setup_board_refs.sh` 检查并从 oracle home 回填。
- 同步/构建（脚本里都在用）：
  ```bash
  BOARD=my-rk3588-host
  COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
    ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
  ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'
  ```
- **MPP 的调试开关是裸小写 env 名**（`mpp_env_get_u32` → `getenv(name)`，无 `MPP_` 前缀）：
  - `rc_debug=0x40`（RC 状态）、`0x80`（QP 项）、`0x20`（bps 行）、`0xf0` 全开；
  - `mpp_syslog_perror=1` 让日志同时进 stderr——否则只进 **syslog**
    （`/var/log/syslog`），这就是"`mpi_enc_test` 什么都不打印"的答案；
  - HAL 里 `hal_h264e_dbg_*` 的开关名尚未确认（`hal_h264e_debug` 未被读到），需要时读
    `mpp_env_get_u32` 的调用点。
- 参考探针：`tools/rc_probe.c`（直接驱动 MPI，能带模式/码率/gop 参数，方便配 rc_debug）；
  `tools/h264_enc_probe.c`（固定 QP 单帧 golden）；`tools/rc_data_probe.c`（MppDataV2 取证）。
- `mpi_enc_test` 与库默认不完全一致：同样参数的 CBR，`mpi_enc_test` 首帧 QP=23，
  库默认路径是 26。**与库对比用 `tools/rc_probe.c`；与 harness 对比要先把它的配置列全**。
- rkvenc 同进程多会话**可用**：1/2/3 线程在 64x64/320x240/640x480 产出一致，且与 C 参考逐 NAL 相同。
- fixture 生成坑：`cat src dst` 会读到刚写的文件；用 `cp` 再 `cat >>`。
  `mpi_enc_test` 对非 16 倍数尺寸按 width×height 取帧，会切错（本仓库只用 16 倍数做逐字节）。

---

## 5. 下一步（按优先级）

### 5.1 AVBR —— 已完成（`12626d2`，三模式 10/10 NAL 相同）

以下为当时的实施记录，保留备查。原计划：

**源码**：`mpp/codec/rc/rc_model_v2.c`

- `calc_avbr_ratio` 961–1060
- `moving_ratio_calc` 899–960
- `moving_judge_update` 272–290
- `bit_statics_update` 291–303
- 初始化：`bits_model_init` 1062–1160（`target_bps = bps_max`、
  `min_still_percent = bps_min*100/bps_max` 或 25，`max_still_qp = 35`，
  `motion_sensitivity = 90`），`bits_model_param_init` 里新增窗口：
  `pre_i_bit`(2)、`pre_i_mean_qp`(2，初值 -1)、`pre_p_bit`(8)、`madi`(8)、`madp`(8)、
  `gop_bits`(igop)。

**现成进度**：`docs/rc-avbr-wip.patch`（本仓库，勿丢）已经包含
`model.rs` 的 AVBR 状态字段、两张表（`MEAN_QP2SCALE`、`BIT2PERCENT`）、
`alloc_with`、`moving_judge_update`、`bit_statics_update`、`moving_ratio_calc`、
`calc_avbr_ratio`、`Measured`/`end_with`，以及 `encoder.rs` 的 `RcMode::Avbr`、
`rc_probe.c` 的 mode=2。**未完成且未验证**：`src/hal/h264e.rs` 里
`RcMode::Avbr` 的四处 match（model 构造、SEI 模式名、`rc:` 匹配、`end_with` 反馈），
以及编译/测试/板上对比。

**要点（写错就会差几 QP）**：

- `bps_change = moving_ratio * bps_target >> 8`，在水位/有符号调整**之前**算；
- `gop_bits = moving_ratio * gop_total_bits >> 8`，用它调 `bits_model_alloc`；
- `bit_diff_ratio` 超支分支乘 `moving_percent = 100 * moving_ratio >> 8`；
- `i_ratio = clip(pre_i_scale >> 4, 10, 200)`，而 **`pre_i_scale` 在该版本从未被赋值**
  （恒 0 ⇒ i_ratio=10，`pre_i_scale > 640` 分支永不进）；
- `ins_ratio` 用 `tab_lnx[idx2] - tab_lnx[idx1]`（**注意顺序与 VBR 相反**），
  之后 `>>2` 再 clip；
- `rgop_dratio` / `agop_dratio` 各 clip 到 ±1；
- 若 `bps_max >= ins_bps && final_qratio > 0 && scale_qp >= max_still_qp<<6`：
  `final_qratio` 去掉 `bit_diff_ratio`，且 `qratio = -6`；
- 反馈额外需要 `quality_real`、`madi`、`madp`（编码器侧在 `encode_frame` 里已有
  `Statistics`，`tests/data` 里有 C 的 STATUS fixture 可核对）。

**验收标准（缺一不可）**：

1. `cargo test --all-targets` 全绿；
2. 用 `tools/rc_probe.c … 2`（AVBR）+ `rc_debug=0xf0 mpp_syslog_perror=1` 抓 C 侧逐帧
   `moving_ratio / bit_target / next_ratio / quality_target`，写成 `tests/data/` fixture，
   加单测逐帧比对；
3. 扩展 `scripts/board_h264_cbr_vs_mpp.sh` 加 `avbr:2` 一格，要求 10/10 NAL 相同；
4. 固定 QP 的 `ONLY=gop` 回归仍 6/6。

### 5.2 场景检测器（`vepu580_h264e_tune_stat_update`）

- 读 STATUS 的 `madi`、`madp`、`md_sad_b16num*`、`madi_b16num*`，连续三帧大运动后把
  ATR 权重与 quant bias 切到另一行。当前 `scene_motion_flag` 固定在初值。
- 现在有了 `rc_debug` 这条通路，先做实验：构造硬切序列，用 `hal_h264e_debug`/寄存器
  捕获观察 ATR 字段是否切换；确认触发条件后再移植。
- 验收：硬切序列上与 C 逐字节相同（目前所有 fixture 都没触发它）。

### 5.3 H.264 编码当前边界

- 已闭合的路径包括固定 QP / CBR / VBR / AVBR、ROI/QP map、GDR、deblock、
  `cal_first_i_start_qp`、场景检测、hier QP / SMART_P、同帧与 super-frame
  重编码、NORMAL drop、CAVLC/CABAC PSKIP、two-pass deflicker、普通 slice split、
  逐 slice low-delay packet、宽度 >4096 的 CIME 扩展行以及 P 帧。证据入口分别是
  `board_h264_slice_vs_mpp.sh`（4/4）与 `board_h264_lowdelay_vs_mpp.sh`（5/5）。
- 当前实现范围内 H.264 编码功能已闭合；没有把 C 参考本身不存在的调度模式
  伪装成待移植路径。
- JPEG 编码的 low-delay packet 输出已闭合：单 job 保持原字节流，`EncodedJpeg`
  在 RST 标记处暴露 packet ranges；320x240/5 packet 与 640x480/10 packet 均与
  C 逐字节、逐 packet 相同。low-delay part 的旋转和 >720p 组合仍明确拒绝；
  JPEG 多核分带已单独关闭下面一行。
- JPEG 默认四 band 多核路径已闭合：GOP 无关的 streaming `MULTI_MSG` 提交、
  `SET_SESSION_FD` delimiter、scratch band 和 reg53 拼接均按 C 实现；
  1920x1080 qf50/qf90 + 1288x722 + rot90/rot270 为 5/5，
  322x242/1366x768 stride 为 2/2。显式 split 产生超出画面的 band 时 C 会输出无 EOI 的无效
  JPEG，记录为 C-BUG，Rust 保留有效流。

### 5.4 H.265 编码（M8，12k）

已走到"寄存器图对上了，还没有把作业发下去"这一步：

- 参数集：`codec::h265::encoder` 的 VPS/SPS/PPS 对 320x240、640x480、1280x720
  **板上逐字节**（`scripts/board_h265_qp_vs_mpp.sh` 3/3），C 的流存在
  `tests/data/h265e_c_fixqp_qp26_*.265`，宿主测试 `parameter_sets_match_the_recorded_c_streams`
  常驻复验。两个坑别再踩：`-t` 的 HEVC 值是十进制 `16777220`（`-t 9` 是 VP8），
  NAL 第二字节是 `nuh_layer_id` 不是类型位，载荷要插防竞争字节。
- 寄存器图：`hal::h265e` 从 `h265e_vepu580_template.rs`（2024-04 C 的 320x240
  FIXQP26 单 IDR trace）出发，再按几何/QP/语法推导。`tools/h265e_trace_diff.py`
  逐字段比对，现在只剩 `enc_pic.mei_stor` 一位不同——C 的 harness 传了 motion-info
  缓冲，我们没申请。别把模板里的地址字当真：它们属于 oracle 的缓冲区。
- 提交路径已落地：`h265-idr`（`src/hal/h265e_session.rs`）自己申请输入/输出/两块 FBC
  重建面（含显示与 MV thumb）/motion-info，填 160..175 的地址字，带上 C 的
  multi-offset 表（161/162 色度、164/166 FBC 体、172 输出容量、175 packet 长度），
  提交六段寄存器并 poll，前缀由 `codec::h265::encoder::build_prefix` 拼出。
  `scripts/board_h265_idr_vs_mpp.sh`：**320x240 / 640x480 / 1280x720 整条码流与 C
  逐字节相同**（860 / 1618 / 3585 字节）。
- P 帧已通：GOP 的长期参考是**首帧 IDR**（SPS 里一条 `poc_lsb 0` 的 LT 条目，
  P 的 `synt_refm` 标 `used_by_lt_flg0`），P 都重建到另一个槽；第三张起
  `ref_pic_lst_mdf_l0`/`lst_entry_l0`/`num_negative_pics` 置 1。P 图还要把
  `lamd_modb_qp` 写进 `iprd_wgt_qp_hevc_0_51`，IDR 保留 intra 表。
  `scripts/board_h265_gop_vs_mpp.sh`：2/3/5 帧在 320x240，以及 2 帧的 640x480、
  1280x720 **整条码流与 C 逐字节相同**；横移 8 像素的动画面（1032 字节）也逐字节。
- CBR 通路在剥离双方构建身份 version-SEI 后逐字节：`h265-gop-rc` 喂 C 的逐帧
  决策（IDR 26/86955、P 25/582）后，CBR PPS、`cbr` SEI 行与两个 slice NAL
  与 C 完全一致（`scripts/board_h265_cbr_vs_mpp.sh`）。扫描版曾把策略 SEI
  错写成 `[15:51]:4`，现改为 harness 实际使用的 `[10:51]:2`。注意 GOP 的 LT
  寄存器状态对 IDR 也要置位。
  STATUS 测量已移植（`measured_from_status`：`quality = qp_sum/(mb64<<6)`、
  `madi/num_b16`、`madp/num_ctu`），CBR 模型环也接上了：首帧决策与 C 一致，
  前缀与 IDR NAL 逐字节。第二帧仍不同（我们 9320 bits/QP27，C 582/QP25）；
  测量 bits 与 C 只差 24，所以问题在模型的 VBV/`calc_ratio` 状态折叠，不在测量。
- 三种码控都已逐字节（纯 GOP、模型自选 QP）：CBR/VBR 1163 字节、AVBR 1164；
  显式决策格 1169 字节。`-rc` 编号以 SEI 实测为准：0:vbr 1:cbr 2:fixqp 3:avbr。
  当前脚本先剥离 version-SEI，因此规范化后 320x240 模型三模式为
  1109/1109/1110 字节；320x240、640x480、720p 共 13/13 格通过。
- H.265 GDR 已闭合：C probe 通过显式 short-term ref cfg 真正触发 `is_i_refresh`；
  Rust 按 IGOP 边界调度 refresh、交替双 DPB、在边界重发 VPS/SPS/PPS +
  recovery-point SEI。320x240/640x480 × row/column 为 **4/4 整条逐字节**。
- slice split 已闭合：普通整 AU 的 `board_h265_slice_vs_mpp.sh` 为 **3/3**
  （CTU 8、BY_BYTE 256、640x480 的 40-slice 格）。
  `SliceSplit` 覆盖 `MPP_ENC_SPLIT_BY_BYTE/BY_CTU`，
  `EncodedFrame.packets` 把 VPS/SPS/PPS+SEI 放在首 packet，并按
  `POLL_HW_IRQ` 的 slice length 切分。`board_h265_lowdelay_vs_mpp.sh` 为
  **4/4**：320x240 CTU 8（3 packet）、BY_BYTE 256（9 packet）、640x480 CTU 2
  （40 packet，跨 32 项 FIFO）和 6 帧 short-term IDR+P（18 packet）。完整码流
  和每个 packet 长度均与 C probe 相同；默认 IDR/GOP/GDR/三模式码控回归均通过。
- ROI/QP map 已闭合：`codec::h265::roi` 复刻参考 helper 的 16×16 区域填充、
  HEVC CTU 标记、CU64/32/16/8 展开、force-split 和 base/QP 两张 DMA 表。
  `board_h265_roi_vs_mpp.sh` 为 **4/4**：相对 QP、绝对 QP+force-intra、多 region、
  640x480；六帧 short-term GOP 整条码流逐字节一致。
- 水平 tiles 已闭合：`build_pps` 写 `tiles_enabled_flag`/`num_tile_columns_minus1`，
  每列一个 job，`reg0252_tile_cfg`/`reg0253_tile_pos`/`reg212_rc_cfg.rc_ctu_num`/
  tile 相对的 CIME line buffer 窗口/`reg0193_dual_core` 握手/`reg0176-177` 交替
  line buffer 全部照 `hal_h265e_v580_set_uniform_tile`。RK3588 只到两列
  （`auto_tile` 或宽度 >4096），四列需要 >8192 宽度、硬件不接受。
  `board_h265_tile_vs_mpp.sh` 为 **2/2 逐字节**；`tools/h265e_trace_diff.py --job=0/1`
  显示除 10 个 dma-buf fd 字与 `mei_stor` 外无一个非地址字不同。
  tile + low-delay 逐 slice 输出**显式拒绝**（C 跨 tile 的 FIFO 行为无证据）。
- `-g 60` 是 **harness 误用**：`sscanf` 只填 `gop_mode=60`，`gop_len=vi_len=0`，MPP 据此生成
  `lt_gap=0` + 两个 `REF_TO_PREV_INTRA` 的病态调度；我们的 API 拒绝 `gop=0`。新 cell 用 `-g 0:60:0`。
- WPP、B 帧与多参考在 C 参考里不存在（见 ROADMAP），不是待写代码。

### 5.5 M9 后处理（IEP）

- VDPP（client 29）在 RK3588 **不存在**；RK3576 的 VDPP2 common/ZME scale、
  DCI histogram、默认 DMSR/ES/sharp register bank 和可替换的自定义
  11×17×8 tap8/tap6 bank 已移植。2024-04 C fake-service 矩阵保留
  10 PASS + 1 个明确 SKIP：旧 C 在 split-YUV420 内又把 chroma 尺寸减半，
  新硬件目标直接使用调用方给出的 chroma 尺寸，差异登记在 DIVERGENCES。
  除默认块外还覆盖自定义 ZME、完整 DMSR/ES/SHARP 输入、coefficient bank
  以及 YUV444 split；真实 YUV420/YUV444 split 已由新 C 真机 5/5 覆盖。
  10240-byte packed histogram 已能拆成 vendor 的
  4096 local + 256 global bin，`vdpp-hist` 会输出 packed/local/global 三份原始
  结果和 U10 平均亮度。
  RK3576 已通过 `BOARD=my-rk3576-host` 运行官方 Luckfox SDK 250301 的精确
  6.1.75 内核 `b16c5d1a69947ec66726165407a8aa75368def53`。板 config 只改成
  `CONFIG_ROCKCHIP_MPP_VDPP=y`，`mpp_vdpp 27961000.vdpp` 已绑定 client 29，
  `probe_hw_support` bit 29 为真。`CONFIG_OF_OVERLAY` 仍未开启，但 built-in
  driver 已不需要 overlay。
  `tools/rk3576-vdpp2-overlay.dts` + `board_enable_vdpp2_overlay.sh` 只有在
  需要动态改 DTB 时才有意义；当前 kernel 的直接路径已经生效。
  `mpp_vdpp` 是 built-in `bool`，不是可加载模块。
  `build_rk3576_vdpp2_kernel.sh` 可离线构建替换用 FIT，且会拒绝与板卡 6.1.75
  不一致或缺少 RK3576 VDPP 支持的源码；该脚本默认只生成产物，不写 eMMC。
  它现在支持 SDK 自带 `CROSS_COMPILE`，只启用 VDPP 配置，并克隆现有 FIT
  的 external-data 布局、load/entry、rollback index 和
  `sha256,rsa2048:dev` 签名声明。
  正式候选为 43,024,896 bytes，SHA-256
  `90649e7d6da5ea889c1c82ed43b135582e1e7d9c59c88e8977bc4938194888df`；
  p3 写后回读一致，第二次启动成功并返回新 boot ID。
  `board_vdpp2_hw_vs_c.sh` 真机 5/5 逐字节通过：scale-only、默认
  enhancement、YUV420 split、YUV444 split 和 histogram。split 输出使用
  `2 * width_vir` 的 byte stride；10,240-byte histogram 已精确截断并解析为
  4096 local + 256 global bin，mean luma U10 为 513。
  首次 Armbian rkr3 FIT 仍未启动，已通过 Loader（`2207:350e`）从原 p3
  恢复；该候选保留为 rejected evidence。
- IEP2（client 28）可绑定，但 `iep2_params` 只有去隔行，没有 YUV scale/crop 字段。
  通用 scale 属于 legacy `/dev/iep`；RK3588 没有该节点，driver 也不绑定。
- `tools/iep2_probe.c` 可按数值 mode 驱动 client 28，并原样输出 dst0/dst1；
  **PD/DECT 需要单独探针**：`iep2_test` 写死 I5O2，永远走不到 `iep2_check_pd`/`iep2_check_ffo`，
  用 `tools/iep2_dect_probe.c`（同一测试改 DECT + 每帧打印 `iep2_api_info`），
  `board_iep_dect_vs_probe.sh` 5/5 逐字节且逐帧状态相同（telecine、逐行、322x242、722x482、1366x768）。
  `board_iep_modes_vs_probe.sh` 为 **9/9**：I5O2（TFF/BFF）、I5O1T/B、I2O2、
  I1O1T/B、DISABLE、BYPASS。T 模式只写 dst0，B 模式只写 dst1，未用槽为零。
- 格式矩阵已闭合：420/422 × SP/Planar × UV/VU 为 **6/6**，输入 fixture、
  C probe 和 Rust 两侧同一格式，两个 raw dst slot 逐字节一致。
- 解码 Frame 与 IEP2 已联动：`IepDeinterlace::import_src_frame()` dup 解码
  NV12 dma-buf，校验单 fd、Y/UV offset 与 horizontal/vertical stride 后直接
  替换内部 source，不经 CPU 像素拷贝。源/目标分配和 Y/UV 地址按实际
  `hor_stride * ver_stride`；RK3588 的 rkvdec H.264 DMA frame→I5O2 以及
  50×34/H=64/V=48 padded frame 对显式 strided source 的逐字节测试均通过。
- 8/10-bit 后处理已补齐：`Frame::convert_format()` 支持 NV12/I420 与
  I420P10 的双向转换，包含 NV12 chroma 解交织/回打包；8→10 位复制和
  10→8 右移在 8-bit 输入域可逆，PTS/DTS/POC/error info 保持不变。
- `src/scale.rs` 已覆盖推理链路需要的 RGB 缩放/letterbox，并新增
  `Frame::letterbox_nv12()`：直接读取 CPU/DMA Frame 的 Y/UV plane，在缩放中
  转 BT.601 RGB，不再生成整帧中间副本。真实 H.264 720p CPU/DMA 输出逐字节一致；
  1080p→640 在 RK3588/RK3576 为 4.93/5.95 ms，旧“NV12→RGB + RGB scale”为
  20.32/24.36 ms。不需要为了 YOLO 去移植 IEP；做完整性再做。

### 5.6 M10 framework（29k，最大缺口）

已闭合：同步 submit+poll；异步任务队列、完成回调/wait/bounded event；
每任务排队/执行/端到端耗时，以及 submitted/done/failed/cancelled 和队列高水位
统计；引用计数 dma-heap 池、import/dup_fd、池复用和并发压测。
`scripts/board_framework_async_vs_sync.sh` 与 `framework_async_hw`/
`framework_pool_hw` 已在 RK3588 通过。

错误恢复也已完成：所有 submit/poll 故障走 close-before-open 会话替换，
不调用会污染 RK3588 驱动状态的 `RESET_SESSION`。板级门同时覆盖会话替换、
H.264 重开后逐字节复用、VP8 截断 packet 的真实 STATUS 错误，以及 H.265
tile 恢复。最后一项用无效 input fd 在 RK3588 触发真实 `int_sta=0`；重开后
清空 tile line buffer、重新执行丢弃预跑，并强制下一帧为带参数集的 IDR，
输出与全新会话逐字节一致。`board_framework_recovery.sh` 为 4/4。

多核 CCU 也已落地：`CcuTopology` 探测 vdec/venc/jpeg 设备树节点，
`BatchServer` 用 `SET_SESSION_FD` 把最多 8 个真实 session 合成一个提交链。
内核 6.1 的批量 wait 不填充每条 `mpp_bat_msg.ret`，所以采用“共享提交、
逐 session poll”的分层回退。RK3588 的两条独立 VEPU2 JPEG 640x480 session
同批编码后，输出分别与各自单会话逐字节一致；RK3576 的两条独立 VEPU510
H.264 320x240 fixed-QP IDR 也通过同一批量路径，输出与各自单会话一致。
两块板各 3/3；无 CCU 或命令不支持时自动退回顺序单会话。

仍缺：内核协议没有 dma-fence；secure/TA 无可用接口或设备。
VP8 与 H.264/HEVC/AV1/VP9 的池化帧已经真正持有 DPB lease；H.265
保留 DPB 引用，AV1 只复用非参考帧，VP9 保护 hardware previous/last
colmv。下一步转到跨 SoC 验收和其余解码边界，不再重复队列、CCU 或恢复基础设施。

### 5.6.1 W12 RK3576 bring-up（新板，不是 RK3588 的别名）

- RK3576 是 Luckfox Omni3576 / RK3576，使用 VDPU383、VEPU510、VPU720 和
  VDPP2；不能复用 RK3588 的 VDPU34x/VEPU580 寄存器写法。
- `service::soc` 已按 device-tree compatible 选择 HAL，RK3588-only 入口在
  RK3576 上显式拒绝；`probe` 会打印 SoC、模型和预期代际。
- 已生成 276-word VDPU383 寄存器表，并完成首个 H.264 硬件路径。RK3576 的
  640×480、12 帧 High profile 动画面与官方 C MPP 逐字节，SHA-256 均为
  `615ee129c46e10f7fb4cb2cf8af3cb370523f7605abee7e7315e073a4421b112`。
- 前两帧 VDPU383 `global_cfg` 和 `rps` 也与 C 调试 dump 逐 word 相同。
  关键板级差异是 704 stride、未压缩 colmv，以及 DPB flags/MVC layer ids
  共用一个 32-bit word。
- H.264 边界 5/5 已在 RK3576 通过：720p/B 30 帧、1080p hard 90 帧、PAFF
  15 帧、LTR 62 帧和缺失参考 61 AU/22 输出帧。前两条、LTR 与缺失参考
  与 C MPP 逐字节；PAFF 与 FFmpeg
  逐字节，C harness 的场序输出是既知差异。入口为
  `scripts/board_rk3576_h264_edges_vs_mpp.sh`。
- H.265 12/12 矩阵已在 RK3576 通过：320×240 单 IDR、640×480 IPPP 与
  B-pyramid、Main10、2×2 tiles、真实 WPP（640×360，5 个 entry point）、
  1280×720 WPP、JCT-VC DSLICE，以及显式 SPS/PPS scaling list；Rust/YUV 与
  C MPP 逐字节相同。176-byte VDPU383 global packet 与 C dump 相同，单测固定
  FNV `0xe97a26406128e428`。JCT-VC
  `LTRPSPS_A_Qualcomm_1.bit` 为 416×240、500 帧，SPS 声明 8 个长期参考，
  各切片反复选择 `lt_idx_sps=4/7`；C/Rust 均输出 500 帧，SHA-256 同为
  `e0d7db1516ed31d3046c63c33240ec0866150774eb74b5e88f67bc186ba51bee`。
  缺失参考格从一个 12 帧 x265 流删除第 6 个
  AUD access unit：C 报 `Could not find ref with POC 5`，两侧均输出 5 帧，
  YUV SHA-256 同为
  `7d67c91340892c5de09e45db43fc160f3991b8f950e30c7d2b0ff5590626a74d`。
- H.265 已新增 16 格确定性损坏矩阵：SPS/PPS/SEI、IDR/多个 P slice 的
  start/middle/end bit flip，以及三种末尾 AU 截断。14 格 C/Rust 输出计数与
  planar YUV 逐字节一致；损坏 SPS 和首个 IDR slice 起点会让 C harness 超时，
  Rust 会及时结束并输出 0 帧。入口为
  `scripts/board_rk3576_h265_corrupt_vs_mpp.sh`。更宽 conformance 矩阵仍需
  RK3576 实跑；功能入口仍是 `scripts/board_rk3576_h265_vs_mpp.sh`。
- VDPU383 AV1 已建立第一层软件 ABI。`src/hal/av1d383.rs` 按
  `vdpu38x_av1d_uncomp_hdr()` 生成 5160-bit、128-bit 对齐的 672-byte
  global header；bit packing 复用 C `mpp_put_bits` 的 64-bit 小端布局，
  末尾保留 16 字节。`av1_tiles_320x240_10.ivf` 首 keyframe 与 RK3576 patched C
  dump 逐字节一致，FNV `0x8da389d2e180d680`。C dump 入口见
  `scripts/av1_vdpu383_dump.patch`。同一 keyframe 的 212-word 串行化
  `Vdpu383RegSet` 也与 C dump 完全一致，包括 control/statistic、stride、
  RCB base/length 和 global/output/CDF/colmv 地址。VDPU383 专用 CDF 由
  `tools/gen_av1_vdpu383_cdf.py` 从 C 的 7400-entry u32 表生成，29,600
  bytes，SHA-256
  `43474cb59fa76fa1ed95cbfb2fb46952b061e948a13581c04380e51d099c100b`。
  320x240 buffer plan 为 working CDF 32,672、segmentation 3,072、colmv
  20,480、RCB 72,448 bytes。`hal::av1d383::Session` 已通过 RkvDec client
  提交真实 VDPU383 首 keyframe；320x240 YUV420p 与 stock C MPP 逐字节，
  SHA-256
  `6f5b544f53584c7de6d3acad5f4611ca2ee3d75ae259ab981ffca5e599b1b9fe`。
  多帧路径现已闭合：`Session::decode` 接收当前 persistent `Frame` 和八个参考槽，
  每个 frame 持有 output/colmv/working-CDF DMA；session 按 C 的
  `ref_info_tbl[].cdf_valid/coeff_idx` 维护主参考的 coefficient-CDF 分区。
  320x240 的 10 个显示帧与 stock C MPP 三平面逐字节相同，共 1,152,000 bytes，
  SHA-256
  `9d339ae3c5b270ac9059548da0d3ff5ef6cb0c94a9bcb11f5bfc93783e868138`。
  完整入口为 `scripts/board_rk3576_av1_vdpu383_sequence.sh`。
  VDPU383 10-bit 也已接通：320x240 的 packed surface 使用 704-byte stride，
  Rust 转成 planar little-endian u16；10 帧共 2,304,000 bytes，SHA-256
  `f3b69b61a06d3967a89b91f11f56883588df53974d4a1b5efb8fbd9e004d49b8`，
  与 C 的 skip-grain 输出逐字节。入口为
  `scripts/board_rk3576_av1_vdpu383_10bit.sh`。
  FGS 仍是硬件边界：C 不启用该块；Rust 编程 `av1_fgs_en`/FGS base 后，
  raster 与 tile 两种模式都返回 STATUS `0x00000000` 并触发 reset。没有新的
  register/trace oracle 前保留已验证的 skip-grain 行为。
- VDPU383 VP9 初始 7/7 已在 RK3576 通过：profile 0/2、8/10-bit、lossless、
  errres tiles 和 720p；普通行与 C MPP 逐字节，720p errres 与 321x241
  继续按既有 libvpx 判据处理 C 侧 bug。VDPU383 的 global header、1312-byte
  keyframe probe、11 个 RCB region 和 `width * height / 2` colmv 已接入。
  `scripts/board_rk3576_vp9_vs_mpp.sh` 是当前入口；RK3576 release LTO 当前会
  在 LLVM SIGSEGV，因此功能门使用 debug。
- VPU720 JPEG 已在 RK3576 通过 19/19：NV12 三尺寸+60-MCU restart，320x240
  NV16/I420/NV21/YUV422P/NV61/YUYV/YVYU/UYVY/VYUY/NV24/YUV444P/YUV400，
  以及 400/420/422/444 输出均与 C MPP 逐字节；实现使用 client 20、43-word
  base register window 和 384-byte 变换 Q-table。`EncoderConfig::input_layout()`
  统一给出平面存储大小、U/V offset、chroma stride 与 UV-swap，VPU720 再映射
  到 C 的 source/output format。C 的 422/444 输出对 NV12 fixture 只有 623
  bytes，Rust 只承诺复现该行为。`scripts/board_rk3576_jpege_vs_mpp.sh` 是
  当前入口；tile input 仍待做。C 的 VPU720 HAL 没有 RGB 输入、rotation 或
  low-delay `part_start`/`part_wait`，Rust 对它们显式拒绝。
- VEPU510 H.264 已完成寄存器 ABI 取证：`tools/gen_vepu510_regs.py` 从 C
  struct 生成 645 个字段；六个写窗口及 148-word STATUS 已与 RK3576 固定 QP IDR
  ioctl trace 对齐。首个 320x240 QP26 Baseline IDR 已在 RK3576 与 C 全流逐字节，
  均为 6479 bytes，SHA-256
  `560ce2420b345c425477770651dc33b52dfee7aebc74b775e29150a774c157df`。
  `src/hal/vepu510_h264.rs` 负责输入/输出/FBC/thumb/smear 缓冲、六段提交和
  `bs_lgth_l32` 回读；`board_rk3576_h264e510_vs_mpp.sh` 是当前入口。
  同一入口的第二格为 IDR+P GOP2：C/Rust 均为 8671 bytes，SHA-256
  `f7cc121537e3fa7e3f075ec052409c7f9bd9be04a4153a94adf05f68cebaad7b`；
  P 作业轮换 recon/thumb/smear 槽，slot0 为参考，并切到捕获的 P syntax/AQ。
  6 帧 GOP 同为逐字节，18225 bytes，SHA-256
  `0e65b51a9c31a8719e0a6c5a16757c9d5783508f17d68c0ca12dda2272811f33`；
  两槽按帧号奇偶交替，frame_num/POC/SQI 已由模板字段生成。
  CBR6 也已逐字节，14187 bytes，SHA-256
  `d9de49270ab4140b1005a8cadf161a6e81c65e73ea65a356b0901656b24ac87c`。
  VBR6/AVBR6 同映射通过，分别为 15616/12313 bytes。下一步扩 ROI 和 split。
  ROI legacy metadata 已映射到四字硬件 region，强制 intra +绝对 QP 两帧为
  10845 bytes，SHA-256
  `a34ef76313cb570f152e8b5d7e0fd43d7ff433a29b5f9086ea59d43e5f3c187a`。
  `setup_vepu510_split` 的 BY_CTU/BY_BYTE 与普通整 AU、LOWDELAY 两路均已接通：
  slice FIFO 为 8 项，`POLL_HW_IRQ` 使用 VEPU510 的 31-bit length + bit-31
  last 格式。BY_CTU=96 两帧为 9478 bytes / 8 packets，BY_BYTE=256 为
  11664 bytes / 40 packets；完整码流和每片长度均与 C 相同。
  GDR 的 C table 构造在普通无 QPMAP 路径中是死代码：`adr_roir` 仍为 0。
  可达语义由刷新帧的 CIME 单行搜索、IGOP 边界不插 IDR、recovery 帧重发
  36-byte SPS/PPS，以及 P-frame smear state 组成。row=2/GOP4 十帧为
  27619 bytes，col=4/GOP6 十三帧为 34339 bytes，逐图长度和完整码流均与 C
  相同。
- VEPU510 H.265 已有首个 320x240 QP26 Main IDR：`src/hal/vepu510_h265_regs.rs`
  是从 2025 RK3576 C header 生成的 730-field/6-window 映射，
  `vepu510_h265_idr_template.rs` 是六段寄存器图和 148-word STATUS 的 RK3576
  trace。硬件 slice 为 4440 bytes，软件前缀 270 bytes，总计 4710 bytes，
  SHA-256
  `e97ddde34fa10eff02889b1b4d49d394df457eb1cbafb04015e047539b37b6d7`。
  RK3576 的 H.265 参数集与 RK3588 不同：`max_cu_size=32`、transform
  depth 额外加 1，且 2025 库在 fixed-QP 下仍写 `cu_qp_delta_enabled=1`。
  P 帧使用第二组 recon/colmv/thumb/smear 槽，读第一组参考；IDR+P GOP2
  合计 6810 bytes，SHA-256
  `7a44882d3da7d72059b28735a38fdf9bb1bf32ee144e78f2d1a61642816f4569`。
  两套参考槽按帧号奇偶轮换；六帧 trace 中只有 POC LSB 随帧号推进，且第二
  个 P 帧起置 `smear_load_en=1`。六帧 GOP 为 16994 bytes，SHA-256
  `8088e821a066e1565e9ab58a64194eccdd34a42000593a731bca2c9f957fe397`。
  CBR6 复用 `rc_model_v2`，目标/上下限为 300/250/350 kbps；RK3576 的
  IDR/P 最小 QP 分别为 15/10，最大 51，CBR 前六帧 QP 轨迹为
  `26/29/29/30/30/32`。C/Rust 均为 11192 bytes，SHA-256
  `5768ab6c768dd5b613179c494ec41081ac104240e1a4d0df0c9ab58ff14fc9fb`。
  VBR6/AVBR6 共用同一 VEPU510 RC 映射，只切换 model allocator 与 SEI mode。
  VBR QP 轨迹为 `26/29/29/30/30/30`，11795 bytes；AVBR 为
  `26/29/33/33/33/33`，9676 bytes。两侧完整码流逐字节一致。
  H.265 split 使用 VEPU510 的 32x32 slice 网格：BY_CTU=8 为 7922 bytes，
  BY_BYTE=256 为 8264 bytes。LOWDELAY 分别输出 20 / 25 个 packet，完整
  码流和逐 packet 长度均与 C 一致。
  H.265 GDR 不写 ROI base table：有效差异是抑制 IGOP IDR、POC 持续递增、
  recovery 帧重发 81-byte 参数集 + 8-byte recovery SEI，以及 smear
  load/store 按 GOP 位置轮转。row=2/GOP4 十帧为 27842 bytes，
  col=4/GOP6 十三帧为 34967 bytes；完整流逐字节一致。
  H.265 ROI 使用与 H.264 相同的四字 VEPU510 region：forced-intra、
  absolute-QP30 的 0,0,160,120 两帧格为 8823 bytes，完整流与 C 相同。
  H.265 10-bit 在 RK3576 的 C MPP 中即不可用：`vepu5xx_set_fmt` 报
  `unsupport frame format`，HAL 报 `invalid src color space: 16`，最终 packet
  为 0。Rust 的 `open_bit_depth(10)` 在 DMA/硬件分配前显式拒绝。板级脚本也
  已去掉对 H.264 13 帧临时输入的隐式依赖。
- VEPU510 H.265 tiles 已按平台边界闭合。2025 C PPS 的自动 tile 分支只覆盖
  RK3566/RK3568/RK3588；RK3576 上 `AUTO_TILE=1` 的输出与普通 320x240 IDR
  完全相同，同为 4710 bytes。Rust 的 `open_tile_columns()` 对多于 1 列在
  DMA/硬件分配前显式拒绝，不伪造 tile 参数集或硬件路径。VEPU510 H.265
  功能/边界矩阵现在为 **15/15**。
- RK3588 回退门仍使用 VDPU34x：既有 7/7 H.264 回归通过，证明分派没有破坏
  RK3588。
- 下一步：完成 AVS+ profile 0x48 的 AEC 和 field picture，以及 VPU720 tile input 与
  VDPP2 剩余组合。RK3588 client 12 的 Rust 公开 7 帧 I/P/B
  cycle 已与 C 原始输出逐字节一致（`47d05886…5fdb20`，vproc 关闭）；C
  第二个公开 6 帧 cycle 也一致（`e6069fc1…78f29`）。C 默认 IEP2 vproc
  输出也已由 Rust `vproc` 模式逐字节复现；完整后处理周期为 9/7 帧，
  SHA-256 分别为 `6ef29ca6…e169` 和 `35ac81e6…f3d1`，Rust/C SDK 均可通过
  vproc 标志选择。AVS2 在当前 RK3588 无设备。
  另有项目生成的 8 帧 interlaced-frame 流：raw I420
  `508883d6…1416`、vproc NV12 11 帧 `1fa2e8ed…c2b0` 均与 C 逐字节；
  独立 field-pair 仍未验证。
  RK3576 未被硬件暴露的
  VP8/MPEG-2/MPEG-4/H.263 要由能力矩阵明确拒绝。

### 5.7 解码器边缘

- AVS+（client 12；raw/vproc、派生 0x48 与 weighted-quant 逐字节，AEC/field 待做）；
  AVS2（当前 RK3588 无设备）
- AVS+ 已进入普通 `sdk::Codec::Avs` 和 C ABI `RK_MPP_CODEC_AVS=10`；
  `examples/sdk_decode avs` 与 `avsdec` 在公开流上输出相同。CLI 支持原生
  NV12；`board_avs_plus_perf.sh` 的 700 帧压力流与 C 逐字节，当前 5 次中位数
  raw 为 Rust 606 ms / C 808 ms，默认 vproc 为 Rust 490 ms / C 809 ms；
  vproc 仍有双峰，但输出 SHA 不变；配对 `time -p` 的 Rust user+sys 为
  0.13–0.22 s，低于 C 两个快样本的 0.26 s。
- 损坏/截断码流的一致性（目前只测了"返回非零退出码"）
- VP8 profile 1/3、AV1 更宽 conformance、VP9 边界矩阵
- H.264/SCC 高 profile? 4:2:2/4:4:4 被 RK3588 rkvdec 拒收，属硬件限制（记录在 ROADMAP）

### 5.8 性能

- 解码吞吐脚本：`scripts/board_bench_vs_mpp.sh`（`BENCH_FILTER=` 选 pass）
- 编码吞吐：`scripts/board_bench_encode_vs_mpp.sh`（`SAMPLES=5`）
- 当前证据只支持“解码/编码不比 C 慢”。同一二进制在第二台 RK3588 验证机上
  720p H.264 可从
  72.9 ms 漂到 120.1 ms；未做交错采样的百分比不进入结论。
- 规则：两侧都**外部计时**、交错采样、给中位数；少于十几帧的流不能单独成结论。

---

## 6. 易错清单（血泪版）

1. **tracer 每段寄存器打印两次**（`before cmd=200` 一次 + 单独一次）。按 `write words=`
   行数计数会把作业数翻倍——这曾导致"每帧重编码"的错误结论。
2. **`words=` 后面第一个 token 是数量不是数据**。当数据读会把整个窗口错位一个字
   （STATUS fixture 曾经全错）。
3. **MPP env 无前缀**：`rc_debug`、`mpp_syslog_perror` 是小写裸名；
   `MPP_RC_DEBUG` 无效。日志默认进 syslog。
4. **`mpi_enc_test` 不打印**不是因为它不打印，是因为进 syslog；加
   `mpp_syslog_perror=1` 就有。
5. **`qp_sum` 按 16×16 块累加**，`num_ctu` 数 32×32 块；除以 `4*num_ctu` 才得到 QP。
   固定 QP 26 @320x240 → `qp_sum=7800`，7800/300=26。
6. **`bit_real` 是硬件字节数×8**，不含软件写的前缀（SPS/PPS/SEI）。
7. **tracer 输出很大**：永远 `grep`，不要 `cat`（曾一次烧掉 4 万 token）。

   **H.265 tile 首帧没有跨会话稳定的 C oracle**（2026-11-04）：同一个 C probe、同一份输入、同一组参数，
   紧跟我们会话时输出 `9724d029..`、紧跟它自己会话时输出 `b27d68dc..`，而且两者**解码出的像素也不同**
   （`7dc42fe5..` 对 `c9fcd4f7..`）——VEPU580 的双核状态跨进程保留，tile 首帧会读到它；中间归还
   512 MiB 归零页也不变，说明状态在硬件块内而非内存，而 `MPP_CMD_RESET_SESSION` 只清队列。
   因此两侧各跑一趟丢弃作业：我们据此把自己钉在 `9724d029..`，`board_h265_tile_vs_mpp.sh` 也在 C
   编码之前先跑一次我方编码，把 C 送进同一状态。**这个预热不是冗余代码**：实测去掉它之后，只要
   前一个脚本的最后一次编码是 C 侧，`auto320` 就 FAIL。逐字节结论的边界只在"该状态内"，写文档不要越过。
8. **`ssh "$BOARD" '...'` 单引号内 `$VAR` 不展开**；需要本地展开时用双引号或分支。
9. 板上 `/tmp` 文件由 root 创建时，`scp` 前要 `sudo chmod 644`；删除要 `sudo rm -f`。
10. **不要用 `cat a b` 造 fixture**（会读到自己）；`cp` 后 `cat >>`。
11. 复杂嵌套 heredoc 反复出问题：写成 `/tmp/x.py` 再 `python3 /tmp/x.py`。
12. 改 Rust 文件若用脚本切片，**先用 git diff 对 HEAD 校验测试模块完整**。
13. **不要把金样过期误判成并发干扰**（2026-11-04 撤回旧结论）：`h264_encoder_hw` 的两个用例
    曾以“两个 rkvenc 会话互相干扰”为由要求 `--test-threads=1`。实测串行同样失败——真正原因是
    金样早于两个 SEI（version + rc_cfg）加入码流；而 `setup_ext_line_bufs` 在 C 里只分配本上下文
    缓冲，且这些尺寸根本不到 >4096 的 ext 路径。重取 C 金样后，1/2/3 线程都通过，并新增了
    并发会话用例。判据：**串行也失败 ⇒ 不是并发问题**。
14. **残留的编码进程会占住硬件**：板上曾挂着一个 7.5 小时前的 `mpi_enc_test`，
    现象是脚本"卡住不报错"而不是失败。跑编码套件（或编排脚本）前先
    `ssh "$BOARD" 'ps -eo pid,etime,comm | grep mpi_'` 看一眼，有残留 `sudo kill -9`；
    杀掉本地 ssh 会话并不会终止远端仍在跑的测试进程。
15. 编排脚本里调 `ssh` 要给子任务 `< /dev/null`：`ssh` 不带 `-n` 会继承 stdin，
    当外层标准输入是管道/文件时会永久阻塞（`board_regress_all.sh` 就这么卡过一次）。
    另外 macOS 自带 bash 3.2 + `set -u`：`SKIP` 这类可选变量必须显式 `SKIP="${SKIP:-}"`。
16. 固定 QP 的 24 格矩阵与 6 格 GOP 是**常回归门**，任何碰编码器的改动都要重跑。

---

## 7. 命令速查

```bash
BOARD=my-rk3588-host

# 本地
cargo test --all-targets && cargo fmt --check

# 板级
ONLY=gop  ./scripts/board_h264_qp_vs_mpp.sh          # 固定 QP GOP：6 格
ONLY=sizes ./scripts/board_h264_qp_vs_mpp.sh         # 固定 QP 24 格（约 10 分钟）
./scripts/board_h264_cbr_vs_mpp.sh                   # CBR + VBR
./scripts/board_h264_rc_capture.sh                   # 重采 RC fixture（REFRESH=1 覆盖）
./scripts/board_h264_status_vs_mpp.sh                # STATUS 字段级对比

# C 侧状态取证（AVBR 会用到）
scp tools/rc_probe.c "$BOARD":/tmp/
ssh "$BOARD" 'cc -O2 -w -I/tmp/mpp-old/inc /tmp/rc_probe.c \
  /tmp/mpp-old/build/mpp/librockchip_mpp.so.1 -o /tmp/rc_probe'
ssh "$BOARD" 'sudo env LD_LIBRARY_PATH=/tmp/mpp-old/build/mpp rc_debug=0xf0 mpp_syslog_perror=1 \
  /tmp/rc_probe /tmp/rk-mpp-rs-move-320x240.nv12 /tmp/out.264 320 240 6 \
  300000 250000 350000 2'            # 最后的 2 = AVBR（1=CBR，0=VBR）
```

---

## 8. 开工检查单

1. `cargo test --workspace --all-targets --offline` 必须 1319 通过（45 个板级 ignore）；不通过先修，别加功能。
2. `git status` 干净（除本文/补丁）；确认在 `e5e9a7f` 或其后。
3. 跑 `scripts/setup_board_refs.sh`（秒级，幂等）确认三个 oracle 齐；缺了会自动从
   `~/rk-mpp-oracles` 回填，两边都没有才会触发 clone+cmake 重建（这一步最贵）。
   之后跑 `ONLY=gop ./scripts/board_h264_qp_vs_mpp.sh` 确认真实判据仍可用。
4. 再跑 `./scripts/board_h264_cbr_vs_mpp.sh`，确认 CBR/VBR 基线仍全过。
5. 之后按第 5 节顺序推进；每完成一个小项：更新 `ROADMAP.md` → `cargo fmt` →
   全量测试 → 板级证据 → 提交（提交信息写清证据与已知边界）。
