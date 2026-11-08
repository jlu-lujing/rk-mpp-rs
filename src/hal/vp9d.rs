//! rkvdec (vdpu34x) VP9 decode HAL for RK3588.
//!
//! Port of `hal/rkdec/vp9d/hal_vp9d_vdpu34x.c::gen_regs` and the VP9 paths in
//! `common/vdpu34x_com.c`. The hardware consumes three software-packed
//! probability images, a shared segment-id map, one RCB scratch buffer and a
//! normal 276-word vdpu34x register file.

use std::io;

use crate::codec::h264::hw::BitWriter;
use crate::codec::vp9::compressed::{CompoundPredictionMode, CompressedHeader, TxMode};
use crate::codec::vp9::hw::{
    build_kf_probe_383, build_prob_default, build_prob_flag_delta, COUNT_OFFSET,
    MAX_SEGMAP_SIZE_ALIGNED, PROBE_BUFFER_SIZE, PROBE_KF_SIZE_383, PROB_SIZE,
};
use crate::codec::vp9::parser::{FrameKind, Header};
use crate::hal::vdpu34x_regs as reg;
use crate::hal::vdpu383_regs as reg383;
use crate::service::buffer_pool::{BufferLease, BufferPool};
use crate::service::dma_heap::DmaBuf;
use crate::service::soc::{require_video_decode, SocProfile, VideoDecodeHal};
use crate::service::{
    Batch, Client, MppService, RcbInfo as RcbConfig, RegOffsetInfo, MPP_CMD_SET_RCB_INFO,
};

pub const PROBABILITY_CONTEXTS: usize = 4;
const RCB_BUFFER_COUNT: usize = 3;
const RCB_REGION_COUNT: usize = 10;
const RCB_ALIGNMENT: usize = 64;
const SEGMENT_MAP_BYTES_PER_CTU: usize = 8 * 8 / 2;
const REG_READBACK_WORDS: usize = 278;
const GLOBAL_HEADER_SIZE_383: usize = 168;

/// One decoded VP9 picture and its compressed colmv surface.
///
/// The video buffer uses the MPP VP9 layout: 256-odd horizontal alignment,
/// 16-line vertical alignment, followed by a 4:2:0 semi-planar chroma plane.
/// MPP reserves a 9 / 5 stride-area tail beyond the visible 3 / 2 NV12 data.
pub struct Frame {
    pub video: BufferLease<DmaBuf>,
    pub colmv: BufferLease<DmaBuf>,
    pub width: usize,
    pub height: usize,
    pub hor_stride: usize,
    pub ver_stride: usize,
    pub bit_depth: u32,
}

impl Frame {
    pub fn new_video_pool(limit: usize) -> BufferPool<DmaBuf> {
        BufferPool::new(limit)
    }

    pub fn new_colmv_pool(limit: usize) -> BufferPool<DmaBuf> {
        BufferPool::new(limit)
    }

    pub fn acquire(
        video_pool: &BufferPool<DmaBuf>,
        colmv_pool: &BufferPool<DmaBuf>,
        width: usize,
        height: usize,
        bit_depth: u32,
    ) -> io::Result<Frame> {
        let (hor_stride, ver_stride, frame_size) = frame_layout(width, height, bit_depth);
        let video = video_pool
            .acquire_lease_reclaiming_idle(frame_size)
            .map_err(|error| io::Error::other(format!("VP9 video pool: {error}")))?;
        let colmv = colmv_pool
            .acquire_lease_reclaiming_idle(colmv_size(width, height))
            .map_err(|error| io::Error::other(format!("VP9 colmv pool: {error}")))?;
        Ok(Frame {
            video,
            colmv,
            width,
            height,
            hor_stride,
            ver_stride,
            bit_depth,
        })
    }

    pub fn acquire_383(
        video_pool: &BufferPool<DmaBuf>,
        colmv_pool: &BufferPool<DmaBuf>,
        width: usize,
        height: usize,
        bit_depth: u32,
    ) -> io::Result<Frame> {
        let (hor_stride, ver_stride, frame_size) = frame_layout(width, height, bit_depth);
        let video = video_pool
            .acquire_lease_reclaiming_idle(frame_size)
            .map_err(|error| io::Error::other(format!("VP9 video pool: {error}")))?;
        let colmv = colmv_pool
            .acquire_lease_reclaiming_idle(width * height / 2)
            .map_err(|error| io::Error::other(format!("VP9 colmv pool: {error}")))?;
        Ok(Frame {
            video,
            colmv,
            width,
            height,
            hor_stride,
            ver_stride,
            bit_depth,
        })
    }

    pub fn alloc(width: usize, height: usize, bit_depth: u32) -> io::Result<Frame> {
        let video_pool = Self::new_video_pool(1);
        let colmv_pool = Self::new_colmv_pool(1);
        Self::acquire(&video_pool, &colmv_pool, width, height, bit_depth)
    }

    pub fn video_fd(&self) -> u32 {
        self.video.fd().expect("pooled VP9 video has a dma-buf fd") as u32
    }

    pub fn colmv_fd(&self) -> u32 {
        self.colmv.fd().expect("pooled VP9 colmv has a dma-buf fd") as u32
    }
}

fn align(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

/// MPP's `mpp_align_256_odd`: always advance to the next 256-byte boundary.
pub fn align_256_odd(value: usize) -> usize {
    align(value, 256) | 256
}

pub fn frame_layout(width: usize, height: usize, _bit_depth: u32) -> (usize, usize, usize) {
    let hor_stride = align_256_odd(width);
    // VP9 leaves the codec vertical stride at zero, so MPP's decoder buffer
    // slots apply their default 16-row alignment. A 240-line picture therefore
    // uses a 240-line stride, not the 64-row alignment used by VP9 coding
    // tools.
    let ver_stride = align(height, 16);
    let y_size = hor_stride * ver_stride;
    // Keep MppBufSlots's default decoded-frame allocation ratio. The hardware
    // may write into the extra tail, so allocating only the visible NV12 size
    // can overrun the DMA buffer and corrupt following decoder state.
    (hor_stride, ver_stride, y_size * 9 / 5)
}

/// `vdpu34x_get_colmv_size(width, height, 64, 16, 8, 1)`.
pub fn colmv_size(width: usize, height: usize) -> usize {
    let segment_w = 64 * 8 * 8 / 64;
    let segment_h = 64;
    let count_w = align(width, segment_w) / segment_w;
    let count_h = align(height, segment_h) / segment_h;
    let head = align(count_w, 16) * count_h;
    let payload = count_w * count_h * 64 * 16;
    align(head + payload, 128)
}

fn rcb_bytes(bits: usize) -> usize {
    align((bits + 7) / 8, RCB_ALIGNMENT)
}

/// Fixed vdpu34x RCB layout after applying the VP9-specific refined sizes.
#[derive(Debug, Default, Clone, Copy)]
pub struct RcbInfo {
    pub dblk: usize,
    pub intra: usize,
    pub transd_row: usize,
    pub strmd_row: usize,
    pub inter_row: usize,
    pub sao: usize,
    pub fbc: usize,
    pub transd_col: usize,
    pub inter_col: usize,
    pub filt_col: usize,
    offsets: [u32; RCB_REGION_COUNT],
    allocation_size: usize,
}

impl RcbInfo {
    pub fn compute(header: &Header) -> RcbInfo {
        let width = align(header.width as usize, 64);
        let height = align(header.height as usize, 64);
        let bit_depth = header.bit_depth as usize;
        let num_tiles = header.log2_tile_rows as usize;
        let ext = num_tiles * 64 * 8;

        let common_sizes = [
            align(width * 22, RCB_ALIGNMENT),
            align(width * 6, RCB_ALIGNMENT),
            align(width, RCB_ALIGNMENT),
            align(width * 3, RCB_ALIGNMENT),
            align(width * 6, RCB_ALIGNMENT),
            align(width * 6, RCB_ALIGNMENT),
            align(width * 11, RCB_ALIGNMENT),
            align(height, RCB_ALIGNMENT),
            align(height * 3, RCB_ALIGNMENT),
            align(height * 67, RCB_ALIGNMENT),
        ];
        let mut offsets = [0u32; RCB_REGION_COUNT];
        let mut allocation_size = 0usize;
        for (offset, size) in offsets.iter_mut().zip(common_sizes) {
            *offset = allocation_size as u32;
            allocation_size += size;
        }

        RcbInfo {
            strmd_row: rcb_bytes(if width > 4096 {
                align(width, 64) * 232 + ext
            } else {
                0
            }),
            transd_row: rcb_bytes(if width > 8192 {
                align(width - 8192, 4) * 2 + ext
            } else {
                0
            }),
            transd_col: rcb_bytes(if height > 8192 {
                align(height - 8192, 4) * 2 + ext
            } else {
                0
            }),
            inter_row: rcb_bytes(width * 36 + ext),
            inter_col: 0,
            intra: rcb_bytes(width * 48 + ext),
            dblk: rcb_bytes(width * (1 + 16 * bit_depth) + num_tiles * 192 * bit_depth + ext),
            sao: 0,
            fbc: 0,
            filt_col: rcb_bytes(height * (4 + 16 * bit_depth) + ext),
            offsets,
            allocation_size,
        }
    }

    pub fn total(&self) -> usize {
        self.allocation_size
    }

    pub fn offsets(&self) -> Vec<RegOffsetInfo> {
        let mut out = Vec::with_capacity(RCB_REGION_COUNT);
        self.write_offsets(&mut out);
        out
    }

    /// Same entries as [`Self::offsets`], appended into a caller-owned
    /// buffer so the decode path does not allocate per frame.
    pub fn write_offsets(&self, out: &mut Vec<RegOffsetInfo>) {
        out.extend(
            [139u32, 133, 134, 136, 137, 140, 141, 135, 138, 142]
                .iter()
                .zip(self.offsets)
                .map(|(&reg_idx, offset)| RegOffsetInfo { reg_idx, offset }),
        );
    }

    #[cfg(test)]
    fn hardware_sizes(&self) -> Vec<RcbConfig> {
        let mut out = Vec::new();
        self.write_hardware_sizes(&mut out);
        out
    }

    /// Same entries as [`Self::hardware_sizes`], appended into a
    /// caller-owned buffer so the decode path does not allocate per frame.
    fn write_hardware_sizes(&self, out: &mut Vec<RcbConfig>) {
        out.extend(
            [
                (139u32, self.dblk),
                (133, self.intra),
                (140, self.sao),
                (141, self.fbc),
                (134, self.transd_row),
                (136, self.strmd_row),
                (138, self.inter_col),
                (142, self.filt_col),
                (135, self.transd_col),
            ]
            .into_iter()
            .filter(|(_, size)| *size > 0)
            .map(|(reg_idx, size)| RcbConfig {
                reg_idx,
                size: size as u32,
            }),
        );
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct RcbInfo383 {
    sizes: [usize; 11],
    offsets: [usize; 11],
    allocation_size: usize,
}

impl RcbInfo383 {
    fn compute(header: &Header) -> Self {
        const COEFFICIENTS: [usize; 11] = [3, 3, 6, 6, 5, 5, 60, 60, 60, 90, 0];

        let width = align(header.width as usize, 64);
        let height = align(header.height as usize, 64);
        let mut offsets = [0usize; 11];
        let mut allocation_size = 0usize;
        for (index, &coefficient) in COEFFICIENTS.iter().enumerate() {
            offsets[index] = allocation_size;
            let axis = if index >= 9 { height } else { width };
            allocation_size += 2 * align(axis * coefficient, RCB_ALIGNMENT);
        }

        let bit_depth = header.bit_depth as usize;
        let tile_rows = 1usize << header.log2_tile_rows;
        let tile_cols = 1usize << header.log2_tile_cols;
        let ext_row = (tile_rows.saturating_sub(1)) * 64 * 8;
        let ext_col = (tile_cols.saturating_sub(1)) * 64 * 8;
        let filtered_append = if width > 4096 { 27648 } else { 8192 };
        let strmd = ((width + 63) / 64) * 250;
        let inter = ((width + 63) / 64) * 2368;
        let intra = align(width, 512) * (bit_depth + 2) * 3;
        let filtered = align(width, 64) * (41 * bit_depth + 13);

        let mut sizes = [0usize; 11];
        sizes[0] = if width > 4096 { rcb_bytes(strmd) } else { 0 };
        sizes[2] = rcb_bytes(inter);
        sizes[3] = if tile_rows > 1 {
            rcb_bytes(inter + ext_row)
        } else {
            0
        };
        sizes[4] = rcb_bytes(intra);
        sizes[5] = if tile_rows > 1 {
            rcb_bytes(intra + ext_row)
        } else {
            0
        };
        sizes[6] = filtered_append + rcb_bytes(filtered / 2);
        sizes[7] = sizes[6];
        sizes[8] = if tile_rows > 1 {
            rcb_bytes(filtered + ext_row)
        } else {
            0
        };
        sizes[9] = if tile_cols > 1 {
            let col = align(height, 64) * (42 * bit_depth + 13);
            rcb_bytes(col + ext_col)
        } else {
            0
        };

        Self {
            sizes,
            offsets,
            allocation_size: align(allocation_size.max(1), 4096),
        }
    }

    fn total(&self) -> usize {
        self.allocation_size
    }

    fn write_regs(&self, r: &mut reg383::Regs, fd: u32) {
        for (index, &size) in self.sizes.iter().enumerate() {
            let base = 140 + index * 2;
            r.w[base] = fd;
            r.w[base + 1] = size as u32;
        }
    }

    fn offsets(&self) -> Vec<RegOffsetInfo> {
        self.offsets
            .iter()
            .zip(self.sizes)
            .enumerate()
            .filter(|(_, (offset, size))| **offset != 0 && *size != 0)
            .map(|(index, (offset, _))| RegOffsetInfo {
                reg_idx: (140 + index * 2) as u32,
                offset: *offset as u32,
            })
            .collect()
    }

    fn hardware_sizes(&self) -> Vec<RcbConfig> {
        const PRIORITY: [usize; 11] = [6, 2, 4, 0, 3, 5, 1, 8, 9, 10, 7];
        PRIORITY
            .iter()
            .filter_map(|&index| {
                (self.sizes[index] > 0).then_some(RcbConfig {
                    reg_idx: (140 + index * 2) as u32,
                    size: self.sizes[index] as u32,
                })
            })
            .collect()
    }
}

/// The address offsets one VP9 job arms: the ten RCB regions plus the
/// colmv-count and the two segmentation-map slots, zero offsets dropped.
/// The scratch buffer is cleared first, so the entry list is rebuilt in
/// full every frame.
fn build_offset_list(
    rcb: &RcbInfo,
    segment_previous: u32,
    segment_current: u32,
    out: &mut Vec<RegOffsetInfo>,
) {
    out.clear();
    rcb.write_offsets(out);
    out.push(RegOffsetInfo {
        reg_idx: 167,
        offset: COUNT_OFFSET as u32,
    });
    out.push(RegOffsetInfo {
        reg_idx: 168,
        offset: segment_previous,
    });
    out.push(RegOffsetInfo {
        reg_idx: 169,
        offset: segment_current,
    });
    // MPP's mpp_service_reg_offset() deliberately ignores zero offsets.
    // Sending an explicit zero is not equivalent on the kernel ABI: the
    // segmentation map swaps one register at a time, so the zero-offset
    // register must stay absent just like in the C HAL.
    out.retain(|info| info.offset != 0);
}

#[derive(Debug, Clone, Copy)]
struct LastFrameInfo {
    last_ref_deltas: [i8; 4],
    last_mode_deltas: [i8; 2],
    segmentation_enable: bool,
    abs_delta: bool,
    feature_data: [[i16; 4]; 8],
    feature_mask: [u8; 8],
    show_frame: bool,
    intra_only: bool,
    frame_type: bool,
    color_space: u8,
    width: u32,
    height: u32,
}

impl Default for LastFrameInfo {
    fn default() -> Self {
        Self {
            last_ref_deltas: [0; 4],
            last_mode_deltas: [0; 2],
            segmentation_enable: false,
            abs_delta: false,
            feature_data: [[0; 4]; 8],
            feature_mask: [0; 8],
            show_frame: false,
            intra_only: false,
            frame_type: false,
            color_space: 0,
            width: 0,
            height: 0,
        }
    }
}

pub struct Vp9d {
    svc: MppService,
    probe: DmaBuf,
    global_383: Option<DmaBuf>,
    probe_kf_383: Option<DmaBuf>,
    segment: DmaBuf,
    default_prob: DmaBuf,
    context_prob: [DmaBuf; PROBABILITY_CONTEXTS],
    rcbs: [DmaBuf; RCB_BUFFER_COUNT],
    rcb_index: usize,
    rcb_info: RcbInfo,
    rcb_info_supported: bool,
    context_valid: [bool; PROBABILITY_CONTEXTS],
    context_poc: [u32; PROBABILITY_CONTEXTS],
    col_reference_poc: u32,
    segment_reference_poc: u32,
    last_segment_swap: bool,
    previous_colmv_fd: Option<u32>,
    last_colmv_fd: Option<u32>,
    pending_segment_offsets: (usize, usize),
    last: LastFrameInfo,
    pub regs: reg::Regs,
    regs_383: reg383::Regs,
    hal: VideoDecodeHal,
    rcb_383: RcbInfo383,
    pub last_status: (u32, u32, u32),
    pub last_hardware_error: bool,
    /// Reused across frames: the ioctl needs owned scratch but not fresh
    /// allocations, and the entries are rebuilt in full every job.
    offsets_scratch: Vec<RegOffsetInfo>,
    rcb_scratch: Vec<RcbConfig>,
    batch: Batch,
}

impl Vp9d {
    pub fn open() -> io::Result<Vp9d> {
        let profile = SocProfile::detect()?;
        let hal = profile.video_decode;
        if !matches!(hal, VideoDecodeHal::Vdpu34x | VideoDecodeHal::Vdpu383) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("no VP9 decode HAL for {hal:?}"),
            ));
        }
        require_video_decode(hal)?;
        let svc = MppService::open(Client::RkvDec as u32)?;
        let rcb_info_supported = svc.cmd_supported(MPP_CMD_SET_RCB_INFO);
        let probe = DmaBuf::alloc("system", PROBE_BUFFER_SIZE, false)?;
        let global_383 = if hal == VideoDecodeHal::Vdpu383 {
            Some(DmaBuf::alloc("system", 352, false)?)
        } else {
            None
        };
        let probe_kf_383 = if hal == VideoDecodeHal::Vdpu383 {
            Some(DmaBuf::alloc("system", PROBE_KF_SIZE_383, false)?)
        } else {
            None
        };
        let segment = DmaBuf::alloc("system", MAX_SEGMAP_SIZE_ALIGNED * 2, false)?;
        let default_prob = DmaBuf::alloc("system", PROB_SIZE, false)?;
        let context_prob = std::array::from_fn(|_| {
            DmaBuf::alloc("system", PROB_SIZE, false).expect("allocate VP9 context")
        });
        let rcbs = std::array::from_fn(|_| {
            DmaBuf::alloc("system", 1 << 20, false).expect("allocate VP9 RCB")
        });
        Ok(Vp9d {
            svc,
            probe,
            global_383,
            probe_kf_383,
            segment,
            default_prob,
            context_prob,
            rcbs,
            rcb_index: 0,
            rcb_info: RcbInfo::default(),
            rcb_info_supported,
            context_valid: [false; PROBABILITY_CONTEXTS],
            context_poc: [0; PROBABILITY_CONTEXTS],
            col_reference_poc: 0,
            segment_reference_poc: 0,
            last_segment_swap: true,
            previous_colmv_fd: None,
            last_colmv_fd: None,
            pending_segment_offsets: (MAX_SEGMAP_SIZE_ALIGNED, 0),
            last: LastFrameInfo::default(),
            regs: reg::Regs::new(),
            regs_383: reg383::Regs::new(),
            hal,
            rcb_383: RcbInfo383::default(),
            last_status: (0, 0, 0),
            last_hardware_error: false,
            offsets_scratch: Vec::with_capacity(RCB_REGION_COUNT + 3),
            rcb_scratch: Vec::with_capacity(RCB_REGION_COUNT),
            batch: Batch::new(),
        })
    }

    pub fn colmv_liveness_fds(&self) -> [Option<u32>; 2] {
        [self.previous_colmv_fd, self.last_colmv_fd]
    }

    pub fn uses_vdpu383(&self) -> bool {
        self.hal == VideoDecodeHal::Vdpu383
    }

    fn write_probability(
        &mut self,
        header: &Header,
        compressed: &CompressedHeader,
        poc: u32,
    ) -> io::Result<()> {
        if self.hal == VideoDecodeHal::Vdpu383 {
            let probe = self
                .probe_kf_383
                .as_mut()
                .expect("VDPU383 probe buffer exists");
            let probe_data = build_kf_probe_383();
            probe.sync_write_begin()?;
            probe.as_mut_slice().fill(0);
            probe.as_mut_slice()[..PROBE_KF_SIZE_383].copy_from_slice(&probe_data);
            probe.sync_write_end()?;
            if is_intra(header) {
                let default = build_prob_default(header, compressed);
                self.default_prob.sync_write_begin()?;
                self.default_prob.as_mut_slice()[..PROB_SIZE].copy_from_slice(&default);
                self.default_prob.sync_write_end()?;
            }
            return Ok(());
        }

        let update = build_prob_flag_delta(header, compressed);
        self.probe.sync_write_begin()?;
        self.probe.as_mut_slice()[..PROB_SIZE].copy_from_slice(&update);
        self.probe.sync_write_end()?;

        if is_intra(header) {
            let default = build_prob_default(header, compressed);
            self.default_prob.sync_write_begin()?;
            self.default_prob.as_mut_slice()[..PROB_SIZE].copy_from_slice(&default);
            self.default_prob.sync_write_end()?;
        }
        if std::env::var_os("RFI_VP9_PROB_DUMP").is_some() {
            let _ = std::fs::write(
                format!("/tmp/rk_mpp_rs_vp9_prob_update_{poc:04}.bin"),
                update,
            );
            if is_intra(header) {
                let _ = std::fs::write(
                    format!("/tmp/rk_mpp_rs_vp9_prob_default_{poc:04}.bin"),
                    &self.default_prob.as_slice()[..PROB_SIZE],
                );
            }
        }
        Ok(())
    }

    fn reset_segment_maps(&mut self, width: u32, height: u32) -> io::Result<()> {
        let map_len = segment_map_len(width, height);
        let second_map = MAX_SEGMAP_SIZE_ALIGNED;
        self.segment.sync_write_begin()?;
        let maps = self.segment.as_mut_slice();
        maps[..map_len].fill(0);
        maps[second_map..second_map + map_len].fill(0);
        self.segment.sync_write_end()
    }

    /// Fill and submit one VP9 picture.
    #[allow(clippy::too_many_arguments)]
    pub fn decode(
        &mut self,
        header: &Header,
        compressed: &CompressedHeader,
        poc: u32,
        current: &Frame,
        references: &[Option<(&Frame, u32)>; 3],
        stream: &DmaBuf,
        stream_len: usize,
    ) -> io::Result<()> {
        super::check_decode_geometry(current.width, current.height)?;
        self.write_probability(header, compressed, poc)?;
        // VP9 resets past segmentation state on intra-only and error-resilient
        // frames (libvpx's vp9_setup_past_independence). The old vdpu34x HAL
        // only flipped the map buffers, leaving stale segment ids visible to
        // temporal segment prediction on the following frame.
        if is_intra(header) || header.error_resilient_mode {
            self.reset_segment_maps(header.width, header.height)?;
        }

        if self.hal == VideoDecodeHal::Vdpu383 {
            let rcb = RcbInfo383::compute(header);
            let total = rcb.total().max(4096);
            for buffer in &mut self.rcbs {
                if total > buffer.len() {
                    *buffer = DmaBuf::alloc("system", align(total, 4096), false)?;
                }
            }
            self.rcb_index = (self.rcb_index + 1) % RCB_BUFFER_COUNT;
            self.rcb_383 = rcb;
            self.fill_regs_383(
                header, compressed, poc, current, references, stream, stream_len,
            );

            if std::env::var_os("RFI_VP9_REG_DUMP").is_some() {
                use std::io::Write;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/tmp/rk_mpp_rs_vp9_regs_383.txt")
                {
                    let _ = writeln!(file, "FRAME {poc}");
                    for index in 8..self.regs_383.w.len() {
                        let _ = writeln!(file, "{index:03}: {:08x}", self.regs_383.w[index]);
                    }
                }
            }

            self.offsets_scratch.clear();
            self.offsets_scratch.extend(self.rcb_383.offsets());
            let (segment_previous, segment_current) = self.pending_segment_offsets;
            self.offsets_scratch.push(RegOffsetInfo {
                reg_idx: 181,
                offset: segment_previous as u32,
            });
            self.offsets_scratch.push(RegOffsetInfo {
                reg_idx: 182,
                offset: segment_current as u32,
            });
            self.offsets_scratch.push(RegOffsetInfo {
                reg_idx: 128,
                offset: (header.header_size & !15) as u32,
            });
            self.offsets_scratch.retain(|info| info.offset != 0);

            self.rcb_scratch.clear();
            self.rcb_scratch.extend(self.rcb_383.hardware_sizes());
            let mut readback = [0u32; REG_READBACK_WORDS];
            self.batch.clear();
            self.batch
                .reg_write(&self.regs_383.w[8..31], 32)
                .reg_write(&self.regs_383.w[64..107], 256)
                .reg_write(&self.regs_383.w[128..162], 512)
                .reg_write(&self.regs_383.w[168..233], 672)
                .reg_read(&mut readback[..reg383::N_WORDS], 0)
                .reg_offsets(&self.offsets_scratch);
            if self.rcb_info_supported && !self.rcb_scratch.is_empty() {
                self.batch.rcb_info(&self.rcb_scratch);
            }
            self.svc.submit_batch_recovering(&mut self.batch)?;
            self.svc.poll_hw_finish_recovering()?;
            let status = readback[15];
            self.last_status = (status, readback[224], readback[225]);
            self.last_hardware_error = status & 1 == 0 || status & 0x7e != 0;
            if self.last_hardware_error {
                let _ = self.svc.restart_after_hardware_error();
            }
            return Ok(());
        }

        let rcb = RcbInfo::compute(header);
        let total = rcb.total().max(4096);
        for buffer in &mut self.rcbs {
            if total > buffer.len() {
                *buffer = DmaBuf::alloc("system", align(total, 4096), false)?;
            }
        }
        self.rcb_index = (self.rcb_index + 1) % RCB_BUFFER_COUNT;
        self.rcb_info = rcb;

        self.fill_regs(
            header, compressed, poc, current, references, stream, stream_len,
        );

        if std::env::var_os("RFI_VP9_REG_DUMP").is_some() {
            use std::io::Write;

            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/rk_mpp_rs_vp9_regs.txt")
            {
                let _ = writeln!(file, "FRAME {poc}");
                for index in 8..self.regs.w.len() {
                    let _ = writeln!(file, "{index:03}: {:08x}", self.regs.w[index]);
                }
            }
        }

        if std::env::var_os("RFI_VP9_SEGMENT_DUMP").is_some() {
            let _ = self.segment.sync_read_begin();
            let _ = std::fs::write(
                format!("/tmp/rk_mpp_rs_vp9_seg_pre_{poc:04}.bin"),
                self.segment.as_slice(),
            );
            let _ = self.segment.sync_read_end();
        }

        // The five register sections are contiguous in the register file, so
        // the batch can point straight into it: the bytes the driver sees are
        // the same words the to_vec() pass used to copy, without a per-frame
        // allocation.
        let (segment_previous, segment_current) = self.pending_segment_offsets;
        build_offset_list(
            &self.rcb_info,
            segment_previous as u32,
            segment_current as u32,
            &mut self.offsets_scratch,
        );

        self.rcb_scratch.clear();
        self.rcb_info.write_hardware_sizes(&mut self.rcb_scratch);

        let mut readback = [0u32; REG_READBACK_WORDS];
        self.batch.clear();
        self.batch
            .reg_write(&self.regs.w[8..64], 32)
            .reg_write(&self.regs.w[64..128], 256)
            .reg_write(&self.regs.w[128..200], 512)
            .reg_write(&self.regs.w[256..276], 1024)
            .reg_read(&mut readback, 0)
            .reg_offsets(&self.offsets_scratch);
        if self.rcb_info_supported && !self.rcb_scratch.is_empty() {
            self.batch.rcb_info(&self.rcb_scratch);
        }
        self.svc.submit_batch_recovering(&mut self.batch)?;
        self.svc.poll_hw_finish_recovering()?;

        self.last_status = (readback[224], readback[225], readback[230]);
        self.last_hardware_error = readback[224] & (1 << 2) == 0;
        if self.last_hardware_error {
            let _ = self.svc.restart_after_hardware_error();
        }
        if std::env::var_os("RFI_VP9_SEGMENT_DUMP").is_some() {
            let _ = self.segment.sync_read_begin();
            let _ = std::fs::write(
                format!("/tmp/rk_mpp_rs_vp9_seg_post_{poc:04}.bin"),
                self.segment.as_slice(),
            );
            let _ = self.segment.sync_read_end();
        }
        let _ = stream;
        let _ = stream_len;
        Ok(())
    }

    fn fill_regs(
        &mut self,
        header: &Header,
        compressed: &CompressedHeader,
        poc: u32,
        current: &Frame,
        references: &[Option<(&Frame, u32)>; 3],
        stream: &DmaBuf,
        stream_len: usize,
    ) {
        self.regs = reg::Regs::new();
        let intra = is_intra(header);
        let frame_context = header.frame_context_idx as usize;

        let had_probability_context =
            self.configure_probability_contexts(header, poc, frame_context);
        let probability_poc = if had_probability_context {
            self.context_poc[frame_context]
        } else {
            0
        };
        let read_probability_index = if had_probability_context {
            frame_context as u32 + 1
        } else {
            0
        };

        let mut last_for_registers = self.last;
        if intra {
            last_for_registers.segmentation_enable = false;
            last_for_registers.intra_only = true;
        }

        let segment_update = (header.segmentation.enabled && header.segmentation.update_map)
            || self.last.width != header.width
            || self.last.height != header.height
            || intra
            || header.error_resilient_mode;
        self.pending_segment_offsets = if self.last_segment_swap {
            (MAX_SEGMAP_SIZE_ALIGNED, 0)
        } else {
            (0, MAX_SEGMAP_SIZE_ALIGNED)
        };
        if segment_update {
            self.segment_reference_poc = poc;
            self.last_segment_swap = !self.last_segment_swap;
        }

        let previous_colmv_fd = select_previous_colmv(
            current.colmv_fd(),
            intra,
            header.error_resilient_mode,
            self.last.show_frame,
            &mut self.previous_colmv_fd,
            &mut self.last_colmv_fd,
        );
        let r = &mut self.regs;
        configure_common(r);
        r.w[9] = 2;
        r.w[16] = (((stream_len + 15) & !15) + 0x80) as u32;
        r.w[12] |= 1 << 1;
        r.w[12] |= 1 << 7;
        if matches!(header.frame_kind, FrameKind::Key) {
            r.w[13] |= 1 << 24;
        }
        r.w[18] = (current.hor_stride / 16) as u32;
        r.w[19] = (current.hor_stride / 16) as u32;
        r.w[20] = ((current.hor_stride / 16) * current.ver_stride) as u32;
        r.w[28] = frame_context as u32 + 1;
        r.w[28] |= read_probability_index << 4;

        r.w[64] = 0;
        r.w[65] = poc;
        configure_segments(r, &last_for_registers);
        r.w[75] |= ((last_for_registers.last_mode_deltas[0] as u32 & 0x7f) << 0)
            | ((last_for_registers.last_mode_deltas[1] as u32 & 0x7f) << 7);
        if segment_update {
            r.w[75] |= 1 << 15;
        }
        r.w[75] |= ((last_for_registers.segmentation_enable as u32) << 16)
            | ((last_for_registers.show_frame as u32) << 17)
            | ((last_for_registers.intra_only as u32) << 18)
            | (((last_for_registers.width == header.width
                && last_for_registers.height == header.height) as u32)
                << 19);
        r.w[76] = tx_mode_number(compressed.tx_mode) as u32;
        r.w[76] |= (compound_mode_number(compressed.compound_prediction_mode) as u32) << 3;
        r.w[78] = stream_len.saturating_sub(header.first_partition_size as usize) as u32;

        for (index, reference) in references.iter().enumerate() {
            if let Some((frame, _)) = reference {
                let horizontal = (frame.hor_stride / 16) as u32;
                let vertical = horizontal * frame.ver_stride as u32;
                let width = frame.width as u32;
                let height = frame.height as u32;
                match index {
                    0 => {
                        r.w[106] = width;
                        r.w[107] = height;
                        r.w[79] = horizontal;
                        r.w[80] = horizontal;
                        r.w[85] = vertical;
                    }
                    1 => {
                        r.w[108] = width;
                        r.w[109] = height;
                        r.w[81] = horizontal;
                        r.w[82] = horizontal;
                        r.w[86] = vertical;
                    }
                    2 => {
                        r.w[110] = width;
                        r.w[111] = height;
                        r.w[83] = horizontal;
                        r.w[84] = horizontal;
                        r.w[87] = vertical;
                    }
                    _ => unreachable!(),
                }
            }
        }

        for index in 0..3 {
            if !intra {
                r.w[88 + index * 2] = header.mvscale[index][0] as u32;
                r.w[89 + index * 2] = header.mvscale[index][1] as u32;
            }
        }
        if !intra {
            for (index, delta) in last_for_registers.last_ref_deltas.iter().enumerate() {
                r.w[94] |= (*delta as u32 & 0x7f) << (7 * index);
            }
        }
        if let Some((_, reference_poc)) = references[0] {
            r.w[95] = reference_poc;
        }
        if let Some((_, reference_poc)) = references[1] {
            r.w[96] = reference_poc;
        }
        if let Some((_, reference_poc)) = references[2] {
            r.w[97] = reference_poc;
        }
        r.w[98] = if self.col_reference_poc != 0 {
            self.col_reference_poc
        } else {
            poc
        };
        if header.show_frame && !matches!(header.frame_kind, FrameKind::ShowExisting) {
            self.col_reference_poc = poc;
        }
        r.w[99] = probability_poc;
        r.w[100] = if segment_update {
            0
        } else {
            self.segment_reference_poc
        };

        configure_probability_control(r, header, compressed, intra, self.last.intra_only);
        configure_addresses(
            r,
            &self.probe,
            &self.segment,
            &self.context_prob[frame_context],
            &self.default_prob,
            &self.rcbs[(self.rcb_index + RCB_BUFFER_COUNT - 1) % RCB_BUFFER_COUNT],
            stream,
            current,
            references,
            had_probability_context,
            previous_colmv_fd,
        );
        self.update_last_frame_info(header);
    }

    fn fill_regs_383(
        &mut self,
        header: &Header,
        compressed: &CompressedHeader,
        poc: u32,
        current: &Frame,
        references: &[Option<(&Frame, u32)>; 3],
        stream: &DmaBuf,
        stream_len: usize,
    ) {
        self.regs_383 = reg383::Regs::new();
        let intra = is_intra(header);
        let frame_context = header.frame_context_idx as usize;
        let had_probability_context =
            self.configure_probability_contexts(header, poc, frame_context);

        let mut last_for_registers = self.last;
        if intra {
            last_for_registers.segmentation_enable = false;
            last_for_registers.intra_only = true;
        }
        let segment_update = (header.segmentation.enabled && header.segmentation.update_map)
            || self.last.width != header.width
            || self.last.height != header.height
            || intra
            || header.error_resilient_mode;
        self.pending_segment_offsets = if self.last_segment_swap {
            (MAX_SEGMAP_SIZE_ALIGNED, 0)
        } else {
            (0, MAX_SEGMAP_SIZE_ALIGNED)
        };
        if segment_update {
            self.segment_reference_poc = poc;
            self.last_segment_swap = !self.last_segment_swap;
        }
        let previous_colmv_fd = select_previous_colmv(
            current.colmv_fd(),
            intra,
            header.error_resilient_mode,
            self.last.show_frame,
            &mut self.previous_colmv_fd,
            &mut self.last_colmv_fd,
        );

        let global = self
            .global_383
            .as_mut()
            .expect("VDPU383 global buffer exists");
        let global_header =
            build_global_header_383(header, compressed, references, &last_for_registers);
        global
            .sync_write_begin()
            .expect("sync VDPU383 global header");
        global.as_mut_slice().fill(0);
        global.as_mut_slice()[..global_header.len()].copy_from_slice(&global_header);
        global.sync_write_end().expect("sync VDPU383 global header");

        let r = &mut self.regs_383;
        configure_common_383(r);
        r.w[65] = ((header.header_size & 0xf) as u32) * 8;
        r.w[66] = (((stream_len + 15) & !15) + 0x80) as u32;
        r.w[67] = 22;
        r.w[68] = (current.hor_stride / 16) as u32;
        r.w[69] = (current.hor_stride / 16) as u32;
        r.w[70] = ((current.hor_stride / 16) * current.ver_stride) as u32;
        r.w[80] = r.w[68];
        r.w[81] = r.w[69];
        r.w[82] = r.w[70];
        r.w[128] = stream.fd() as u32;
        r.w[131] = global.fd() as u32;
        r.w[168] = current.video_fd();
        r.w[169] = current.video_fd();
        r.w[183] = self
            .probe_kf_383
            .as_ref()
            .expect("VDPU383 probe buffer exists")
            .fd() as u32;
        r.w[184] = if had_probability_context {
            self.context_prob[frame_context].fd() as u32
        } else {
            self.default_prob.fd() as u32
        };
        r.w[185] = self.context_prob[frame_context].fd() as u32;
        r.w[181] = self.segment.fd() as u32;
        r.w[182] = self.segment.fd() as u32;
        r.w[192] = current.video_fd();
        r.w[194] = current.video_fd();

        for (index, reference) in references.iter().enumerate() {
            if let Some((frame, _)) = reference {
                r.w[83 + index * 3] = (frame.hor_stride / 16) as u32;
                r.w[84 + index * 3] = (frame.hor_stride / 16) as u32;
                r.w[85 + index * 3] = ((frame.hor_stride / 16) * frame.ver_stride) as u32;
                r.w[170 + index] = frame.video_fd();
                r.w[195 + index] = frame.video_fd();
            } else {
                r.w[170 + index] = current.video_fd();
                r.w[195 + index] = current.video_fd();
            }
        }
        r.w[216] = current.colmv_fd();
        r.w[217] = previous_colmv_fd;
        self.rcb_383.write_regs(
            r,
            self.rcbs[(self.rcb_index + RCB_BUFFER_COUNT - 1) % RCB_BUFFER_COUNT].fd() as u32,
        );
        self.update_last_frame_info(header);
    }

    fn configure_probability_contexts(
        &mut self,
        header: &Header,
        poc: u32,
        frame_context: usize,
    ) -> bool {
        let had_context = update_probability_context(
            header,
            poc,
            frame_context,
            &mut self.context_valid,
            &mut self.context_poc,
        );
        if std::env::var_os("RFI_VP9_PROB_LAST_DUMP").is_some() {
            let source = if had_context {
                &self.context_prob[frame_context]
            } else {
                &self.default_prob
            };
            let _ = source.sync_read_begin();
            let _ = std::fs::write(
                format!("/tmp/rk_mpp_rs_vp9_prob_last_{poc:04}.bin"),
                &source.as_slice()[..PROB_SIZE],
            );
            let _ = source.sync_read_end();
        }
        had_context
    }

    fn update_last_frame_info(&mut self, header: &Header) {
        self.last.last_ref_deltas = header.loop_filter.ref_deltas;
        self.last.last_mode_deltas = header.loop_filter.mode_deltas;
        self.last.abs_delta = header.segmentation.abs_delta;
        self.last.feature_data = header.segmentation.feature_data;
        self.last.feature_mask = header.segmentation.feature_mask;
        if !self.last.segmentation_enable {
            self.last.segmentation_enable = header.segmentation.enabled;
        }
        self.last.show_frame = header.show_frame;
        self.last.intra_only = is_intra(header);
        self.last.frame_type = !matches!(header.frame_kind, FrameKind::Key);
        self.last.color_space = 0;
        self.last.width = header.width;
        self.last.height = header.height;
        if is_intra(header) {
            // The C HAL resets this before programming the current keyframe.
            self.last.segmentation_enable = header.segmentation.enabled;
        }
    }
}

fn update_probability_context(
    header: &Header,
    poc: u32,
    frame_context: usize,
    context_valid: &mut [bool; PROBABILITY_CONTEXTS],
    context_poc: &mut [u32; PROBABILITY_CONTEXTS],
) -> bool {
    if is_intra(header) || header.error_resilient_mode {
        if is_intra(header) || header.error_resilient_mode || header.reset_frame_context == 3 {
            *context_valid = [false; PROBABILITY_CONTEXTS];
        } else if header.reset_frame_context == 2 {
            context_valid[frame_context] = false;
        }
    }
    let had_probability_context = context_valid[frame_context];
    if !context_valid[frame_context] {
        context_valid[frame_context] = header.refresh_frame_context;
        context_poc[frame_context] = poc;
    }
    if !header.frame_parallel_mode {
        context_poc[frame_context] = poc;
    }
    had_probability_context
}

fn is_intra(header: &Header) -> bool {
    matches!(header.frame_kind, FrameKind::Key | FrameKind::IntraOnly)
}

/// Mirror MPP's `pre_mv_base_addr` / `mv_base_addr` pair. A hidden frame keeps
/// the older shown-frame colmv live for the next normal inter frame.
fn select_previous_colmv(
    current_fd: u32,
    intra: bool,
    error_resilient_mode: bool,
    last_show_frame: bool,
    previous_colmv_fd: &mut Option<u32>,
    last_colmv_fd: &mut Option<u32>,
) -> u32 {
    if !intra && !error_resilient_mode && last_show_frame {
        *previous_colmv_fd = *last_colmv_fd;
    }
    let selected = previous_colmv_fd.unwrap_or(current_fd);
    *last_colmv_fd = Some(current_fd);
    if previous_colmv_fd.is_none() {
        *previous_colmv_fd = Some(current_fd);
    }
    selected
}

fn configure_common(r: &mut reg::Regs) {
    r.w[11] |= (1 << 1) | (1 << 5) | (1 << 6);
    r.w[10] |= 1;
    r.w[13] |= 1;
    r.w[26] = 0x800f_ffef;
    r.w[32] = 0x0003_ffff;
    r.set(reg::w11::PIX_RANGE_DETECTION_E, 1)
        .set(reg::w256::AXI_PERF_WORK_E, 1)
        .set(reg::w256::AXI_PERF_CLR_E, 1)
        .set(reg::w256::AXI_CNT_TYPE, 1)
        .set(reg::w257::ADDR_ALIGN_TYPE, 1)
        .set(reg::w270::BUS2MC_BUFFER_QOS_LEVEL, 255)
        .set(reg::w270::AXI_RD_HURRY_LEVEL, 3)
        .set(reg::w270::AXI_WR_HURRY_LEVEL, 1)
        .set(reg::w270::AXI_WR_QOS, 1)
        .set(reg::w270::AXI_RD_QOS, 3)
        .set(reg::w271::REG271_WR_WAIT_CYCLE_QOS, 0);
}

fn configure_common_383(r: &mut reg383::Regs) {
    r.w[8] = 2;
    r.w[9] = 0;
    r.w[10] = 0x3ff;
    r.w[13] = 0x3ffff;
    r.w[16] = 1;
    r.w[20] = 0xffff_ffdf;
    r.w[21] = 0x3fff_ffff;
    r.w[28] = 1 | (1 << 3) | (11 << 4);
    r.w[29] = 1 | (1 << 3) | (17 << 4);
    r.w[30] = 0;
}

fn build_global_header_383(
    header: &Header,
    _compressed: &CompressedHeader,
    references: &[Option<(&Frame, u32)>; 3],
    last: &LastFrameInfo,
) -> [u8; GLOBAL_HEADER_SIZE_383] {
    const LITERAL_TO_FILTER: [u32; 4] = [1, 0, 2, 3];
    let frame_type = !matches!(header.frame_kind, FrameKind::Key) as u32;
    let mut w = BitWriter::new();
    w.put(frame_type, 1);
    w.put(header.error_resilient_mode as u32, 1);
    w.put((header.bit_depth - 8) as u32, 3);
    w.put(1, 2); // yuv420
    w.put(header.width, 16);
    w.put(header.height, 16);
    w.put(
        (header.intra_only || matches!(header.frame_kind, FrameKind::Key)) as u32,
        1,
    );
    for bias in &header.ref_frame_sign_bias {
        w.put(*bias as u32, 1);
    }
    w.put(header.allow_high_precision_mv as u32, 1);
    if header.intra_only || matches!(header.frame_kind, FrameKind::Key) {
        w.put(0, 3);
    } else if header.interp_filter == 4 {
        w.put(4, 3);
    } else {
        w.put(LITERAL_TO_FILTER[header.interp_filter as usize & 3], 3);
    }
    w.put(header.frame_parallel_mode as u32, 1);
    w.put(header.refresh_frame_context as u32, 1);
    w.put(header.loop_filter.level as u32, 6);
    w.put(header.loop_filter.sharpness as u32, 3);
    w.put(header.loop_filter.mode_ref_delta_enabled as u32, 1);
    w.put(header.loop_filter.mode_ref_delta_update as u32, 1);
    for delta in header.loop_filter.ref_deltas {
        w.put(delta as u32 & 0x7f, 7);
    }
    for delta in header.loop_filter.mode_deltas {
        w.put(delta as u32 & 0x7f, 7);
    }
    w.put(header.quantization.base_q_idx as u32, 8);
    w.put(header.quantization.y_dc_delta_q as u32 & 0x1f, 5);
    w.put(header.quantization.uv_dc_delta_q as u32 & 0x1f, 5);
    w.put(header.quantization.uv_ac_delta_q as u32 & 0x1f, 5);
    w.put(header.quantization.lossless as u32, 1);
    for probability in header.segmentation.pred_probs {
        w.put(probability as u32, 8);
    }
    for probability in header.segmentation.tree_probs {
        w.put(probability as u32, 8);
    }
    w.put(header.segmentation.enabled as u32, 1);
    w.put(header.segmentation.update_map as u32, 1);
    w.put(header.segmentation.temporal_update as u32, 1);
    w.put(header.segmentation.abs_delta as u32, 1);
    let use_prev_frame_mvs = !header.error_resilient_mode
        && header.width == last.width
        && header.height == last.height
        && !last.intra_only
        && last.show_frame;
    w.put(use_prev_frame_mvs as u32, 1);
    for segment in 0..8 {
        for feature in 0..4 {
            w.put(
                ((header.segmentation.feature_mask[segment] >> feature) & 1) as u32,
                1,
            );
        }
    }
    for segment in 0..8 {
        w.put(
            header.segmentation.feature_data[segment][0] as u32 & 0x1ff,
            9,
        );
        w.put(
            header.segmentation.feature_data[segment][1] as u32 & 0x7f,
            7,
        );
        w.put(header.segmentation.feature_data[segment][2] as u32 & 0x3, 2);
    }
    w.put(header.first_partition_size as u32, 16);
    for reference in references {
        let (width, height) = reference
            .map(|(frame, _)| (frame.width as u32, frame.height as u32))
            .unwrap_or((0, 0));
        w.put(width, 16);
        w.put(height, 16);
    }
    for delta in last.last_mode_deltas {
        w.put(delta as u32 & 0x7f, 7);
    }
    for delta in last.last_ref_deltas {
        w.put(delta as u32 & 0x7f, 7);
    }
    w.put(last.segmentation_enable as u32, 1);
    w.put(last.show_frame as u32, 1);
    w.put(header.intra_only as u32, 1);
    w.put(
        (header.width == last.width && header.height == last.height) as u32,
        1,
    );
    w.put(last.color_space as u32 & 0x7, 3);
    w.put((!last.frame_type) as u32, 1);
    w.put(0, 1);
    w.put(1, 1);
    w.put(1, 1);
    w.put(1, 1);
    for scale in header.mvscale {
        w.put(scale[0] as u32, 16);
        w.put(scale[1] as u32, 16);
    }
    let tile_cols = 1u32 << header.log2_tile_cols;
    let tile_rows = 1u32 << header.log2_tile_rows;
    w.put(tile_cols, 7);
    w.put(tile_rows, 3);
    let ctu_cols = align(header.width as usize, 64) / 64;
    let ctu_rows = align(header.height as usize, 64) / 64;
    for index in 0..64u32 {
        let width = if index < tile_cols {
            tile_ctus(index, ctu_cols as u32, header.log2_tile_cols as u32)
        } else {
            0
        };
        w.put(width, 10);
    }
    for index in 0..4u32 {
        let height = if index < tile_rows {
            tile_ctus(index, ctu_rows as u32, header.log2_tile_rows as u32)
        } else {
            0
        };
        w.put(height, 10);
    }
    w.align(64);
    let bytes = w.bytes();
    assert!(
        bytes.len() <= GLOBAL_HEADER_SIZE_383,
        "VP9 VDPU383 global header overflow: {}",
        bytes.len()
    );
    let mut output = [0u8; GLOBAL_HEADER_SIZE_383];
    output[..bytes.len()].copy_from_slice(&bytes);
    output
}

fn tile_ctus(index: u32, total: u32, log2: u32) -> u32 {
    let start = (index * total) >> log2;
    let end = ((index + 1) * total) >> log2;
    end.min(total).saturating_sub(start.min(total))
}

fn configure_segments(r: &mut reg::Regs, last: &LastFrameInfo) {
    for segment in 0..8 {
        let mask = last.feature_mask[segment];
        let data = last.feature_data[segment];
        let mut word = 0u32;
        if segment == 0 {
            word |= last.abs_delta as u32;
        }
        word |= ((mask & 1 != 0) as u32) << 1;
        word |= ((data[0] as u32) & 0x1ff) << 2;
        word |= ((mask & 2 != 0) as u32) << 11;
        word |= ((data[1] as u32) & 0x7f) << 12;
        word |= ((mask & 4 != 0) as u32) << 19;
        word |= ((data[2] as u32) & 0x3) << 20;
        word |= ((mask & 8 != 0) as u32) << 22;
        r.w[67 + segment] = word;
    }
}

fn segment_map_len(width: u32, height: u32) -> usize {
    let ctu_cols = align(width as usize, 64) / 64;
    let ctu_rows = align(height as usize, 64) / 64;
    ctu_cols * ctu_rows * SEGMENT_MAP_BYTES_PER_CTU
}

fn configure_probability_control(
    r: &mut reg::Regs,
    header: &Header,
    compressed: &CompressedHeader,
    intra: bool,
    last_key_frame: bool,
) {
    r.w[103] |= 1 << 20; // prob_update_en
    r.w[103] |= (intra as u32) << 23;
    r.w[103] |= (1 << 25) | (1 << 26) | (1 << 27);
    if !intra {
        r.w[103] |= ((compressed.tx_mode == TxMode::Select) as u32) << 24;
        r.w[103] |= ((header.interp_filter == 4) as u32) << 28;
    }
    r.w[103] |= ((!header.error_resilient_mode && !header.frame_parallel_mode) as u32) << 21;
    r.w[103] |= (header.refresh_frame_context as u32) << 22;
    r.w[103] |= (header.allow_high_precision_mv as u32) << 29;
    r.w[103] |= (last_key_frame as u32) << 30;
}

#[allow(clippy::too_many_arguments)]
fn configure_addresses(
    r: &mut reg::Regs,
    probe: &DmaBuf,
    segment: &DmaBuf,
    context_prob: &DmaBuf,
    default_prob: &DmaBuf,
    rcb: &DmaBuf,
    stream: &DmaBuf,
    current: &Frame,
    references: &[Option<(&Frame, u32)>; 3],
    had_probability_context: bool,
    previous_colmv_fd: u32,
) {
    r.w[128] = stream.fd() as u32;
    r.w[129] = stream.fd() as u32;
    r.w[130] = current.video_fd();
    r.w[131] = current.colmv_fd();
    let rcb_fd = rcb.fd() as u32;
    r.w[133] = rcb_fd;
    r.w[134] = rcb_fd;
    r.w[135] = rcb_fd;
    r.w[136] = rcb_fd;
    r.w[137] = rcb.fd() as u32;
    r.w[138] = rcb_fd;
    r.w[139] = rcb_fd;
    r.w[140] = rcb_fd;
    r.w[141] = rcb_fd;
    r.w[142] = rcb_fd;
    r.w[160] = probe.fd() as u32;
    r.w[162] = if had_probability_context {
        context_prob.fd() as u32
    } else {
        default_prob.fd() as u32
    };
    r.w[167] = probe.fd() as u32;
    r.w[168] = segment.fd() as u32;
    r.w[169] = segment.fd() as u32;
    r.w[172] = context_prob.fd() as u32;
    for (index, reference) in references.iter().enumerate() {
        if let Some((frame, _)) = reference {
            r.w[164 + index] = frame.video_fd();
            r.w[181 + index] = frame.colmv_fd();
        } else {
            r.w[164 + index] = current.video_fd();
            r.w[181 + index] = current.colmv_fd();
        }
    }
    r.w[170] = previous_colmv_fd;
}

fn tx_mode_number(mode: TxMode) -> u8 {
    match mode {
        TxMode::Only4x4 => 0,
        TxMode::Only8x8 => 1,
        TxMode::Only16x16 => 2,
        TxMode::Only32x32 => 3,
        TxMode::Select => 4,
    }
}

fn compound_mode_number(mode: CompoundPredictionMode) -> u8 {
    match mode {
        CompoundPredictionMode::SingleReference => 0,
        CompoundPredictionMode::CompoundReference => 1,
        CompoundPredictionMode::Select => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::vp9::parser::{LoopFilter, Quantization, Segmentation};

    fn header(width: u32, height: u32, bit_depth: u8) -> Header {
        Header {
            profile: crate::codec::vp9::parser::Profile::Profile0,
            frame_kind: FrameKind::Key,
            show_frame: true,
            error_resilient_mode: false,
            intra_only: false,
            refresh_frame_flags: 0xff,
            width,
            height,
            render_width: width,
            render_height: height,
            bit_depth,
            ref_frame_idx: [0; 3],
            ref_frame_sign_bias: [false; 3],
            frame_to_show_map_idx: None,
            refresh_frame_context: true,
            reset_frame_context: 0,
            allow_high_precision_mv: false,
            interp_filter: 0,
            use_prev_in_find_mv_refs: false,
            frame_context_idx: 0,
            frame_parallel_mode: false,
            loop_filter: LoopFilter::default(),
            quantization: Quantization::default(),
            segmentation: Segmentation::default(),
            mvscale: [[16, 16]; 3],
            log2_tile_cols: 0,
            log2_tile_rows: 0,
            first_partition_size: 0,
            header_size: 0,
        }
    }

    #[test]
    fn frame_layout_matches_mpp_256_odd_alignment() {
        assert_eq!(align_256_odd(64), 256);
        assert_eq!(align_256_odd(256), 256);
        // MppBufSlots allocates stride * stride_y * 9 / 5 for decoded
        // uncompressed frames. The extra tail is required even though the
        // visible NV12 data only occupies the first 3 / 2 fraction.
        assert_eq!(frame_layout(64, 64, 8), (256, 64, 29491));
        assert_eq!(frame_layout(64, 64, 10), (256, 64, 29491));
        assert_eq!(frame_layout(320, 240, 8), (768, 240, 331776));
        assert_eq!(frame_layout(1920, 1080, 8), (2304, 1088, 4512153));
    }

    #[test]
    fn colmv_size_matches_vdpu34x_formula() {
        assert_eq!(colmv_size(64, 64), 1152);
    }

    #[test]
    fn segment_map_reset_only_covers_active_ctus() {
        assert_eq!(segment_map_len(1280, 720), 20 * 12 * 32);
        assert_eq!(segment_map_len(4096, 2304), 64 * 36 * 32);
        assert_eq!(segment_map_len(8192, 4608), MAX_SEGMAP_SIZE_ALIGNED);
    }

    #[test]
    fn rcb_offsets_use_fixed_layout() {
        let info = RcbInfo::compute(&header(256, 256, 8));
        let offsets: Vec<u32> = info.offsets().iter().map(|entry| entry.offset).collect();
        assert_eq!(
            offsets,
            [0, 5632, 7168, 7424, 8192, 9728, 11264, 14080, 14336, 15104]
        );
        assert_eq!(info.total(), 32256);
    }

    /// The reused-scratch offset path must produce exactly the entry list the
    /// original per-frame `Vec` build produced: the ten RCB offsets, the
    /// colmv-count slot, the two segmentation slots in their swap order, and
    /// no zero-offset entry (the kernel ABI treats an explicit zero as absent
    /// only when it is not sent at all).
    #[test]
    fn offset_list_matches_the_original_per_frame_build() {
        for (width, height, depth) in [(256, 256, 8u8), (1280, 720, 8), (3840, 2160, 10)] {
            let info = RcbInfo::compute(&header(width, height, depth));
            for (previous, current) in [
                (MAX_SEGMAP_SIZE_ALIGNED as u32, 0u32),
                (0u32, MAX_SEGMAP_SIZE_ALIGNED as u32),
            ] {
                // The original sequence, spelled out exactly as the pre-
                // optimisation decode() built it into a fresh Vec.
                let mut expected = info.offsets();
                expected.push(RegOffsetInfo {
                    reg_idx: 167,
                    offset: COUNT_OFFSET as u32,
                });
                expected.push(RegOffsetInfo {
                    reg_idx: 168,
                    offset: previous,
                });
                expected.push(RegOffsetInfo {
                    reg_idx: 169,
                    offset: current,
                });
                expected.retain(|entry| entry.offset != 0);

                let mut got = vec![RegOffsetInfo {
                    reg_idx: 0,
                    offset: u32::MAX,
                }]; // dirty scratch must be fully rebuilt
                build_offset_list(&info, previous, current, &mut got);
                assert_eq!(got, expected, "{width}x{height}@{depth}");
            }
        }
    }

    /// The RCB size list built into a reused scratch equals the one the
    /// original allocating `hardware_sizes()` produced, for a layout where
    /// the width-dependent zero-size regions must stay dropped.
    #[test]
    fn rcb_size_list_matches_the_original_per_frame_build() {
        for (width, height, depth) in [(256, 256, 8u8), (4096, 2304, 8), (8192, 4608, 10)] {
            let info = RcbInfo::compute(&header(width, height, depth));
            let expected = info.hardware_sizes();
            let mut got = vec![RcbConfig {
                reg_idx: 0,
                size: u32::MAX,
            }];
            // The write_* helpers append; the decode path clears first.
            got.clear();
            info.write_hardware_sizes(&mut got);
            assert_eq!(got.len(), expected.len(), "{width}x{height}@{depth}");
            for (got, want) in got.iter().zip(&expected) {
                assert_eq!((got.reg_idx, got.size), (want.reg_idx, want.size));
            }
        }
    }

    #[test]
    fn vdpu383_rcb_refines_the_320x240_layout() {
        let info = RcbInfo383::compute(&header(320, 240, 8));
        assert_eq!(info.total(), 180224);
        assert_eq!(info.offsets[0], 0);
        assert_eq!(info.sizes[2], 1536);
        assert_eq!(info.sizes[4], 1920);
        assert_eq!(info.sizes[6], 15040);
        assert_eq!(info.sizes[7], 15040);
        assert_eq!(info.sizes[1], 0);
        assert_eq!(info.sizes[3], 0);
        assert_eq!(info.sizes[9], 0);
    }

    #[test]
    fn vdpu383_tile_widths_are_ctu_counts() {
        assert_eq!(tile_ctus(0, 5, 0), 5);
        assert_eq!(tile_ctus(0, 10, 1), 5);
        assert_eq!(tile_ctus(1, 10, 1), 5);
        assert_eq!(tile_ctus(3, 10, 2), 3);
    }

    #[test]
    fn intra_only_resets_context_before_selecting_probability_source() {
        let mut h = header(64, 64, 8);
        h.frame_kind = FrameKind::IntraOnly;
        h.intra_only = true;
        h.refresh_frame_context = false;
        let mut valid = [true; PROBABILITY_CONTEXTS];
        let mut pocs = [1, 2, 3, 4];

        let had_context = update_probability_context(&h, 7, 0, &mut valid, &mut pocs);

        assert!(!had_context);
        assert!(!valid[0]);
        assert_eq!(pocs[0], 7);
    }

    #[test]
    fn hidden_frame_keeps_the_last_shown_colmv_base() {
        let mut previous_colmv = None;
        let mut last_colmv = None;
        let selected =
            [(1, false), (2, true), (3, false), (4, true)].map(|(current, last_show_frame)| {
                select_previous_colmv(
                    current,
                    false,
                    false,
                    last_show_frame,
                    &mut previous_colmv,
                    &mut last_colmv,
                )
            });

        assert_eq!(selected, [1, 1, 1, 3]);
    }

    #[test]
    fn common_controls_match_vdpu34x_mpp() {
        let mut registers = reg::Regs::new();
        configure_common(&mut registers);

        assert_eq!(registers.w[11], 0x0100_0062);
        assert_eq!(registers.w[13] & 1, 1);
        assert_eq!(registers.w[26], 0x800f_ffef);
        assert_eq!(registers.w[32], 0x0003_ffff);
    }

    #[test]
    fn last_key_frame_flag_describes_the_previous_frame() {
        let mut h = header(64, 64, 8);
        h.frame_parallel_mode = true;
        let compressed = CompressedHeader {
            tx_mode: TxMode::Only4x4,
            allow_compound_prediction: false,
            compound_prediction_mode: CompoundPredictionMode::SingleReference,
            probabilities: Default::default(),
            updates: Default::default(),
        };
        let mut registers = reg::Regs::new();

        configure_probability_control(&mut registers, &h, &compressed, true, false);

        assert_eq!(registers.w[103], 0x0ed0_0000);
    }
}
