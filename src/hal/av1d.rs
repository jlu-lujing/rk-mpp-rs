//! AV1D_VPU hardware data-plane layout for RK3588.
//!
//! This module mirrors the deterministic buffers built by
//! `hal/vpu/av1d/hal_av1d_vdpu.c`. Keeping these layouts separate from the
//! kernel submission path lets tests compare them byte-for-byte with the C
//! HAL before any hardware state is involved.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::codec::av1::cdf::{Av1Cdf, CDF_SIZE, MV_CDF_OFFSET};
use crate::codec::av1::picparams::Av1PicParams;
use crate::service::dma_heap::DmaBuf;
use crate::service::{Batch, Client, MppService, RegOffsetInfo};

mod film_grain;
#[path = "av1_film_grain_table.rs"]
mod film_grain_table;
pub use film_grain::{build_film_grain_memory, FilmGrainError, FILM_GRAIN_MEMORY_SIZE};

pub const AV1_TILE_INFO_SIZE: usize = 128 * 16;
pub const GLOBAL_MODEL_COUNT: usize = 7;
pub const GLOBAL_MODEL_BYTES: usize = 6 * 4 + 4 * 2;
pub const GLOBAL_MODEL_SIZE: usize = GLOBAL_MODEL_COUNT * GLOBAL_MODEL_BYTES;
pub const GLOBAL_MODEL_BUFFER_SIZE: usize = align(GLOBAL_MODEL_SIZE, 2048);
pub const PROBABILITY_BUFFER_SIZE: usize = align(CDF_SIZE, 2048);
pub const FILM_GRAIN_BUFFER_SIZE: usize = align(film_grain::FILM_GRAIN_MEMORY_SIZE, 2048);
pub const TILE_BUFFER_SIZE: usize = 4096;
pub const AV1_REGISTER_COUNT: usize = 512;
pub const AV1_MAX_PIC_BUFFERS: usize = 24;

const AV1_REF_SCALE_SHIFT: u32 = 14;
const MAX_FRAME_DISTANCE: i32 = 31;
static NEXT_TRACE_JOB: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLayout {
    pub width: usize,
    pub height: usize,
    pub luma_size: usize,
    pub chroma_size: usize,
    pub direction_motion_size: usize,
    pub tile_output_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterLayout {
    pub db_data_col: usize,
    pub db_ctrl_col: usize,
    pub cdef_col: usize,
    pub superres_col: usize,
    pub lr_col: usize,
    pub offsets: [usize; 5],
    pub allocation_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegOffset {
    pub reg_idx: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Av1DmaBuffers {
    /// Internal tile-decoder output. Luma, chroma, motion vectors and segment
    /// state all live in this allocation at different offsets.
    pub current_output: u32,
    /// Mapping from the parser's logical picture slot to its DMA fd.
    pub references: [Option<u32>; AV1_MAX_PIC_BUFFERS],
    pub stream: u32,
    pub stream_len: u32,
    pub global_model: u32,
    pub tile_info: u32,
    pub probability: u32,
    pub probability_out: u32,
    pub filter: u32,
    pub tile: u32,
    /// Scaling LUTs followed by the cropped luma and interleaved chroma noise.
    pub film_grain: u32,
    /// The post-processor's final NV12 output.
    pub display_output: u32,
    pub display_hor_stride: u32,
    pub display_ver_stride: u32,
    /// `true` requests MPP's 8-bit YUV420SP post-processor output. A 10-bit
    /// source needs `false`, which makes the post-processor emit packed 10-bit
    /// YUV420SP using the source bit depth.
    pub display_yuv420sp: bool,
}

impl Default for Av1DmaBuffers {
    fn default() -> Self {
        Self {
            current_output: 0,
            references: [None; AV1_MAX_PIC_BUFFERS],
            stream: 0,
            stream_len: 0,
            global_model: 0,
            tile_info: 0,
            probability: 0,
            probability_out: 0,
            filter: 0,
            tile: 0,
            film_grain: 0,
            display_output: 0,
            display_hor_stride: 0,
            display_ver_stride: 0,
            display_yuv420sp: true,
        }
    }
}

/// One AV1 DPB picture.
///
/// `tile_output` is the internal surface referenced by inter frames.
/// `display` is the post-processed surface returned to callers: NV12 for
/// 8-bit and Rockchip packed 4:2:0 for 10-bit.
pub struct Frame {
    pub tile_output: DmaBuf,
    pub display: DmaBuf,
    pub width: usize,
    pub height: usize,
    pub display_hor_stride: usize,
    pub display_ver_stride: usize,
    pub texture_index: u8,
    pub max_width: usize,
    pub max_height: usize,
    pub bit_depth: u32,
}

impl Frame {
    pub(crate) fn alloc(
        max_width: usize,
        max_height: usize,
        width: usize,
        height: usize,
        bit_depth: u32,
        texture_index: u8,
    ) -> io::Result<Self> {
        let layout = frame_layout(max_width, max_height, bit_depth);
        let display = display_layout(width, height, bit_depth)?;
        Ok(Self {
            tile_output: DmaBuf::alloc("system", layout.tile_output_size, false)?,
            display: DmaBuf::alloc("system", display.allocation_size, false)?,
            width,
            height,
            display_hor_stride: display.hor_stride,
            display_ver_stride: display.ver_stride,
            texture_index,
            max_width,
            max_height,
            bit_depth,
        })
    }

    pub fn y_plane(&self) -> &[u8] {
        let size = self.display_hor_stride * self.display_ver_stride;
        &self.display.as_slice()[..size]
    }

    pub fn uv_plane(&self) -> &[u8] {
        let y_size = self.display_hor_stride * self.display_ver_stride;
        &self.display.as_slice()[y_size..y_size + y_size / 2]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayLayout {
    pub hor_stride: usize,
    pub ver_stride: usize,
    pub allocation_size: usize,
}

/// MPP's display allocation for AV1's YUV420 semi-planar output. The 10-bit
/// format is Rockchip's packed layout: samples are contiguous bit fields with
/// no P010 alignment gap, and each row is padded to eight bytes.
pub fn display_layout(width: usize, height: usize, bit_depth: u32) -> io::Result<DisplayLayout> {
    if !matches!(bit_depth, 8 | 10) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("AV1 MPP output does not support {bit_depth}-bit depth"),
        ));
    }
    let hor_stride = align(width * bit_depth as usize, 8 * 8) / 8;
    let ver_stride = align(height, 8);
    let allocation_size = hor_stride
        .checked_mul(ver_stride)
        .and_then(|luma| luma.checked_mul(3))
        .map(|bytes| bytes / 2)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "AV1 display overflow"))?;
    Ok(DisplayLayout {
        hor_stride,
        ver_stride,
        allocation_size,
    })
}

/// Persistent AV1D_VPU resources and one-job submission path.
pub struct Av1d {
    svc: MppService,
    probability_in: DmaBuf,
    probability_out: DmaBuf,
    global_model: DmaBuf,
    tile_info: DmaBuf,
    film_grain: DmaBuf,
    tile: DmaBuf,
    filter: Option<DmaBuf>,
    pub last_status: (u32, u32, u32),
    pub last_hardware_error: bool,
    pub last_probability: Vec<u8>,
    /// Reused across jobs: the stream packet grows monotonically, and the
    /// offset list and request batch are rebuilt in full every job.
    stream: Option<DmaBuf>,
    offsets_scratch: Vec<RegOffsetInfo>,
    batch: Batch,
}

impl Av1d {
    pub fn open() -> io::Result<Self> {
        Ok(Self {
            svc: MppService::open(Client::Av1Dec as u32)?,
            probability_in: DmaBuf::alloc("system", PROBABILITY_BUFFER_SIZE, false)?,
            probability_out: DmaBuf::alloc("system", PROBABILITY_BUFFER_SIZE, false)?,
            global_model: DmaBuf::alloc("system", GLOBAL_MODEL_BUFFER_SIZE, false)?,
            tile_info: DmaBuf::alloc("system", AV1_TILE_INFO_SIZE, false)?,
            film_grain: DmaBuf::alloc("system", FILM_GRAIN_BUFFER_SIZE, false)?,
            tile: DmaBuf::alloc("system", TILE_BUFFER_SIZE, false)?,
            filter: None,
            last_status: (0, 0, 0),
            last_hardware_error: false,
            last_probability: Vec::new(),
            stream: None,
            offsets_scratch: Vec::with_capacity(24),
            batch: Batch::new(),
        })
    }

    /// Write the software-owned tables and submit one AV1 hardware job.
    ///
    /// The caller keeps every referenced `Frame` alive until this returns.
    #[allow(clippy::too_many_arguments)]
    pub fn decode(
        &mut self,
        params: &Av1PicParams,
        cdf: &Av1Cdf,
        stream_data: &[u8],
        current: &Frame,
        references: &[Option<&Frame>; AV1_MAX_PIC_BUFFERS],
        state: &mut Av1RegisterState,
    ) -> io::Result<()> {
        super::check_decode_geometry(current.width, current.height)?;
        let intra = matches!(params.format.frame_type, 0 | 2);
        let probability = build_probability_buffer(cdf, intra);
        let trace_directory = std::env::var_os("RK_MPP_AV1_PROB_DUMP_DIR");
        let trace_job = trace_directory
            .as_ref()
            .map(|_| NEXT_TRACE_JOB.fetch_add(1, Ordering::Relaxed));
        if let (Some(directory), Some(job)) = (&trace_directory, trace_job) {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory)?;
            std::fs::write(directory.join(format!("prob_in_{job:03}.bin")), probability)?;
        }
        write_dma(&mut self.probability_in, &probability)?;
        write_dma(&mut self.global_model, &build_global_model(params))?;
        write_dma(&mut self.tile_info, &build_tile_info(params, true))?;
        if params.film_grain.apply_grain != 0 {
            let film_grain = film_grain::build_film_grain_memory(params).map_err(|error| {
                io::Error::new(io::ErrorKind::InvalidInput, format!("{error:?}"))
            })?;
            write_dma(&mut self.film_grain, &film_grain)?;
            if let (Some(directory), Some(job)) = (&trace_directory, trace_job) {
                let directory = std::path::PathBuf::from(directory);
                std::fs::write(
                    directory.join(format!("film_grain_{job:03}.bin")),
                    film_grain,
                )?;
            }
        }

        let filter = filter_layout(usize::from(params.height), params.tile_cols_log2.min(6));
        if self
            .filter
            .as_ref()
            .is_none_or(|buffer| buffer.len() < filter.allocation_size)
        {
            self.filter = Some(DmaBuf::alloc("system", filter.allocation_size, false)?);
        }

        // Reuse and grow the stream packet across jobs the way the VP8
        // decoder's load_stream does; zeroing the tail keeps the bytes the
        // hardware sees identical to a fresh zeroed allocation.
        let padded_stream_len = align(stream_data.len(), 128).max(128);
        if self
            .stream
            .as_ref()
            .is_none_or(|buffer| buffer.len() < padded_stream_len)
        {
            self.stream = None;
            self.stream = Some(DmaBuf::alloc("system", padded_stream_len, false)?);
        }
        let stream = self
            .stream
            .as_mut()
            .expect("stream buffer was just created");
        stream.sync_write_begin()?;
        {
            let window = stream.as_mut_slice();
            window[..stream_data.len()].copy_from_slice(stream_data);
            window[stream_data.len()..padded_stream_len].fill(0);
        }
        stream.sync_write_end()?;

        let mut reference_fds = [None; AV1_MAX_PIC_BUFFERS];
        for (destination, frame) in reference_fds.iter_mut().zip(references) {
            *destination = frame.map(|frame| frame.tile_output.fd() as u32);
        }
        let buffers = Av1DmaBuffers {
            current_output: current.tile_output.fd() as u32,
            references: reference_fds,
            stream: stream.fd() as u32,
            stream_len: u32::try_from(stream_data.len())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "AV1 stream too large"))?,
            global_model: self.global_model.fd() as u32,
            tile_info: self.tile_info.fd() as u32,
            probability: self.probability_in.fd() as u32,
            probability_out: self.probability_out.fd() as u32,
            filter: self.filter.as_ref().unwrap().fd() as u32,
            tile: self.tile.fd() as u32,
            film_grain: self.film_grain.fd() as u32,
            display_output: current.display.fd() as u32,
            display_hor_stride: current.display_hor_stride as u32,
            display_ver_stride: current.display_ver_stride as u32,
            display_yuv420sp: params.bitdepth <= 8,
        };
        let registers = Av1Registers::build(params, &buffers, state)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, format!("{error:?}")))?;
        if let (Some(directory), Some(job)) = (&trace_directory, trace_job) {
            use std::fmt::Write as _;

            let directory = std::path::PathBuf::from(directory);
            let mut text = String::with_capacity(registers.words().len() * 25);
            for (index, word) in registers.words().iter().enumerate() {
                writeln!(text, "reg[{index:3}] = {word:08x}").unwrap();
            }
            std::fs::write(directory.join(format!("reg_in_{job:03}.txt")), text)?;
        }
        self.offsets_scratch.clear();
        write_armed_offsets(&registers, &mut self.offsets_scratch);
        let mut readback = [0u32; AV1_REGISTER_COUNT];
        self.batch.clear();
        self.batch
            .reg_write(registers.words(), 0)
            .reg_read(&mut readback, 0)
            .reg_offsets(&self.offsets_scratch);
        self.svc.submit_batch(&mut self.batch)?;
        self.svc.poll_hw_finish()?;

        self.last_status = (readback[1], readback[261], readback[262]);
        self.last_hardware_error = readback[1] & (1 << 12) == 0;
        self.probability_out.sync_read_begin()?;
        self.last_probability.clear();
        self.last_probability
            .extend_from_slice(&self.probability_out.as_slice()[..CDF_SIZE]);
        if let (Some(directory), Some(job)) = (trace_directory, trace_job) {
            let directory = std::path::PathBuf::from(directory);
            std::fs::write(
                directory.join(format!("prob_out_{job:03}.bin")),
                &self.last_probability,
            )?;
        }
        self.probability_out.sync_read_end()?;
        Ok(())
    }
}

/// The address-offset entries one job arms: every nonzero declared offset,
/// in declaration order. Appended into caller-owned scratch; the entry list
/// is identical to the original per-frame `filter().map().collect()` pass.
pub fn write_armed_offsets(registers: &Av1Registers, out: &mut Vec<RegOffsetInfo>) {
    out.extend(
        registers
            .address_offsets()
            .iter()
            .filter(|offset| offset.offset != 0)
            .map(|offset| RegOffsetInfo {
                reg_idx: offset.reg_idx,
                offset: offset.offset,
            }),
    );
}

fn write_dma(buffer: &mut DmaBuf, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() > buffer.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "AV1 DMA buffer is too small",
        ));
    }
    buffer.sync_write_begin()?;
    buffer.as_mut_slice()[..bytes.len()].copy_from_slice(bytes);
    buffer.sync_write_end()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Av1RegisterState {
    pub previous_output_slot: i8,
    pub sign_bias: [u8; 8],
    pub resolution_change: bool,
}

impl Default for Av1RegisterState {
    fn default() -> Self {
        Self {
            previous_output_slot: -1,
            sign_bias: [0; 8],
            resolution_change: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Av1RegError {
    EmptyPicture,
    NoTiles,
    InvalidReferenceSlot(i8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Av1Registers {
    words: [u32; AV1_REGISTER_COUNT],
    offsets: Vec<RegOffset>,
}

impl Default for Av1Registers {
    fn default() -> Self {
        Self {
            words: [0; AV1_REGISTER_COUNT],
            offsets: Vec::new(),
        }
    }
}

const fn align(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

/// MPP's tile output allocation from `vdpu_av1d_setup_tile_bufs`.
pub fn frame_layout(max_width: usize, max_height: usize, bit_depth: u32) -> FrameLayout {
    let width = align(max_width * bit_depth as usize, 16 * 8) / 8;
    let height = align(max_height, 16);
    let luma_size = width * height;
    let chroma_size = luma_size / 2;
    let num_sbs = (align(max_width, 64) / 64 + 1) * (align(max_height, 64) / 64 + 1);
    let direction_motion_size = align(num_sbs * 24 * 128 / 8, 16) * 2;
    let tile_output_size = luma_size + chroma_size + direction_motion_size + 512;

    FrameLayout {
        width,
        height,
        luma_size,
        chroma_size,
        direction_motion_size,
        tile_output_size,
    }
}

/// Filter-column scratch allocation from `vdpu_av1d_filtermem_alloc`.
pub fn filter_layout(height: usize, tile_cols_log2: u8) -> FilterLayout {
    let pic_height = align(height, 64);
    let height_in_sb = pic_height / 64;
    let stripe_num = (pic_height + 8).div_ceil(64);
    let tile_cols = 1usize << tile_cols_log2;

    let db_data_col = align(pic_height * 12 * 10 / 8, 128);
    let db_ctrl_col = align(pic_height * 2 * 16 / 4, 128);
    let cdef_col = align(height_in_sb * 44 * 10 * 16 / 8, 128);
    let superres_col = align(height_in_sb * (3040 + 1280), 128);
    let lr_col = align(stripe_num * 1536 * 10 / 8, 128);

    let mut offsets = [0usize; 5];
    let mut size = 0usize;
    for (offset, region) in
        offsets
            .iter_mut()
            .zip([db_data_col, db_ctrl_col, cdef_col, superres_col, lr_col])
    {
        *offset = size;
        size += region * tile_cols;
    }

    FilterLayout {
        db_data_col,
        db_ctrl_col,
        cdef_col,
        superres_col,
        lr_col,
        offsets,
        allocation_size: align(size, 4096),
    }
}

/// C's global-model DMA record: `wmmat[0,1,3,2,4,5]` followed by four i16
/// shear values. The C source writes the fields into a packed 32-byte record.
pub fn build_global_model(params: &Av1PicParams) -> [u8; GLOBAL_MODEL_BUFFER_SIZE] {
    let mut output = [0u8; GLOBAL_MODEL_BUFFER_SIZE];
    for reference in 0..GLOBAL_MODEL_COUNT {
        let base = reference * GLOBAL_MODEL_BYTES;
        let source = params.frame_refs[reference];
        for (word, matrix_index) in [0usize, 1, 3, 2, 4, 5].into_iter().enumerate() {
            output[base + word * 4..base + word * 4 + 4]
                .copy_from_slice(&source.wmmat[matrix_index].to_le_bytes());
        }
        let shear_base = base + 24;
        for (index, value) in [source.alpha, source.beta, source.gamma, source.delta]
            .into_iter()
            .enumerate()
        {
            output[shear_base + index * 2..shear_base + index * 2 + 2]
                .copy_from_slice(&value.to_le_bytes());
        }
    }
    output
}

/// C's tile-info buffer. With `tile_transpose` enabled (the AV1 HAL default),
/// records are stored column-major while tile stream offsets retain AV1's
/// row-major tile id.
pub fn build_tile_info(params: &Av1PicParams, transpose: bool) -> [u8; AV1_TILE_INFO_SIZE] {
    let mut output = [0u8; AV1_TILE_INFO_SIZE];
    let cols = params.tiles.cols as usize;
    let rows = params.tiles.rows as usize;

    for x in 0..cols {
        for y in 0..rows {
            let tile_id = y * cols + x;
            let output_index = if transpose { x * rows + y } else { tile_id };
            if output_index >= 128 {
                break;
            }
            let base = output_index * 16;
            let width = u32::from(params.tiles.widths[x]);
            let height = u32::from(params.tiles.heights[y]);
            let start = params.tiles.tile_offset_start[tile_id];
            let end = params.tiles.tile_offset_end[tile_id];
            output[base..base + 4].copy_from_slice(&width.to_le_bytes());
            output[base + 4..base + 8].copy_from_slice(&height.to_le_bytes());
            output[base + 8..base + 12].copy_from_slice(&start.to_le_bytes());
            output[base + 12..base + 16].copy_from_slice(&end.to_le_bytes());
        }
    }
    output
}

/// Materialize the exact `AV1CDFs` image consumed by the hardware. Intra
/// pictures replace only the 138-byte `mv_cdf` region with the intrabc CDF.
pub fn build_probability_buffer(cdf: &Av1Cdf, intra: bool) -> [u8; CDF_SIZE] {
    let mut output = [0u8; CDF_SIZE];
    output.copy_from_slice(cdf.regular.as_bytes());
    if intra {
        output[MV_CDF_OFFSET..MV_CDF_OFFSET + cdf.intrabc_mv.as_bytes().len()]
            .copy_from_slice(cdf.intrabc_mv.as_bytes());
    }
    output
}

impl Av1Registers {
    /// Build the RK3588 AV1D_VPU register packet. The bit positions and
    /// several deliberate C quirks follow `hal_av1d_vdpu_reg.h` and
    /// `vdpu_av1d_gen_regs`; notably, a reference slot of zero is valid for
    /// bookkeeping but is programmed as slot zero by the C HAL.
    pub fn build(
        params: &Av1PicParams,
        buffers: &Av1DmaBuffers,
        state: &mut Av1RegisterState,
    ) -> Result<Self, Av1RegError> {
        if params.width == 0 || params.height == 0 {
            return Err(Av1RegError::EmptyPicture);
        }
        if params.tiles.cols == 0 || params.tiles.rows == 0 {
            return Err(Av1RegError::NoTiles);
        }
        let max_width = usize::from(params.max_width.max(params.width));
        let max_height = usize::from(params.max_height.max(params.height));
        let layout = frame_layout(max_width, max_height, u32::from(params.bitdepth));
        let filter = filter_layout(usize::from(params.height), params.tile_cols_log2.min(6));
        let mv_offset = (layout.luma_size + layout.chroma_size + 64) as u32;
        let stream_offset = params.frame_tag_size & !0xf;

        let mut regs = Self::default();
        regs.set_common_controls(params);
        regs.set_picture_dimensions(params);
        regs.set_segmentation(params);
        regs.set_loop_filter(params);
        regs.set_cdef(params);
        regs.set_loop_restoration(params);
        regs.set_film_grain(params, buffers);
        regs.set_probability_and_tile_buffers(buffers);
        regs.set_superres(params, buffers);
        regs.set_tile_registers(params, buffers);
        regs.set_fixed_engine_controls();
        regs.set_output_buffers(
            params,
            buffers,
            layout.luma_size as u32,
            mv_offset,
            stream_offset,
        );
        regs.set_post_processor(params, buffers);

        if params.format.frame_type != 0 || bit(params.coding, 25) != 0 {
            regs.set_reference_frames(params, buffers, state, layout.luma_size as u32, mv_offset)?;
        }

        // C performs these offset calls in several helpers. The driver treats
        // them as an unordered register+offset map.
        regs.offsets.extend([
            RegOffset {
                reg_idx: 85,
                offset: filter.offsets[2] as u32,
            },
            RegOffset {
                reg_idx: 89,
                offset: filter.offsets[3] as u32,
            },
            RegOffset {
                reg_idx: 91,
                offset: filter.offsets[4] as u32,
            },
            RegOffset {
                reg_idx: 183,
                offset: filter.offsets[1] as u32,
            },
        ]);
        regs.offsets.sort_unstable_by_key(|offset| offset.reg_idx);
        Ok(regs)
    }

    pub fn words(&self) -> &[u32; AV1_REGISTER_COUNT] {
        &self.words
    }

    pub fn address_offsets(&self) -> &[RegOffset] {
        &self.offsets
    }

    pub fn field(&self, register: usize, shift: u32, width: u32) -> u32 {
        let mask = field_mask(width);
        (self.words[register] >> shift) & mask
    }

    fn set_field(&mut self, register: usize, shift: u32, width: u32, value: u32) {
        let mask = field_mask(width);
        let shifted = mask << shift;
        self.words[register] = (self.words[register] & !shifted) | ((value & mask) << shift);
    }

    fn set_common_controls(&mut self, params: &Av1PicParams) {
        self.set_field(1, 0, 1, 1); // sw_dec_e
        self.set_field(1, 5, 1, 0); // sw_dec_abort_e
        self.set_field(1, 7, 1, 0); // sw_dec_tile_int_e
        self.set_field(2, 10, 1, 1); // sw_dec_clk_gate_e

        self.set_field(3, 8, 1, 1); // sw_dec_out_ec_bypass
        self.set_field(3, 12, 1, 1); // sw_write_mvs_e
        self.set_field(3, 16, 1, 0); // sw_dec_out_ec_byte_word
        self.set_field(3, 26, 1, bit(params.coding, 32)); // sw_skip_mode
        self.set_field(3, 27, 5, 17); // sw_dec_mode = AV1

        self.set_field(
            5,
            0,
            1,
            u32::from(
                (params.format.frame_type != 0 && params.format.frame_type != 2)
                    || bit(params.coding, 25) != 0,
            ),
        ); // sw_ref_scaling_enable
        self.set_field(5, 1, 1, u32::from(params.loop_filter.filter_level[0] > 32));
        self.set_field(5, 2, 1, bit(params.coding, 41)); // error resilient
        self.set_field(5, 3, 1, bit(params.coding, 20)); // force integer MV
        self.set_field(5, 4, 1, bit(params.coding, 25)); // allow intrabc
        self.set_field(5, 5, 1, bit(params.coding, 19)); // screen tools
        self.set_field(5, 6, 1, bit(params.coding, 33)); // reduced tx set
        self.set_field(5, 7, 1, bit(params.coding, 17)); // dual filter
        self.set_field(5, 8, 1, bit(params.coding, 18)); // joint compound
        self.set_field(5, 9, 1, bit(params.coding, 28)); // filter intra
        self.set_field(5, 10, 1, bit(params.coding, 13)); // intra edge filter
        self.set_field(5, 11, 1, bit(params.coding, 38)); // temporal MVP
        self.set_field(5, 12, 1, bit(params.coding, 14)); // inter-intra
        self.set_field(5, 13, 1, bit(params.coding, 15)); // masked compound
        self.set_field(5, 14, 1, u32::from(cdef_enabled(params)));
        self.set_field(5, 15, 1, bit(params.coding, 27)); // switchable motion mode
        self.set_field(5, 16, 1, u32::from(params.format.show_frame != 0));
        self.set_field(5, 18, 1, bit(params.coding, 16)); // warped motion
        self.set_field(5, 19, 1, bit(params.coding, 30)); // disable CDF update
        self.set_field(
            5,
            21,
            1,
            u32::from(params.loop_filter.delta_lf_present != 0),
        );
        self.set_field(5, 22, 1, u32::from(params.loop_filter.delta_lf_multi != 0));
        self.set_field(5, 23, 2, u32::from(params.loop_filter.delta_lf_res));
        self.set_field(5, 25, 7, (params.frame_tag_size & 0xf) * 8);

        self.set_field(7, 30, 1, u32::from(params.format.mono_chrome != 0));
        self.set_field(
            7,
            12,
            1,
            u32::from(params.film_grain.clip_to_restricted_range != 0),
        );
        self.set_field(7, 1, 2, u32::from(params.quantization.delta_q_res));
        self.set_field(7, 0, 1, u32::from(params.quantization.delta_q_present != 0));

        self.set_field(
            8,
            16,
            1,
            u32::from(matches!(params.format.frame_type, 0 | 2)),
        );
        self.set_field(8, 8, 8, u32::from(params.quantization.base_qindex));
        self.set_field(8, 6, 2, u32::from(params.bitdepth.saturating_sub(8)));
        self.set_field(8, 4, 2, u32::from(params.bitdepth.saturating_sub(8)));

        self.set_field(11, 8, 3, u32::from(params.interp_filter));
        self.set_field(11, 7, 1, bit(params.coding, 26)); // high precision MV
        self.set_field(11, 4, 2, if bit(params.coding, 31) != 0 { 2 } else { 0 });
        let tx_mode = (params.coding >> 35) & 0x7;
        self.set_field(
            11,
            27,
            3,
            if tx_mode != 0 {
                (tx_mode + 2) as u32
            } else {
                0
            },
        );

        self.set_field(12, 10, 3, if bit(params.coding, 12) != 0 { 7 } else { 6 });
        self.set_field(12, 13, 3, 3);
        self.set_field(12, 16, 3, 0);

        self.set_field(13, 25, 7, params.quantization.y_dc_delta_q as i8 as u32);
        self.set_field(13, 18, 7, params.quantization.u_dc_delta_q as i8 as u32);
        self.set_field(13, 11, 7, params.quantization.u_ac_delta_q as i8 as u32);
        self.set_field(13, 9, 1, u32::from(params.coded_lossless != 0));
        self.set_field(47, 27, 4, u32::from(params.quantization.qm_y));
        self.set_field(48, 27, 4, u32::from(params.quantization.qm_u));
        self.set_field(49, 14, 4, u32::from(params.quantization.qm_v));
        self.set_field(28, 25, 7, params.quantization.v_dc_delta_q as i8 as u32);
        self.set_field(29, 25, 7, params.quantization.v_ac_delta_q as i8 as u32);
        self.set_field(
            31,
            26,
            4,
            u32::from(if params.skip_ref0 != 0 {
                params.skip_ref0
            } else {
                1
            }),
        );
        self.set_field(
            32,
            26,
            4,
            u32::from(if params.skip_ref1 != 0 {
                params.skip_ref1
            } else {
                1
            }),
        );
    }

    fn set_picture_dimensions(&mut self, params: &Av1PicParams) {
        self.set_field(4, 6, 13, (align(usize::from(params.height), 8) >> 3) as u32);
        self.set_field(4, 19, 13, (align(usize::from(params.width), 8) >> 3) as u32);
        self.set_field(
            12,
            26,
            3,
            (align(usize::from(params.height), 8) - usize::from(params.height)) as u32,
        );
        self.set_field(
            12,
            29,
            3,
            (align(usize::from(params.width), 8) - usize::from(params.width)) as u32,
        );
        self.set_field(8, 17, 15, params.upscaled_width);
        self.set_field(9, 14, 3, u32::from(params.superres_denom));
    }

    fn set_segmentation(&mut self, params: &Av1PicParams) {
        self.set_field(
            13,
            2,
            1,
            u32::from(params.segmentation.temporal_update != 0),
        );
        self.set_field(13, 1, 1, u32::from(params.segmentation.update_map != 0));
        self.set_field(13, 0, 1, u32::from(params.segmentation.enabled != 0));

        for plane in 0..4 {
            let value = match plane {
                0 => params.loop_filter.filter_level[0],
                1 => params.loop_filter.filter_level[1],
                2 => params.loop_filter.filter_level_u,
                _ => params.loop_filter.filter_level_v,
            };
            self.set_field(14 + plane, 26, 6, u32::from(value));
        }

        let mut values = [[0i32; 8]; 8];
        let mut quant_sign = 0u32;
        let mut last_active = 0u32;
        let mut preskip = 0u32;
        if params.segmentation.enabled != 0 {
            for segment in 0..8 {
                let mask = params.segmentation.feature_mask[segment];
                let source = params.segmentation.feature_data[segment];
                if mask & (1 << 0) != 0 {
                    values[segment][0] = source[0].abs().clamp(0, 255);
                    if source[0] < 0 {
                        quant_sign |= 1 << segment;
                    }
                }
                for feature in 1..5 {
                    if mask & (1 << feature) != 0 {
                        values[segment][feature] = source[feature].clamp(-63, 63);
                    }
                }
                if params.format.frame_type != 0 && mask & (1 << 5) != 0 {
                    values[segment][5] = source[5] + 1;
                }
                if mask & (1 << 6) != 0 {
                    values[segment][6] = 1;
                }
                if mask & (1 << 7) != 0 {
                    values[segment][7] = 1;
                }
                for feature in 0..8 {
                    if mask & (1 << feature) != 0 {
                        if feature >= 5 {
                            preskip = 1;
                        }
                        last_active = last_active.max(segment as u32);
                    }
                }
            }
        }

        self.set_field(9, 17, 3, last_active);
        self.set_field(5, 21, 1, preskip);
        self.set_field(12, 2, 8, quant_sign);
        for segment in 0..8 {
            let value_register = if segment < 6 {
                14 + segment
            } else {
                31 + segment - 6
            };
            self.set_field(value_register, 0, 8, values[segment][0] as u32);
            self.set_field(value_register, 14, 1, values[segment][6] as u32);
            self.set_field(value_register, 15, 4, values[segment][5] as u32);
            self.set_field(value_register, 19, 7, values[segment][1] as u32);

            let delta_register = 20 + segment;
            self.set_field(delta_register, 0, 7, values[segment][2] as u32);
            self.set_field(delta_register, 7, 7, values[segment][3] as u32);
            self.set_field(delta_register, 14, 7, values[segment][4] as u32);
            self.set_field(delta_register, 21, 1, values[segment][7] as u32);
        }
    }

    fn set_loop_filter(&mut self, params: &Av1PicParams) {
        self.set_field(
            3,
            14,
            1,
            u32::from(
                params.loop_filter.filter_level[0] == 0 && params.loop_filter.filter_level[1] == 0,
            ),
        );
        self.set_field(30, 28, 3, u32::from(params.loop_filter.sharpness_level));

        let deltas = if params.loop_filter.mode_ref_delta_enabled != 0 {
            params.loop_filter.ref_deltas
        } else {
            [0; 8]
        };
        let modes = if params.loop_filter.mode_ref_delta_enabled != 0 {
            params.loop_filter.mode_deltas
        } else {
            [0; 2]
        };
        self.set_field(59, 21, 7, deltas[0] as u32);
        self.set_field(59, 14, 7, deltas[1] as u32);
        self.set_field(59, 7, 7, deltas[2] as u32);
        self.set_field(59, 0, 7, deltas[3] as u32);
        self.set_field(30, 7, 7, deltas[4] as u32);
        self.set_field(30, 0, 7, deltas[5] as u32);
        self.set_field(49, 7, 7, deltas[6] as u32);
        self.set_field(49, 0, 7, deltas[7] as u32);
        self.set_field(30, 21, 7, modes[0] as u32);
        self.set_field(30, 14, 7, modes[1] as u32);
    }

    fn set_cdef(&mut self, params: &Av1PicParams) {
        self.set_field(7, 5, 2, u32::from(params.cdef.bits));
        self.set_field(7, 3, 2, u32::from(params.cdef.damping));
        let mut luma_primary = 0u32;
        let mut luma_secondary = 0u32;
        let mut chroma_primary = 0u32;
        let mut chroma_secondary = 0u32;
        for index in 0..8 {
            if index == (1usize << params.cdef.bits) {
                break;
            }
            luma_primary |= u32::from(params.cdef.y_strengths[index].primary) << (index * 4);
            luma_secondary |= u32::from(params.cdef.y_strengths[index].secondary) << (index * 2);
            chroma_primary |= u32::from(params.cdef.uv_strengths[index].primary) << (index * 4);
            chroma_secondary |= u32::from(params.cdef.uv_strengths[index].secondary) << (index * 2);
        }
        self.words[263] = luma_primary;
        self.set_field(53, 16, 16, luma_secondary);
        self.words[264] = chroma_primary;
        self.set_field(53, 0, 16, chroma_secondary);
    }

    fn set_loop_restoration(&mut self, params: &Av1PicParams) {
        let mut lr_type = 0u32;
        let mut unit_size = 0u32;
        for index in 0..3 {
            lr_type |= u32::from(params.loop_filter.frame_restoration_type[index]) << (index * 2);
            unit_size |=
                u32::from(params.loop_filter.log2_restoration_unit_size[index]) << (index * 2);
        }
        self.set_field(18, 26, 6, lr_type);
        self.set_field(19, 26, 6, unit_size);
    }

    fn set_film_grain(&mut self, params: &Av1PicParams, buffers: &Av1DmaBuffers) {
        let grain = &params.film_grain;
        self.set_field(7, 7, 1, u32::from(grain.apply_grain != 0));
        if grain.apply_grain == 0 {
            return;
        }

        self.set_field(7, 8, 1, u32::from(grain.num_y_points > 0));
        self.set_field(7, 9, 1, u32::from(grain.num_cb_points > 0));
        self.set_field(7, 10, 1, u32::from(grain.num_cr_points > 0));
        self.set_field(7, 11, 1, u32::from(grain.overlap_flag != 0));
        self.set_field(7, 12, 1, u32::from(grain.clip_to_restricted_range != 0));
        self.set_field(7, 13, 1, u32::from(grain.chroma_scaling_from_luma != 0));
        self.set_field(7, 14, 16, u32::from(grain.grain_seed));
        self.set_field(8, 0, 4, u32::from(grain.scaling_shift_minus8) + 8);

        if grain.chroma_scaling_from_luma != 0 {
            self.set_field(28, 0, 9, 0);
            self.set_field(28, 9, 8, 64);
            self.set_field(28, 17, 8, 0);
            self.set_field(29, 0, 9, 0);
            self.set_field(29, 9, 8, 64);
            self.set_field(29, 17, 8, 0);
        } else {
            self.set_field(28, 0, 9, (i32::from(grain.cb_offset) - 256) as u32);
            self.set_field(28, 9, 8, (i32::from(grain.cb_luma_mult) - 128) as u32);
            self.set_field(28, 17, 8, (i32::from(grain.cb_mult) - 128) as u32);
            self.set_field(29, 0, 9, (i32::from(grain.cr_offset) - 256) as u32);
            self.set_field(29, 9, 8, (i32::from(grain.cr_luma_mult) - 128) as u32);
            self.set_field(29, 17, 8, (i32::from(grain.cr_mult) - 128) as u32);
        }
        self.words[94] = 0;
        self.words[95] = buffers.film_grain;
    }

    fn set_probability_and_tile_buffers(&mut self, buffers: &Av1DmaBuffers) {
        self.words[171] = buffers.probability_out;
        self.words[173] = buffers.probability;
        self.words[83] = buffers.global_model;
        self.words[167] = buffers.tile_info;
    }

    fn set_superres(&mut self, params: &Av1PicParams, buffers: &Av1DmaBuffers) {
        const RS_SCALE_SUBPEL_BITS: i64 = 14;
        const RS_SCALE_EXTRA_BITS: i64 = RS_SCALE_SUBPEL_BITS - 6;
        let denominator = if bit(params.coding, 34) != 0 {
            u32::from(params.superres_denom) + 9
        } else {
            8
        };
        let upscaled = params.upscaled_width.max(1) as i64;
        let mut luma_step = RS_SCALE_SUBPEL_BITS as u32;
        let mut chroma_step = RS_SCALE_SUBPEL_BITS as u32;
        let mut luma_inverse = RS_SCALE_SUBPEL_BITS as u32;
        let mut chroma_inverse = RS_SCALE_SUBPEL_BITS as u32;
        let mut initial_luma = 0u32;
        let mut initial_chroma = 0u32;
        let mut scaled = 0u32;

        if denominator > 8 {
            let mut downscaled =
                (upscaled * 8 + i64::from(denominator / 2)) / i64::from(denominator);
            downscaled = downscaled.max(16.min(upscaled));
            if downscaled != upscaled {
                scaled = 1;
                let down_chroma = (downscaled + 1) >> 1;
                let up_chroma = (upscaled + 1) >> 1;
                let luma = ((downscaled << RS_SCALE_SUBPEL_BITS) + upscaled / 2) / upscaled;
                let chroma = ((down_chroma << RS_SCALE_SUBPEL_BITS) + up_chroma / 2) / up_chroma;
                let luma_error = upscaled * luma - (downscaled << RS_SCALE_SUBPEL_BITS);
                let chroma_error = up_chroma * chroma - (down_chroma << RS_SCALE_SUBPEL_BITS);
                let extra = 1i64 << (RS_SCALE_EXTRA_BITS - 1);
                luma_step = luma as u32;
                chroma_step = chroma as u32;
                luma_inverse =
                    (((upscaled << RS_SCALE_SUBPEL_BITS) + downscaled / 2) / downscaled) as u32;
                chroma_inverse =
                    (((up_chroma << RS_SCALE_SUBPEL_BITS) + down_chroma / 2) / down_chroma) as u32;
                initial_luma = (((-((upscaled - downscaled) << (RS_SCALE_SUBPEL_BITS - 1))
                    + upscaled / 2)
                    / upscaled
                    + extra
                    - luma_error / 2)
                    & ((1 << RS_SCALE_SUBPEL_BITS) - 1)) as u32;
                initial_chroma = (((-((up_chroma - down_chroma) << (RS_SCALE_SUBPEL_BITS - 1))
                    + up_chroma / 2)
                    / up_chroma
                    + extra
                    - chroma_error / 2)
                    & ((1 << RS_SCALE_SUBPEL_BITS) - 1)) as u32;
            }
        }

        self.set_field(51, 0, 14, chroma_step);
        self.set_field(51, 14, 14, luma_step);
        self.set_field(298, 0, 16, chroma_inverse);
        self.set_field(298, 16, 16, luma_inverse);
        self.set_field(52, 0, 14, initial_chroma);
        self.set_field(52, 14, 14, initial_luma);
        self.set_field(5, 17, 1, scaled);
        self.words[89] = buffers.filter;
    }

    fn set_tile_registers(&mut self, params: &Av1PicParams, buffers: &Av1DmaBuffers) {
        let cols = usize::from(params.tiles.cols);
        let rows = usize::from(params.tiles.rows);
        let context_update = usize::from(params.tiles.context_update_id);
        let y = context_update / cols;
        let x = context_update % cols;
        self.set_field(11, 11, 1, u32::from(x == 0));
        self.set_field(
            10,
            1,
            1,
            u32::from(params.tiles.cols > 1 || params.tiles.rows > 1),
        );
        self.set_field(10, 17, 7, u32::from(params.tiles.cols));
        self.set_field(10, 10, 7, u32::from(params.tiles.rows));
        self.set_field(9, 20, 12, (x * rows + y) as u32);
        self.set_field(10, 0, 1, 1); // tile transpose
        self.set_field(11, 30, 2, u32::from(params.tiles.tile_sz_mag));
        self.words[167] = buffers.tile_info;
    }

    fn set_fixed_engine_controls(&mut self) {
        self.set_field(55, 31, 1, 0);
        self.set_field(55, 0, 16, 8);
        self.set_field(58, 8, 3, 2);
        self.set_field(58, 0, 8, 16);
        self.set_field(266, 30, 1, 0);
        self.set_field(265, 18, 10, 64);
        self.set_field(265, 8, 10, 64);
        self.set_field(318, 0, 31, 0x0fff_ffff);
        self.set_field(318, 31, 1, 1);
        self.set_field(319, 0, 31, 0x0fff_ffff);
        self.set_field(319, 31, 1, 1);
        self.set_field(314, 0, 16, 64);
    }

    fn set_output_buffers(
        &mut self,
        params: &Av1PicParams,
        buffers: &Av1DmaBuffers,
        luma_size: u32,
        mv_offset: u32,
        stream_offset: u32,
    ) {
        self.words[65] = buffers.current_output;
        self.words[99] = buffers.current_output;
        self.words[133] = buffers.current_output;
        self.offsets.extend([
            RegOffset {
                reg_idx: 99,
                offset: luma_size,
            },
            RegOffset {
                reg_idx: 133,
                offset: mv_offset,
            },
        ]);

        let stream_len = align(buffers.stream_len as usize, 128) as u32;
        self.set_field(258, 0, 32, stream_len);
        self.set_field(6, 0, 32, stream_len);
        self.words[259] = 0;
        self.words[169] = buffers.stream;
        self.offsets.push(RegOffset {
            reg_idx: 169,
            offset: stream_offset,
        });

        self.words[175] = buffers.tile;
        self.words[177] = buffers.tile;
        self.words[179] = buffers.filter;
        self.words[183] = buffers.filter;

        // `picture_dimensions` runs before `superres`; reapply the denominator
        // here so future reordering cannot silently change the register image.
        self.set_field(9, 14, 3, u32::from(params.superres_denom));
    }

    fn set_post_processor(&mut self, params: &Av1PicParams, buffers: &Av1DmaBuffers) {
        self.set_field(320, 0, 1, 1);
        self.set_field(322, 27, 5, 0);
        self.set_field(394, 16, 8, 1);
        self.set_field(394, 24, 8, 1);
        self.set_field(331, 0, 16, u32::from(params.height / 2));
        self.set_field(331, 16, 16, u32::from(params.width / 2));
        self.set_field(332, 0, 16, u32::from(params.height));
        self.set_field(332, 16, 16, u32::from(params.width));
        self.set_field(329, 0, 16, buffers.display_hor_stride);
        self.set_field(329, 16, 16, buffers.display_hor_stride);
        self.set_field(322, 18, 5, u32::from(buffers.display_yuv420sp) * 3);
        self.words[326] = buffers.display_output;
        self.words[328] = buffers.display_output;
        self.offsets.push(RegOffset {
            reg_idx: 328,
            offset: buffers
                .display_hor_stride
                .saturating_mul(buffers.display_ver_stride),
        });
    }

    fn set_reference_frames(
        &mut self,
        params: &Av1PicParams,
        buffers: &Av1DmaBuffers,
        state: &mut Av1RegisterState,
        luma_size: u32,
        mv_offset: u32,
    ) -> Result<(), Av1RegError> {
        let intrabc = bit(params.coding, 25) != 0;
        let frame_is_intra = matches!(params.format.frame_type, 0 | 2);
        if frame_is_intra || params.order_hint_bits == 0 {
            state.sign_bias.fill(0);
        } else {
            for logical in 0..7 {
                let reference = params.frame_refs[logical];
                if reference.index >= 0 {
                    state.sign_bias[logical + 1] = u8::from(
                        relative_distance(
                            params,
                            reference.order_hint as i32,
                            i32::from(params.order_hint),
                        ) > 0,
                    );
                }
            }
        }

        let mut referenced = [false; AV1_MAX_PIC_BUFFERS];
        if !intrabc {
            for logical in 0..7 {
                let index = params.frame_refs[logical].index;
                if index >= 0 {
                    let slot = usize::try_from(index)
                        .ok()
                        .filter(|slot| *slot < AV1_MAX_PIC_BUFFERS)
                        .ok_or(Av1RegError::InvalidReferenceSlot(index))?;
                    referenced[slot] = true;
                }
            }
            self.set_field(
                4,
                0,
                4,
                referenced.iter().filter(|present| **present).count() as u32,
            );
        } else {
            self.set_field(4, 0, 4, 1);
        }

        const WIDTH_REGS: [usize; 7] = [33, 34, 35, 43, 44, 45, 46];
        const SCALE_REGS: [usize; 7] = [36, 37, 38, 39, 40, 41, 42];
        const LUMA_REGS: [usize; 7] = [67, 69, 71, 73, 75, 77, 79];
        const CHROMA_REGS: [usize; 7] = [101, 103, 105, 107, 109, 111, 113];
        const MOTION_REGS: [usize; 7] = [135, 137, 139, 141, 143, 145, 147];
        let mut previous_valid = false;

        for logical in 0..7 {
            let reference = params.frame_refs[logical];
            let (width, height, index) = if intrabc {
                (
                    u32::from(params.width),
                    u32::from(params.height),
                    i32::from(params.curr_pic_texture_index),
                )
            } else {
                (
                    reference.width,
                    reference.height,
                    if reference.index > 0 {
                        i32::from(reference.index)
                    } else {
                        0
                    },
                )
            };
            let slot = usize::try_from(index)
                .ok()
                .filter(|slot| *slot < AV1_MAX_PIC_BUFFERS)
                .ok_or(Av1RegError::InvalidReferenceSlot(index as i8))?;
            previous_valid |= index == i32::from(state.previous_output_slot);
            let fd = buffers.references[slot].unwrap_or(0);

            self.set_field(WIDTH_REGS[logical], 0, 16, height);
            self.set_field(WIDTH_REGS[logical], 16, 16, width);
            self.set_field(
                SCALE_REGS[logical],
                16,
                16,
                scaled_dimension(width, u32::from(params.width)),
            );
            self.set_field(
                SCALE_REGS[logical],
                0,
                16,
                scaled_dimension(height, u32::from(params.height)),
            );
            self.words[LUMA_REGS[logical]] = fd;
            self.words[CHROMA_REGS[logical]] = fd;
            self.words[MOTION_REGS[logical]] = fd;
            self.offsets.push(RegOffset {
                reg_idx: CHROMA_REGS[logical] as u32,
                offset: luma_size,
            });
            self.offsets.push(RegOffset {
                reg_idx: MOTION_REGS[logical] as u32,
                offset: mv_offset,
            });
        }

        self.set_field(59, 28, 1, u32::from(state.sign_bias[1]));
        self.set_field(59, 29, 1, u32::from(state.sign_bias[2]));
        self.set_field(59, 30, 1, u32::from(state.sign_bias[3]));
        self.set_field(59, 31, 1, u32::from(state.sign_bias[4]));
        self.set_field(9, 2, 1, u32::from(state.sign_bias[5]));
        self.set_field(9, 3, 1, u32::from(state.sign_bias[6]));
        self.set_field(9, 4, 1, u32::from(state.sign_bias[7]));

        if intrabc {
            state.previous_output_slot = params.curr_pic_texture_index as i8;
        } else if !previous_valid {
            state.previous_output_slot = params.frame_refs[0].index;
        }

        self.set_reference_global_motion(params);
        self.set_reference_temporal_state(params, buffers, mv_offset)?;
        if params.primary_ref_frame < 7 {
            let primary = params.frame_refs[usize::from(params.primary_ref_frame)];
            let current_mi_width = u32::from(params.width).div_ceil(8);
            let current_mi_height = u32::from(params.height).div_ceil(8);
            state.resolution_change = current_mi_width != primary.width.div_ceil(8)
                || current_mi_height != primary.height.div_ceil(8);
        }
        Ok(())
    }

    fn set_reference_global_motion(&mut self, params: &Av1PicParams) {
        const REGS: [usize; 7] = [184, 185, 186, 187, 188, 257, 262];
        for logical in 0..7 {
            self.set_field(
                REGS[logical],
                27,
                2,
                u32::from(params.frame_refs[logical].wmtype),
            );
        }
    }

    fn set_reference_temporal_state(
        &mut self,
        params: &Av1PicParams,
        buffers: &Av1DmaBuffers,
        mv_offset: u32,
    ) -> Result<(), Av1RegError> {
        let current_hint = i32::from(params.order_hint);
        let current_mi_width = u32::from(params.width).div_ceil(8);
        let current_mi_height = u32::from(params.height).div_ceil(8);
        let mut selected_refs = [0usize; 3];
        let mut mf_types = [0i32; 3];
        let mut selected = 0usize;
        let mut remaining = 2i32;

        let last = params.frame_refs[0];
        let has_usable = |reference: &crate::codec::av1::picparams::PicReference| {
            reference.width.div_ceil(8) == current_mi_width
                && reference.height.div_ceil(8) == current_mi_height
                && reference.intra_only == 0
                && reference.is_intra_frame == 0
        };
        fn select_motion_field(
            mf_types: &mut [i32; 3],
            selected_refs: &mut [usize; 3],
            selected: &mut usize,
            logical: usize,
            reference_type: i32,
        ) {
            if *selected < mf_types.len() {
                mf_types[*selected] = reference_type;
                selected_refs[*selected] = logical;
                *selected += 1;
            }
        }
        if last.index >= 0 {
            let overlay = last.alt_frame_offset as i32 == params.frame_refs[3].order_hint as i32;
            if !overlay && has_usable(&last) {
                select_motion_field(&mut mf_types, &mut selected_refs, &mut selected, 0, 1);
            }
            remaining -= 1;
        }

        for (logical, reference_type) in [(4, 5), (5, 6)] {
            let reference = params.frame_refs[logical];
            let relative = relative_distance(params, reference.order_hint as i32, current_hint);
            if relative > 0 && reference.index >= 0 && has_usable(&reference) && selected < 3 {
                select_motion_field(
                    &mut mf_types,
                    &mut selected_refs,
                    &mut selected,
                    logical,
                    reference_type,
                );
                remaining -= 1;
            }
        }
        let alt = params.frame_refs[6];
        if remaining >= 0
            && alt.index >= 0
            && has_usable(&alt)
            && relative_distance(params, alt.order_hint as i32, current_hint) > 0
            && selected < 3
        {
            select_motion_field(&mut mf_types, &mut selected_refs, &mut selected, 6, 7);
            remaining -= 1;
        }
        let last2 = params.frame_refs[1];
        if remaining >= 0 && last2.index >= 0 && has_usable(&last2) && selected < 3 {
            select_motion_field(&mut mf_types, &mut selected_refs, &mut selected, 1, 2);
        }

        let mut current_offsets = [0i32; 7];
        let mut reverse_offsets = [0i32; 7];
        for logical in 0..7 {
            let reference = params.frame_refs[logical];
            if reference.index >= 0 {
                current_offsets[logical] =
                    relative_distance(params, current_hint, reference.order_hint as i32);
                reverse_offsets[logical] =
                    relative_distance(params, reference.order_hint as i32, current_hint);
            }
        }

        if std::env::var_os("RK_MPP_AV1_TRACE").is_some() {
            eprintln!(
                "av1-motion-field selected={selected} refs={selected_refs:?} types={mf_types:?} \
                 offsets={current_offsets:?}"
            );
            for logical in 0..7 {
                let reference = params.frame_refs[logical];
                eprintln!(
                    "av1-motion-ref logical={logical} index={} mi={}x{} intra_only={} \
                     intra={} order={} usable={} relative={}",
                    reference.index,
                    reference.width.div_ceil(8),
                    reference.height.div_ceil(8),
                    reference.intra_only,
                    reference.is_intra_frame,
                    reference.order_hint,
                    has_usable(&reference),
                    relative_distance(params, reference.order_hint as i32, current_hint),
                );
            }
        }

        self.set_field(11, 0, 1, 0);
        self.set_field(11, 1, 1, 0);
        self.set_field(11, 2, 1, 0);
        self.set_field(11, 3, 1, 0);
        if bit(params.coding, 38) != 0 {
            for index in 0..selected {
                let offset = current_offsets[mf_types[index] as usize - 1];
                if (-MAX_FRAME_DISTANCE..=MAX_FRAME_DISTANCE).contains(&offset) {
                    self.set_field(11, 3 - index as u32, 1, 1);
                    self.populate_motion_field_offsets(params, selected_refs[index], index);
                }
            }
        }

        if params.segmentation.enabled != 0 && params.primary_ref_frame < 7 {
            let reference = params.frame_refs[usize::from(params.primary_ref_frame)];
            if reference.index >= 0 {
                let slot = usize::try_from(reference.index)
                    .ok()
                    .filter(|slot| *slot < AV1_MAX_PIC_BUFFERS)
                    .ok_or(Av1RegError::InvalidReferenceSlot(reference.index))?;
                self.words[81] = buffers.references[slot].unwrap_or(0);
                self.offsets.push(RegOffset {
                    reg_idx: 81,
                    offset: mv_offset,
                });
                self.set_field(11, 0, 1, 1);
            }
        }

        const OFFSET_REGS: [usize; 7] = [184, 185, 186, 187, 188, 257, 262];
        for logical in 0..7 {
            self.set_field(OFFSET_REGS[logical], 9, 9, current_offsets[logical] as u32);
            self.set_field(OFFSET_REGS[logical], 0, 9, reverse_offsets[logical] as u32);
        }
        self.set_field(9, 5, 3, (mf_types[0] - 1) as u32);
        self.set_field(9, 8, 3, (mf_types[1] - 1) as u32);
        self.set_field(9, 11, 3, (mf_types[2] - 1) as u32);
        Ok(())
    }

    fn populate_motion_field_offsets(
        &mut self,
        params: &Av1PicParams,
        logical: usize,
        temporal_group: usize,
    ) {
        let reference = params.frame_refs[logical];
        let hint = reference.order_hint as i32;
        let offsets = [
            reference.lst_frame_offset,
            reference.lst2_frame_offset,
            reference.lst3_frame_offset,
            reference.gld_frame_offset,
            reference.bwd_frame_offset,
            reference.alt2_frame_offset,
            reference.alt_frame_offset,
        ]
        .map(|value| relative_distance(params, hint, value as i32));
        for (field, offset) in offsets.into_iter().enumerate() {
            let (register, shift) = match temporal_group {
                0 => ([20, 21, 22, 23, 24, 25, 26][field], 22),
                1 => match field {
                    0 => (27, 22),
                    1 => (47, 0),
                    2 => (47, 9),
                    3 => (47, 18),
                    4 => (48, 0),
                    5 => (48, 9),
                    _ => (48, 18),
                },
                _ => match field {
                    0 => (184, 18),
                    1 => (185, 18),
                    2 => (186, 18),
                    3 => (187, 18),
                    4 => (188, 18),
                    5 => (257, 18),
                    _ => (262, 18),
                },
            };
            self.set_field(register, shift, 9, offset as u32);
        }
    }
}

fn field_mask(width: u32) -> u32 {
    match width {
        0 => 0,
        32.. => u32::MAX,
        _ => (1u32 << width) - 1,
    }
}

fn bit(value: u64, index: u32) -> u32 {
    ((value >> index) & 1) as u32
}

fn scaled_dimension(value: u32, current: u32) -> u32 {
    if current == 0 {
        0
    } else {
        ((value << AV1_REF_SCALE_SHIFT) + current / 2) / current
    }
}

fn relative_distance(params: &Av1PicParams, a: i32, b: i32) -> i32 {
    if params.order_hint_bits == 0 {
        return 0;
    }
    let bits = i32::from(params.order_hint_bits) - 1;
    let mask = (1i32 << bits) - 1;
    let marker = 1i32 << bits;
    let difference = a - b;
    (difference & mask) - (difference & marker)
}

fn cdef_enabled(params: &Av1PicParams) -> bool {
    params.cdef.bits != 0
        || params.cdef.damping != 0
        || params.cdef.y_strengths[0].primary != 0
        || params.cdef.y_strengths[0].secondary != 0
        || params.cdef.uv_strengths[0].primary != 0
        || params.cdef.uv_strengths[0].secondary != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_are_sized_from_the_c_hal_constants() {
        assert_eq!(GLOBAL_MODEL_SIZE, 224);
        assert_eq!(GLOBAL_MODEL_BUFFER_SIZE, 2_048);
        assert_eq!(TILE_BUFFER_SIZE, 4_096);
    }

    #[test]
    fn display_layout_matches_mpp_packed_8_and_10_bit_output() {
        assert_eq!(
            display_layout(320, 240, 8).unwrap(),
            DisplayLayout {
                hor_stride: 320,
                ver_stride: 240,
                allocation_size: 115_200,
            }
        );
        assert_eq!(
            display_layout(320, 240, 10).unwrap(),
            DisplayLayout {
                hor_stride: 400,
                ver_stride: 240,
                allocation_size: 144_000,
            }
        );
        assert!(display_layout(320, 240, 12).is_err());
    }
}
