//! Rockchip SoC identification and codec-block selection.
//!
//! The MPP kernel client numbers are shared across SoCs, but the userspace HAL
//! behind them is not. RK3588 drives VDPU34x/VEPU580 register files, while
//! RK3576 moves to VDPU383/VEPU510 and also has a different JPEG encoder and
//! VDPP generation. Keep that choice explicit before building a register job.

use std::fs;
use std::io;

pub const COMPATIBLE_PATH: &str = "/proc/device-tree/compatible";
pub const MODEL_PATH: &str = "/proc/device-tree/model";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocId {
    Rk3588,
    Rk3576,
    Rk3568,
    Rk3566,
    Rk3562,
    Rk3528,
    Rk3572,
    Rv1126B,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoDecodeHal {
    Vdpu34x,
    Vdpu383,
    Vdpu382,
    Vdpu384a,
    Vdpu384b,
    Vdpu2,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoEncodeHal {
    Vepu580,
    Vepu510,
    Vepu511,
    Vepu540c,
    None,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JpegEncodeHal {
    Vepu2,
    Vpu720,
    None,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoPostProcessor {
    Vdpp,
    Vdpp2,
    None,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocProfile {
    pub id: SocId,
    pub compatible: String,
    pub model: String,
    pub video_decode: VideoDecodeHal,
    pub video_encode: VideoEncodeHal,
    pub jpeg_encode: JpegEncodeHal,
    pub video_post_processor: VideoPostProcessor,
}

impl SocProfile {
    pub fn detect() -> io::Result<Self> {
        let compatible = read_dt_string(COMPATIBLE_PATH)?;
        let model = read_dt_string(MODEL_PATH)
            .unwrap_or_default()
            .trim_end_matches('\0')
            .to_owned();
        Ok(Self::from_device_tree(&compatible, &model))
    }

    pub fn from_device_tree(compatible: &str, model: &str) -> Self {
        let id = classify_compatible(compatible);
        Self {
            id,
            compatible: compatible.to_owned(),
            model: model.to_owned(),
            ..profile_for(id)
        }
    }

    pub fn compatible_tokens(&self) -> impl Iterator<Item = &str> {
        self.compatible
            .split('\0')
            .map(str::trim)
            .filter(|token| !token.is_empty())
    }
}

pub fn require_video_decode(expected: VideoDecodeHal) -> io::Result<SocProfile> {
    let profile = SocProfile::detect()?;
    if profile.video_decode != expected {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "this HAL requires {:?}, but {:?} selects {:?}",
                expected, profile.id, profile.video_decode
            ),
        ));
    }
    Ok(profile)
}

pub fn require_video_encode(expected: VideoEncodeHal) -> Result<SocProfile, String> {
    let profile = SocProfile::detect().map_err(|error| format!("detect SoC: {error}"))?;
    if profile.video_encode != expected {
        return Err(format!(
            "this encoder requires {:?}, but {:?} selects {:?}",
            expected, profile.id, profile.video_encode
        ));
    }
    Ok(profile)
}

pub fn require_jpeg_encode(expected: JpegEncodeHal) -> io::Result<SocProfile> {
    let profile = SocProfile::detect()?;
    if profile.jpeg_encode != expected {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "this JPEG encoder requires {:?}, but {:?} selects {:?}",
                expected, profile.id, profile.jpeg_encode
            ),
        ));
    }
    Ok(profile)
}

fn profile_for(id: SocId) -> SocProfile {
    let (video_decode, video_encode, jpeg_encode, video_post_processor) = match id {
        SocId::Rk3588 => (
            VideoDecodeHal::Vdpu34x,
            VideoEncodeHal::Vepu580,
            JpegEncodeHal::Vepu2,
            VideoPostProcessor::None,
        ),
        SocId::Rk3576 => (
            VideoDecodeHal::Vdpu383,
            VideoEncodeHal::Vepu510,
            JpegEncodeHal::Vpu720,
            VideoPostProcessor::Vdpp2,
        ),
        SocId::Rk3568 | SocId::Rk3566 => (
            VideoDecodeHal::Vdpu382,
            VideoEncodeHal::Vepu540c,
            JpegEncodeHal::Vepu2,
            VideoPostProcessor::Vdpp,
        ),
        SocId::Rk3562 => (
            VideoDecodeHal::Vdpu382,
            VideoEncodeHal::Vepu540c,
            JpegEncodeHal::None,
            VideoPostProcessor::Vdpp,
        ),
        SocId::Rk3528 => (
            VideoDecodeHal::Vdpu382,
            VideoEncodeHal::Vepu540c,
            JpegEncodeHal::None,
            VideoPostProcessor::Vdpp,
        ),
        SocId::Rk3572 => (
            VideoDecodeHal::Vdpu384b,
            VideoEncodeHal::Vepu511,
            JpegEncodeHal::Vpu720,
            VideoPostProcessor::Vdpp2,
        ),
        SocId::Rv1126B => (
            VideoDecodeHal::Vdpu384a,
            VideoEncodeHal::Vepu511,
            JpegEncodeHal::None,
            VideoPostProcessor::Vdpp,
        ),
        SocId::Unknown => (
            VideoDecodeHal::Unknown,
            VideoEncodeHal::Unknown,
            JpegEncodeHal::Unknown,
            VideoPostProcessor::Unknown,
        ),
    };
    SocProfile {
        id,
        compatible: String::new(),
        model: String::new(),
        video_decode,
        video_encode,
        jpeg_encode,
        video_post_processor,
    }
}

fn classify_compatible(compatible: &str) -> SocId {
    for token in compatible.split('\0') {
        let token = token.trim();
        let id = match token {
            "rockchip,rk3588" | "rockchip,rk3588s" => SocId::Rk3588,
            "rockchip,rk3576" => SocId::Rk3576,
            "rockchip,rk3568" => SocId::Rk3568,
            "rockchip,rk3566" => SocId::Rk3566,
            "rockchip,rk3562" => SocId::Rk3562,
            "rockchip,rk3528" => SocId::Rk3528,
            "rockchip,rk3572" => SocId::Rk3572,
            "rockchip,rv1126b" => SocId::Rv1126B,
            _ => continue,
        };
        return id;
    }
    SocId::Unknown
}

fn read_dt_string(path: &str) -> io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rk3576_selects_vdpu383_and_vepu510() {
        let profile =
            SocProfile::from_device_tree("luckfox,omni3576\0rockchip,rk3576\0", "Omni3576");
        assert_eq!(profile.id, SocId::Rk3576);
        assert_eq!(profile.video_decode, VideoDecodeHal::Vdpu383);
        assert_eq!(profile.video_encode, VideoEncodeHal::Vepu510);
        assert_eq!(profile.jpeg_encode, JpegEncodeHal::Vpu720);
        assert_eq!(profile.video_post_processor, VideoPostProcessor::Vdpp2);
    }

    #[test]
    fn rk3588_keeps_the_existing_hal_family() {
        let profile = SocProfile::from_device_tree("rockchip,rk3588\0", "NanoPC-T6");
        assert_eq!(profile.id, SocId::Rk3588);
        assert_eq!(profile.video_decode, VideoDecodeHal::Vdpu34x);
        assert_eq!(profile.video_encode, VideoEncodeHal::Vepu580);
        assert_eq!(profile.jpeg_encode, JpegEncodeHal::Vepu2);
        assert_eq!(profile.video_post_processor, VideoPostProcessor::None);
    }

    #[test]
    fn compatible_tokens_ignore_empty_device_tree_entries() {
        let profile = SocProfile::from_device_tree("board,model\0\0rockchip,rk3576\0", "");
        assert_eq!(
            profile.compatible_tokens().collect::<Vec<_>>(),
            vec!["board,model", "rockchip,rk3576"]
        );
    }

    #[test]
    fn unknown_compatible_stays_unknown() {
        let profile = SocProfile::from_device_tree("vendor,mystery\0", "");
        assert_eq!(profile.id, SocId::Unknown);
        assert_eq!(profile.video_decode, VideoDecodeHal::Unknown);
    }
}
