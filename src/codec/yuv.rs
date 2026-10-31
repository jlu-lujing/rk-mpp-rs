//! Frame readback shared by the video decoders.
//!
//! Decoders read their output straight out of an uncached DMA mapping, so every
//! extra pass over a plane costs full DRAM traffic.  The generic per-sample
//! copies that grew up around the first decoder read the interleaved chroma
//! plane two or three times, one byte at a time, which at 1080p cost more than
//! the hardware job itself.  This module reads each byte exactly once, eight
//! bytes per load, which is also what the compiler can vectorise.

/// One 8-bit NV12 frame (luma plane, then interleaved UV plane, both `stride`
/// bytes wide) into a tightly packed planar YUV 4:2:0 buffer.
///
/// Chroma dimensions are the truncated halves of the luma size, matching the
/// per-sample loops this replaced: an odd luma edge column is dropped rather
/// than rounded up.
pub fn nv12_to_planar(y: &[u8], uv: &[u8], width: usize, height: usize, stride: usize) -> Vec<u8> {
    let chroma_width = width / 2;
    let chroma_height = height / 2;
    let luma_size = width * height;
    let chroma_size = chroma_width * chroma_height;
    let size = luma_size + 2 * chroma_size;
    // Every byte of the buffer is written below (luma rows, then both chroma
    // planes), so the spare capacity never holds a readable initialisation
    // pass; a zeroing allocation would add a full frame of DRAM writes.
    let mut out = Vec::with_capacity(size);
    // SAFETY: the length is only grown over bytes this function then fills in
    // full; `out` is never read before that, and every element is a plain u8.
    unsafe { out.set_len(size) };

    for row in 0..height {
        let src = row * stride;
        out[row * width..][..width].copy_from_slice(&y[src..][..width]);
    }

    let u_base = luma_size;
    let v_base = luma_size + chroma_size;
    let row_bytes = chroma_width * 2;
    for row in 0..chroma_height {
        let off = row * stride;
        let u_row = u_base + row * chroma_width;
        let v_row = v_base + row * chroma_width;
        // The fast path needs eight readable source bytes per step; a short
        // final row falls back to the per-sample copy.
        if uv.len() < off + row_bytes {
            for sample in 0..chroma_width {
                out[u_row + sample] = uv[off + 2 * sample];
                out[v_row + sample] = uv[off + 2 * sample + 1];
            }
            continue;
        }
        let mut sample = 0;
        while sample + 4 <= chroma_width {
            let src = off + sample * 2;
            let word = u64::from_le_bytes(uv[src..][..8].try_into().unwrap());
            out[u_row + sample..][..4].copy_from_slice(&even_bytes(word).to_le_bytes());
            out[v_row + sample..][..4].copy_from_slice(&odd_bytes(word).to_le_bytes());
            sample += 4;
        }
        while sample < chroma_width {
            out[u_row + sample] = uv[off + 2 * sample];
            out[v_row + sample] = uv[off + 2 * sample + 1];
            sample += 1;
        }
    }

    out
}

/// The even (U) bytes of a little-endian word, packed into four.
fn even_bytes(word: u64) -> u32 {
    let mut packed = word & 0x00ff_00ff_00ff_00ff;
    packed = (packed | (packed >> 8)) & 0x0000_ffff_0000_ffff;
    packed = (packed | (packed >> 16)) & 0x0000_0000_ffff_ffff;
    packed as u32
}

/// The odd (V) bytes of a little-endian word, packed into four.
fn odd_bytes(word: u64) -> u32 {
    even_bytes(word >> 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference copy: the straightforward byte-per-sample version.
    fn reference(y: &[u8], uv: &[u8], width: usize, height: usize, stride: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for row in 0..height {
            let off = row * stride;
            out.extend_from_slice(&y[off..off + width]);
        }
        for row in 0..height / 2 {
            let off = row * stride;
            for sample in 0..width / 2 {
                out.push(uv[off + 2 * sample]);
            }
        }
        for row in 0..height / 2 {
            let off = row * stride;
            for sample in 0..width / 2 {
                out.push(uv[off + 2 * sample + 1]);
            }
        }
        out
    }

    fn frames(width: usize, height: usize, stride: usize) -> (Vec<u8>, Vec<u8>) {
        let coded_height = height + 1;
        let mut y = vec![0u8; stride * coded_height];
        let mut uv = vec![0u8; stride * (coded_height / 2)];
        for (i, byte) in y.iter_mut().enumerate() {
            *byte = (i * 7) as u8;
        }
        for (i, byte) in uv.iter_mut().enumerate() {
            *byte = (255 - i as u8) | ((i % 3) as u8);
        }
        // Widen the stride padding so a wrong stride is visible.
        for row in 0..coded_height {
            for column in width..stride {
                y[row * stride + column] = 0xaa;
            }
        }
        (y, uv)
    }

    #[test]
    fn matches_the_byte_per_sample_copy() {
        for (width, height, stride) in [
            (4, 2, 4),
            (8, 8, 16),
            (64, 8, 64),
            (6, 6, 16),
            (130, 4, 192),
            (1280, 8, 1280),
        ] {
            let (y, uv) = frames(width, height, stride);
            assert_eq!(
                nv12_to_planar(&y, &uv, width, height, stride),
                reference(&y, &uv, width, height, stride),
                "{width}x{height} stride {stride}"
            );
        }
    }

    #[test]
    fn chroma_planes_are_full_rows_not_interleaved() {
        let (y, uv) = frames(8, 4, 8);
        let out = nv12_to_planar(&y, &uv, 8, 4, 8);
        assert_eq!(out.len(), 8 * 4 + 2 * 4 * 2);
        // Row 0 U then row 1 U must be adjacent: that is the whole point of the
        // de-interleave, and the bug the original code almost had.
        assert_eq!(&out[32..40], &reference(&y, &uv, 8, 4, 8)[32..40]);
    }

    #[test]
    fn packs_even_and_odd_bytes() {
        let word = u64::from_le_bytes([1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(even_bytes(word).to_le_bytes(), [1, 3, 5, 7]);
        assert_eq!(odd_bytes(word).to_le_bytes(), [2, 4, 6, 8]);
    }
}
