use deskctl_protocol::{ErrorCode, ToolError};
use serde_json::{json, Value};
use windows::core::PWSTR;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    SetForegroundWindow, SetWindowPos, ShowWindow, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_RESTORE,
};

pub fn windows_list() -> Result<Vec<Value>, ToolError> {
    let mut handles: Vec<HWND> = Vec::new();
    unsafe {
        EnumWindows(Some(enum_window), LPARAM((&raw mut handles) as isize))
            .map_err(|error| ToolError::new(ErrorCode::PermissionDenied, error.to_string()))?;
    }
    let mut rows = Vec::new();
    for hwnd in handles {
        if let Ok(value) = info_for(hwnd) {
            rows.push(value);
        }
    }
    Ok(rows)
}

unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    if !unsafe { IsWindowVisible(hwnd).as_bool() } {
        return BOOL(1);
    }
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return BOOL(1);
    }
    let handles = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    handles.push(hwnd);
    BOOL(1)
}

pub fn window_info(hwnd: i64) -> Result<Value, ToolError> {
    let hwnd = handle(hwnd)?;
    info_for(hwnd)
}

pub fn find_window_by_title(title: &str) -> Result<Option<i64>, ToolError> {
    let lowered = title.to_lowercase();
    let rows = windows_list()?;
    let exact: Vec<&Value> = rows
        .iter()
        .filter(|row| row["title"].as_str().is_some_and(|value| value == title))
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0]["hwnd"].as_i64());
    }
    if exact.len() > 1 {
        return Err(ToolError::new(
            ErrorCode::TargetUnavailable,
            "multiple windows have the same title; select one by hwnd",
        ));
    }
    let matches: Vec<&Value> = rows
        .iter()
        .filter(|row| {
            row["title"]
                .as_str()
                .is_some_and(|value| value.to_lowercase().contains(&lowered))
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(one["hwnd"].as_i64()),
        [] => Ok(None),
        _ => Err(ToolError::new(
            ErrorCode::TargetUnavailable,
            "multiple windows match the title; select one by hwnd",
        )),
    }
}

pub fn activate_window(hwnd: i64) -> Result<bool, ToolError> {
    let hwnd = handle(hwnd)?;
    if !unsafe { IsWindow(hwnd).as_bool() } {
        return Err(unavailable(hwnd));
    }
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            std::thread::sleep(std::time::Duration::from_millis(350));
        }
        let target_thread = GetWindowThreadProcessId(hwnd, None);
        let current_thread = GetCurrentThreadId();
        let _ = AttachThreadInput(current_thread, target_thread, true);
        let _ = SetForegroundWindow(hwnd);
        if GetForegroundWindow() != hwnd {
            let _ = SetWindowPos(
                hwnd,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            let _ = SetForegroundWindow(hwnd);
        }
        let _ = AttachThreadInput(current_thread, target_thread, false);
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
    Ok(unsafe { GetForegroundWindow() } == hwnd)
}

pub fn move_window(hwnd: i64, x: i32, y: i32) -> Result<(), ToolError> {
    let hwnd = handle(hwnd)?;
    let info = info_for(hwnd)?;
    let width = info["width"].as_i64().unwrap_or_default() as i32;
    let height = info["height"].as_i64().unwrap_or_default() as i32;
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )
        .map_err(|error| unavailable_with(hwnd, error.to_string()))?;
    }
    Ok(())
}

pub fn resize_window(hwnd: i64, width: u32, height: u32) -> Result<(), ToolError> {
    if width == 0 || height == 0 {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            "width and height must be positive",
        ));
    }
    let hwnd = handle(hwnd)?;
    let info = info_for(hwnd)?;
    let x = info["x"].as_i64().unwrap_or_default() as i32;
    let y = info["y"].as_i64().unwrap_or_default() as i32;
    let width = i32::try_from(width)
        .map_err(|_| ToolError::new(ErrorCode::InvalidParams, "width is too large for Win32"))?;
    let height = i32::try_from(height)
        .map_err(|_| ToolError::new(ErrorCode::InvalidParams, "height is too large for Win32"))?;
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )
        .map_err(|error| unavailable_with(hwnd, error.to_string()))?;
    }
    Ok(())
}

fn info_for(hwnd: HWND) -> Result<Value, ToolError> {
    if !unsafe { IsWindow(hwnd).as_bool() } {
        return Err(unavailable(hwnd));
    }
    let mut rect = RECT::default();
    unsafe {
        GetWindowRect(hwnd, &mut rect)
            .map_err(|error| unavailable_with(hwnd, error.to_string()))?;
    }
    let mut length = unsafe { GetWindowTextLengthW(hwnd) };
    if length < 0 {
        length = 0;
    }
    let mut title = vec![0u16; length as usize + 1];
    let title_len = unsafe { GetWindowTextW(hwnd, &mut title) };
    title.truncate(title_len.max(0) as usize);

    let mut class = vec![0u16; 256];
    let class_len = unsafe { GetClassNameW(hwnd, &mut class) };
    class.truncate(class_len.max(0) as usize);

    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    let process_name = process_name(pid);
    let foreground = unsafe { GetForegroundWindow() } == hwnd;

    Ok(json!({
        "hwnd": hwnd.0 as i64,
        "title": String::from_utf16_lossy(&title),
        "class_name": String::from_utf16_lossy(&class),
        "process_id": pid,
        "process_name": process_name,
        "x": rect.left,
        "y": rect.top,
        "width": (rect.right - rect.left).max(0),
        "height": (rect.bottom - rect.top).max(0),
        "visible": unsafe { IsWindowVisible(hwnd).as_bool() },
        "minimized": unsafe { IsIconic(hwnd).as_bool() },
        "foreground": foreground
    }))
}

fn process_name(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = vec![0u16; 32768];
        let mut length = u32::try_from(buffer.len()).ok()?;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        );
        let _ = windows::Win32::Foundation::CloseHandle(process);
        result.ok()?;
        let path = String::from_utf16_lossy(&buffer[..length as usize]);
        std::path::Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }
}

fn handle(hwnd: i64) -> Result<HWND, ToolError> {
    let raw = isize::try_from(hwnd)
        .map_err(|_| ToolError::new(ErrorCode::InvalidParams, "invalid hwnd"))?;
    if raw == 0 {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            "hwnd must not be zero",
        ));
    }
    Ok(HWND(raw))
}

fn unavailable(hwnd: HWND) -> ToolError {
    ToolError::new(
        ErrorCode::TargetUnavailable,
        format!("window {} is unavailable", hwnd.0),
    )
    .retryable(true)
}

fn unavailable_with(hwnd: HWND, message: String) -> ToolError {
    ToolError::new(
        ErrorCode::TargetUnavailable,
        format!("window {} operation failed: {message}", hwnd.0),
    )
    .retryable(true)
}
