# 发布门禁对照表

- 文档版本：0.1
- 日期：2026-10-18
- 适用：Windows x64 首个可用版本（IMPL-086 发布验收）
- 关联：[acceptance-criteria.md](acceptance-criteria.md) §13 发布门禁、[implementation-plan.md](implementation-plan.md) M8、[implementation-status.md](implementation-status.md)

## 1. 逐项对照

| 门禁项 | 状态 | 证据 |
|---|---|---|
| AC-SCOPE 全部通过 | 通过 | `Cargo.lock` 无 `tauri`/`nuphus`/`reqwest`/`openai`/`anthropic`；无 `src-tauri/`；契约测试覆盖 26/28 工具与 profile 过滤 |
| 传统桌面工具 P0 用例 | 通过 | AC-CAP/WIN/MOUSE/KEY/CLIP 真机实测；截图格式与压缩见 [capture-compression.md](capture-compression.md) |
| 语义 UIA P0 用例 | 通过 | AC-SEM-001..011 真机闭环（execute → verify）；AC-SEM-012 `agent_step` 需外部决策 provider，属可选功能 |
| 多 session 并发与隔离 | 通过 | `scripts/security-check.ps1` 12/12（AC-SESS-003 8 并发输入串行 0 失败、AC-SESS-005/007/009）；AC-PERF-008 8 并发 0 失败、每轮 8 个唯一 capture_id |
| MCP initialize / tools/list / tools/call | 通过 | `crates/deskctl-cli/tests/mcp_smoke.rs`（3 项）；含协议版本校验与 notifications/cancelled |
| CLI 验收 | 通过 | AC-CLI-001/002/003/005/007/008/010；退出码 2/4/5/7 真机实测 |
| 错误与重试验收 | 通过 | AC-ERR-001/002/004/005/007/009 |
| 安全验收 | 通过 | `scripts/security-check.ps1` 12/12：AC-SEC-001/002/003/004/005/006/009/010；AC-SEC-007/008 见第 2 节 |
| 模型随包与 doctor | 通过 | `scripts/verify-models.ps1` 哈希全通过；`scripts/build-installer.ps1` 校验 13 项包内容与 51 条 zip 条目；解包后 `deskctl doctor` 的 `onnx_runtime.resolved` 指向包内 `models/onnxruntime.dll`，`desktop_perceive` 从包内模型完成 OCR/YOLO 感知 |
| 性能基线记录 | 通过 | [performance-baseline.md](performance-baseline.md)：AC-PERF-001..005、007、008、009 本机 release 实测 |
| 文档与 skills | 通过 | AC-DOC；zip 校验 5 个 `skills/*/SKILL.md` 与 `contracts/` |
| 无 Tauri/LLM/工作流依赖 | 通过 | 同 AC-SCOPE 证据（`Cargo.toml` 与 `Cargo.lock` 均无相关依赖） |
| 安装包在干净 Windows 机器上通过 | 待流水线 | 本机已产出便携包 `dist/deskctl-windows-x64.zip`（33 MB、51 条目：exe、ONNX Runtime、OCR/YOLO 模型、LICENSE/NOTICE/THIRD_PARTY_NOTICES、skills、contracts），并在解包目录冒烟通过 `doctor`、`desktop_screenshot`、`desktop_perceive`；Inno Setup 模板 `installer/deskctl.iss` 已就绪，干净机安装与代码签名需发布流水线 |
| P0/P1 缺陷清零 | 通过 | P0 named-pipe 并发缺陷已修复并有 8 并发回归；发布审计修复 `doctor` 在便携包布局下误报 ONNX Runtime 缺失（现同时搜索 exe 目录与 `models/`）；其余为已记录的已知限制（[known-limitations.md](known-limitations.md)）与可选功能缺口（AC-SEM-012） |

## 2. 首期范围外或未验证

- AC-PKG-004 在线模型更新：未实现（低优先级）
- AC-PKG-005 卸载保留用户截图目录：Inno 模板未含卸载校验
- AC-SEC-007/008：模板/视觉读取限制与日志脱敏未逐条实测
- Windows 10 真机回归：未执行（开发机为 Windows 11）
- AC-EXT-*：以文档与现有扩展点为准，未做端到端新增验证

## 3. 复现命令

```powershell
cargo test --workspace -j 1
pwsh scripts/security-check.ps1
pwsh scripts/perf-baseline.ps1
pwsh scripts/build-installer.ps1
```

`security-check.ps1` 默认使用 `target\release\deskctl.exe`，可用 `-Binary` 指向其他构建目录；
`build-installer.ps1` 读取 `CARGO_TARGET_DIR`，在有 Inno Setup（`iscc.exe`）时生成安装器，否则回退为便携 zip。
