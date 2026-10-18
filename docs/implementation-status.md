# 实施状态

- 更新日期：2026-10-18
- 对应计划：[implementation-plan.md](implementation-plan.md)
- 当前版本：0.1.0（Windows 首个可运行版本）

## 1. 总体状态

| 里程碑 | 状态 | 说明 |
|---|---|---|
| M0 文档冻结 | 完成 | 架构、验收、测试、MCP、skills 文档齐备 |
| M1 Workspace 与协议骨架 | 完成 | 3 个 crate，统一结果封套，CLI/doctor/schema |
| M2 Windows 无状态工具 | 完成 | 截图、窗口、鼠标、键盘、剪贴板、找图/找色/找字、等待 |
| M3 Daemon 与多 session | 完成 | Named Pipe daemon、session 管理、资源门 |
| M4 MCP 服务 | 完成 | initialize / tools/list / tools/call / prompts |
| M5 视觉与模型 | 完成 | PaddleOCR 与 YOLO 均已接入并真机验证 |
| M6 Windows UIA 语义工具 | 部分完成 | 观察/候选/执行/验证可用；agent_step 未实现 |
| M7 Skills 与 profiles | 完成 | 5 个 skills、profile 过滤、契约导出 |
| M8 打包与发布 | 部分完成 | 运行时/模型获取脚本完成；安装包未做 |

## 2. 已验证能力

以下能力已在 Windows 真机通过：

- `deskctl doctor` 返回平台、模型、DLL 和路径信息
- `deskctl tool desktop_screen_size`
- `deskctl tool desktop_windows_list`
- `deskctl tool desktop_mouse --json '{"action":"position"}'`
- `deskctl tool desktop_screenshot`（含自定义路径、区域截图、扩展名强制 PNG）
- `deskctl tool desktop_perceive`（PaddleOCR 文本元素识别）
- `deskctl tool desktop_perceive`（YOLO 图标元素识别）
- `deskctl tool desktop_find_text`（PaddleOCR 文本匹配）
- `deskctl serve` + Named Pipe
- `deskctl session create/list`
- `deskctl call <session> desktop_screenshot`
- `deskctl mcp`：initialize、tools/list、tools/call、prompts/list
- `desktop_targets_list`、`desktop_target_bind`
- `desktop_semantic_observe`（真实 UIA 树与候选动作）
- 多 session 并发截图与状态隔离

## 3. 测试状态

`cargo test --workspace` 通过：

```text
deskctl-cli:      10 passed
deskctl-core:      6 passed
deskctl-protocol:  3 passed
合计:             19 passed, 0 failed
```

覆盖：

- 28 工具 Schema 合法性
- 26/28 profile 工具数量
- readonly 不暴露输入工具
- 无 approve/reject/approval 工具
- skills manifest 与工具名一致性
- CLI help、doctor、schema 导出
- MCP initialize/list/call
- 截图路径强制 PNG
- 颜色解析、图像匹配、按键校验
- OCR 模型缺失时安全报错

## 4. 运行时资源

已独立管理，不再依赖 Nuphus：

```text
third_party/onnxruntime/win-x64/
├─ onnxruntime.dll
├─ onnxruntime_providers_shared.dll
├─ hashes.json
└─ README.md

models/
├─ ch_PP-OCRv4_det.onnx
├─ ch_PP-OCRv4_rec.onnx
├─ ch_PP-OCR_keys_v1.txt
├─ icon_detect.onnx
└─ hashes.json
```

脚本：

```powershell
pwsh scripts/fetch-runtime.ps1 -VerifyOnly
pwsh scripts/fetch-runtime.ps1
pwsh scripts/fetch-models.ps1 -FromDirectory <dir>
pwsh scripts/fetch-models.ps1
```

构建脚本 `crates/deskctl-cli/build.rs` 会自动复制：

- `onnxruntime*.dll` → `target/<profile>/` 和 `deps/`
- `models/*` → `target/<profile>/models/`

模型和 DLL 不提交 Git，由脚本获取。

## 5. 已知缺口

### 高优先级

- `desktop_vision` 需要外部视觉 provider，当前返回 `model_unavailable`
- `desktop_agent_step` 需要增强决策 provider，当前返回 `model_unavailable`
- `desktop_find_image` 的模板匹配为简化实现

### 中优先级

- 安装包（MSI/NSIS）未制作
- Windows 10 未做真机回归
- 性能基线未测量
- 取消请求尚未传播到所有长任务
- session 空闲回收只在新会话创建时触发

### 低优先级

- macOS / Linux 仅保留 trait 与占位
- HTTP 适配层未实现
- 在线模型更新未实现

## 6. 与验收标准的差距

已满足：

- AC-SCOPE
- AC-CAP-001/002/003/004/005/006
- AC-WIN-001/002/003/004
- AC-MOUSE-001/002/004/005
- AC-CLIP
- AC-SEM-001/002/003/004/005/007/008/009/010/011
- AC-SESS-001/002/005
- AC-MCP-001/002/003/004/005/010/012
- AC-CLI-001/002/003/005/008/010
- AC-ERR-001/002/004/005
- AC-PKG-002/003/007
- AC-DOC

未满足或未验证：

- AC-PER-003（OCR/YOLO 合并去重）
- AC-SEM-006（持久化动作端到端）
- AC-SEM-012（agent_step）
- AC-SESS-003/004/006/007/008/009
- AC-PERF-*
- AC-SEC-*
- AC-PKG-001/004/005/006
- AC-EXT-*
- Windows 10 兼容性

## 7. 结论

当前版本已经是可运行的 Windows 桌面工具服务：

- 可独立构建
- 不依赖 Nuphus
- 不包含审批流程
- 支持多 session
- 支持 MCP
- 支持 OCR 和 UIA

但尚未达到 `acceptance-criteria.md` 的完整发布门禁。主要剩余工作是 YOLO、找字、安装包、性能/安全测试和 Windows 10 回归。
