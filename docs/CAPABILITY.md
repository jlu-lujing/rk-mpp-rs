# RK3588 MPP 能力矩阵（以 t6 上的 C MPP 为 oracle）

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
| H.264（rkvdec） | 完成 | frame/MBAFF/PAFF/长参考/错误路径逐字节 |
| H.265（rkvdec） | 完成 | Main/Main10/tiles/WPP/DSLICE/LTR/scaling list 逐字节；4:2:2/4:4:4 被硬件拒收（平台边界） |
| VP9（rkvdec） | 完成 | profile 0/2、10-bit、lossless、tiles、errres、resize、show-existing、hidden alt-ref；边界两格由 libvpx 判定为 C bug |
| AV1（av1d） | 完成 | Main 8/10-bit、film grain、2x2 tile、96×96–1920×1080、长 GOP、多 sequence、320↔640 切换；C/Rust/libaom 三方逐字节；64×64 为 C bug |
| MPEG-2 / MPEG-4 / H.263 / VP8 | 完成 | 逐字节；隔行 MPEG-2 用 FFmpeg 作 oracle（C 会错误回绕输入）；VP8 profile 1/3 为派生流验证 |
| AVS+ / AVS2 | 不可验证 | 无 conformance 流、无硬件 |
| H.265 SCC | 不存在 | MPP 源码没有 palette/SCC 路径 |

## 编码

| 能力 | 状态 | 证据 / 边界 |
|---|---|---|
| H.264（VEPU580） | 完成 | 固定 QP 24 格、P 帧 11 尺寸（含 FBC 阈值两侧）、CBR/VBR/AVBR、hier QP、SMART_P、two-pass deflicker、level、deblock、ROI/QP map、GDR 4/4、slice split 4/4、逐 slice 输出 5/5，全部逐字节或逐 job STATUS |
| H.265（VEPU580） | 完成 | IDR/P GOP、CBR/VBR/AVBR、GDR 4/4、ROI 4/4、deblock/SAO 4/4、BY_CTU/BY_BYTE 3/3、low-delay 4/4、水平 tiles 2/2 |
| JPEG（vepu） | 完成 | baseline 4:2:0、restart、90/270 旋转、RGB/YUV 输入、低延迟 slices、RK3588 四 band 多核，逐字节/逐 packet |
| WPP / B 帧 / 多参考 | 不存在 | C 参考没有实现（标志恒 0、无 entry point、无 B_FRAME、`num_ref = 1`），不是本 crate 的缺口 |
| harness `-g 60` | BY-DESIGN | harness 解析成 `gop_len=0` 的病态 smart-GOP；本 crate 拒绝 `gop=0`，有效形式逐字节 |
| 同进程多会话 | 完成 | 1/2/3 线程在 64×64/320×240/640×480 产出一致且与 C 相同；旧的“每进程单会话”说法已撤回 |

## 后处理与框架

| 能力 | 状态 | 证据 / 边界 |
|---|---|---|
| RGB 缩放 / letterbox | 完成 | 自研实现比 RGA 快 2.9–3.8× |
| IEP2 模式 | 完成 | I5O2/I5O1T/B、I2O2、I1O1T/B、DISABLE、BYPASS 9/9，含 TFF/BFF 与 1280×720，原始双 dst slot |
| IEP2 格式 | 完成 | 420/422 × SP/Planar × UV/VU 6/6 |
| IEP2 PD/DECT 逆 telecine | 完成 | 场序 + pulldown 状态机；telecine/逐行 + 322×242/722×482/1366×768 共 5/5，逐字节且逐帧状态相同 |
| VDPP（client 29） | 不存在 | RK3588 无该硬件；10-bit↔8-bit/HDR 随之不可验证 |
| legacy `/dev/iep` scale | 不可验证 | t6 没有该节点；IEP2 只有去隔行 |
| 异步任务队列 / 缓冲池 | 完成 | 回调、等待、事件、逐任务 queue/run/turnaround timing、累计成功/失败/取消与队列高水位统计、引用计数 dma-heap 池；VP8 与 H.264/HEVC/AV1/VP9 DPB surface 均持有 lease；t6 async demo、H.264/HEVC/AV1/VP9 池化边界测试全过 |
| fence / 多核 CCU / secure | 受内核限制 | 该内核/驱动不暴露对应接口，未伪造 |
| RK3576 平台适配 | 进行中 | VDPU383 H.264 IPPP + 4/4 边界（720p/B、1080p hard、PAFF、LTR；PAFF 由 FFmpeg 判定 C 侧场序差异）、H.265 初始 10/10、VP9 初始 7/7（profile 0/2、8/10-bit、lossless、tiles；两个 C bug 由 libvpx 判定）；AV1/AVS2、编码、JPEG 与 VDPP2 待完成 |

## 软件参考解码器（M11，自研 ground truth）

| 部分 | 状态 |
|---|---|
| H.264 CAVLC I-slice（预测/变换/DC Hadamard/deblock） | 完成，与原始合成源逐像素 |
| H.264 CABAC（I16/I4x4/I8x8/I_PCM/mb_qp_delta/混合宏块） | 完成；P/B-slice CABAC 未做 |
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

当前 ROADMAP：**94 完成 / 6 未完成**（含缩进子项为 132 / 11）。未完成项里 5 项属于上表的
平台/不存在边界，1 项是 M11 的完整 AV1 软件解码。M11 已落地 103 个 slice（含真实 2×2 tile tile-group 的逐 tile 熵闭合与像素门；inter 的 tile-group 驱动、帧级邻居携带、temporal/MV/chroma-mask 修正和 10 帧三平面门均已落地）：符号解码、默认
CDF、partition/块模式、系数语法、Round2 逆变换与 libaom 矩形向量、量化、重建、帧内预测、CDEF、loop
restoration 滤波与 subexp/tile-CDF 参数解码、帧/条带驱动、`read_lr`/`read_lr_unit` 码流层、
§7.14 去块滤波、§8.1/§5.2/§5.5 位流层（描述符、OBU+IVF 解复用、完整序列头，已在
5 个真实 IVF fixture 的 50 帧上验证），以及 §5.9 完整帧头（8 个真实 IVF 共 112 个帧头逐位
对齐，并经 ffmpeg `trace_headers` 交叉核对），真实 tile-group 的字节范围拆包、不依赖
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
t6 `cargo test --release --all-targets` 为 **1170 个测试枚举 / 1136 常规通过 / 0 失败 / 34 个板级 ignored**；
t6 `cargo test --release --lib` 为 **978 passed / 0 failed**。34 个板级 ignored 测试已有逐项通过记录，
完整 t6 回归与性能闸门见 `docs/T6_VALIDATION.md`。
