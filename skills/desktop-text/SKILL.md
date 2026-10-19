---
name: desktop-text
description: "Windows text and clipboard input with deskctl. Use when an agent must enter Unicode text (including CJK or emoji), send hotkeys, press/release or repeat keys, or move long text through the clipboard. Focus the target first, then use desktop_input with mode=type/hotkey/key/key_down/key_up; for long text use desktop_clipboard_write, paste with ctrl+v, then desktop_clipboard_clean. Designed for the standard profile with a foreground target; on minimal, desktop_input type/hotkey is available but clipboard tools are not. Never put passwords, tokens, or secrets on the clipboard; prefer a stable target_locator over a temporary hwnd; verify by re-observing instead of trusting dispatch_state=sent. Use desktop-semantic to locate the input control when UIA is available."
---

# Desktop Text

Use for text input, hotkeys and clipboard operations.

## Flow

1. Focus the target input with a semantic locator or screenshot + perceive.
2. `desktop_input` with `mode=type`.
3. `desktop_input` with `mode=hotkey` for shortcuts.
4. For long text, `desktop_clipboard_write`, paste with `desktop_input` `mode=hotkey`, `keys=["ctrl","v"]`, then `desktop_clipboard_clean`.

## Rules

- Ensure the target window is in the foreground before typing.
- Never put passwords, tokens or secrets on the clipboard.
- Verify the result by re-observing, not by trusting `sent`.
- Prefer a stable `target_locator` over a temporary `hwnd`.
- Unicode input is supported; avoid shell or script injection.
