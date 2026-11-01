//! Codec (de)serialisation front-ends.
//!
//! Each format owns a parser that turns the elementary stream into the syntax
//! the hardware HAL wants. `Jpeg` is implemented; the video codecs of the
//! official MPP (H.264/H.265/VP8/VP9/AV1/MPEG2/MPEG4/AVS) are the roadmap.

pub mod av1;
pub mod h263;
pub mod h264;
pub mod hevc;
pub mod jpeg;
pub mod m2v;
pub mod mpg4;
pub mod vp8;
pub mod vp9;
pub mod yuv;

/// What a decoder produces for one packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Mpeg2,
    H263,
    Mpeg4,
    Avc,
    Mjpeg,
    Vp8,
    Vp9,
    Hevc,
    Av1,
    Avs,
    Avs2,
}

impl Codec {
    /// MPP's `MppCodingType` value.
    pub fn hw_value(self) -> u32 {
        match self {
            Codec::Mpeg2 => 2,
            Codec::H263 => 3,
            Codec::Mpeg4 => 4,
            Codec::Avc => 7,
            Codec::Mjpeg => 8,
            Codec::Vp8 => 9,
            Codec::Vp9 => 10,
            Codec::Hevc => 0x0100_000b,
            Codec::Av1 => 0x0100_000c,
            Codec::Avs => 0x0100_000d,
            Codec::Avs2 => 0x0100_000e,
        }
    }
}

/// Common interface every hardware decoder implements.
pub trait VideoDecoder {
    /// Decode one packet; `out` receives the letterboxed RGB888 frame.
    fn decode(&mut self, packet: &[u8], out: &mut [u8]) -> Result<(), String>;
    /// Source dimensions of the last decoded frame.
    fn dimensions(&self) -> (usize, usize);
    fn codec(&self) -> Codec;
}
