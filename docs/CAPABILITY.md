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
| 异步任务队列 / 缓冲池 | 完成 | 回调、等待、事件、引用计数 dma-heap 池，板上 async demo 与同步路径逐字节 |
| fence / 多核 CCU / secure | 受内核限制 | 该内核/驱动不暴露对应接口，未伪造 |

## 软件参考解码器（M11，自研 ground truth）

| 部分 | 状态 |
|---|---|
| H.264 CAVLC I-slice（预测/变换/DC Hadamard/deblock） | 完成，与原始合成源逐像素 |
| H.264 CABAC（I16/I4x4/I8x8/I_PCM/mb_qp_delta/混合宏块） | 完成；P/B-slice CABAC 未做 |
| HEVC 变换 | 完成 |
| AV1 符号解码器（规范 §8.2） | 完成：init_symbol/read_symbol/CDF 自适应/read_bool/read_literal/exit_symbol |
| AV1 默认 CDF 表 | 完成：96 张、19,677 个值，生成器 + FNV 校验（含 3D 表与 `128 * 125` 乘积更正） |
| AV1 块几何 / partition / intra 模式 | 完成：22 种 BLOCK_SIZES、上下文、`y_mode`/`intra_frame_y_mode`/`uv_mode`+CFL、`tx_depth`、32 张扫描表 |
| AV1 系数语法 | 完成：EOB 类别与 extra bits、`coeff_base`/`eob`/`br`、signs/Golomb、`Quant[]`、tile 级 above/left 上下文 |
| AV1 帧内预测 | 完成 DC/V/H/SMOOTH/SMOOTH_V/H/PAETH、方向预测（含 sub-90 分支）、CFL、边缘滤波与强度选择 |
| AV1 逆变换 | 完成 bit-exact 蝶形 DCT、ADST4/8/16 与 FLIPADST、2D 变换、量化与残差重建；identity scaling 待与 libaom 核 |
| AV1 CDEF | 完成：方向搜索、主/次抽头、约束差值，9 个测试（含 8/10-bit 与色度几何） |
| AV1 loop restoration 滤波 | 完成：Wiener、self-guided（两遍 box + 投影）、stripe 边界源切换，8 个测试含两个独立规范向量 |
| AV1 LR 参数解码（§8.3 subexp） | 完成：`NS(n)` / `decode_subexp_bool` / `inverse_recenter` / `decode_signed_subexp_with_ref_bool`，7 个测试 |
| AV1 LR 帧/条带驱动（§7.17） | 完成：unit 网格几何、`stripeNum`、4x4 prelude、写回，11 个测试；网格可由码流层直接填充 |
| AV1 `read_lr` / `read_lr_unit` 码流层 | 完成：tile 级可变 CDF 与 `RefLrWiener`/`RefSgrXqd` 复位、`use_wiener`/`use_sgrproj`/`restoration_type` 符号、Wiener 两遍抽头与 SGRPROJ set/xqd（含 radius=0 的 `Clip3`）、superres 与 intrabc、unit 网格遍历，15 个测试。此前记为「缺 `TileWienerCdf`/`TileSgrprojCdf` 默认表」是误判：三张 `Default_*` 表本就在 `cdf_defaults.rs` 841–856 行，`Tile*` 是它们的每 tile 可变副本 |
| AV1 全帧 tile 驱动 / 去块 / libaom 像素比对 | 未做：宿主 `aomdec`/`aomenc`/`ffmpeg` 齐备、fixture 在 `tests/data`，但软件解码链路未闭合 |

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

当前 ROADMAP：**93 完成 / 6 未完成**（含缩进子项为 131 / 11）。未完成项里 5 项属于上表的
平台/不存在边界，1 项是 M11 的完整 AV1 软件解码。M11 已落地 39 个 slice：符号解码、默认
CDF、partition/块模式、系数语法、bit-exact 逆变换、量化、重建、帧内预测、CDEF、loop
restoration 滤波与 subexp 参数解码、帧/条带驱动、`read_lr`/`read_lr_unit` 码流层。仍缺
去块与全帧 tile 解码，因此 libaom 像素比对尚未开始（ADST8/16 快照、identity scaling、
蝶形数值对齐也还欠 libaom 佐证）。`cargo test --all-targets`：**917 通过 /
0 失败 / 27 板级 ignore**，`cargo fmt --check` 干净。
