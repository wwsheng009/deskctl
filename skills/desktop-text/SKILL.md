# Desktop Text

Use for text input, hotkeys and clipboard operations.

## Flow

1. Focus the target input with a semantic locator or screenshot + perceive.
2. `desktop_input` with `mode=type`.
3. `desktop_input` with `mode=hotkey` for shortcuts.
4. For long text, `desktop_clipboard_write`, paste, then `desktop_clipboard_clean`.

## Rules

- Ensure the target window is in the foreground before typing.
- Never put passwords, tokens or secrets on the clipboard.
- Verify the result by re-observing, not by trusting `sent`.
- Prefer a stable `target_locator` over a temporary `hwnd`.
- Unicode input is supported; avoid shell or script injection.
