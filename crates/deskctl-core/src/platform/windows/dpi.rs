//! Process DPI awareness.
//!
//! 未声明 DPI 感知时，Windows 让进程看到"虚拟化"（逻辑）坐标：
//! `SetCursorPos`/`GetCursorPos`/`GetWindowRect` 返回逻辑值，而屏幕截图
//! （xcap/DXGI）始终是物理像素。在 150% 缩放显示器上这会造成 1.5 倍
//! 偏移的点击。声明 Per-Monitor-V2 感知后，鼠标输入、窗口矩形、UIA 边界
//! 与截图统一到同一物理像素坐标系。

use serde_json::{json, Value};
use windows::Win32::UI::HiDpi::{
    GetAwarenessFromDpiAwarenessContext, GetDpiForSystem, GetThreadDpiAwarenessContext,
    SetProcessDpiAwareness, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, DPI_AWARENESS_PER_MONITOR_AWARE,
    DPI_AWARENESS_SYSTEM_AWARE, PROCESS_PER_MONITOR_DPI_AWARE,
};

/// 尽力声明进程级 DPI 感知；需在任何截图/输入调用之前执行一次。
/// 返回实际生效的级别（无法设置时返回 `unknown`）。
pub fn init_process_dpi_awareness() -> &'static str {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_ok() {
            return "per-monitor-v2";
        }
        if SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE).is_ok() {
            return "per-monitor";
        }
    }
    "unknown"
}

/// 当前线程生效的 DPI 感知级别与系统 DPI（96 = 100%）。
pub fn diagnostics() -> Value {
    unsafe {
        let context = GetThreadDpiAwarenessContext();
        let awareness = GetAwarenessFromDpiAwarenessContext(context);
        let name = if awareness.0 == DPI_AWARENESS_PER_MONITOR_AWARE.0 {
            "per-monitor"
        } else if awareness.0 == DPI_AWARENESS_SYSTEM_AWARE.0 {
            "system"
        } else {
            "unaware"
        };
        let dpi = GetDpiForSystem();
        json!({
            "awareness": name,
            "system_dpi": dpi,
            "scale": f64::from(dpi) / 96.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_report_a_known_awareness() {
        let value = diagnostics();
        let awareness = value["awareness"].as_str().unwrap_or_default();
        assert!(matches!(awareness, "unaware" | "system" | "per-monitor"));
        assert!(value["system_dpi"].as_u64().is_some_and(|dpi| dpi > 0));
    }
}
