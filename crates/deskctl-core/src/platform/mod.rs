//! Platform abstraction. Windows is the only implementation in the first release.

#[cfg(windows)]
pub mod windows;

pub use self::imp::*;

#[cfg(windows)]
mod imp {
    pub use super::windows::capture::{
        capture_screen, capture_window, foreground_hwnd, save_capture, save_capture_with_format,
        screen_size, CapturedImage,
    };
    pub use super::windows::clipboard::{clean as clipboard_clean, write_text as clipboard_write};
    pub use super::windows::dpi::{diagnostics as dpi_diagnostics, init_process_dpi_awareness};
    pub use super::windows::input::{
        drag, hotkey, is_valid_key, key_to_vk, mouse_click, mouse_move, mouse_position,
        mouse_scroll, type_text,
    };
    pub use super::windows::uia::{observe_window, perform_action, UiNode, UiaAction};
    pub use super::windows::window::{
        activate_window, find_window_by_title, move_window, resize_window, window_info,
        windows_list,
    };
}

#[cfg(not(windows))]
mod imp {
    use crate::session::ScreenRect;
    use deskctl_protocol::{ErrorCode, ToolError};

    #[derive(Debug, Clone)]
    pub struct CapturedImage {
        pub image: image::RgbaImage,
        pub screen_rect: ScreenRect,
        pub foreground_hwnd: Option<i64>,
    }

    fn unsupported() -> ToolError {
        ToolError::new(
            ErrorCode::PlatformNotSupported,
            "desktop automation requires Windows 10/11",
        )
        .retryable(false)
    }

    pub fn init_process_dpi_awareness() -> &'static str {
        "unsupported"
    }

    pub fn dpi_diagnostics() -> serde_json::Value {
        serde_json::json!({ "awareness": "unsupported" })
    }

    pub fn capture_screen(_region: Option<ScreenRect>) -> Result<CapturedImage, ToolError> {
        Err(unsupported())
    }
    pub fn capture_window(_hwnd: i64) -> Result<CapturedImage, ToolError> {
        Err(unsupported())
    }
    pub fn screen_size() -> Result<(u32, u32), ToolError> {
        Err(unsupported())
    }
    pub fn foreground_hwnd() -> Result<Option<i64>, ToolError> {
        Err(unsupported())
    }
    pub fn save_capture(
        _image: &image::RgbaImage,
        _path: Option<&str>,
        _default_dir: &std::path::Path,
        _policy: &crate::capture_policy::CapturePolicy,
    ) -> Result<crate::capture_policy::EncodedImage, ToolError> {
        Err(unsupported())
    }
    pub fn save_capture_with_format(
        _image: &image::RgbaImage,
        _path: Option<&str>,
        _default_dir: &std::path::Path,
        _policy: &crate::capture_policy::CapturePolicy,
        _format: Option<&str>,
    ) -> Result<crate::capture_policy::EncodedImage, ToolError> {
        Err(unsupported())
    }
    pub fn windows_list() -> Result<Vec<serde_json::Value>, ToolError> {
        Err(unsupported())
    }
    pub fn window_info(_hwnd: i64) -> Result<serde_json::Value, ToolError> {
        Err(unsupported())
    }
    pub fn activate_window(_hwnd: i64) -> Result<bool, ToolError> {
        Err(unsupported())
    }
    pub fn move_window(_hwnd: i64, _x: i32, _y: i32) -> Result<(), ToolError> {
        Err(unsupported())
    }
    pub fn resize_window(_hwnd: i64, _w: u32, _h: u32) -> Result<(), ToolError> {
        Err(unsupported())
    }
    pub fn find_window_by_title(_title: &str) -> Result<Option<i64>, ToolError> {
        Err(unsupported())
    }
    pub fn mouse_position() -> Result<(i32, i32), ToolError> {
        Err(unsupported())
    }
    pub fn mouse_move(_x: i32, _y: i32) -> Result<(), ToolError> {
        Err(unsupported())
    }
    pub fn mouse_click(_x: i32, _y: i32, _button: &str, _clicks: u8) -> Result<(), ToolError> {
        Err(unsupported())
    }
    pub fn mouse_scroll(_direction: &str, _amount: u32) -> Result<(), ToolError> {
        Err(unsupported())
    }
    pub fn drag(_start: (i32, i32), _end: (i32, i32), _duration_ms: u64) -> Result<(), ToolError> {
        Err(unsupported())
    }
    pub fn type_text(_text: &str) -> Result<(), ToolError> {
        Err(unsupported())
    }
    pub fn hotkey(_keys: &[&str]) -> Result<(), ToolError> {
        Err(unsupported())
    }
    pub fn key_to_vk(_key: &str) -> Result<u16, ToolError> {
        Err(unsupported())
    }
    pub fn is_valid_key(_key: &str) -> bool {
        false
    }
    pub fn clipboard_write(_text: &str) -> Result<(), ToolError> {
        Err(unsupported())
    }
    pub fn clipboard_clean() -> Result<(), ToolError> {
        Err(unsupported())
    }

    #[derive(Debug, Clone)]
    pub struct UiNode {
        pub value: serde_json::Value,
    }
    #[derive(Debug, Clone, Copy)]
    pub enum UiaAction {
        Invoke,
        Toggle,
        Select,
        Expand,
        Collapse,
        Focus,
        SetValue,
    }
    pub fn observe_window(_hwnd: i64, _limit: usize) -> Result<Vec<UiNode>, ToolError> {
        Err(unsupported())
    }
    pub fn perform_action(
        _hwnd: i64,
        _node: &UiNode,
        _action: UiaAction,
        _value: Option<&str>,
    ) -> Result<(), ToolError> {
        Err(unsupported())
    }
}
