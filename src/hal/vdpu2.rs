//! Shared VDPU2 ("legacy VPU") plumbing: register word packing, the frame
//! surface layout, the interrupt bits and the one-job submission path.
//!
//! Every legacy codec the block runs - MPEG-2, MPEG-4, H.263, VP8 - is
//! programmed through the same 159-word register window on client 1, so the
//! per-codec HALs only differ in how they fill words 50 to 158.

use std::io;

use crate::service::dma_heap::DmaBuf;
use crate::service::{Batch, MppService, RegOffsetInfo};

/// `M2VD_VDPU2_REG_NUM`, shared by every vdpu2 legacy codec.
pub const REG_NUM: usize = 159;

/// Word indices the decode path touches. The names follow MPEG-2, which is the
/// codec whose HAL is the clearest to read; MPEG-4 and H.263 reuse the same
/// words under different names.
pub const REG_FILTER: usize = 50;
pub const REG_STREAM_LEN: usize = 51;
pub const REG_START_MB: usize = 52;
pub const REG_DEC_MODE: usize = 53;
pub const REG_ENDIAN: usize = 54;
pub const REG_IRQ: usize = 55;
pub const REG_BUS: usize = 56;
pub const REG_PIC_CTRL: usize = 57;
pub const REG_QP_TABLE: usize = 61;
pub const REG_DIRECT_MV: usize = 62;
pub const REG_CUR_PIC: usize = 63;
pub const REG_VLC_BASE: usize = 64;
pub const REG_PIC_SIZE: usize = 120;
pub const REG_PIC_EXT: usize = 122;
pub const REG_REF0: usize = 131;
pub const REG_REF2: usize = 134;
pub const REG_REF3: usize = 135;
pub const REG_MV_ACC: usize = 136;
pub const REG_REF1: usize = 148;

/// `sw55` interrupt bits. The C HAL treats `dec_error_int | dec_buffer_int`
/// as a failed job.
pub const IRQ_RDY: u32 = 1 << 4;
pub const IRQ_BUS: u32 = 1 << 5;
pub const IRQ_BUFFER: u32 = 1 << 6;
pub const IRQ_ASO: u32 = 1 << 8;
pub const IRQ_SLICE: u32 = 1 << 9;
pub const IRQ_PIC_INF: u32 = 1 << 10;
pub const IRQ_ERROR: u32 = 1 << 12;
pub const IRQ_TIMEOUT: u32 = 1 << 13;
pub const IRQ_FAILURE: u32 = IRQ_ERROR | IRQ_BUFFER;

/// `DEC_BUS_BURST_LENGTH_16`
pub const BUS_BURST_16: u32 = 16;

/// Accumulate one register word the way a C bitfield struct does: each declared
/// field lands above the previous one, starting from bit 0.
#[derive(Default, Clone, Copy)]
pub struct Word {
    value: u32,
    width: u32,
}

impl Word {
    pub fn field(self, value: u32, bits: u32) -> Self {
        assert!(bits < 32, "register field wider than a word");
        assert!(
            self.width + bits <= 32,
            "register field overflows its word at bit {}",
            self.width
        );
        let mask = (1u32 << bits) - 1;
        Word {
            value: self.value | ((value & mask) << self.width),
            width: self.width + bits,
        }
    }

    pub fn finish(self) -> u32 {
        assert_eq!(self.width, 32, "register word packs {} bits", self.width);
        self.value
    }
}

/// One VDPU2 picture surface: NV12 with 16-byte aligned rows.
pub struct Frame {
    pub texture: DmaBuf,
    pub width: usize,
    pub height: usize,
    pub hor_stride: usize,
    pub ver_stride: usize,
}

/// Rows are aligned to 16 pixels like the C decoder's frame slots; the chroma
/// plane starts one luma stride times one vertical stride into the buffer.
pub fn frame_layout(width: usize, height: usize) -> (usize, usize, usize) {
    let hor_stride = width.div_ceil(16) * 16;
    let ver_stride = height.div_ceil(16) * 16;
    // Two spare chroma rows: the C allocator asks for 1.2x the frame and a
    // field-coded pair can touch slightly past the visible chroma plane.
    let size = hor_stride * ver_stride * 3 / 2 + hor_stride * 16;
    (hor_stride, ver_stride, size)
}

impl Frame {
    pub fn alloc(width: usize, height: usize) -> io::Result<Self> {
        let (hor_stride, ver_stride, size) = frame_layout(width, height);
        Ok(Self {
            texture: DmaBuf::alloc("system", size, false)?,
            width,
            height,
            hor_stride,
            ver_stride,
        })
    }

    pub fn fd(&self) -> u32 {
        self.texture.fd() as u32
    }

    pub fn matches(&self, width: usize, height: usize) -> bool {
        self.width == width && self.height == height
    }

    pub fn y_plane(&self) -> &[u8] {
        let size = self.hor_stride * self.ver_stride;
        &self.texture.as_slice()[..size]
    }

    pub fn uv_plane(&self) -> &[u8] {
        let size = self.hor_stride * self.ver_stride;
        &self.texture.as_slice()[size..size + size / 2]
    }
}

/// Write the register file, arm the address offsets, send and poll. Returns the
/// `sw55` image that was read back.
pub fn run_job(
    svc: &mut MppService,
    registers: &[u32; REG_NUM],
    offsets: &[RegOffsetInfo],
) -> io::Result<u32> {
    let mut readback = [0u32; REG_NUM];
    let mut batch = Batch::new();
    batch
        .reg_write(registers, 0)
        .reg_read(&mut readback, 0)
        .reg_offsets(offsets);
    svc.submit_batch(&mut batch)?;
    svc.poll_hw_finish()?;
    Ok(readback[REG_IRQ])
}

/// Append a register image to `path` in the text the C HAL debug dumps use, so
/// a Rust and a C run diff without reformatting either one.
pub fn dump_registers_to(path: &str, label: &str, registers: &[u32; REG_NUM]) {
    use std::fmt::Write as _;
    use std::io::Write;

    let mut text = String::with_capacity(REG_NUM * 24);
    let _ = writeln!(text, "{label}");
    for index in 50..REG_NUM {
        let _ = writeln!(text, "reg[{}] = {:#010x}", index, registers[index]);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(text.as_bytes());
    }
}
