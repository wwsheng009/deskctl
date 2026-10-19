---
name: desktop-window
description: "Windows window management with deskctl. Use when a task needs to enumerate visible windows, inspect bounds and process details, activate a specific window, move or resize it, or capture one window as PNG/JPEG/WebP. Resolve ambiguity with an explicit hwnd when multiple windows share a title; re-read window info after every move or resize; window handles are process-local and must not be reused across sessions. desktop_window_activate retries foreground elevation and reports attempts/method, so check those fields when activation does not stick. Designed for the standard profile; on minimal, use desktop-basic for window list, activation, and capture. For screen regions or element-level actions use desktop-basic; for semantic UIA actions use desktop-semantic."
---

# Desktop Window

Use for window enumeration, activation, movement, resizing and window capture.

## Flow

1. `desktop_windows_list`.
2. `desktop_window_info` for the selected window.
3. `desktop_window_activate`.
4. `desktop_window_move` or `desktop_window_resize`.
5. `desktop_window_info` to verify the new bounds.
6. `desktop_window_screenshot` when a visual record is needed.

## Rules

- Multiple windows with the same title require explicit `hwnd` selection.
- Re-read window information after every move or resize.
- Window handles are process-local and must not be reused across runs.
- `desktop_window_activate` retries foreground elevation and reports `attempts` and `method`; check them when activation does not stick.
- Window capture requires the target to still exist; re-check on failure.
