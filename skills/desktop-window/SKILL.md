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
- Window capture requires the target to still exist; re-check on failure.
