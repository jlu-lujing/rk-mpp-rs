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
