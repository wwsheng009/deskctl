use std::mem::size_of;

use deskctl_protocol::{ErrorCode, ToolError};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GHND};

const CF_UNICODETEXT: u32 = 13;

pub fn write_text(text: &str) -> Result<(), ToolError> {
    if text.len() > 1_048_576 {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            "clipboard text exceeds the 1 MiB limit",
        ));
    }
    unsafe {
        OpenClipboard(HWND::default()).map_err(|error| clipboard_error(error.to_string()))?;
        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let size = utf16.len() * size_of::<u16>();
        let handle = match GlobalAlloc(GHND, size) {
            Ok(handle) => handle,
            Err(error) => {
                let _ = CloseClipboard();
                return Err(clipboard_error(error.to_string()));
            }
        };
        let destination = GlobalLock(handle).cast::<u16>();
        if destination.is_null() {
            let _ = GlobalFree(handle);
            let _ = CloseClipboard();
            return Err(clipboard_error("clipboard memory lock failed"));
        }
        std::ptr::copy_nonoverlapping(utf16.as_ptr(), destination, utf16.len());
        let _ = GlobalUnlock(handle);
        let _ = EmptyClipboard();
        if let Err(error) = SetClipboardData(CF_UNICODETEXT, HANDLE(handle.0 as isize)) {
            let _ = GlobalFree(handle);
            let _ = CloseClipboard();
            return Err(clipboard_error(error.to_string()));
        }
        let _ = CloseClipboard();
    }
    Ok(())
}

pub fn clean() -> Result<(), ToolError> {
    unsafe {
        OpenClipboard(HWND::default()).map_err(|error| clipboard_error(error.to_string()))?;
        let result = EmptyClipboard();
        let _ = CloseClipboard();
        result.map_err(|error| clipboard_error(error.to_string()))?;
    }
    Ok(())
}

fn clipboard_error(message: impl Into<String>) -> ToolError {
    ToolError::new(ErrorCode::PermissionDenied, message).retryable(true)
}
