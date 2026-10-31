//! RGB888 bilinear resize + letterbox into a fixed destination.
//!
//! Replaces the RGA blit we used from the C implementation. The mapping is
//! computed once per resolution (x) and per row (y); the pixel loop is integer
//! only, so it vectorises well and needs no driver round-trip.

#[derive(Clone, Copy, Default)]
struct XMap {
    x0: u32,
    x1: u32,
    wx: u16,
}

pub struct Scaler {
    dw: usize,
    dh: usize,
    xmap: Vec<XMap>,
    map_sw: usize,
    map_nw: usize,
    map_scale: f32,
    map_wx_zero: bool,
}

pub struct Geometry {
    pub pad_left: i32,
    pub pad_top: i32,
    pub scale: f32,
    pub new_w: usize,
    pub new_h: usize,
}

impl Scaler {
    pub fn new(dw: usize, dh: usize) -> Self {
        Scaler {
            dw,
            dh,
            xmap: Vec::new(),
            map_sw: 0,
            map_nw: 0,
            map_scale: 0.0,
            map_wx_zero: false,
        }
    }

    fn geometry(&self, sw: usize, sh: usize) -> Geometry {
        let mut s = self.dw as f32 / sw as f32;
        let sy = self.dh as f32 / sh as f32;
        if sy < s {
            s = sy;
        }
        let mut nw = (sw as f32 * s + 0.5) as usize;
        let mut nh = (sh as f32 * s + 0.5) as usize;
        if nw > self.dw {
            nw = self.dw;
        }
        if nh > self.dh {
            nh = self.dh;
        }
        Geometry {
            pad_left: ((self.dw - nw) / 2) as i32,
            pad_top: ((self.dh - nh) / 2) as i32,
            scale: s,
            new_w: nw,
            new_h: nh,
        }
    }

    fn build_map(&mut self, sw: usize, g: &Geometry) {
        if self.map_sw == sw && self.map_nw == g.new_w && self.map_scale == g.scale {
            return;
        }
        self.xmap.clear();
        self.xmap.reserve(g.new_w);
        let inv = 1.0f32 / g.scale;
        for x in 0..g.new_w {
            let mut fx = (x as f32 + 0.5) * inv - 0.5;
            if fx < 0.0 {
                fx = 0.0;
            }
            let max = (sw - 1) as f32;
            if fx > max {
                fx = max;
            }
            let x0 = fx as u32;
            let x1 = if x0 as usize + 1 < sw { x0 + 1 } else { x0 };
            self.xmap.push(XMap {
                x0,
                x1,
                wx: ((fx - x0 as f32) * 256.0) as u16,
            });
        }
        self.map_sw = sw;
        self.map_nw = g.new_w;
        self.map_scale = g.scale;
        self.map_wx_zero = self.xmap.iter().all(|m| m.wx == 0);
    }

    /// Resize `src` (sw x sh, RGB888) into `dst` (dw x dh, RGB888), letterboxed
    /// and padded with 114.
    pub fn letterbox(&mut self, src: &[u8], sw: usize, sh: usize, dst: &mut [u8]) -> Geometry {
        self.letterbox_strided(src, sw * 3, sw, sh, dst)
    }

    /// Same, for a source whose rows are `src_stride` bytes apart (the decoder
    /// writes RGB with a stride aligned to 16 pixels).
    pub fn letterbox_strided(
        &mut self,
        src: &[u8],
        src_stride: usize,
        sw: usize,
        sh: usize,
        dst: &mut [u8],
    ) -> Geometry {
        assert!(src.len() >= src_stride * (sh - 1) + sw * 3);
        assert_eq!(dst.len(), self.dw * self.dh * 3);
        let g = self.geometry(sw, sh);
        self.build_map(sw, &g);
        // When the scaled image covers the whole destination every byte is
        // written below, so the letterbox fill would be wasted work.
        let covers_all =
            g.pad_left == 0 && g.pad_top == 0 && g.new_w == self.dw && g.new_h == self.dh;
        if !covers_all {
            dst.fill(114);
        }

        let inv = 1.0f32 / g.scale;
        let dw3 = self.dw * 3;
        let xmap = &self.xmap;
        for y in 0..g.new_h {
            let mut fy = (y as f32 + 0.5) * inv - 0.5;
            if fy < 0.0 {
                fy = 0.0;
            }
            let max = (sh - 1) as f32;
            if fy > max {
                fy = max;
            }
            let y0 = fy as usize;
            let y1 = if y0 + 1 < sh { y0 + 1 } else { y0 };
            let wy = ((fy - y0 as f32) * 256.0) as u32;
            let iwy = 256 - wy;
            let r0 = &src[y0 * src_stride..y0 * src_stride + sw * 3];
            let r1 = &src[y1 * src_stride..y1 * src_stride + sw * 3];
            let out_row =
                &mut dst[((g.pad_top as usize + y) * self.dw + g.pad_left as usize) * 3..];
            let out = &mut out_row[..g.new_w * 3];

            /* Integer-aligned cases (e.g. 1920x1080 -> 640 at exactly 1/3)
             * need no interpolation at all; a 1:1 mapping is a plain copy. */
            if wy == 0 && self.map_wx_zero {
                let mut contiguous = true;
                for (x, m) in xmap.iter().enumerate() {
                    if m.x0 as usize != x {
                        contiguous = false;
                        break;
                    }
                }
                if contiguous && xmap.len() == sw {
                    out.copy_from_slice(&r0[..sw * 3]);
                } else {
                    for (x, m) in xmap.iter().enumerate() {
                        let a = (m.x0 as usize) * 3;
                        out[x * 3..x * 3 + 3].copy_from_slice(&r0[a..a + 3]);
                    }
                }
                continue;
            }
            if wy == 0 {
                for (x, m) in xmap.iter().enumerate() {
                    let wx = m.wx as u32;
                    let iwx = 256 - wx;
                    let a = (m.x0 as usize) * 3;
                    let b = (m.x1 as usize) * 3;
                    let o = x * 3;
                    out[o] = ((r0[a] as u32 * iwx + r0[b] as u32 * wx) >> 8) as u8;
                    out[o + 1] = ((r0[a + 1] as u32 * iwx + r0[b + 1] as u32 * wx) >> 8) as u8;
                    out[o + 2] = ((r0[a + 2] as u32 * iwx + r0[b + 2] as u32 * wx) >> 8) as u8;
                }
                continue;
            }
            for (x, m) in xmap.iter().enumerate() {
                let wx = m.wx as u32;
                let iwx = 256 - wx;
                let a = (m.x0 as usize) * 3;
                let b = (m.x1 as usize) * 3;
                // Unrolled: three channels, two rows, two taps.
                let t0 = (r0[a] as u32 * iwx + r0[b] as u32 * wx) >> 8;
                let t1 = (r0[a + 1] as u32 * iwx + r0[b + 1] as u32 * wx) >> 8;
                let t2 = (r0[a + 2] as u32 * iwx + r0[b + 2] as u32 * wx) >> 8;
                let u0 = (r1[a] as u32 * iwx + r1[b] as u32 * wx) >> 8;
                let u1 = (r1[a + 1] as u32 * iwx + r1[b + 1] as u32 * wx) >> 8;
                let u2 = (r1[a + 2] as u32 * iwx + r1[b + 2] as u32 * wx) >> 8;
                let o = x * 3;
                out[o] = (((t0 * iwy) + (u0 * wy)) >> 8) as u8;
                out[o + 1] = (((t1 * iwy) + (u1 * wy)) >> 8) as u8;
                out[o + 2] = (((t2 * iwy) + (u2 * wy)) >> 8) as u8;
            }
            let _ = dw3;
        }
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_when_sizes_match() {
        let (w, h) = (16, 12);
        let src: Vec<u8> = (0..w * h * 3).map(|i| (i * 7 % 251) as u8).collect();
        let mut dst = vec![0u8; w * h * 3];
        let mut s = Scaler::new(w, h);
        let g = s.letterbox(&src, w, h, &mut dst);
        assert_eq!(g.pad_left, 0);
        assert_eq!(g.pad_top, 0);
        assert_eq!(g.new_w, w);
        assert_eq!(g.new_h, h);
        // scale == 1 -> the mapping is the identity, up to rounding
        for i in 0..src.len() {
            let d = (src[i] as i32 - dst[i] as i32).abs();
            assert!(d <= 1, "pixel {i}: {} vs {}", src[i], dst[i]);
        }
    }

    #[test]
    fn downscale_letterbox_geometry() {
        let mut s = Scaler::new(640, 640);
        let src = vec![0u8; 1920 * 1080 * 3];
        let mut dst = vec![0u8; 640 * 640 * 3];
        let g = s.letterbox(&src, 1920, 1080, &mut dst);
        assert_eq!(g.new_w, 640);
        assert_eq!(g.new_h, 360);
        assert_eq!(g.pad_top, 140);
        assert_eq!(g.pad_left, 0);
        // padding is 114
        assert!(dst[..140 * 640 * 3].iter().all(|&v| v == 114));
    }

    #[test]
    fn integer_ratio_is_exact_decimation() {
        // 30x21 -> 10x7 is exactly 3:1, so bilinear must reduce to picking the
        // source pixel at (3x+1, 3y+1).
        let (sw, sh) = (30usize, 21usize);
        let src: Vec<u8> = (0..sw * sh * 3).map(|i| (i * 13 % 253) as u8).collect();
        let mut dst = vec![0u8; 10 * 10 * 3];
        let mut s = Scaler::new(10, 10);
        let g = s.letterbox(&src, sw, sh, &mut dst);
        assert_eq!((g.new_w, g.new_h), (10, 7));
        for y in 0..g.new_h {
            for x in 0..g.new_w {
                let sy = 3 * y + 1;
                let sx = 3 * x + 1;
                for c in 0..3 {
                    let want = src[(sy * sw + sx) * 3 + c];
                    let got =
                        dst[((g.pad_top as usize + y) * 10 + g.pad_left as usize + x) * 3 + c];
                    assert_eq!(want, got, "at ({x},{y}) c{c}");
                }
            }
        }
    }

    #[test]
    fn flat_image_stays_flat() {
        let src = vec![200u8; 100 * 50 * 3];
        let mut dst = vec![0u8; 64 * 64 * 3];
        let mut s = Scaler::new(64, 64);
        let g = s.letterbox(&src, 100, 50, &mut dst);
        let base = (g.pad_top as usize * 64 + g.pad_left as usize) * 3;
        for y in 0..g.new_h {
            for x in 0..g.new_w * 3 {
                let v = dst[base + y * 64 * 3 + x];
                assert_eq!(v, 200);
            }
        }
    }
}
