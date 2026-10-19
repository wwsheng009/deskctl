---
name: desktop-basic
description: "Windows desktop control with deskctl for screenshot-driven automation. Use when an agent must observe the screen or interact without a stable UI Automation target: capture the full screen, a region, or a window; list and activate windows; locate OCR/YOLO elements with desktop_perceive; click via capture_id + element_id; type text; or wait for a visual change. Available on the minimal and standard profiles; minimal lacks desktop_perceive, desktop_window_info, and desktop_wait_for, so locate targets from a fresh screenshot and use raw coordinates. Re-observe after every action, never reuse stale element IDs, and verify the business result rather than trusting dispatch_state=sent. Prefer desktop-semantic for stable UIA targets, desktop-window for window-only work, and desktop-text for text or clipboard-only work. Deskctl has no approval flow; act only when the task intent is clear."
---

# Desktop Basic

Use for basic screenshot, window, mouse and keyboard work.

## Flow

1. `desktop_windows_list` or `desktop_window_info` to identify the target.
2. `desktop_window_activate` if the target is not in the foreground.
3. `desktop_screenshot`.
4. `desktop_perceive` to locate elements in that screenshot.
5. `desktop_mouse` with `capture_id` + `element_id`.
6. `desktop_screenshot` or `desktop_wait_for` to verify.

## Rules

- Prefer `capture_id + element_id`; do not guess screen coordinates.
- Re-observe after every action; never reuse stale element ids.
- Do not persist `hwnd` across sessions; list windows again.
- `dispatch_state=sent` does not mean the business goal succeeded.
- There is no approval flow; only act when the task intent is clear.
- `desktop_perceive`, `desktop_window_info` and `desktop_wait_for` are standard-profile tools; on `minimal`, locate targets from a fresh screenshot and use raw coordinates.
