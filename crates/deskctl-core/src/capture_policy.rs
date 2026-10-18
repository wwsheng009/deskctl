//! Screenshot output policy: format, compression, resizing and size limits.

use std::path::{Path, PathBuf};

use deskctl_protocol::{ErrorCode, ToolError};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, RgbaImage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Png,
    Jpeg,
    Webp,
}

impl OutputFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Webp => "webp",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }

    pub fn parse(value: &str) -> Result<Self, ToolError> {
        match value.to_ascii_lowercase().as_str() {
            "png" => Ok(Self::Png),
            "jpeg" | "jpg" => Ok(Self::Jpeg),
            "webp" => Ok(Self::Webp),
            other => Err(ToolError::new(
                ErrorCode::InvalidParams,
                format!("unsupported image format '{other}'; expected png, jpeg or webp"),
            )),
        }
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CapturePolicy {
    /// Default output format when the caller does not pick one.
    pub format: OutputFormat,
    /// JPEG/WebP quality, 1..=100. PNG ignores this.
    pub quality: u8,
    /// PNG compression level. 0 = fast, 9 = smallest.
    pub png_compression: u8,
    /// Optional maximum width; larger images are downscaled.
    pub max_width: Option<u32>,
    /// Optional maximum height; larger images are downscaled.
    pub max_height: Option<u32>,
    /// Hard cap on the encoded file size. The encoder reduces quality or
    /// dimensions until the file fits, or fails with a clear error.
    pub max_bytes: Option<usize>,
    /// Preserve the source resolution when possible.
    pub allow_upscale: bool,
}

impl Default for CapturePolicy {
    fn default() -> Self {
        Self {
            format: OutputFormat::Png,
            quality: 85,
            png_compression: 6,
            max_width: None,
            max_height: None,
            max_bytes: None,
            allow_upscale: false,
        }
    }
}

impl CapturePolicy {
    /// Build a policy from the tool arguments, falling back to daemon defaults.
    pub fn from_args(
        args: &serde_json::Value,
        defaults: &CapturePolicy,
    ) -> Result<Self, ToolError> {
        let mut policy = defaults.clone();
        if let Some(format) = args.get("format").and_then(|value| value.as_str()) {
            policy.format = OutputFormat::parse(format)?;
        }
        if let Some(quality) = args.get("quality").and_then(|value| value.as_u64()) {
            if !(1..=100).contains(&quality) {
                return Err(ToolError::new(
                    ErrorCode::InvalidParams,
                    "quality must be between 1 and 100",
                ));
            }
            policy.quality = quality as u8;
        }
        if let Some(max_width) = args.get("max_width").and_then(|value| value.as_u64()) {
            policy.max_width = Some(max_width.clamp(1, 16_384) as u32);
        }
        if let Some(max_height) = args.get("max_height").and_then(|value| value.as_u64()) {
            policy.max_height = Some(max_height.clamp(1, 16_384) as u32);
        }
        if let Some(max_bytes) = args.get("max_bytes").and_then(|value| value.as_u64()) {
            policy.max_bytes = Some(max_bytes.clamp(1024, 64 * 1024 * 1024) as usize);
        }
        Ok(policy)
    }
}

pub struct EncodedImage {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    pub format: OutputFormat,
    pub resized: bool,
    pub quality: u8,
}

pub fn encode_and_save(
    image: &RgbaImage,
    requested_path: Option<&str>,
    default_dir: &Path,
    policy: &CapturePolicy,
) -> Result<EncodedImage, ToolError> {
    encode_and_save_with_format(image, requested_path, default_dir, policy, None)
}

pub fn encode_and_save_with_format(
    image: &RgbaImage,
    requested_path: Option<&str>,
    default_dir: &Path,
    policy: &CapturePolicy,
    requested_format: Option<&str>,
) -> Result<EncodedImage, ToolError> {
    if requested_path.is_some_and(|value| value.contains('\0')) {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            "path contains a NUL byte",
        ));
    }

    let requested = requested_path
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let format = match requested_format {
        Some(format) => OutputFormat::parse(format)?,
        None => requested
            .as_deref()
            .and_then(OutputFormat::from_path)
            .unwrap_or(policy.format),
    };
    let mut path = match requested {
        Some(path) if path.is_absolute() => path,
        Some(path) => std::env::current_dir()
            .map_err(|error| {
                ToolError::new(ErrorCode::Internal, format!("resolve current dir: {error}"))
            })?
            .join(path),
        None => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            default_dir.join(format!(
                "capture-{stamp}-{}.{}",
                uuid::Uuid::new_v4().simple(),
                format.extension()
            ))
        }
    };
    path.set_extension(format.extension());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut width = image.width();
    let mut height = image.height();
    let mut work = image.clone();
    let mut resized = false;
    if let Some((new_width, new_height)) = target_size(
        width,
        height,
        policy.max_width,
        policy.max_height,
        policy.allow_upscale,
    ) {
        work = image::imageops::resize(
            &work,
            new_width,
            new_height,
            image::imageops::FilterType::Lanczos3,
        );
        width = new_width;
        height = new_height;
        resized = true;
    }

    let mut quality = policy.quality;
    let mut encoded = encode(&work, format, quality, policy.png_compression)?;
    if let Some(limit) = policy.max_bytes {
        let mut attempts = 0;
        while encoded.len() > limit && attempts < 6 {
            attempts += 1;
            match format {
                OutputFormat::Png => {
                    if let Some((new_width, new_height)) =
                        target_size(width, height, Some(width / 2), Some(height / 2), false)
                    {
                        work = image::imageops::resize(
                            &work,
                            new_width,
                            new_height,
                            image::imageops::FilterType::Lanczos3,
                        );
                        width = new_width;
                        height = new_height;
                        resized = true;
                        encoded = encode(&work, format, quality, policy.png_compression)?;
                    } else {
                        break;
                    }
                }
                OutputFormat::Jpeg | OutputFormat::Webp => {
                    if quality > 40 {
                        quality = quality.saturating_sub(15).max(40);
                        encoded = encode(&work, format, quality, policy.png_compression)?;
                    } else if let Some((new_width, new_height)) = target_size(
                        width,
                        height,
                        Some(width * 3 / 4),
                        Some(height * 3 / 4),
                        false,
                    ) {
                        work = image::imageops::resize(
                            &work,
                            new_width,
                            new_height,
                            image::imageops::FilterType::Lanczos3,
                        );
                        width = new_width;
                        height = new_height;
                        resized = true;
                        encoded = encode(&work, format, quality, policy.png_compression)?;
                    } else {
                        break;
                    }
                }
            }
        }
        if encoded.len() > limit {
            return Err(ToolError::new(
                ErrorCode::InvalidParams,
                format!(
                    "encoded screenshot is {} bytes, above max_bytes={limit}; lower quality or dimensions",
                    encoded.len()
                ),
            ));
        }
    }

    std::fs::write(&path, &encoded).map_err(|error| {
        ToolError::new(
            ErrorCode::Internal,
            format!("failed to write {}: {error}", path.display()),
        )
    })?;
    let bytes = std::fs::metadata(&path)
        .map_err(|error| {
            ToolError::new(
                ErrorCode::Internal,
                format!("failed to verify screenshot output: {error}"),
            )
        })?
        .len() as usize;
    Ok(EncodedImage {
        path,
        width,
        height,
        bytes,
        format,
        resized,
        quality: if format == OutputFormat::Webp {
            0
        } else {
            quality
        },
    })
}

fn target_size(
    width: u32,
    height: u32,
    max_width: Option<u32>,
    max_height: Option<u32>,
    allow_upscale: bool,
) -> Option<(u32, u32)> {
    let mut scale = 1.0f64;
    if let Some(limit) = max_width {
        if width > limit {
            scale = scale.min(limit as f64 / width as f64);
        }
    }
    if let Some(limit) = max_height {
        if height > limit {
            scale = scale.min(limit as f64 / height as f64);
        }
    }
    if scale < 1.0 {
        let new_width = ((width as f64 * scale).round() as u32).max(1);
        let new_height = ((height as f64 * scale).round() as u32).max(1);
        return Some((new_width, new_height));
    }
    if allow_upscale && scale > 1.0 {
        return Some((
            (width as f64 * scale).round() as u32,
            (height as f64 * scale).round() as u32,
        ));
    }
    None
}

fn encode(
    image: &RgbaImage,
    format: OutputFormat,
    quality: u8,
    png_compression: u8,
) -> Result<Vec<u8>, ToolError> {
    let mut bytes = Vec::new();
    match format {
        OutputFormat::Png => {
            let compression = match png_compression {
                0..=2 => CompressionType::Fast,
                3..=6 => CompressionType::Default,
                _ => CompressionType::Best,
            };
            let encoder =
                PngEncoder::new_with_quality(&mut bytes, compression, FilterType::Adaptive);
            encoder
                .write_image(
                    image.as_raw(),
                    image.width(),
                    image.height(),
                    ExtendedColorType::Rgba8,
                )
                .map_err(|error| {
                    ToolError::new(ErrorCode::Internal, format!("PNG encode failed: {error}"))
                })?;
        }
        OutputFormat::Jpeg => {
            let rgb = image::DynamicImage::ImageRgba8(image.clone()).to_rgb8();
            let encoder = JpegEncoder::new_with_quality(&mut bytes, quality);
            encoder
                .write_image(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    ExtendedColorType::Rgb8,
                )
                .map_err(|error| {
                    ToolError::new(ErrorCode::Internal, format!("JPEG encode failed: {error}"))
                })?;
        }
        OutputFormat::Webp => {
            let rgb = image::DynamicImage::ImageRgba8(image.clone()).to_rgb8();
            // `image` 0.25 only exposes lossless WebP encoding. Quality is
            // accepted for protocol compatibility but does not change output.
            image::codecs::webp::WebPEncoder::new_lossless(&mut bytes)
                .write_image(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    ExtendedColorType::Rgb8,
                )
                .map_err(|error| {
                    ToolError::new(ErrorCode::Internal, format!("WebP encode failed: {error}"))
                })?;
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> RgbaImage {
        RgbaImage::from_fn(640, 480, |x, y| {
            image::Rgba([(x % 255) as u8, (y % 255) as u8, 120, 255])
        })
    }

    fn output_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("deskctl-capture-policy");
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn default_format_is_png_and_extension_selects_format() {
        let dir = output_dir();
        let policy = CapturePolicy::default();
        let output = encode_and_save(&image(), Some("shot-default"), &dir, &policy).unwrap();
        assert_eq!(output.format, OutputFormat::Png);
        assert_eq!(
            output.path.extension().and_then(|v| v.to_str()),
            Some("png")
        );
        assert!(output.bytes > 0);

        let jpeg = encode_and_save(&image(), Some("shot.jpg"), &dir, &policy).unwrap();
        assert_eq!(jpeg.format, OutputFormat::Jpeg);
        assert_eq!(jpeg.path.extension().and_then(|v| v.to_str()), Some("jpg"));
    }

    #[test]
    fn jpeg_quality_reduces_size() {
        let dir = output_dir();
        let low = CapturePolicy {
            format: OutputFormat::Jpeg,
            quality: 30,
            ..Default::default()
        };
        let high = CapturePolicy {
            format: OutputFormat::Jpeg,
            quality: 95,
            ..Default::default()
        };
        let low = encode_and_save(&image(), Some("low.jpg"), &dir, &low).unwrap();
        let high = encode_and_save(&image(), Some("high.jpg"), &dir, &high).unwrap();
        assert!(low.bytes < high.bytes);
    }

    #[test]
    fn max_dimensions_downscale() {
        let dir = output_dir();
        let policy = CapturePolicy {
            max_width: Some(320),
            max_height: Some(240),
            ..Default::default()
        };
        let output = encode_and_save(&image(), Some("scaled.png"), &dir, &policy).unwrap();
        assert_eq!((output.width, output.height), (320, 240));
        assert!(output.resized);
    }

    #[test]
    fn max_bytes_is_enforced_or_rejected() {
        let dir = output_dir();
        let policy = CapturePolicy {
            format: OutputFormat::Jpeg,
            quality: 95,
            max_bytes: Some(2048),
            ..Default::default()
        };
        match encode_and_save(&image(), Some("limited.jpg"), &dir, &policy) {
            Ok(output) => assert!(output.bytes <= 2048),
            Err(error) => assert_eq!(error.code, ErrorCode::InvalidParams),
        }
    }

    #[test]
    fn invalid_format_is_rejected() {
        let args = serde_json::json!({ "format": "bmp" });
        let error = CapturePolicy::from_args(&args, &CapturePolicy::default()).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidParams);
    }
}
