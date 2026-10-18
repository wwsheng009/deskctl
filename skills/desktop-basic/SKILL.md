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
