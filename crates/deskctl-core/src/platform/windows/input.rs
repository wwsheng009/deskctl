use std::mem::size_of;

use deskctl_protocol::{ErrorCode, ToolError};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, mouse_event, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

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
}
