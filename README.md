# Deskctl

Deskctl is a standalone Windows desktop-control service for LLMs and agents.
It ships as one `deskctl.exe` and exposes a stateless CLI, a local named-pipe
daemon, multi-session JSON-RPC, MCP stdio, and 28 `desktop_*` tools. It contains
no Tauri UI, LLM SDK, workflow engine, or approval flow.

> Implementation status: Windows first-release scope implemented. See
> `docs/implementation-status.md` for the requirement-to-evidence audit and
> `docs/known-limitations.md` for remaining release caveats.

## Features

- Screenshot: full screen, region, window, custom output path (always PNG)
- Window list/info/activate/move/resize and window screenshots
- Mouse click, double click, hover, move, scroll, drag
- Unicode text input, key chords, target activation
- Clipboard write/clean with no content logging
- Template image, color, multi-color, OCR text lookup, and wait-for tools
- PaddleOCR + YOLO ONNX perception with stable `capture_id + element_id`
- Windows UIA target binding, observation, candidate execution, persistent
  semantic locators, and post-condition verification
- Multi-session isolation, per-session tokens, resource gate, cancellation
- MCP `initialize`, `tools/list`, `tools/call`, prompts, progress, cancellation
- `doctor` reports versions, capabilities, socket/token state, model hashes

## Requirements

- Windows 10/11 x64
- Rust 1.92 (see `rust-toolchain.toml`)
- Models and ONNX Runtime in `models/` or
  `third_party/onnxruntime/win-x64/` (already fetched in this workspace)

## Build and verify

```powershell
cargo build --release -p deskctl-cli
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
.\target\release\deskctl.exe doctor
.\scripts\verify-models.ps1
.\scripts\perf-baseline.ps1      # AC-PERF matrix -> artifacts/perf-baseline.json
.\scripts\security-check.ps1     # AC-SEC/AC-SESS matrix -> artifacts/security-check.json
```

## CLI

```powershell
deskctl --help
deskctl doctor

# Stateless tools
deskctl tool desktop_screenshot --json '{"path":"shots/screen.jpg"}'
deskctl tool desktop_windows_list
deskctl tool desktop_mouse --json '{"action":"position"}'
deskctl tool desktop_find_text --json @args.json
Get-Content args.json | deskctl tool desktop_find_image --stdin
```

Screenshot paths are accepted as absolute or relative paths. Parent directories
are created and the extension follows the selected format: an explicit `format`
argument wins, otherwise a recognized `.png`/`.jpg`/`.jpeg`/`.webp` extension
selects the encoder, and anything else falls back to the default (`shot.bmp`
becomes `shot.png`). See [docs/capture-compression.md](docs/capture-compression.md).

Exit codes: `0` success, `1` internal/tool failure, `2` invalid parameters or
unknown tool, `4` stale state/restart, `5` resource busy, `6` platform
unsupported, `7` policy/authentication denial.

## Daemon and sessions

```powershell
deskctl serve
deskctl session create
deskctl session list
deskctl call <session_id> desktop_screenshot --json '{}'
deskctl call <session_id> desktop_wait_for --request-id wait-1 --json '{"condition":"stable","timeout_ms":30000}'
deskctl cancel wait-1
deskctl session inspect <session_id>
deskctl session close <session_id>
deskctl shutdown
```

The daemon listens only on the local named pipe `\\.\pipe\deskctl` by default.
Every request requires the access token from the token file. Session tokens are
stored locally after `session create`; capture, target, and observation state
never crosses sessions.

## MCP

```powershell
deskctl mcp
```

The MCP process owns one implicit session. Stdout contains only newline-delimited
JSON-RPC; logs go to stderr. Supported methods include `initialize`, `ping`,
`tools/list`, `tools/call`, `prompts/list`, `prompts/get`, progress
notifications, and cancellation notifications. Exiting the process closes the
session and invalidates all tokens.

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"example","version":"1.0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"desktop_screen_size","arguments":{}}}
```

## Profiles

| Profile | Tools | Purpose |
|---|---:|---|
| `minimal` | 6 | Screenshot, window list/activate/capture, mouse, input |
| `standard` | 26 | Default core tools |
| `full` | 28 | Adds `desktop_vision` and `desktop_agent_step` |
| `readonly` | 9 | Capture/read/perceive/find/verify only |

Use `--profile full`. `desktop_vision` requires a separately configured external
provider; `desktop_agent_step` requires the optional enhanced-decision provider.
Both return `model_unavailable` when the provider is absent.

## Contracts and skills

- `contracts/tools/` contains all 28 tool contracts in full profile.
- Regenerate with `deskctl --profile full schema --out contracts/tools`.
- `skills/desktop-basic`, `desktop-window`, `desktop-text`,
  `desktop-semantic`, and `desktop-recovery` contain manifests and `SKILL.md`.
- MCP prompts read the same `SKILL.md` files.

## Safety

Deskctl is intentionally approval-free. It is **not** a security sandbox.

- Access is controlled by a local token file and the current Windows user.
- Screen captures are written wherever the caller requests; callers own path
  safety.
- Template and perceived image paths are restricted to configured image roots.
- OCR dictionary names are basename-only and cannot traverse paths.
- Input text, clipboard contents, and screenshot pixels are not logged.
- UIA values are returned as fingerprints, not raw document content.
- `dispatch_state=sent` does not mean the business action succeeded. Never
  automatically retry `sent`, `partial`, or `unknown` actions.

## Packaging

```powershell
.\scripts\build-installer.ps1
```

The script builds a release binary and either invokes Inno Setup
(`installer/deskctl.iss`) or creates `dist/deskctl-windows-x64.zip`. The package
contains the executable, models, runtime, skills, contracts, licenses, and
notices.

## Documentation

- `docs/architecture.md`
- `docs/implementation-plan.md`
- `docs/implementation-status.md`
- `docs/acceptance-criteria.md`
- `docs/test-plan.md`
- `docs/mcp-service.md`
- `docs/llm-skills.md`
- `docs/known-limitations.md`
- `docs/performance-baseline.md`

## License

Apache-2.0. See `LICENSE`, `NOTICE`, and `THIRD_PARTY_NOTICES.md`.
