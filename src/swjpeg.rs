//! Software JPEG decode (pure Rust, `zune-jpeg`) with the same output contract
//! as the hardware decoder: a letterboxed RGB888 frame.
//!
//! Used to keep working when the VPU is busy (its queue is shared) and to test
//! whether spreading decode across CPU cores beats the single hardware block.

use crate::scale::{Geometry, Scaler};

pub struct SwJpeg {
    data: Vec<u8>,
    rgb: Vec<u8>,
    w: usize,
    h: usize,
    scaler: Scaler,
}

impl SwJpeg {
    pub fn open(path: &str, dst_w: usize, dst_h: usize) -> Result<SwJpeg, String> {
        let data = std::fs::read(path).map_err(|e| e.to_string())?;
        let mut s = SwJpeg {
            data,
            rgb: Vec::new(),
            w: 0,
            h: 0,
            scaler: Scaler::new(dst_w, dst_h),
        };
        // Decode once to learn the dimensions and size the buffers.
        let (w, h, px) = s.decode()?;
        s.w = w;
        s.h = h;
        s.rgb = px;
        Ok(s)
    }

    pub fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    fn decode(&self) -> Result<(usize, usize, Vec<u8>), String> {
        let mut dec = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(&self.data[..]));
        let px = dec.decode().map_err(|e| format!("zune-jpeg: {e:?}"))?;
        let info = dec.info().ok_or("missing image info")?;
        let (w, h) = (info.width as usize, info.height as usize);
        let out = match dec.output_colorspace() {
            Some(zune_core::colorspace::ColorSpace::Luma) => {
                // Expand grayscale to RGB.
                let mut rgb = Vec::with_capacity(w * h * 3);
                for v in px {
                    rgb.extend_from_slice(&[v, v, v]);
                }
                rgb
            }
            _ => px,
        };
        if out.len() < w * h * 3 {
            return Err(format!("short decode: {} < {}", out.len(), w * h * 3));
        }
        Ok((w, h, out))
    }

    /// Decode and letterbox into `dst`.
    pub fn frame(&mut self, dst: &mut [u8]) -> Result<Geometry, String> {
        let (w, h, px) = self.decode()?;
        self.w = w;
        self.h = h;
        self.rgb = px;
        Ok(self.scaler.letterbox(&self.rgb, w, h, dst))
    }
}
