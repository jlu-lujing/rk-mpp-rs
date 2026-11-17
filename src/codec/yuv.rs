//! Frame readback shared by the video decoders.
//!
//! Decoders read their output straight out of an uncached DMA mapping, so every
//! extra pass over a plane costs full DRAM traffic.  The generic per-sample
//! copies that grew up around the first decoder read the interleaved chroma
//! plane two or three times, one byte at a time, which at 1080p cost more than
//! the hardware job itself.  This module reads each byte exactly once, eight
//! bytes per load, which is also what the compiler can vectorise.

/// Copy one 8-bit NV12 frame into tightly packed NV12, removing only the
/// hardware row stride. This keeps chroma interleaved for NV12-capable display
/// and conversion consumers and avoids the planar deinterleave pass.
pub fn nv12_to_packed(y: &[u8], uv: &[u8], width: usize, height: usize, stride: usize) -> Vec<u8> {
    let chroma_height = height / 2;
    let luma_size = width * height;
    let chroma_size = width * chroma_height;
    let mut out = Vec::with_capacity(luma_size + chroma_size);
    for row in 0..height {
        let src = row * stride;
        out.extend_from_slice(&y[src..src + width]);
    }
    for row in 0..chroma_height {
        let src = row * stride;
        out.extend_from_slice(&uv[src..src + width]);
    }
    debug_assert_eq!(out.len(), luma_size + chroma_size);
    out
}

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
        // NEON splits sixteen UV pairs per iteration: two 16-byte loads feed
        // one uzp1 (U) and one uzp2 (V) 16-byte store. This is the same pass
        // the word-at-a-time path below performs, with a quarter of the
        // instructions on the hardware that actually runs it.
        #[cfg(target_arch = "aarch64")]
        if std::arch::is_aarch64_feature_detected!("neon") {
            // SAFETY: `off + row_bytes <= uv.len()` was checked above, the
            // output buffer was sized for the full planar frame, and NEON is
            // present. The helper handles its own scalar tail.
            unsafe {
                deinterleave_uv_neon(
                    uv.as_ptr().add(off),
                    out.as_mut_ptr().add(u_row),
                    out.as_mut_ptr().add(v_row),
                    chroma_width,
                );
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

/// Deinterleave `pairs` NV12 chroma samples into separate U and V rows.
///
/// # Safety
/// `src` must have `pairs * 2` readable bytes, `u` and `v` must each have
/// `pairs` writable bytes, and the caller must have confirmed NEON support.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn deinterleave_uv_neon(src: *const u8, u: *mut u8, v: *mut u8, pairs: usize) {
    use core::arch::aarch64::*;
    let mut i = 0;
    while i + 16 <= pairs {
        let a = vld1q_u8(src.add(i * 2));
        let b = vld1q_u8(src.add(i * 2 + 16));
        vst1q_u8(u.add(i), vuzp1q_u8(a, b));
        vst1q_u8(v.add(i), vuzp2q_u8(a, b));
        i += 16;
    }
    while i + 8 <= pairs {
        let a = vld1q_u8(src.add(i * 2));
        vst1_u8(u.add(i), vget_low_u8(vuzp1q_u8(a, a)));
        vst1_u8(v.add(i), vget_low_u8(vuzp2q_u8(a, a)));
        i += 8;
    }
    while i < pairs {
        *u.add(i) = *src.add(2 * i);
        *v.add(i) = *src.add(2 * i + 1);
        i += 1;
    }
}

/// Rockchip's packed 10-bit NV12 into planar little-endian 16-bit samples.
///
/// Four adjacent samples occupy exactly five input bytes, so the common path
/// fills four output words with one load and one store instead of decoding
/// every sample through a dynamic bit-depth helper.
pub fn packed10_nv12_to_planar(
    y: &[u8],
    uv: &[u8],
    width: usize,
    height: usize,
    stride: usize,
) -> Vec<u8> {
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    let luma_samples = width * height;
    let chroma_samples = chroma_width * chroma_height;
    let size = (luma_samples + 2 * chroma_samples) * 2;
    let mut out = Vec::with_capacity(size);
    // SAFETY: every byte up to `size` is filled below. `u8` needs no drop and
    // the spare capacity is never exposed as initialized.
    unsafe { out.set_len(size) };

    for row in 0..height {
        unpack_packed10_row(
            &y[row * stride..],
            &mut out[row * width * 2..][..width * 2],
            width,
        );
    }

    let u_base = luma_samples * 2;
    let (u_plane, v_plane) = out[u_base..].split_at_mut(chroma_samples * 2);
    for row in 0..chroma_height {
        let u_row = &mut u_plane[row * chroma_width * 2..][..chroma_width * 2];
        let v_row = &mut v_plane[row * chroma_width * 2..][..chroma_width * 2];
        unpack_packed10_uv_row(&uv[row * stride..], u_row, v_row, chroma_width);
    }
    out
}

fn unpack_packed10_row(src: &[u8], dst: &mut [u8], samples: usize) {
    let groups = samples / 4;
    for group in 0..groups {
        let packed = packed10_word(&src[group * 5..]);
        let words = (packed & 0x03ff)
            | ((packed >> 10) & 0x03ff) << 16
            | ((packed >> 20) & 0x03ff) << 32
            | ((packed >> 30) & 0x03ff) << 48;
        dst[group * 8..group * 8 + 8].copy_from_slice(&words.to_le_bytes());
    }
    let mut sample = groups * 4;
    while sample < samples {
        let value = read_packed10_sample(src, sample);
        dst[sample * 2..sample * 2 + 2].copy_from_slice(&value.to_le_bytes());
        sample += 1;
    }
}

fn unpack_packed10_uv_row(src: &[u8], u: &mut [u8], v: &mut [u8], samples: usize) {
    let groups = samples / 2;
    for group in 0..groups {
        let packed = packed10_word(&src[group * 5..]);
        let u_words = ((packed & 0x03ff) | ((packed >> 20) & 0x03ff) << 16) as u32;
        let v_words = (((packed >> 10) & 0x03ff) | ((packed >> 30) & 0x03ff) << 16) as u32;
        let offset = group * 4;
        u[offset..offset + 4].copy_from_slice(&u_words.to_le_bytes());
        v[offset..offset + 4].copy_from_slice(&v_words.to_le_bytes());
    }
    if samples % 2 != 0 {
        let column = groups * 2;
        let u_value = read_packed10_sample(src, column * 2);
        let v_value = read_packed10_sample(src, column * 2 + 1);
        u[column * 2..column * 2 + 2].copy_from_slice(&u_value.to_le_bytes());
        v[column * 2..column * 2 + 2].copy_from_slice(&v_value.to_le_bytes());
    }
}

fn packed10_word(src: &[u8]) -> u64 {
    u64::from(src[0])
        | u64::from(src[1]) << 8
        | u64::from(src[2]) << 16
        | u64::from(src[3]) << 24
        | u64::from(src[4]) << 32
}

fn read_packed10_sample(src: &[u8], sample: usize) -> u16 {
    let bit = sample * 10;
    let byte = bit / 8;
    let shift = bit % 8;
    let word = u32::from(src[byte])
        | u32::from(src.get(byte + 1).copied().unwrap_or(0)) << 8
        | u32::from(src.get(byte + 2).copied().unwrap_or(0)) << 16;
    ((word >> shift) & 0x03ff) as u16
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
    fn packed_nv12_removes_stride_without_reordering_chroma() {
        let (y, uv) = frames(8, 4, 16);
        let got = nv12_to_packed(&y, &uv, 8, 4, 16);
        let mut want = Vec::new();
        for row in 0..4 {
            want.extend_from_slice(&y[row * 16..row * 16 + 8]);
        }
        for row in 0..2 {
            want.extend_from_slice(&uv[row * 16..row * 16 + 8]);
        }
        assert_eq!(got, want);
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
    fn packed10_fast_path_matches_the_planar_sample_order() {
        for (width, height) in [(8usize, 2usize), (7, 3), (130, 6)] {
            let chroma_width = width.div_ceil(2);
            let chroma_height = height.div_ceil(2);
            let stride = (width * 10).div_ceil(8) + 5;
            let mut y = vec![0u8; stride * height];
            let mut uv = vec![0u8; stride * chroma_height];
            let mut y_samples = Vec::new();
            let mut uv_samples = Vec::new();

            for row in 0..height {
                let samples: Vec<u16> = (0..width)
                    .map(|column| ((row * width + column) * 7 & 0x03ff) as u16)
                    .collect();
                pack_packed10(&samples, &mut y[row * stride..]);
                y_samples.extend_from_slice(&samples);
            }
            for row in 0..chroma_height {
                let samples: Vec<u16> = (0..chroma_width * 2)
                    .map(|column| ((row * chroma_width + column) * 11 & 0x03ff) as u16)
                    .collect();
                pack_packed10(&samples, &mut uv[row * stride..]);
                uv_samples.push(samples);
            }

            let actual = packed10_nv12_to_planar(&y, &uv, width, height, stride);
            let mut expected = Vec::new();
            for sample in y_samples {
                expected.extend_from_slice(&sample.to_le_bytes());
            }
            for component in 0..2 {
                for row in &uv_samples {
                    for sample in row.iter().skip(component).step_by(2) {
                        expected.extend_from_slice(&sample.to_le_bytes());
                    }
                }
            }
            assert_eq!(actual, expected, "{width}x{height}");
        }
    }

    fn pack_packed10(samples: &[u16], row: &mut [u8]) {
        for (index, sample) in samples.iter().copied().enumerate() {
            let bit = index * 10;
            let byte = bit / 8;
            let shift = bit % 8;
            let word = u32::from(sample) << shift;
            row[byte] |= word as u8;
            row[byte + 1] |= (word >> 8) as u8;
            if shift > 6 {
                row[byte + 2] |= (word >> 16) as u8;
            }
        }
    }

    #[test]
    fn packs_even_and_odd_bytes() {
        let word = u64::from_le_bytes([1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(even_bytes(word).to_le_bytes(), [1, 3, 5, 7]);
        assert_eq!(odd_bytes(word).to_le_bytes(), [2, 4, 6, 8]);
    }
}
