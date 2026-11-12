//! Rockchip SoC identification and codec-block selection.
//!
//! The MPP kernel client numbers are shared across SoCs, but the userspace HAL
//! behind them is not. RK3588 drives VDPU34x/VEPU580 register files, while
//! RK3576 moves to VDPU383/VEPU510 and also has a different JPEG encoder and
//! VDPP generation. Keep that choice explicit before building a register job.

use std::fs;
use std::io;
use std::path::Path;

pub const COMPATIBLE_PATH: &str = "/proc/device-tree/compatible";
pub const MODEL_PATH: &str = "/proc/device-tree/model";
const VDEC_CCU_STATUS_PATH: &str = "/proc/device-tree/rkvdec-ccu/status";
const VENC_CCU_STATUS_PATH: &str = "/proc/device-tree/rkvenc-ccu/status";
const VDEC_CCU_PATHS: &[&str] = &[
    "/proc/device-tree/rkvdec-core@fdc48000/rockchip,ccu",
    "/proc/device-tree/rkvdec-core@fdc38100/rockchip,ccu",
];
const JPEG_CCU_PATHS: &[&str] = &[
    "/proc/device-tree/jpege-core@fdba0000/rockchip,ccu",
    "/proc/device-tree/jpege-core@fdba8000/rockchip,ccu",
];

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

/// Distributed-core handshake nodes advertised by the board device tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CcuTopology {
    pub video_decode: bool,
    pub video_encode: bool,
    pub jpeg_encode: bool,
}

impl CcuTopology {
    pub fn detect() -> Self {
        Self {
            video_decode: dt_status_enabled(Path::new(VDEC_CCU_STATUS_PATH))
                || VDEC_CCU_PATHS.iter().any(|path| Path::new(path).exists()),
            video_encode: dt_status_enabled(Path::new(VENC_CCU_STATUS_PATH)),
            jpeg_encode: JPEG_CCU_PATHS.iter().any(|path| Path::new(path).exists()),
        }
    }
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

pub fn require_video_post_process(expected: VideoPostProcessor) -> io::Result<SocProfile> {
    let profile = SocProfile::detect()?;
    if profile.video_post_processor != expected {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "this post-processor requires {:?}, but {:?} selects {:?}",
                expected, profile.id, profile.video_post_processor
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

fn dt_status_enabled(path: &Path) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    matches!(
        String::from_utf8_lossy(&bytes)
            .trim_end_matches('\0')
            .trim(),
        "okay" | "ok"
    )
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

    #[test]
    fn device_tree_status_accepts_only_enabled_nodes() {
        let root = std::env::temp_dir().join(format!("rk-mpp-dt-status-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let okay = root.join("okay");
        let disabled = root.join("disabled");
        std::fs::write(&okay, b"okay\0").unwrap();
        std::fs::write(&disabled, b"disabled\0").unwrap();
        assert!(dt_status_enabled(&okay));
        assert!(!dt_status_enabled(&disabled));
        assert!(!dt_status_enabled(&root.join("missing")));
        std::fs::remove_dir_all(root).unwrap();
    }
}
