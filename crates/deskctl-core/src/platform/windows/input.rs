use std::mem::size_of;
use std::time::Duration;

use deskctl_protocol::{ErrorCode, ToolError};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, mouse_event, GetDoubleClickTime, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD,
    INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSEINPUT, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SetCursorPos, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::humanize::sleep_cancellable;

pub fn mouse_position() -> Result<(i32, i32), ToolError> {
    let mut point = POINT::default();
    unsafe {
        GetCursorPos(&mut point).map_err(|error| input_error(error.to_string()))?;
    }
    Ok((point.x, point.y))
}

pub fn mouse_move(x: i32, y: i32) -> Result<(), ToolError> {
    unsafe {
        SetCursorPos(x, y).map_err(|error| input_error(error.to_string()))?;
    }
    let (actual_x, actual_y) = mouse_position()?;
    if (actual_x - x).abs() > 2 || (actual_y - y).abs() > 2 {
        return Err(input_error(format!(
            "cursor did not reach target: expected ({x},{y}), got ({actual_x},{actual_y})"
        )));
    }
    Ok(())
}

pub fn mouse_click(x: i32, y: i32, button: &str, clicks: u8) -> Result<(), ToolError> {
    mouse_move(x, y)?;
    let (down, up) = match button {
        "left" => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        _ => {
            return Err(ToolError::new(
                ErrorCode::InvalidParams,
                "button must be left, right or middle",
            ))
        }
    };
    for _ in 0..clicks.max(1) {
        unsafe {
            mouse_event(down, 0, 0, 0, 0);
            std::thread::sleep(std::time::Duration::from_millis(30));
            mouse_event(up, 0, 0, 0, 0);
        }
        if clicks > 1 {
            std::thread::sleep(std::time::Duration::from_millis(60));
        }
    }
    Ok(())
}

pub fn mouse_scroll(direction: &str, amount: u32) -> Result<(), ToolError> {
    if amount == 0 || amount > 100 {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            "amount must be between 1 and 100",
        ));
    }
    let (flag, delta) = match direction {
        "up" => (MOUSEEVENTF_WHEEL, 120),
        "down" => (MOUSEEVENTF_WHEEL, -120),
        "right" => (MOUSEEVENTF_HWHEEL, 120),
        "left" => (MOUSEEVENTF_HWHEEL, -120),
        _ => {
            return Err(ToolError::new(
                ErrorCode::InvalidParams,
                "direction must be up, down, left or right",
            ))
        }
    };
    for _ in 0..amount {
        unsafe {
            mouse_event(flag, 0, 0, delta, 0);
        }
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    Ok(())
}

pub fn drag(start: (i32, i32), end: (i32, i32), duration_ms: u64) -> Result<(), ToolError> {
    mouse_move(start.0, start.1)?;
    unsafe {
        mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
    }
    let steps = (duration_ms / 10).clamp(1, 100) as i32;
    for step in 1..=steps {
        let x = start.0 + (end.0 - start.0) * step / steps;
        let y = start.1 + (end.1 - start.1) * step / steps;
        unsafe {
            SetCursorPos(x, y).map_err(|error| input_error(error.to_string()))?;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    unsafe {
        mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
    }
    Ok(())
}

pub fn type_text(text: &str) -> Result<(), ToolError> {
    if text.is_empty() {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            "text must not be empty",
        ));
    }
    if text.len() > 65_536 {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            "text exceeds the 65536 byte limit",
        ));
    }
    let mut inputs = Vec::with_capacity(text.encode_utf16().count() * 2);
    for unit in text.encode_utf16() {
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: unit,
                    dwFlags: KEYEVENTF_UNICODE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: unit,
                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        return Err(ToolError::new(
            ErrorCode::DispatchUnknown,
            format!(
                "only {sent} of {} unicode events were accepted",
                inputs.len()
            ),
        )
        .retryable(false));
    }
    Ok(())
}

pub fn hotkey(keys: &[&str]) -> Result<(), ToolError> {
    if keys.is_empty() {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            "hotkey requires at least one key",
        ));
    }
    let virtual_keys = keys
        .iter()
        .map(|key| key_to_vk(key))
        .collect::<Result<Vec<_>, _>>()?;
    for (index, key) in virtual_keys.iter().enumerate() {
        unsafe {
            keybd_event(*key as u8, 0, Default::default(), 0);
        }
        if index + 1 < virtual_keys.len() {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
    for key in virtual_keys.iter().rev() {
        unsafe {
            keybd_event(*key as u8, 0, KEYEVENTF_KEYUP, 0);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Ok(())
}

pub fn is_valid_key(key: &str) -> bool {
    key_to_vk(key).is_ok()
}

pub fn key_to_vk(key: &str) -> Result<u16, ToolError> {
    match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => Ok(0x0D),
        "tab" => Ok(0x09),
        "esc" | "escape" => Ok(0x1B),
        "space" => Ok(0x20),
        "backspace" => Ok(0x08),
        "delete" | "del" => Ok(0x2E),
        "insert" => Ok(0x2D),
        "up" | "arrowup" => Ok(0x26),
        "down" | "arrowdown" => Ok(0x28),
        "left" | "arrowleft" => Ok(0x25),
        "right" | "arrowright" => Ok(0x27),
        "home" => Ok(0x24),
        "end" => Ok(0x23),
        "pageup" => Ok(0x21),
        "pagedown" => Ok(0x22),
        "ctrl" | "control" => Ok(0x11),
        "shift" => Ok(0x10),
        "alt" | "option" | "opt" => Ok(0x12),
        "win" | "command" | "cmd" | "meta" | "super" => Ok(0x5B),
        "f1" => Ok(0x70),
        "f2" => Ok(0x71),
        "f3" => Ok(0x72),
        "f4" => Ok(0x73),
        "f5" => Ok(0x74),
        "f6" => Ok(0x75),
        "f7" => Ok(0x76),
        "f8" => Ok(0x77),
        "f9" => Ok(0x78),
        "f10" => Ok(0x79),
        "f11" => Ok(0x7A),
        "f12" => Ok(0x7B),
        "a" => Ok(0x41),
        "b" => Ok(0x42),
        "c" => Ok(0x43),
        "d" => Ok(0x44),
        "e" => Ok(0x45),
        "f" => Ok(0x46),
        "g" => Ok(0x47),
        "h" => Ok(0x48),
        "i" => Ok(0x49),
        "j" => Ok(0x4A),
        "k" => Ok(0x4B),
        "l" => Ok(0x4C),
        "m" => Ok(0x4D),
        "n" => Ok(0x4E),
        "o" => Ok(0x4F),
        "p" => Ok(0x50),
        "q" => Ok(0x51),
        "r" => Ok(0x52),
        "s" => Ok(0x53),
        "t" => Ok(0x54),
        "u" => Ok(0x55),
        "v" => Ok(0x56),
        "w" => Ok(0x57),
        "x" => Ok(0x58),
        "y" => Ok(0x59),
        "z" => Ok(0x5A),
        "0" => Ok(0x30),
        "1" => Ok(0x31),
        "2" => Ok(0x32),
        "3" => Ok(0x33),
        "4" => Ok(0x34),
        "5" => Ok(0x35),
        "6" => Ok(0x36),
        "7" => Ok(0x37),
        "8" => Ok(0x38),
        "9" => Ok(0x39),
        _ => Err(ToolError::new(
            ErrorCode::InvalidParams,
            format!("unknown key: {key}"),
        )),
    }
}

/// 键盘按键的三种写法（名字 / `vk:` 码 / `scan:` 码）解析结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySpec {
    pub vk: Option<u16>,
    pub scan: Option<u16>,
    pub extended: bool,
    /// 写法来源：`name` | `vk` | `scan`（用于审计字段 `key_mode`）。
    pub source: &'static str,
}

/// 解析 `key` 参数：
/// - `"enter"`：既有键名表（大小写不敏感、支持别名）；
/// - `"vk:0x41"` / `"vk:65"`：虚拟键码 1..254；
/// - `"scan:0x1E"`：扫描码；`"scan:0xE01D"` 用 0xE0 前缀表示扩展键。
pub fn parse_key_spec(spec: &str) -> Result<KeySpec, ToolError> {
    let trimmed = spec.trim();
    let lowered = trimmed.to_ascii_lowercase();
    if let Some(rest) = lowered.strip_prefix("vk:") {
        let vk = parse_key_number(rest).ok_or_else(|| invalid_key_spec(spec))?;
        if !(1..=254).contains(&vk) {
            return Err(ToolError::new(
                ErrorCode::InvalidParams,
                "vk must be between 1 and 254 (e.g. vk:0x41 or vk:13)",
            )
            .retryable(false));
        }
        return Ok(KeySpec {
            vk: Some(vk as u16),
            scan: None,
            extended: false,
            source: "vk",
        });
    }
    if let Some(rest) = lowered.strip_prefix("scan:") {
        let raw = parse_key_number(rest).ok_or_else(|| invalid_key_spec(spec))?;
        let (scan, extended) = if raw > 0xFF {
            if raw >> 8 != 0xE0 {
                return Err(invalid_key_spec(spec));
            }
            ((raw & 0xFF) as u16, true)
        } else {
            (raw as u16, false)
        };
        if scan == 0 {
            return Err(invalid_key_spec(spec));
        }
        return Ok(KeySpec {
            vk: None,
            scan: Some(scan),
            extended,
            source: "scan",
        });
    }
    let vk = key_to_vk(trimmed).map_err(|_| invalid_key_spec(spec))?;
    Ok(KeySpec {
        vk: Some(vk),
        scan: None,
        extended: false,
        source: "name",
    })
}

fn parse_key_number(text: &str) -> Option<u64> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) if !hex.is_empty() => u64::from_str_radix(hex, 16).ok(),
        _ => text.parse::<u64>().ok(),
    }
}

fn invalid_key_spec(spec: &str) -> ToolError {
    ToolError::new(
        ErrorCode::InvalidParams,
        format!("invalid key spec '{spec}'; expected \"enter\", \"vk:0x41\" or \"scan:0x1E\""),
    )
    .retryable(false)
}

/// 发送一次按键（按下或释放）。扫描码走 `KEYEVENTF_SCANCODE`（游戏/远程
/// 桌面等只认扫描码的程序也能生效）；`force_extended` 强制附加
/// `KEYEVENTF_EXTENDEDKEY`。
pub fn send_key(spec: &KeySpec, key_up: bool, force_extended: bool) -> Result<(), ToolError> {
    let extended = spec.extended || force_extended;
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    let keyboard = if let Some(scan) = spec.scan {
        flags |= KEYEVENTF_SCANCODE;
        KEYBDINPUT {
            wVk: VIRTUAL_KEY(0),
            wScan: scan,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        }
    } else {
        KEYBDINPUT {
            wVk: VIRTUAL_KEY(spec.vk.unwrap_or(0)),
            wScan: 0,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        }
    };
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: keyboard },
    };
    let sent = unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    if sent != 1 {
        return Err(ToolError::new(
            ErrorCode::DispatchUnknown,
            format!("keyboard event was not accepted (sent {sent} of 1)"),
        )
        .retryable(true));
    }
    Ok(())
}

/// 鼠标按键按下/释放（SendInput）。
pub fn mouse_button(button: &str, down: bool) -> Result<(), ToolError> {
    let flag = match (button, down) {
        ("left", true) => MOUSEEVENTF_LEFTDOWN,
        ("left", false) => MOUSEEVENTF_LEFTUP,
        ("right", true) => MOUSEEVENTF_RIGHTDOWN,
        ("right", false) => MOUSEEVENTF_RIGHTUP,
        ("middle", true) => MOUSEEVENTF_MIDDLEDOWN,
        ("middle", false) => MOUSEEVENTF_MIDDLEUP,
        _ => {
            return Err(ToolError::new(
                ErrorCode::InvalidParams,
                "button must be left, right or middle",
            )
            .retryable(false))
        }
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flag,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    if sent != 1 {
        return Err(ToolError::new(
            ErrorCode::DispatchUnknown,
            format!("mouse button event was not accepted (sent {sent} of 1)"),
        )
        .retryable(true));
    }
    Ok(())
}

/// 虚拟桌面（多屏联合）边界：原点与尺寸（物理像素）。
pub fn virtual_desktop_bounds() -> Result<(i32, i32, i32, i32), ToolError> {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if width <= 0 || height <= 0 {
            return Err(input_error("virtual desktop metrics are unavailable"));
        }
        Ok((x, y, width, height))
    }
}

/// 绝对坐标移动（`MOUSEEVENTF_ABSOLUTE|MOUSEEVENTF_VIRTUALDESK`，0..65535
/// 归一化）。不使用相对事件，避免鼠标加速度扭曲轨迹。
pub fn mouse_move_absolute(x: i32, y: i32) -> Result<(), ToolError> {
    let (origin_x, origin_y, width, height) = virtual_desktop_bounds()?;
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: normalize_absolute(x, origin_x, width),
                dy: normalize_absolute(y, origin_y, height),
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    if sent != 1 {
        return Err(ToolError::new(
            ErrorCode::DispatchUnknown,
            format!("mouse move event was not accepted (sent {sent} of 1)"),
        )
        .retryable(true));
    }
    Ok(())
}

fn normalize_absolute(value: i32, origin: i32, extent: i32) -> i32 {
    let span = (extent - 1).max(1) as f64;
    let relative = f64::from(value.saturating_sub(origin)) / span;
    (relative * 65535.0).round().clamp(0.0, 65535.0) as i32
}

/// 沿绝对坐标点列移动；每 16 个点检查一次取消。返回实际发送的点数；
/// 整条路径发完后做一次 ±1px 的精确落点校正（绝对坐标归一化有舍入）。
pub fn mouse_move_path(
    points: &[(i32, i32)],
    delays_ms: &[u64],
    cancel: &dyn Fn() -> bool,
) -> Result<usize, ToolError> {
    let mut sent = 0usize;
    for (index, point) in points.iter().enumerate() {
        if index % 16 == 0 && cancel() {
            break;
        }
        mouse_move_absolute(point.0, point.1)?;
        sent += 1;
        if let Some(delay) = delays_ms.get(index) {
            if *delay > 0 && !sleep_cancellable(*delay, cancel) {
                break;
            }
        }
    }
    if sent == points.len() {
        if let Some(last) = points.last() {
            if let Ok((current_x, current_y)) = mouse_position() {
                if current_x != last.0 || current_y != last.1 {
                    let _ = unsafe { SetCursorPos(last.0, last.1) };
                }
            }
        }
    }
    Ok(sent)
}

/// 系统双击时间（毫秒），约束多击间隔。
pub fn double_click_time_ms() -> u64 {
    unsafe { u64::from(GetDoubleClickTime()) }
}

/// 组合键：全部按下 → 保持 `hold_ms` → 逆序释放。返回 `false` 表示保持
/// 期间被取消（按键仍会完整释放，避免粘键）。
pub fn hotkey_hold(
    keys: &[&str],
    hold_ms: u64,
    cancel: &dyn Fn() -> bool,
) -> Result<bool, ToolError> {
    let virtual_keys = keys
        .iter()
        .map(|key| key_to_vk(key))
        .collect::<Result<Vec<_>, _>>()?;
    for (index, key) in virtual_keys.iter().enumerate() {
        unsafe {
            keybd_event(*key as u8, 0, Default::default(), 0);
        }
        if index + 1 < virtual_keys.len() {
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    let completed = sleep_cancellable(hold_ms, cancel);
    for key in virtual_keys.iter().rev() {
        unsafe {
            keybd_event(*key as u8, 0, KEYEVENTF_KEYUP, 0);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(completed)
}

fn input_error(message: impl Into<String>) -> ToolError {
    ToolError::new(ErrorCode::PermissionDenied, message).retryable(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_keys_before_dispatch() {
        assert!(is_valid_key("ctrl"));
        assert!(is_valid_key("F5"));
        assert!(!is_valid_key("invented"));
        assert!(key_to_vk("ENTER").is_ok());
    }

    #[test]
    fn parses_key_specs_in_three_forms() {
        let named = parse_key_spec("Enter").unwrap();
        assert_eq!(named.vk, Some(0x0D));
        assert_eq!(named.source, "name");
        assert!(!named.extended);

        let vk_hex = parse_key_spec("vk:0x41").unwrap();
        assert_eq!(vk_hex.vk, Some(0x41));
        assert_eq!(vk_hex.source, "vk");

        let vk_decimal = parse_key_spec("vk:13").unwrap();
        assert_eq!(vk_decimal.vk, Some(0x0D));

        let scan = parse_key_spec("scan:0x1E").unwrap();
        assert_eq!(scan.scan, Some(0x1E));
        assert!(!scan.extended);
        assert_eq!(scan.source, "scan");

        let extended = parse_key_spec("scan:0xE01D").unwrap();
        assert_eq!(extended.scan, Some(0x1D));
        assert!(extended.extended);
    }

    #[test]
    fn rejects_invalid_key_specs_with_examples() {
        assert!(parse_key_spec("vk:0").is_err());
        assert!(parse_key_spec("vk:255").is_err());
        assert!(parse_key_spec("scan:0x0").is_err());
        assert!(parse_key_spec("scan:0x1234").is_err());
        assert!(parse_key_spec("invented").is_err());
        let error = parse_key_spec("nope").unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidParams);
        assert!(error.message.contains("scan:0x1E"));
    }
}
