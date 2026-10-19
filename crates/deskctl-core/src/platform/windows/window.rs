use deskctl_protocol::{ErrorCode, ToolError};
use serde_json::{json, Value};
use windows::core::PWSTR;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP};
use windows::Win32::UI::Input::KeyboardAndMouse::{IsWindowEnabled, SetActiveWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetAncestor, GetClassNameW, GetClientRect, GetForegroundWindow,
    GetLastActivePopup, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, SetForegroundWindow,
    SetWindowPos, ShowWindow, SwitchToThisWindow, WindowFromPoint, GA_ROOT, HWND_NOTOPMOST,
    HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_RESTORE,
    SW_SHOW,
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

/// 屏幕坐标处最顶层窗口（取根窗口），用于点击前的遮挡检测。
/// 无窗口时返回 None。
pub fn window_at_point(x: i32, y: i32) -> Result<Option<Value>, ToolError> {
    let point = POINT { x, y };
    let hwnd = unsafe { WindowFromPoint(point) };
    if hwnd.0 == 0 {
        return Ok(None);
    }
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let target = if root.0 != 0 { root } else { hwnd };
    Ok(Some(info_for(target)?))
}

/// 模态对话框（原生 Save/Open 等）会将其所有者窗口置为禁用；此时输入与焦点
/// 必须重定向到它的当前活动弹窗，否则会被错误地抢回主窗口。窗口正常启用时
/// 原样返回。
pub fn active_input_window(hwnd: i64) -> Result<i64, ToolError> {
    let raw = handle(hwnd)?;
    if !unsafe { IsWindow(raw) }.as_bool() {
        return Err(unavailable(raw));
    }
    if unsafe { IsWindowEnabled(raw) }.as_bool() {
        return Ok(hwnd);
    }
    let popup = unsafe { GetLastActivePopup(raw) };
    if popup.0 == 0 || popup == raw {
        return Ok(hwnd);
    }
    Ok(i64::try_from(popup.0).unwrap_or(hwnd))
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

/// 激活结果：是否成功、尝试次数与生效方式（供 `desktop_window_activate` 诊断）。
#[derive(Debug, Clone, Copy)]
pub struct ActivationReport {
    pub active: bool,
    pub attempts: u32,
    pub method: &'static str,
}

pub fn activate_window(hwnd: i64) -> Result<bool, ToolError> {
    Ok(activate_window_report(hwnd)?.active)
}

/// 前置窗口自愈：最多 3 轮；第 2 轮起先发送 Alt 键（取得"最近输入"资格）
/// 再调用 `SetForegroundWindow`，绕过 Windows 前台锁。
pub fn activate_window_report(hwnd: i64) -> Result<ActivationReport, ToolError> {
    let hwnd = handle(hwnd)?;
    if !unsafe { IsWindow(hwnd).as_bool() } {
        return Err(unavailable(hwnd));
    }
    unsafe {
        // 隐藏窗口（SW_HIDE）同样无法截图/置前：先恢复显示。
        if !IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            std::thread::sleep(std::time::Duration::from_millis(250));
        }

        let current_thread = GetCurrentThreadId();
        let target_thread = GetWindowThreadProcessId(hwnd, None);

        // A background process is not allowed to change the foreground window
        // unless it shares an input queue with the current foreground thread.
        // Attaching to both the foreground and the target input queues is the
        // documented workaround.
        let mut attached: Vec<u32> = Vec::new();
        let mut active = GetForegroundWindow() == hwnd;
        let mut attempts = 0u32;
        let mut method = if active {
            "already_foreground"
        } else {
            "failed"
        };
        for attempt in 1..=3u32 {
            attempts = attempt;
            if active {
                break;
            }
            for thread in [
                GetWindowThreadProcessId(GetForegroundWindow(), None),
                target_thread,
            ] {
                if thread != 0
                    && thread != current_thread
                    && !attached.contains(&thread)
                    && AttachThreadInput(current_thread, thread, true).as_bool()
                {
                    attached.push(thread);
                }
            }
            if attempt >= 2 {
                // Sending a menu key press gives this process the "last input"
                // status, which lifts the foreground lock for SetForegroundWindow.
                keybd_event(0x12 /* VK_MENU */, 0, Default::default(), 0);
                keybd_event(0x12, 0, KEYEVENTF_KEYUP, 0);
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            let _ = BringWindowToTop(hwnd);
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetActiveWindow(hwnd);
            let _ = SetForegroundWindow(hwnd);
            if GetForegroundWindow() == hwnd {
                active = true;
                method = if attempt >= 2 {
                    "alt_unlock"
                } else {
                    "set_foreground"
                };
                break;
            }
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            let _ = SetWindowPos(
                hwnd,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            let _ = SetForegroundWindow(hwnd);
            std::thread::sleep(std::time::Duration::from_millis(80));
            if GetForegroundWindow() == hwnd {
                active = true;
                method = "topmost";
                break;
            }
            // Alt-tab equivalent: succeeds where SetForegroundWindow is
            // rejected by the foreground lock.
            SwitchToThisWindow(hwnd, true);
            std::thread::sleep(std::time::Duration::from_millis(120));
            if GetForegroundWindow() == hwnd {
                active = true;
                method = "switch_to_this_window";
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }

        for thread in attached {
            let _ = AttachThreadInput(current_thread, thread, false);
        }
        std::thread::sleep(std::time::Duration::from_millis(60));
        let active = active || GetForegroundWindow() == hwnd;
        Ok(ActivationReport {
            active,
            attempts,
            method,
        })
    }
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
    let mut client = RECT::default();
    let mut client_origin = POINT { x: 0, y: 0 };
    let client_ok = unsafe { GetClientRect(hwnd, &mut client) }.is_ok();
    if client_ok {
        unsafe {
            let _ = ClientToScreen(hwnd, &mut client_origin);
        }
    }
    let client_rect = if client_ok {
        json!({
            "x": client_origin.x,
            "y": client_origin.y,
            "width": (client.right - client.left).max(0),
            "height": (client.bottom - client.top).max(0),
            "offset_x": client_origin.x - rect.left,
            "offset_y": client_origin.y - rect.top
        })
    } else {
        Value::Null
    };
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
        "client_rect": client_rect,
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
