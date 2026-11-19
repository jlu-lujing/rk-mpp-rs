//! Hardware abstraction layers: one module per SoC codec block.
//!
//! `jpegd` drives `rockchip,rkv-jpeg-decoder-v1`. The video blocks of the
//! official MPP (vdpu1/vdpu2/rkvdec/av1d/venc/vepu) are the roadmap.

/// The largest coded picture the RK3588 decoder blocks can be programmed with.
/// A stream that claims more is refused before any register write or buffer
/// allocation - the 2024-04 corpus sweep lost the board on an untrusted stream,
/// and geometry is the cheapest thing to validate first.
pub const MAX_DECODE_DIMENSION: usize = 8192;

/// Validate the coded size a parsed stream asks the hardware to produce.
pub fn check_decode_geometry(width: usize, height: usize) -> std::io::Result<()> {
    if width == 0 || height == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "decoded picture has a zero dimension",
        ));
    }
    if width > MAX_DECODE_DIMENSION || height > MAX_DECODE_DIMENSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("decoded picture {width}x{height} exceeds the {MAX_DECODE_DIMENSION} limit"),
        ));
    }
    Ok(())
}

pub(crate) fn copy_visible_nv12_to_padded(
    destination: &mut [u8],
    source: &[u8],
    width: usize,
    height: usize,
    coded_width: usize,
    coded_height: usize,
    initialize_padding: bool,
) -> Result<(), String> {
    if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
        return Err("visible NV12 geometry must be non-zero and even".to_string());
    }
    if coded_width < width || coded_height < height {
        return Err(format!(
            "coded NV12 geometry {coded_width}x{coded_height} is smaller than {width}x{height}"
        ));
    }
    let visible_luma = width
        .checked_mul(height)
        .ok_or_else(|| "visible NV12 size overflow".to_string())?;
    let visible_size = visible_luma
        .checked_mul(3)
        .map(|bytes| bytes / 2)
        .ok_or_else(|| "visible NV12 size overflow".to_string())?;
    if source.len() < visible_size {
        return Err(format!(
            "visible NV12 source is {} bytes, needs {visible_size}",
            source.len()
        ));
    }
    let coded_luma = coded_width
        .checked_mul(coded_height)
        .ok_or_else(|| "coded NV12 size overflow".to_string())?;
    let coded_size = coded_luma
        .checked_mul(3)
        .map(|bytes| bytes / 2)
        .ok_or_else(|| "coded NV12 size overflow".to_string())?;
    if destination.len() < coded_size {
        return Err(format!(
            "coded NV12 destination is {} bytes, needs {coded_size}",
            destination.len()
        ));
    }

    if initialize_padding {
        destination[..coded_size].fill(0);
    }
    for row in 0..height {
        let source_start = row * width;
        let destination_start = row * coded_width;
        destination[destination_start..destination_start + width]
            .copy_from_slice(&source[source_start..source_start + width]);
    }
    for row in 0..height / 2 {
        let source_start = visible_luma + row * width;
        let destination_start = coded_luma + row * coded_width;
        destination[destination_start..destination_start + width]
            .copy_from_slice(&source[source_start..source_start + width]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_beyond_the_hardware_limit_is_refused() {
        assert!(check_decode_geometry(8192, 8192).is_ok());
        assert!(check_decode_geometry(0, 240).is_err());
        assert!(check_decode_geometry(8193, 240).is_err());
        assert!(check_decode_geometry(320, 8193).is_err());
        let err = check_decode_geometry(65535, 2).unwrap_err();
        assert!(err.to_string().contains("8192"));
    }

    #[test]
    fn visible_nv12_is_copied_without_disturbing_the_padding_after_init() {
        let mut source = vec![0x11; 4 * 4];
        source.extend([0x22; 4 * 2]);
        let mut coded = vec![0xff; 16 * 16 * 3 / 2];
        copy_visible_nv12_to_padded(&mut coded, &source, 4, 4, 16, 16, true).unwrap();
        for row in 0..4 {
            assert_eq!(&coded[row * 16..row * 16 + 4], &[0x11; 4]);
            assert_eq!(&coded[row * 16 + 4..(row + 1) * 16], &[0; 12]);
        }
        for row in 0..2 {
            let offset = 16 * 16 + row * 16;
            assert_eq!(&coded[offset..offset + 4], &[0x22; 4]);
            assert_eq!(&coded[offset + 4..offset + 16], &[0; 12]);
        }
        assert!(coded[4 * 16..16 * 16].iter().all(|value| *value == 0));
        assert!(coded[16 * 16 + 2 * 16..].iter().all(|value| *value == 0));

        let replacement = vec![0x33; source.len()];
        copy_visible_nv12_to_padded(&mut coded, &replacement, 4, 4, 16, 16, false).unwrap();
        assert_eq!(&coded[12..16], &[0; 4], "padding stays zero on reuse");
    }
}

pub mod av1d;
pub mod av1d383;
pub mod h263d;
pub mod h264d;
pub mod h264d383;
pub mod h264e;
mod h264e_vepu580_template;
pub mod h265e;
pub mod h265e_regs;
pub mod h265e_session;
pub mod h265e_vepu580_template;
pub mod hevc;
pub mod hevc383;
pub mod iep;
pub mod iep_pd;
pub mod jpegd;
pub mod jpege;
pub mod jpege720;
pub mod jpege_tables;
pub mod m2vd;
pub mod mpg4d;
pub mod osd;
pub mod vdpp2;
mod vdpp2_coeff;
mod vdpp2_defaults;
mod vdpp2_enhance;
mod vdpp2_sharp;
pub mod vdpu2;
pub mod vdpu34x_regs;
pub mod vdpu383_regs;
pub mod vepu510;
pub mod vepu510_h264;
pub mod vepu510_h265;
#[allow(dead_code)]
mod vepu510_h265_idr_template;
#[allow(dead_code)]
mod vepu510_h265_mpi_idr_template;
#[allow(dead_code)]
mod vepu510_h265_mpi_p_template;
#[allow(dead_code)]
mod vepu510_h265_p_template;
pub mod vepu510_h265_regs;
pub mod vepu510_high_idr_template;
pub mod vepu510_high_p_template;
pub mod vepu510_idr_template;
pub mod vepu510_p_template;
pub mod vepu510_regs;
pub mod vepu580_regs;
pub mod vp8d;
pub mod vp9d;
