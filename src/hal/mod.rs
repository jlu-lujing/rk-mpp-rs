//! Hardware abstraction layers: one module per SoC codec block.
//!
//! `jpegd` drives `rockchip,rkv-jpeg-decoder-v1`. The video blocks of the
//! official MPP (vdpu1/vdpu2/rkvdec/av1d/venc/vepu) are the roadmap.

pub mod jpegd;
pub mod vdpu34x_regs;
