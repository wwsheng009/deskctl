use std::path::{Path, PathBuf};

use deskctl_protocol::{ErrorCode, ToolError};
use image::RgbaImage;
use uuid::Uuid;
use xcap::{Monitor, Window};

use crate::capture_policy::{encode_and_save, CapturePolicy, EncodedImage};
use crate::session::ScreenRect;

#[derive(Debug, Clone)]
pub struct CapturedImage {
    pub image: RgbaImage,
    pub screen_rect: ScreenRect,
    pub foreground_hwnd: Option<i64>,
}

pub fn screen_size() -> Result<(u32, u32), ToolError> {
    let monitor = primary_monitor()?;
    let width = monitor
        .width()
        .map_err(|error| capture_error(error.to_string()))?;
    let height = monitor
        .height()
        .map_err(|error| capture_error(error.to_string()))?;
    Ok((width, height))
}

pub fn capture_screen(region: Option<ScreenRect>) -> Result<CapturedImage, ToolError> {
    let monitor = primary_monitor()?;
    let mx = monitor.x().map_err(|e| capture_error(e.to_string()))?;
    let my = monitor.y().map_err(|e| capture_error(e.to_string()))?;
    let mw = monitor.width().map_err(|e| capture_error(e.to_string()))?;
    let mh = monitor.height().map_err(|e| capture_error(e.to_string()))?;
    let image = monitor
        .capture_image()
        .map_err(|e| capture_error(e.to_string()))?;
    let rect = region.unwrap_or(ScreenRect {
        x: mx,
        y: my,
        width: mw,
        height: mh,
    });
    if rect.width == 0 || rect.height == 0 {
        return Err(invalid("region width and height must be positive"));
    }
    let crop_x = rect.x.saturating_sub(mx);
    let crop_y = rect.y.saturating_sub(my);
    if crop_x < 0
        || crop_y < 0
        || crop_x as u32 >= mw
        || crop_y as u32 >= mh
        || crop_x as u32 + rect.width > mw
        || crop_y as u32 + rect.height > mh
    {
        return Err(invalid("requested region is outside the primary monitor"));
    }
    let cropped = image::imageops::crop_imm(
        &image,
        crop_x as u32,
        crop_y as u32,
        rect.width,
        rect.height,
    )
    .to_image();
    Ok(CapturedImage {
        image: cropped,
        screen_rect: rect,
        foreground_hwnd: foreground_hwnd().ok().flatten(),
    })
}

pub fn capture_window(hwnd: i64) -> Result<CapturedImage, ToolError> {
    let hwnd_raw =
        isize::try_from(hwnd).map_err(|_| invalid("hwnd is outside the platform handle range"))?;
    let before = crate::platform::windows::window::window_info(hwnd)?;
    let before_rect = rect_from_info(&before)?;
    let window = Window::all()
        .map_err(|e| capture_error(e.to_string()))?
        .into_iter()
        .find(|window| window.id().is_ok_and(|id| id as isize == hwnd_raw))
        .ok_or_else(|| {
            ToolError::new(
                ErrorCode::TargetUnavailable,
                format!("window {hwnd} is unavailable"),
            )
            .retryable(true)
        })?;
    let image = window
        .capture_image()
        .map_err(|e| capture_error(e.to_string()))?;
    let after = crate::platform::windows::window::window_info(hwnd)?;
    let after_rect = rect_from_info(&after)?;
    if before_rect.x != after_rect.x
        || before_rect.y != after_rect.y
        || before_rect.width != after_rect.width
        || before_rect.height != after_rect.height
    {
        return Err(capture_error(
            "window changed while it was being captured; retry with a fresh observation",
        ));
    }
    let width = image.width();
    let height = image.height();
    Ok(CapturedImage {
        image,
        screen_rect: ScreenRect {
            x: after_rect.x,
            y: after_rect.y,
            width,
            height,
        },
        foreground_hwnd: foreground_hwnd().ok().flatten(),
    })
}

pub fn foreground_hwnd() -> Result<Option<i64>, ToolError> {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let hwnd = unsafe { GetForegroundWindow() };
    Ok((hwnd.0 != 0).then_some(hwnd.0 as i64))
}

pub fn save_capture(
    image: &RgbaImage,
    path: Option<&str>,
    default_dir: &Path,
    policy: &CapturePolicy,
) -> Result<EncodedImage, ToolError> {
    encode_and_save(image, path, default_dir, policy)
}

pub fn normalize_png_path(path: Option<&str>, default_dir: &Path) -> Result<PathBuf, ToolError> {
    if path.is_some_and(|value| value.contains('\0')) {
        return Err(invalid("path contains a NUL byte"));
    }
    let path = match path {
        Some(value) if !value.trim().is_empty() => PathBuf::from(value),
        _ => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            default_dir.join(format!("capture-{stamp}-{}.png", Uuid::new_v4().simple()))
        }
    };
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(match path.extension().and_then(|value| value.to_str()) {
        Some(extension)
            if matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp"
            ) =>
        {
            path
        }
        _ => path.with_extension("png"),
    })
}

fn primary_monitor() -> Result<Monitor, ToolError> {
    let monitors = Monitor::all().map_err(|e| capture_error(e.to_string()))?;
    monitors
        .into_iter()
        .find(|monitor| monitor.is_primary().unwrap_or(false))
        .ok_or_else(|| capture_error("no primary monitor available"))
}

fn rect_from_info(value: &serde_json::Value) -> Result<ScreenRect, ToolError> {
    Ok(ScreenRect {
        x: value["x"]
            .as_i64()
            .and_then(|v| i32::try_from(v).ok())
            .ok_or_else(|| invalid("window x is unavailable"))?,
        y: value["y"]
            .as_i64()
            .and_then(|v| i32::try_from(v).ok())
            .ok_or_else(|| invalid("window y is unavailable"))?,
        width: value["width"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| invalid("window width is unavailable"))?,
        height: value["height"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| invalid("window height is unavailable"))?,
    })
}

fn capture_error(message: impl Into<String>) -> ToolError {
    ToolError::new(ErrorCode::PermissionDenied, message).retryable(true)
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::new(ErrorCode::InvalidParams, message).retryable(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_path_keeps_a_recognized_extension() {
        let dir = std::env::temp_dir().join("deskctl-path-test");
        let path = normalize_png_path(Some("foo.jpg"), &dir).unwrap();
        assert_eq!(path.extension().and_then(|v| v.to_str()), Some("jpg"));
    }

    #[test]
    fn empty_path_uses_default_directory() {
        let dir = std::env::temp_dir().join("deskctl-default-test");
        let path = normalize_png_path(Some("  "), &dir).unwrap();
        assert!(path.starts_with(&dir));
        assert_eq!(path.extension().and_then(|v| v.to_str()), Some("png"));
    }
}
