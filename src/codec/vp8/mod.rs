//! VP8 decode: the syntax of `mpp/codec/dec/vp8`, the reference-buffer
//! bookkeeping of the same files, and the [`decoder`] that drives the vdpu2
//! hardware ported in [`crate::hal::vp8d`].

pub mod decoder;
pub mod dpb;
pub mod parser;
pub mod tables;
