//! RGB888 bilinear resize + letterbox into a fixed destination.
//!
//! Replaces the RGA blit we used from the C implementation. The mapping is
//! computed once per resolution (x) and per row (y); the pixel loop is integer
//! only, so it vectorises well and needs no driver round-trip.
//!
//! Two integer-ratio row kernels accelerate the common cases without
//! changing a single output byte:
//!
//! * `Decimate`: an exact odd-integer downscale (`wx == wy == 0`) is a pure
//!   byte permutation; aarch64 runs it through NEON table lookups and other
//!   targets through the original scalar pick loop.
//! * `Weighted`: an exact even-integer downscale (constant `wx`) keeps the
//!   scalar arithmetic exact - the interpolation rewrite `a + ((b-a)*w) >> 8`
//!   is floor-equal to `(a*(256-w) + b*w) >> 8` - and performs it a chunk at
//!   a time.

#[derive(Clone, Copy, Default)]
struct XMap {
    x0: u32,
    x1: u32,
    wx: u16,
}

/// The shape of the x mapping, detected once per resolution. `General`
/// keeps every row on the original scalar loop; the uniform modes are the
/// integer-ratio cases the vector kernels cover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MapMode {
    General,
    Decimate {
        step: usize,
        offset: usize,
    },
    Weighted {
        step: usize,
        offset: usize,
        weight: u16,
    },
}

#[derive(Clone, Copy)]
struct GeneralChunk {
    base: u32,
    index_a: [u8; 16],
    index_b: [u8; 16],
    weights: [i16; 16],
}

#[inline]
fn bilinear(a: u32, b: u32, c: u32, d: u32, wx: u32, wy: u32) -> u32 {
    let top = a * (256 - wx) + b * wx;
    let bottom = c * (256 - wx) + d * wx;
    (top * (256 - wy) + bottom * wy + 32_768) >> 16
}

#[inline]
fn write_bt601_rgb(y: i32, u: i32, v: i32, out: &mut [u8]) {
    let c = (y - 16).max(0);
    let d = u - 128;
    let e = v - 128;
    let clamp = |value: i32| value.clamp(0, 255) as u8;
    out[0] = clamp((298 * c + 409 * e + 128) >> 8);
    out[1] = clamp((298 * c - 100 * d - 208 * e + 128) >> 8);
    out[2] = clamp((298 * c + 516 * d + 128) >> 8);
}

pub struct Scaler {
    dw: usize,
    dh: usize,
    xmap: Vec<XMap>,
    chroma_xmap: Vec<XMap>,
    map_sw: usize,
    map_nw: usize,
    map_scale: f32,
    map_wx_zero: bool,
    mode: MapMode,
    general_chunks: Vec<GeneralChunk>,
    neon_disabled: bool,
    scratch0: Vec<u8>,
    scratch1: Vec<u8>,
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
            chroma_xmap: Vec::new(),
            map_sw: 0,
            map_nw: 0,
            map_scale: 0.0,
            map_wx_zero: false,
            mode: MapMode::General,
            general_chunks: Vec::new(),
            neon_disabled: !neon_available(),
            scratch0: Vec::new(),
            scratch1: Vec::new(),
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
        self.chroma_xmap.clear();
        self.chroma_xmap.reserve(g.new_w);
        let chroma_width = sw.div_ceil(2);
        let max_chroma = (chroma_width - 1) as f32;
        for x in 0..g.new_w {
            let mut fx = ((x as f32 + 0.5) * inv - 0.5) * 0.5;
            if fx < 0.0 {
                fx = 0.0;
            }
            if fx > max_chroma {
                fx = max_chroma;
            }
            let x0 = fx as u32;
            let x1 = if x0 as usize + 1 < chroma_width {
                x0 + 1
            } else {
                x0
            };
            self.chroma_xmap.push(XMap {
                x0,
                x1,
                wx: ((fx - x0 as f32) * 256.0) as u16,
            });
        }
        self.mode = classify_map(&self.xmap);
        self.general_chunks.clear();
        if self.mode == MapMode::General {
            self.general_chunks = general_chunks(&self.xmap);
        }
        self.scratch0.clear();
        self.scratch1.clear();
    }

    /// Bench and test hook: force the scalar rows even where NEON exists.
    pub fn set_neon_disabled(&mut self, disabled: bool) {
        self.neon_disabled = disabled;
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
        let mode = self.mode;
        let general_chunks = &self.general_chunks;
        let map_wx_zero = self.map_wx_zero;
        let neon_disabled = self.neon_disabled;
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
            let r0 = &src[y0 * src_stride..y0 * src_stride + sw * 3];
            let r1 = &src[y1 * src_stride..y1 * src_stride + sw * 3];
            let out_row =
                &mut dst[((g.pad_top as usize + y) * self.dw + g.pad_left as usize) * 3..];
            let out = &mut out_row[..g.new_w * 3];

            // The vector kernels cover whole output pixels only; whatever
            // they leave is finished by the identical scalar loop over the
            // remaining mapping entries, so every output row is the same
            // bytes whichever paths produced it.
            let done = if neon_disabled {
                0
            } else {
                fast_row(
                    mode,
                    &mut self.scratch0,
                    &mut self.scratch1,
                    r0,
                    r1,
                    out,
                    general_chunks,
                    wy,
                )
            };
            if done == 0 {
                Self::row_scalar(out, xmap, r0, r1, wy, map_wx_zero, sw);
            } else {
                let (_, mut out) = out.split_at_mut(done * 3);
                Self::row_scalar(&mut out, &xmap[done..], r0, r1, wy, false, sw);
            }
        }
        g
    }

    /// Resize strided NV12 directly into RGB888 model input.
    ///
    /// Luma and chroma are bilinearly sampled from the decoder surfaces. This
    /// avoids materialising either a full-frame packed NV12 copy or an
    /// intermediate RGB frame when the source is a DMA-BUF-backed SDK frame.
    pub fn letterbox_nv12_strided(
        &mut self,
        y_plane: &[u8],
        uv_plane: &[u8],
        src_stride: usize,
        sw: usize,
        sh: usize,
        dst: &mut [u8],
    ) -> Geometry {
        assert!(sw > 0 && sh > 0 && sw % 2 == 0 && sh % 2 == 0);
        assert!(src_stride >= sw);
        assert!(y_plane.len() >= src_stride * (sh - 1) + sw);
        assert!(uv_plane.len() >= src_stride * (sh / 2 - 1) + sw);
        assert_eq!(dst.len(), self.dw * self.dh * 3);
        let g = self.geometry(sw, sh);
        self.build_map(sw, &g);
        let covers_all =
            g.pad_left == 0 && g.pad_top == 0 && g.new_w == self.dw && g.new_h == self.dh;
        if !covers_all {
            dst.fill(114);
        }

        let inv = 1.0f32 / g.scale;
        let chroma_height = sh / 2;
        let max_chroma_y = (chroma_height - 1) as f32;
        for y in 0..g.new_h {
            let mut fy = (y as f32 + 0.5) * inv - 0.5;
            if fy < 0.0 {
                fy = 0.0;
            }
            let max_y = (sh - 1) as f32;
            if fy > max_y {
                fy = max_y;
            }
            let y0 = fy as usize;
            let y1 = if y0 + 1 < sh { y0 + 1 } else { y0 };
            let wy = ((fy - y0 as f32) * 256.0) as u32;
            let mut cy = fy * 0.5;
            if cy > max_chroma_y {
                cy = max_chroma_y;
            }
            let cy0 = cy as usize;
            let cy1 = if cy0 + 1 < chroma_height {
                cy0 + 1
            } else {
                cy0
            };
            let cwy = ((cy - cy0 as f32) * 256.0) as u32;

            let y_row0 = &y_plane[y0 * src_stride..(y0 + 1) * src_stride];
            let y_row1 = &y_plane[y1 * src_stride..(y1 + 1) * src_stride];
            let uv_row0 = &uv_plane[cy0 * src_stride..(cy0 + 1) * src_stride];
            let uv_row1 = &uv_plane[cy1 * src_stride..(cy1 + 1) * src_stride];
            let out_row = &mut dst
                [((g.pad_top as usize + y) * self.dw + g.pad_left as usize) * 3..][..g.new_w * 3];
            for (x, (luma, chroma)) in self.xmap.iter().zip(&self.chroma_xmap).enumerate() {
                let y_value = bilinear(
                    y_row0[luma.x0 as usize] as u32,
                    y_row0[luma.x1 as usize] as u32,
                    y_row1[luma.x0 as usize] as u32,
                    y_row1[luma.x1 as usize] as u32,
                    luma.wx.into(),
                    wy,
                );
                let u_value = bilinear(
                    uv_row0[chroma.x0 as usize * 2] as u32,
                    uv_row0[chroma.x1 as usize * 2] as u32,
                    uv_row1[chroma.x0 as usize * 2] as u32,
                    uv_row1[chroma.x1 as usize * 2] as u32,
                    chroma.wx.into(),
                    cwy,
                );
                let v_value = bilinear(
                    uv_row0[chroma.x0 as usize * 2 + 1] as u32,
                    uv_row0[chroma.x1 as usize * 2 + 1] as u32,
                    uv_row1[chroma.x0 as usize * 2 + 1] as u32,
                    uv_row1[chroma.x1 as usize * 2 + 1] as u32,
                    chroma.wx.into(),
                    cwy,
                );
                let pixel = &mut out_row[x * 3..x * 3 + 3];
                write_bt601_rgb(y_value as i32, u_value as i32, v_value as i32, pixel);
            }
        }
        g
    }

    /// The original scalar row, kept byte-for-byte as the reference the
    /// vector kernels must reproduce (and the fallback they fall back to).
    fn row_scalar(
        out: &mut [u8],
        xmap: &[XMap],
        r0: &[u8],
        r1: &[u8],
        wy: u32,
        wx_zero: bool,
        sw: usize,
    ) {
        /* Integer-aligned cases (e.g. 1920x1080 -> 640 at exactly 1/3)
         * need no interpolation at all; a 1:1 mapping is a plain copy. */
        if wy == 0 && wx_zero {
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
            return;
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
            return;
        }
        let iwy = 256 - wy;
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
    }
}

fn general_chunks(xmap: &[XMap]) -> Vec<GeneralChunk> {
    let mut chunks = Vec::with_capacity(xmap.len() / 4);
    for maps in xmap.chunks_exact(4) {
        let mut base = u32::MAX;
        for map in maps {
            base = base.min(map.x0 * 3).min(map.x1 * 3);
        }
        let mut index_a = [64u8; 16];
        let mut index_b = [64u8; 16];
        let mut weights = [0i16; 16];
        let mut valid = true;
        for (pixel, map) in maps.iter().enumerate() {
            weights[pixel * 3..pixel * 3 + 3].fill(map.wx as i16);
            for channel in 0..3 {
                let a = map.x0 * 3 + channel as u32 - base;
                let b = map.x1 * 3 + channel as u32 - base;
                if a > 63 || b > 63 {
                    valid = false;
                    break;
                }
                let lane = pixel * 3 + channel;
                index_a[lane] = a as u8;
                index_b[lane] = b as u8;
            }
            if !valid {
                break;
            }
        }
        if !valid {
            break;
        }
        chunks.push(GeneralChunk {
            base,
            index_a,
            index_b,
            weights,
        });
    }
    chunks
}

/// Number of leading output pixels the vector kernels produced for this
/// row; `0` sends the whole row through `row_scalar`. A free function so
/// the row loop can keep borrowing the mapping while the scratch moves.
fn fast_row(
    mode: MapMode,
    scratch0: &mut Vec<u8>,
    scratch1: &mut Vec<u8>,
    r0: &[u8],
    r1: &[u8],
    out: &mut [u8],
    general: &[GeneralChunk],
    wy: u32,
) -> usize {
    if !neon_available() {
        return 0;
    }
    #[cfg(target_arch = "aarch64")]
    {
        if mode == MapMode::General {
            // SAFETY: NEON was checked above, and row_bilinear validates every
            // 64-byte source window and output chunk before loading/storing.
            return unsafe { kernels::row_bilinear(r0, r1, out, general, wy) };
        }
        let (step, offset) = match mode {
            MapMode::General => unreachable!(),
            MapMode::Decimate { step, offset } => {
                if wy != 0 || step == 0 {
                    return 0;
                }
                if step == 1 && offset == 0 {
                    // The 1:1 mapping: let the original whole-row memcpy
                    // shortcut handle it instead.
                    return 0;
                }
                (step, offset)
            }
            MapMode::Weighted {
                step,
                offset,
                weight,
            } => {
                if step == 0 {
                    return 0;
                }
                let stride = step * 3;
                if wy == 0 {
                    // SAFETY: NEON is available; row_interp bounds-checks every
                    // source window and output chunk.
                    return unsafe { kernels::row_interp(r0, out, stride, offset * 3, weight) };
                }
                let pixels = out.len() / 3;
                scratch0.clear();
                scratch0.resize(pixels * 3, 0);
                scratch1.clear();
                scratch1.resize(pixels * 3, 0);
                // SAFETY: both scratch rows are sized for the output, NEON is
                // available, and each call performs its own bounds checks.
                let n0 = unsafe { kernels::row_interp(r0, scratch0, stride, offset * 3, weight) };
                let n1 = unsafe { kernels::row_interp(r1, scratch1, stride, offset * 3, weight) };
                let pixels = n0.min(n1);
                // SAFETY: row_combine additionally clamps to the shortest of
                // all three slices, so the scratch/output lengths are safe.
                let bytes = unsafe { kernels::row_combine(scratch0, scratch1, out, wy) };
                return pixels.min(bytes / 3);
            }
        };
        // SAFETY: NEON is available and row_decimate validates its source
        // window before each vector load and output store.
        unsafe { kernels::row_decimate(r0, out, step * 3, offset * 3) }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = (r0, r1, out, general, wy);
        0
    }
}

/// The uniform-shape test behind `MapMode`. Any deviation - a varying
/// weight, a non-constant stride, a tap that is not the next pixel - keeps
/// the map `General`, so the kernels only ever see the pattern they model.
fn classify_map(xmap: &[XMap]) -> MapMode {
    if xmap.len() < 2 {
        return MapMode::General;
    }
    let step = xmap[1].x0.saturating_sub(xmap[0].x0);
    let offset = xmap[0].x0;
    let weight = xmap[0].wx;
    if step == 0 {
        return MapMode::General;
    }
    for (x, m) in xmap.iter().enumerate() {
        if m.x0 != offset + step * x as u32 || m.wx != weight {
            return MapMode::General;
        }
        if m.x1 != m.x0 + 1 {
            // The right edge may clamp x1 onto x0; nothing else may deviate.
            if x + 1 != xmap.len() || m.x1 != m.x0 {
                return MapMode::General;
            }
        }
    }
    if weight == 0 {
        MapMode::Decimate {
            step: step as usize,
            offset: offset as usize,
        }
    } else {
        MapMode::Weighted {
            step: step as usize,
            offset: offset as usize,
            weight,
        }
    }
}

fn neon_available() -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        std::arch::is_aarch64_feature_detected!("neon")
    }
    #[cfg(not(target_arch = "aarch64"))]
    false
}

#[cfg(target_arch = "aarch64")]
mod kernels {
    use core::arch::aarch64::*;

    /// Output pixels per kernel chunk; twelve output bytes, every lane of
    /// the table result is a real output byte.
    const PIXELS: usize = 4;

    /// Table indices for four output pixels: byte `(j / 3) * stride + j % 3`
    /// of the chunk's source window, with the remaining lanes out of range.
    fn gather_indices(stride: usize) -> [u8; 16] {
        let mut idx = [64u8; 16];
        for (j, lane) in idx.iter_mut().enumerate().take(12) {
            *lane = ((j / 3) * stride + j % 3) as u8;
        }
        idx
    }

    /// Store the first twelve lanes of a table result.
    ///
    /// # Safety
    /// `out` must have twelve writable bytes.
    #[target_feature(enable = "neon")]
    unsafe fn store_chunk(out: *mut u8, chunk: uint8x16_t) {
        vst1_u8(out, vget_low_u8(chunk));
        // Byte 12 of the row comes from lane 0 of the vector's high u32
        // half; the lane store is an unaligned-capable single store.
        vst1_lane_u32::<0>(
            out.add(8).cast::<u32>(),
            vget_high_u32(vreinterpretq_u32_u8(chunk)),
        );
    }

    /// `a + ((b - a) * w) >> 8` on eight byte lanes with an arithmetic
    /// shift. Floor division through the negative product matches the
    /// scalar `(a * (256 - w) + b * w) >> 8` exactly, because
    /// `floor((256a + m) / 256) = a + floor(m / 256)`.
    ///
    /// # Safety
    /// Requires NEON, which is guaranteed by the `target_feature` attribute
    /// on callers executing this function.
    #[target_feature(enable = "neon")]
    unsafe fn interp8(a: uint8x8_t, b: uint8x8_t, w: int16x8_t, neg8: int16x8_t) -> uint8x8_t {
        let a16 = vmovl_u8(a);
        let b16 = vmovl_u8(b);
        let d = vsubq_s16(vreinterpretq_s16_u16(b16), vreinterpretq_s16_u16(a16));
        // d stays within +-255 and w within 255, so the i16 product is exact.
        let m = vmulq_s16(d, w);
        let t = vaddq_s16(vshlq_s16(m, neg8), vreinterpretq_s16_u16(a16));
        vmovn_u16(vreinterpretq_u16_s16(t))
    }

    /// Horizontal interpolation of one source row at a constant weight:
    /// `out[3p+c] = (src[base+p*stride+c]*(256-w) + src[base+p*stride+c+1]*w) >> 8`.
    /// Returns the number of leading output pixels produced (a multiple of
    /// four); the chunk loop stops before any base whose table loads would
    /// read past `src.len()`.
    ///
    /// # Safety
    /// Requires NEON.
    #[target_feature(enable = "neon")]
    pub unsafe fn row_interp(
        src: &[u8],
        out: &mut [u8],
        stride: usize,
        offset: usize,
        weight: u16,
    ) -> usize {
        // The two taps live in one 64-byte table window: the twelfth output
        // byte reads the second tap at 3 * stride + 2 + 3.
        if stride == 0 || 3 * stride + 6 > 64 {
            return 0;
        }
        let pixels = out.len() / 3;
        let idx_a = gather_indices(stride);
        let mut idx_b = idx_a;
        for lane in &mut idx_b {
            // The second tap is one full source pixel ahead: the same
            // channel of x0 + 1. The out-of-range marker stays out of
            // range: 64 lanes and above read as zero from any table size.
            *lane = lane.saturating_add(3);
        }
        let (idx_a, idx_b) = (vld1q_u8(idx_a.as_ptr()), vld1q_u8(idx_b.as_ptr()));
        let w = vdupq_n_s16(weight as i16);
        let neg8 = vdupq_n_s16(-8);
        let mut done = 0;
        while done + PIXELS <= pixels {
            let base = offset + done * stride;
            if base + 64 > src.len() {
                break;
            }
            let p = src.as_ptr().add(base);
            let regs = uint8x16x4_t(
                vld1q_u8(p),
                vld1q_u8(p.add(16)),
                vld1q_u8(p.add(32)),
                vld1q_u8(p.add(48)),
            );
            let a = vqtbl4q_u8(regs, idx_a);
            let b = vqtbl4q_u8(regs, idx_b);
            let low = interp8(vget_low_u8(a), vget_low_u8(b), w, neg8);
            let high = interp8(vget_high_u8(a), vget_high_u8(b), w, neg8);
            store_chunk(out.as_mut_ptr().add(done * 3), vcombine_u8(low, high));
            done += PIXELS;
        }
        done
    }

    /// Vertical combine of two horizontally interpolated rows at a constant
    /// weight, byte for byte the same arithmetic as `row_interp`. Returns
    /// the number of leading bytes produced; every byte is independent.
    ///
    /// # Safety
    /// Requires NEON, and `t`, `u` and `out` share at least `out.len()`
    /// valid bytes.
    #[target_feature(enable = "neon")]
    pub unsafe fn row_combine(t: &[u8], u: &[u8], out: &mut [u8], wy: u32) -> usize {
        let len = out.len().min(t.len()).min(u.len());
        let w = vdupq_n_s16(wy as i16);
        let neg8 = vdupq_n_s16(-8);
        let mut i = 0;
        while i + 16 <= len {
            let a = vld1q_u8(t.as_ptr().add(i));
            let b = vld1q_u8(u.as_ptr().add(i));
            let low = interp8(vget_low_u8(a), vget_low_u8(b), w, neg8);
            let high = interp8(vget_high_u8(a), vget_high_u8(b), w, neg8);
            vst1q_u8(out.as_mut_ptr().add(i), vcombine_u8(low, high));
            i += 16;
        }
        while i + 8 <= len {
            let a = vld1_u8(t.as_ptr().add(i));
            let b = vld1_u8(u.as_ptr().add(i));
            vst1_u8(out.as_mut_ptr().add(i), interp8(a, b, w, neg8));
            i += 8;
        }
        i
    }

    /// # Safety
    /// Requires NEON and a readable 64-byte window at `src`.
    #[target_feature(enable = "neon")]
    unsafe fn interpolate_chunk(
        src: *const u8,
        index_a: uint8x16_t,
        index_b: uint8x16_t,
        weight_low: int16x8_t,
        weight_high: int16x8_t,
        neg8: int16x8_t,
    ) -> uint8x16_t {
        let regs = uint8x16x4_t(
            vld1q_u8(src),
            vld1q_u8(src.add(16)),
            vld1q_u8(src.add(32)),
            vld1q_u8(src.add(48)),
        );
        let a = vqtbl4q_u8(regs, index_a);
        let b = vqtbl4q_u8(regs, index_b);
        let low = interp8(vget_low_u8(a), vget_low_u8(b), weight_low, neg8);
        let high = interp8(vget_high_u8(a), vget_high_u8(b), weight_high, neg8);
        vcombine_u8(low, high)
    }

    /// General non-integer bilinear rows. Each precomputed chunk gathers four
    /// pixels from a 64-byte source window, interpolates horizontally and then
    /// combines the two source rows vertically. The caller's scalar row
    /// finishes anything this returns as incomplete.
    ///
    /// # Safety
    /// Requires NEON. Every chunk's `base + 64` must be checked by this
    /// function; `r0`, `r1` and `out` remain borrowed for the duration.
    #[target_feature(enable = "neon")]
    pub unsafe fn row_bilinear(
        r0: &[u8],
        r1: &[u8],
        out: &mut [u8],
        chunks: &[super::GeneralChunk],
        wy: u32,
    ) -> usize {
        let neg8 = vdupq_n_s16(-8);
        let vertical = vdupq_n_s16(wy as i16);
        let mut done = 0usize;
        for chunk in chunks {
            if (done + PIXELS) * 3 > out.len() {
                break;
            }
            let base = chunk.base as usize;
            if base + 64 > r0.len() || base + 64 > r1.len() {
                break;
            }
            let index_a = vld1q_u8(chunk.index_a.as_ptr());
            let index_b = vld1q_u8(chunk.index_b.as_ptr());
            let weight_low = vld1q_s16(chunk.weights.as_ptr());
            let weight_high = vld1q_s16(chunk.weights.as_ptr().add(8));
            let horizontal0 = interpolate_chunk(
                r0.as_ptr().add(base),
                index_a,
                index_b,
                weight_low,
                weight_high,
                neg8,
            );
            let result = if wy == 0 {
                horizontal0
            } else {
                let horizontal1 = interpolate_chunk(
                    r1.as_ptr().add(base),
                    index_a,
                    index_b,
                    weight_low,
                    weight_high,
                    neg8,
                );
                let low = interp8(
                    vget_low_u8(horizontal0),
                    vget_low_u8(horizontal1),
                    vertical,
                    neg8,
                );
                let high = interp8(
                    vget_high_u8(horizontal0),
                    vget_high_u8(horizontal1),
                    vertical,
                    neg8,
                );
                vcombine_u8(low, high)
            };
            store_chunk(out.as_mut_ptr().add(done * 3), result);
            done += PIXELS;
        }
        done
    }

    /// Byte-exact decimation of one source row:
    /// `out[3p+c] = src[offset + p*stride + c]`. A pure permutation, so the
    /// table indices fully determine it. Returns the number of leading
    /// output pixels produced (a multiple of four); the chunk loop stops
    /// before any base whose table loads would read past `src.len()`.
    ///
    /// # Safety
    /// Requires NEON.
    #[target_feature(enable = "neon")]
    pub unsafe fn row_decimate(src: &[u8], out: &mut [u8], stride: usize, offset: usize) -> usize {
        // The twelfth output byte reads byte 3 * stride + 2; the smallest
        // window that can hold it is two table registers.
        if stride == 0 || 3 * stride + 2 > 63 {
            return 0;
        }
        let pixels = out.len() / 3;
        let idx = gather_indices(stride);
        let idx = vld1q_u8(idx.as_ptr());
        let mut done = 0;
        while done + PIXELS <= pixels {
            let base = offset + done * stride;
            let loads = if stride <= 9 {
                32
            } else if stride <= 15 {
                48
            } else {
                64
            };
            if base + loads > src.len() {
                break;
            }
            let p = src.as_ptr().add(base);
            let result = if stride <= 9 {
                let regs = uint8x16x2_t(vld1q_u8(p), vld1q_u8(p.add(16)));
                vqtbl2q_u8(regs, idx)
            } else if stride <= 15 {
                let regs = uint8x16x3_t(vld1q_u8(p), vld1q_u8(p.add(16)), vld1q_u8(p.add(32)));
                vqtbl3q_u8(regs, idx)
            } else {
                let regs = uint8x16x4_t(
                    vld1q_u8(p),
                    vld1q_u8(p.add(16)),
                    vld1q_u8(p.add(32)),
                    vld1q_u8(p.add(48)),
                );
                vqtbl4q_u8(regs, idx)
            };
            store_chunk(out.as_mut_ptr().add(done * 3), result);
            done += PIXELS;
        }
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg_bytes(len: usize) -> Vec<u8> {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        (0..len)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (state >> 33) as u8
            })
            .collect()
    }

    #[test]
    fn strided_nv12_identity_uses_bt601_limited_range() {
        let (w, h, stride) = (4usize, 2usize, 8usize);
        let y = vec![76u8; stride * h];
        let mut uv = vec![0u8; stride * (h / 2)];
        for pair in uv.chunks_exact_mut(2) {
            pair.copy_from_slice(&[85, 255]);
        }
        let mut dst = vec![0u8; w * h * 3];
        let mut scaler = Scaler::new(w, h);
        let geometry = scaler.letterbox_nv12_strided(&y, &uv, stride, w, h, &mut dst);
        assert_eq!((geometry.new_w, geometry.new_h), (w, h));
        for pixel in dst.chunks_exact(3) {
            assert_eq!(pixel, [255, 0, 0]);
        }
    }

    #[test]
    fn nv12_letterbox_keeps_padding_outside_the_scaled_picture() {
        let (w, h, stride) = (8usize, 4usize, 12usize);
        let y = vec![76u8; stride * h];
        let mut uv = vec![0u8; stride * (h / 2)];
        for pair in uv.chunks_exact_mut(2) {
            pair.copy_from_slice(&[85, 255]);
        }
        let mut dst = vec![0u8; 4 * 4 * 3];
        let mut scaler = Scaler::new(4, 4);
        let geometry = scaler.letterbox_nv12_strided(&y, &uv, stride, w, h, &mut dst);
        assert_eq!((geometry.new_w, geometry.new_h), (4, 2));
        assert_eq!(geometry.pad_top, 1);
        assert!(dst[..4 * 3].iter().all(|&value| value == 114));
        for pixel in dst[4 * 3..12 * 3].chunks_exact(3) {
            assert_eq!(pixel, [255, 0, 0]);
        }
        assert!(dst[12 * 3..].iter().all(|&value| value == 114));
    }

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

    /// Every integer-ratio shape the kernels claim, plus shapes they must
    /// refuse, must produce the exact same destination bytes as the pure
    /// scalar reference with random source pixels. This is the invariant
    /// the vector rows are built on: they are an implementation detail of
    /// the output, never a participant in it.
    #[test]
    fn the_vector_rows_are_byte_identical_to_the_scalar_reference() {
        for (sw, sh, dw, dh) in [
            (1920, 1080, 640, 640), // the production 3:1 letterbox
            (60, 42, 20, 20),       // 3:1 both axes
            (1280, 720, 640, 640),  // exact 2:1 (weighted)
            (40, 24, 20, 20),       // 2:1 both axes
            (150, 90, 30, 30),      // exact 5:1
            (70, 14, 10, 10),       // 7:1
            (300, 20, 20, 20),      // 15:1
            (64, 32, 32, 32),       // 2:1 small
            (33, 22, 11, 7),        // 3:1 with an uneven letterbox
            (24, 10, 8, 8),         // 3:1 x, 2:1 y letterbox mix
            (1930, 1080, 640, 640), // not a ratio at all: stays General
            (1281, 721, 640, 640),  // near 2:1 but not exact: stays General
        ] {
            let src = lcg_bytes(sw * sh * 3);
            let mut scalar = Scaler::new(dw, dh);
            scalar.neon_disabled = true;
            let mut vector = Scaler::new(dw, dh);
            let mut dst_scalar = vec![0u8; dw * dh * 3];
            let mut dst_vector = vec![1u8; dw * dh * 3];
            let g_scalar = scalar.letterbox(&src, sw, sh, &mut dst_scalar);
            let g_vector = vector.letterbox(&src, sw, sh, &mut dst_vector);
            let mode = vector.mode;
            assert_eq!(
                (g_scalar.new_w, g_scalar.new_h),
                (g_vector.new_w, g_vector.new_h)
            );
            let first_difference = (0..dst_scalar.len())
                .find(|i| dst_scalar[*i] != dst_vector[*i])
                .unwrap_or(usize::MAX);
            assert_eq!(
                first_difference,
                usize::MAX,
                "{sw}x{sh} -> {dw}x{dh} diverged at byte {first_difference}                  (row {}, col byte {}, mode {:?}, scalar {} vs vector {})",
                first_difference / (dw * 3),
                first_difference % (dw * 3),
                mode,
                dst_scalar[first_difference],
                dst_vector[first_difference],
            );
        }
    }

    /// On aarch64 the production letterbox must actually be in a kernel
    /// mode - otherwise the identity test above would compare two scalar
    /// runs and prove nothing.
    #[cfg(target_arch = "aarch64")]
    #[test]
    fn the_production_shapes_reach_the_vector_kernels() {
        let mut s = Scaler::new(640, 640);
        let g = s.geometry(1920, 1080);
        s.build_map(1920, &g);
        assert_eq!(
            s.mode,
            MapMode::Decimate { step: 3, offset: 1 },
            "1920x1080 -> 640 must decimate"
        );
        let mut s = Scaler::new(640, 640);
        let g = s.geometry(1280, 720);
        s.build_map(1280, &g);
        assert_eq!(
            s.mode,
            MapMode::Weighted {
                step: 2,
                offset: 0,
                weight: 128
            },
            "1280x720 -> 640 must take the weighted kernel"
        );
    }

    #[test]
    fn the_map_classifier_only_accepts_uniform_patterns() {
        let mk = |xmap: Vec<XMap>| classify_map(&xmap);
        assert_eq!(
            mk(vec![
                XMap {
                    x0: 1,
                    x1: 2,
                    wx: 0
                },
                XMap {
                    x0: 4,
                    x1: 5,
                    wx: 0
                },
                XMap {
                    x0: 7,
                    x1: 8,
                    wx: 0
                },
            ]),
            MapMode::Decimate { step: 3, offset: 1 }
        );
        assert_eq!(
            mk(vec![
                XMap {
                    x0: 0,
                    x1: 1,
                    wx: 128
                },
                XMap {
                    x0: 2,
                    x1: 3,
                    wx: 128
                },
            ]),
            MapMode::Weighted {
                step: 2,
                offset: 0,
                weight: 128
            }
        );
        // A single weight deviation stays General.
        assert_eq!(
            mk(vec![
                XMap {
                    x0: 0,
                    x1: 1,
                    wx: 128
                },
                XMap {
                    x0: 2,
                    x1: 3,
                    wx: 96
                },
            ]),
            MapMode::General
        );
        // So does a non-constant stride.
        assert_eq!(
            mk(vec![
                XMap {
                    x0: 0,
                    x1: 1,
                    wx: 64
                },
                XMap {
                    x0: 3,
                    x1: 4,
                    wx: 64
                },
                XMap {
                    x0: 5,
                    x1: 6,
                    wx: 64
                },
            ]),
            MapMode::General
        );
        // The right edge may clamp its tap onto x0; anything earlier may not.
        assert_eq!(
            mk(vec![
                XMap {
                    x0: 1,
                    x1: 2,
                    wx: 0
                },
                XMap {
                    x0: 4,
                    x1: 4,
                    wx: 0
                },
            ]),
            MapMode::Decimate { step: 3, offset: 1 }
        );
        assert_eq!(
            mk(vec![
                XMap {
                    x0: 1,
                    x1: 1,
                    wx: 0
                },
                XMap {
                    x0: 4,
                    x1: 5,
                    wx: 0
                },
            ]),
            MapMode::General
        );
    }

    /// A row too short for a whole kernel chunk must still be decimated
    /// exactly (pure scalar path), and the vector path's partial row must be
    /// finished exactly by the scalar loop: same bytes, both ways.
    #[test]
    fn short_rows_and_row_tails_stay_exact() {
        for (sw, sh, dw, dh) in [
            (18, 6, 6, 6),        // 3:1, 6-pixel rows: 4 via kernel, 2 via scalar
            (15, 9, 5, 5),        // 3:1, 5-pixel rows
            (12, 8, 6, 6),        // 2:1 weighted, 6-pixel rows: 4 kernel + 2 scalar
            (1920, 12, 640, 640), // real row width, one source row only
        ] {
            let src = lcg_bytes(sw * sh * 3);
            let mut scalar = Scaler::new(dw, dh);
            scalar.neon_disabled = true;
            let mut vector = Scaler::new(dw, dh);
            let mut dst_scalar = vec![0u8; dw * dh * 3];
            let mut dst_vector = vec![2u8; dw * dh * 3];
            scalar.letterbox(&src, sw, sh, &mut dst_scalar);
            vector.letterbox(&src, sw, sh, &mut dst_vector);
            assert_eq!(dst_scalar, dst_vector, "{sw}x{sh} -> {dw}x{dh}");
        }
    }

    /// The 3:1 pick itself, independent of any dispatch: the kernel result
    /// must equal picking src[(3x+1)*3+c] pixel by pixel.
    #[cfg(target_arch = "aarch64")]
    #[test]
    fn the_gather_kernel_equals_pixel_by_pixel_picking() {
        let src = lcg_bytes(300 * 3);
        let mut out = vec![9u8; 100 * 3];
        // SAFETY: the test is aarch64-only and sizes the source/output for the
        // row_decimate bounds checks.
        let n = unsafe { kernels::row_decimate(&src, &mut out, 9, 3) };
        assert_eq!(n % 4, 0);
        for p in 0..n {
            for c in 0..3 {
                assert_eq!(
                    out[p * 3 + c],
                    src[(3 * p + 1) * 3 + c],
                    "pixel {p} channel {c}"
                );
            }
        }
        assert_eq!(n, 100);
    }

    /// The weighted kernel, independent of any dispatch: its output must
    /// equal the scalar `(a*(256-w) + b*w) >> 8` per byte.
    #[cfg(target_arch = "aarch64")]
    #[test]
    fn the_weighted_kernel_equals_the_scalar_taps() {
        for weight in [1u16, 64, 128, 191, 255] {
            for stride in [6usize, 9, 12, 18] {
                let src = lcg_bytes(160 * 3);
                let mut out = vec![0u8; 20 * 3];
                // SAFETY: the test is aarch64-only and allocates the complete
                // source/output windows expected by row_interp.
                let n = unsafe { kernels::row_interp(&src, &mut out, stride, 0, weight) };
                assert_eq!(n, 20);
                for p in 0..n {
                    for c in 0..3 {
                        let a = src[p * stride + c] as u32;
                        let b = src[p * stride + c + 3] as u32;
                        let want = (a * (256 - weight as u32) + b * weight as u32) >> 8;
                        assert_eq!(
                            out[p * 3 + c] as u32,
                            want,
                            "weight {weight} stride {stride} pixel {p} channel {c}"
                        );
                    }
                }
            }
        }
    }

    /// The vertical combine kernel against the scalar two-row formula.
    #[cfg(target_arch = "aarch64")]
    #[test]
    fn the_combine_kernel_equals_the_scalar_rows() {
        for wy in [0u32, 1, 127, 128, 255] {
            let t = lcg_bytes(70);
            let u = lcg_bytes(70);
            let mut out = vec![0u8; 64];
            // SAFETY: the test is aarch64-only and all three slices are at
            // least the requested output length.
            let n = unsafe { kernels::row_combine(&t, &u, &mut out, wy) };
            assert_eq!(n, 64);
            let iwy = 256 - wy;
            for i in 0..n {
                let want = ((t[i] as u32 * iwy) + (u[i] as u32 * wy)) >> 8;
                assert_eq!(out[i] as u32, want, "wy {wy} byte {i}");
            }
        }
    }
}
