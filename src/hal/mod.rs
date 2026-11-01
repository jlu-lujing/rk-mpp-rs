//! Hardware abstraction layers: one module per SoC codec block.
//!
//! `jpegd` drives `rockchip,rkv-jpeg-decoder-v1`. The video blocks of the
//! official MPP (vdpu1/vdpu2/rkvdec/av1d/venc/vepu) are the roadmap.

pub mod av1d;
pub mod h263d;
pub mod h264d;
pub mod h264e;
mod h264e_vepu580_template;
pub mod hevc;
pub mod iep;
pub mod jpegd;
pub mod jpege;
pub mod jpege_tables;
pub mod m2vd;
pub mod mpg4d;
pub mod vdpu2;
pub mod vdpu34x_regs;
pub mod vepu580_regs;
pub mod vp8d;
pub mod vp9d;
