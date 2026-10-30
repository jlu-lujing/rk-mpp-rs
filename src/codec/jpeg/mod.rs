//! JPEG decode: parser, hardware table buffer and jpegd register layout.

pub mod parser;
pub mod regs;
pub mod tables;

pub use parser::{parse, Error as ParseError, JpegInfo};
pub use regs::Regs;
pub use tables::Tables;
