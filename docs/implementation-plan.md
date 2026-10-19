# Deskctl 实施文档

- 文档版本：0.1
- 日期：2026-10-18
- 状态：Draft
- 项目目录：`E:\projects\ai\deskctl`
- 参考实现：`E:\projects\ai\nuphus`
- 关联文档：
  - [architecture.md](architecture.md)
  - [feasibility-analysis.md](feasibility-analysis.md)
  - [acceptance-criteria.md](acceptance-criteria.md)
  - [test-plan.md](test-plan.md)
  - [mcp-service.md](mcp-service.md)
  - [llm-skills.md](llm-skills.md)
  - [extensibility.md](extensibility.md)

## 1. 实施目标

在 Windows 上交付一个可独立运行、可被 LLM/Agent 调用的桌面工具服务 `deskctl`：

- 单二进制 `deskctl.exe`
- `deskctl tool` 一次性无状态调用
- `deskctl serve` 常驻 daemon
- `deskctl call` 有状态工具调用
- `deskctl mcp` MCP stdio server
- 多 session 并发
- 26 个核心工具，可选 28 个
- OCR/YOLO 模型随安装包
- 允许自定义截图输出路径
- 不包含审批

## 2. 实施原则

1. **垂直切片**：每个阶段都产出可运行、可测试的东西，不先写完整“大框架”。
2. **先 Windows，后平台**：首期只实现 Windows 平台；macOS/Linux 只保留 trait 和模块位置。
3. **优先抽取**：从 Nuphus 复制并解耦 Windows 桌面代码，不从零重写。
4. **协议先行**：工具 Schema、结果封套、错误码先稳定，再写执行器。
5. **状态明确**：有状态工具必须走 session；无状态工具才允许 `tool` 模式。
6. **多 session 可并发**：session 状态隔离，桌面写操作通过资源门串行。
7. **无审批**：不实现 `approve/reject`；风险作为元数据返回。
8. **每阶段有门禁**：没有通过对应 AC 用例，不进入下一阶段。

## 3. 目标仓库结构

```text
deskctl/
├─ Cargo.toml                  # workspace
├─ Cargo.lock
├─ rust-toolchain.toml
├─ .cargo/
│  └─ config.toml
├─ crates/
│  ├─ deskctl-protocol/
│  │  ├─ src/
│  │  │  ├─ lib.rs
│  │  │  ├─ tool.rs           # 工具名、元数据、profile
│  │  │  ├─ schema.rs         # 工具 Schema
│  │  │  ├─ result.rs         # 统一结果封套
│  │  │  └─ error.rs          # 错误码、dispatch_state
│  │  └─ Cargo.toml
│  ├─ deskctl-core/
│  │  ├─ src/
│  │  │  ├─ lib.rs
│  │  │  ├─ registry.rs
│  │  │  ├─ session.rs
│  │  │  ├─ resource_gate.rs
│  │  │  ├─ platform/
│  │  │  │  ├─ mod.rs
│  │  │  │  └─ windows/
│  │  │  ├─ tools/
│  │  │  ├─ automation/
│  │  │  ├─ vision/
│  │  │  └─ doctor.rs
│  │  └─ Cargo.toml
│  └─ deskctl-cli/
│     ├─ src/
│     │  ├─ main.rs
│     │  ├─ commands/
│     │  ├─ mcp/
│     │  ├─ jsonrpc/
│     │  └─ output.rs
│     └─ Cargo.toml
├─ contracts/
│  └─ tools/                   # 28 个工具的正式 Schema
├─ skills/
│  ├─ desktop-basic/
│  ├─ desktop-window/
│  ├─ desktop-text/
│  ├─ desktop-semantic/
│  └─ desktop-recovery/
├─ tests/
│  ├─ contract/
│  ├─ integration/
│  ├─ e2e/
│  ├─ fixtures/
│  └─ perf/
├─ scripts/
│  ├─ package-models.ps1
│  ├─ build-installer.ps1
│  └─ verify-models.ps1
├─ docs/
├─ LICENSE
└─ NOTICE
```

首期只建三个 crate；`deskctl-platform`、`deskctl-vision`、`deskctl-runtime` 在边界稳定后拆出。

## 4. 里程碑总览

| 里程碑 | 目标 | 主要交付 | 依赖 |
|---|---|---|---|
| M0 | 文档冻结 | 架构、验收、测试、MCP、skills | 无 |
| M1 | Workspace 与协议骨架 | 3 crate、基础类型、CLI help | M0 |
| M2 | 无状态 Windows 工具 | screenshot/window/input/clipboard | M1 |
| M3 | Daemon 与多 session | serve/call/session/资源门 | M2 |
| M4 | MCP 服务 | initialize/tools/list/tools/call | M3 |
| M5 | 视觉与模型 | OCR/YOLO/perceive/doctor | M2、M3 |
| M6 | Windows UIA | 8 个语义工具 | M3、M5 |
| M7 | Skills 与 profiles | skills/、profile 过滤 | M4 |
| M8 | 打包与发布 | 安装包、模型、签名、验收 | 全部 |

## 5. 任务分解

估算为粗略人日，假设 1 名熟悉 Rust 和 Windows 的工程师，不含长时间真机兼容性排查。

### M0：文档冻结

| ID | 任务 | 交付 | 依赖 | 估算 |
|---|---|---|---|---|
| IMPL-000 | 冻结架构与验收 | docs 评审通过 | 无 | 1 |
| IMPL-001 | 确认 MCP 协议版本和工具 Schema 版本 | 版本常量 | 无 | 0.5 |
| IMPL-002 | 确认模型许可证和随包策略 | 许可证清单 | 无 | 1 |

**门禁：** 架构评审通过；没有阻塞性开放问题。

### M1：Workspace 与协议骨架

| ID | 任务 | 交付 | 依赖 | 估算 |
|---|---|---|---|---|
| IMPL-010 | 创建 Cargo workspace | `Cargo.toml`、3 crate | M0 | 0.5 |
| IMPL-011 | 实现 `ToolName`、`ToolMeta`、`ToolProfile` | protocol 类型 | IMPL-010 | 1 |
| IMPL-012 | 实现统一结果封套 | `ok/data/error/meta` | IMPL-011 | 1 |
| IMPL-013 | 实现错误码和 `DispatchState` | error.rs | IMPL-012 | 1 |
| IMPL-014 | 实现 CLI 骨架 | `clap`、help、子命令 | IMPL-010 | 1.5 |
| IMPL-015 | 实现 `doctor` 最小版本 | 平台/模型/socket 检查 | IMPL-014 | 1 |
| IMPL-016 | 建立契约测试框架 | schema/executor 对应检查 | IMPL-011 | 1 |

**门禁：**

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `deskctl --help` 正常
- `deskctl doctor` 返回结构化 JSON

### M2：Windows 无状态工具

| ID | 任务 | 交付 | 依赖 | 估算 |
|---|---|---|---|---|
| IMPL-020 | 抽取 Windows screenshot | 全屏/区域/窗口/自定义路径 | M1 | 3 |
| IMPL-021 | 抽取窗口枚举与信息 | `windows_list`、`window_info` | IMPL-020 | 2 |
| IMPL-022 | 窗口激活/移动/缩放 | 窗口操作 | IMPL-021 | 2 |
| IMPL-023 | 鼠标输入 | click/double/hover/move/scroll | IMPL-020 | 3 |
| IMPL-024 | 键盘输入 | type/hotkey/target_locator | IMPL-020 | 3 |
| IMPL-025 | 剪贴板 | write/clean | IMPL-020 | 1 |
| IMPL-026 | 找图/找色/多颜色 | find_image/color/multi_color | IMPL-020 | 3 |
| IMPL-027 | 找字 | find_text + dict 校验 | IMPL-020 | 2 |
| IMPL-028 | wait_for | change/stable/color | IMPL-020 | 2 |
| IMPL-029 | 工具 profile | minimal/standard/full/readonly | IMPL-011 | 1 |

**门禁：**

- 至少 10 个核心工具在 Windows 真机可用
- 自定义截图路径通过 AC-CAP-004/005
- 多 session 尚未接入，但工具执行器可被单 session 调用
- 契约测试通过

### M3：Daemon、Session 与 MCP 前置

| ID | 任务 | 交付 | 依赖 | 估算 |
|---|---|---|---|---|
| IMPL-030 | daemon 生命周期 | `serve`、单实例锁、socket | M2 | 3 |
| IMPL-031 | session 模型 | create/list/inspect/close | IMPL-030 | 2 |
| IMPL-032 | `call` 客户端 | JSON-RPC over Named Pipe | IMPL-030 | 2 |
| IMPL-033 | 资源门 | DesktopInput/DesktopSemantic/Capture/Vision | IMPL-031 | 3 |
| IMPL-034 | 多 session 并发 | session 隔离、公平队列 | IMPL-033 | 3 |
| IMPL-035 | 取消与超时 | request_id、cancel、dispatch_state | IMPL-030 | 2 |
| IMPL-036 | 结果封套适配 | 旧工具结果 -> 新封套 | IMPL-012 | 2 |

**门禁：**

- 两个 session 可并发截图
- 输入动作不交叉
- session token 不能跨 session
- daemon 重启后旧 token 失效
- AC-SESS-001..009 通过

### M4：MCP 服务

| ID | 任务 | 交付 | 依赖 | 估算 |
|---|---|---|---|---|
| IMPL-040 | MCP stdio 循环 | 消息解析/序列化 | M3 | 2 |
| IMPL-041 | initialize | 版本、capabilities | IMPL-040 | 1 |
| IMPL-042 | tools/list | profile/平台过滤 | IMPL-041 | 2 |
| IMPL-043 | tools/call | 调用与错误映射 | IMPL-042 | 3 |
| IMPL-044 | 取消/进度 | cancellation、progress | IMPL-043 | 2 |
| IMPL-045 | prompts | 5 个 skills prompt | IMPL-043 | 1.5 |
| IMPL-046 | stdout 纯净与日志分离 | stderr 日志 | IMPL-040 | 1 |

**门禁：**

- 至少一个真实 MCP 客户端可列出并调用工具
- stdout 只有协议 JSON
- 无 approval 方法
- AC-MCP-001..012 通过

### M5：视觉与模型

| ID | 任务 | 交付 | 依赖 | 估算 |
|---|---|---|---|---|
| IMPL-050 | 抽取 PaddleOCR | OCR 文本和框 | M2 | 3 |
| IMPL-051 | 抽取 YOLO | 元素检测 | M2 | 3 |
| IMPL-052 | `desktop_perceive` | OCR+YOLO 合并 | IMPL-050/051 | 3 |
| IMPL-053 | 模型路径与校验 | bundled/offline/download | IMPL-030 | 2 |
| IMPL-054 | `doctor` 扩展 | 模型哈希、ONNX Runtime | IMPL-053 | 1 |
| IMPL-055 | 降级策略 | model_unavailable | IMPL-052 | 1 |

**门禁：**

- 模型随包路径可用
- 无模型时非视觉工具正常
- AC-PER-*、AC-PKG-* 通过

### M6：Windows UIA 语义工具

| ID | 任务 | 交付 | 依赖 | 估算 |
|---|---|---|---|---|
| IMPL-060 | UIA COM 初始化 | STA worker | M3 | 2 |
| IMPL-061 | 目标列表/绑定 | targets_list/target_bind | IMPL-060 | 3 |
| IMPL-062 | 语义观察 | observe/candidate | IMPL-061 | 4 |
| IMPL-063 | 候选执行 | semantic_execute | IMPL-062 | 3 |
| IMPL-064 | 持久化动作 | semantic_action | IMPL-063 | 3 |
| IMPL-065 | 状态验证 | verify_state | IMPL-063 | 2 |
| IMPL-066 | 无审批改造 | 移除 authorize 调用 | IMPL-063 | 1 |
| IMPL-067 | 可选 agent_step | enhanced-decision | IMPL-063 | 3 |

**门禁：**

- 7 个核心语义工具通过
- 无审批路径确认
- 跨 session token 隔离
- AC-SEM-* 通过

### M7：Skills 与文档

| ID | 任务 | 交付 | 依赖 | 估算 |
|---|---|---|---|---|
| IMPL-070 | 创建 `skills/` | 5 个 skill.json + SKILL.md | M4 | 2 |
| IMPL-071 | MCP prompts 同步 | prompt 与 skill 一致 | IMPL-045/070 | 1 |
| IMPL-072 | 工具 Schema 文档 | contracts/tools | M2/M6 | 2 |
| IMPL-073 | 使用示例 | README/examples | IMPL-070 | 1 |

**门禁：**

- 所有 skill 引用的工具存在
- MCP prompt 与 skill 内容一致
- AC-DOC-* 通过

### M8：打包与发布

| ID | 任务 | 交付 | 依赖 | 估算 |
|---|---|---|---|---|
| IMPL-080 | Windows 构建脚本 | release 构建 | 全部 | 2 |
| IMPL-081 | 模型打包 | OCR/YOLO/ONNX Runtime | M5 | 2 |
| IMPL-082 | 安装包 | Windows installer | IMPL-080/081 | 3 |
| IMPL-083 | 许可证与 NOTICE | PaddleOCR/YOLO/ONNX | M0 | 1 |
| IMPL-084 | 性能基线 | 报告 | M4/M6 | 2 |
| IMPL-085 | 安全测试 | 报告 | M4/M6 | 2 |
| IMPL-086 | 发布验收 | 全部 AC | 全部 | 2 |

**门禁：**

- [acceptance-criteria.md](acceptance-criteria.md) 发布门禁全部通过
- 安装包在干净 Windows 机器上通过
- 无 Tauri/LLM/工作流依赖
- P0/P1 缺陷清零

## 6. 代码抽取顺序

从 Nuphus 抽取时，严格按以下顺序，避免一开始就陷入平台细节：

### 6.1 第一批：协议与通用类型

来源：

- `src/tools/desktop_schemas.rs`
- `src/tools/registry.rs` 中的工具元数据
- `src/api` 中的 `ToolDefinition` 结构

目标：

- `deskctl-protocol::schema`
- `deskctl-protocol::tool`
- `deskctl-protocol::result`
- `deskctl-protocol::error`

动作：

1. 复制 28 个工具名和 Schema。
2. 去掉与 Nuphus ToolDef 强耦合的字段。
3. 增加 `requires_session`、`requires_observation`、`single_use_token`。
4. 固化 `ok/data/error/meta`。
5. 固化 `dispatch_state`、`risk`、`retryable`。

### 6.2 第二批：Windows 基础能力

来源：

- `src/desktop/client.rs`
- `src-tauri/crates/desktop-api/src/input/*`
- `src-tauri/crates/desktop-api/src/clipboard.rs`
- `src-tauri/crates/desktop-api/src/vision/capture.rs`
- `src/desktop/capture_context.rs`

目标：

- `deskctl-core::platform::windows::capture`
- `deskctl-core::platform::windows::input`
- `deskctl-core::platform::windows::window`
- `deskctl-core::platform::windows::clipboard`

动作：

1. 保留 Windows 条件编译。
2. 去掉 Tauri 命令依赖。
3. 把 `crate::` 路径改为 `deskctl_core::` 或内部模块。
4. 保留 `DesktopActionError`/`DispatchState` 语义。
5. 自定义截图路径按新规则实现。
6. 不复制 `desktop_approval.rs`。

### 6.3 第三批：工具执行器

来源：

- `src/tools/desktop_executors.rs`

目标：

- `deskctl-core::tools::legacy`
- `deskctl-core::tools::registry`

动作：

1. 20 个传统工具按功能拆成模块。
2. 结果统一转换为新封套。
3. 增加 session token 校验。
4. 去掉任何审批调用。
5. 统一错误码。

### 6.4 第四批：语义自动化

来源：

- `src/tools/semantic_desktop.rs`
- `src/desktop_automation/*`
- `src/tools/desktop_verification.rs`

目标：

- `deskctl-core::automation`
- `deskctl-core::tools::semantic`
- `deskctl-core::session`

动作：

1. 复制观察、候选、执行、验证状态机。
2. 删除 `desktop_approval::authorize`。
3. 把 `NeedsConfirmation` 转为带 `risk` 的直接执行。
4. 接入多 session 的 `semantic_space`。
5. 保留 UIA COM worker 和线程固定。
6. 接入资源门。

### 6.5 第五批：视觉与模型

来源：

- `src/desktop/paddle_ocr.rs`
- `src/desktop/ui_perception.rs`
- `src-tauri/crates/desktop-api/src/vision/*`
- `src-tauri/crates/desktop-api/src/input/*`

目标：

- `deskctl-core::vision`
- `scripts/package-models.ps1`

动作：

1. 复制 PaddleOCR 和 YOLO 推理。
2. 统一模型目录解析。
3. 模型随包路径优先。
4. 保留下载和离线安装。
5. 增加 SHA-256 校验。

### 6.6 抽取时的解耦清单

- [ ] 移除 Tauri 依赖
- [ ] 移除前端事件
- [ ] 移除 LLM 客户端
- [ ] 移除工作流引擎
- [ ] 移除记忆/知识库
- [ ] 移除 `desktop_approval`
- [ ] 保留 `risk.rs` 风险分类
- [ ] 改成多 session 状态
- [ ] 统一结果封套
- [ ] 保留 Windows `cfg`
- [ ] 保留 macOS/Linux 源码位置但不构建
- [ ] 保留 Apache-2.0 许可证和版权声明

## 7. 首个垂直切片

目标：在最短时间内得到一个可运行的 Windows 最小闭环。

### Sprint 1 目标

```text
deskctl doctor
deskctl tool desktop_screenshot --json ...
deskctl tool desktop_windows_list
deskctl tool desktop_window_info --json ...
deskctl tool desktop_mouse --json ...
```

### Sprint 1 任务顺序

| 顺序 | 提交 | 内容 |
|---|---|---|
| 1 | `chore: create workspace` | 三个 crate 和基础配置 |
| 2 | `feat(protocol): tool metadata` | ToolName/ToolMeta/Profile |
| 3 | `feat(protocol): result envelope` | ok/data/error/meta |
| 4 | `test: contract skeleton` | Schema ↔ executor 对应 |
| 5 | `feat(cli): command skeleton` | tool/serve/call/session/mcp/doctor |
| 6 | `feat(cli): doctor` | 平台和文件检查 |
| 7 | `feat(windows): screenshot` | 全屏/区域/自定义路径 |
| 8 | `feat(tools): screenshot` | tool 模式接入 |
| 9 | `feat(windows): windows list/info` | 窗口枚举 |
| 10 | `feat(windows): mouse` | click/move/position |
| 11 | `test: screenshot/window/mouse` | Windows 集成测试 |
| 12 | `docs: first slice` | 命令示例和已知限制 |

### Sprint 1 门禁

- `cargo test --workspace` 通过
- `deskctl doctor` 输出 JSON
- 全屏/区域截图可用
- 自定义截图路径可用
- 窗口列表/信息可用
- 鼠标点击/移动可用
- 不依赖 Tauri/LLM/审批

## 8. 关键路径

```text
M1 workspace/protocol
  -> M2 stateless Windows tools
  -> M3 daemon/session
  -> M4 MCP
  -> M6 UIA semantic
  -> M8 packaging
```

可并行：

- M5 视觉/模型可以在 M2 后独立推进
- M7 skills 可以在 M4 后独立推进
- M8 安装包可以在 M4/M5 后提前做 smoke

## 9. 测试与验收门禁

| 阶段 | 必跑测试 | 对应 AC |
|---|---|---|
| M1 | 单元、契约、clippy | AC-SCOPE、AC-DOC |
| M2 | Windows 截图/窗口/输入集成 | AC-CAP、AC-WIN、AC-MOUSE、AC-INPUT |
| M3 | 多 session、资源门、取消 | AC-SESS、AC-ERR |
| M4 | MCP 契约和真实客户端 | AC-MCP、AC-CLI |
| M5 | OCR/YOLO、模型缺失/损坏 | AC-PER、AC-PKG |
| M6 | UIA 集成和 E2E | AC-SEM |
| M7 | skills/manifest/prompts | AC-DOC、AC-MCP-012 |
| M8 | 安装包、性能、安全、发布 | AC-PKG、AC-PERF、AC-SEC |

每个阶段结束必须更新：

- 已通过 AC 清单
- 未通过 AC 和原因
- 已知限制
- 性能/资源记录

## 10. 发布工程

### 10.1 构建

```text
cargo build --release -p deskctl-cli
```

输出：

```text
target/release/deskctl.exe
```

### 10.2 模型打包

```text
scripts/package-models.ps1
  -> models/ch_PP-OCRv4_det.onnx
  -> models/ch_PP-OCRv4_rec.onnx
  -> models/ch_PP-OCR_keys_v1.txt
  -> models/icon_detect.onnx
  -> models/onnxruntime.dll
  -> models/hashes.json
```

模型随安装包；在线下载只作为更新/修复路径。

### 10.3 安装包

- Windows x64 installer
- 包含 `deskctl.exe`、模型、ONNX Runtime、许可证
- 安装后运行 `deskctl doctor`
- 卸载不删除用户自定义截图目录

### 10.4 发布版本

- `deskctl` server 版本
- 协议版本
- 工具 Schema 版本
- 模型包版本
- skills 版本

版本号写入：

- `Cargo.toml`
- `initialize` 响应
- `doctor` 输出
- 安装包元数据

## 11. 风险登记

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| Windows UIA 在 Electron/WPF 上兼容性差 | 中 | 高 | 用真实应用矩阵测试；保留视觉 fallback |
| COM/STA 线程模型导致死锁 | 中 | 高 | 专用 worker、线程固定、超时 |
| 多 session 输入交叉 | 中 | 高 | 全局资源门 + 真机并发测试 |
| 自定义截图路径被滥用 | 中 | 中 | 文档明确无沙箱；调用方负责 |
| 无审批导致误操作 | 高 | 高 | profile/readonly/日志脱敏；不作为沙箱 |
| 模型体积过大 | 高 | 中 | 模型随包 + 可选在线下载；安装包说明 |
| 模型许可证不兼容 | 中 | 高 | 发布前法务/许可证审查 |
| 抽取代码与 Nuphus 漂移 | 中 | 中 | 先复制，后续以 Deskctl 为主；记录版本 |
| MCP 客户端兼容差异 | 中 | 中 | 版本协商、mock + 多客户端测试 |
| 性能回退 | 中 | 中 | release 基准、p95 记录、热点优化 |

## 12. 待确认事项

1. 代码策略：直接复制并解耦，还是先 path 依赖 `nuphus` 做 spike。
2. MCP 协议版本：实现时锁定官方稳定 revision。
3. 默认工具 profile 是否使用 `standard`（26 个）。
4. 默认 session 上限和资源门队列策略。
5. 是否首期实现 `readonly` profile 和配置化拒绝规则。
6. 是否首期实现 `desktop_vision` 和 `desktop_agent_step`。
7. PaddleOCR/YOLO/ONNX Runtime 的许可证与再分发方式。
8. 项目是否正式定名 `deskctl`。

## 13. Definition of Done

每个任务完成必须满足：

- [ ] 代码通过 `cargo fmt`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] 新增/修改逻辑有单元测试
- [ ] 契约变化有契约测试
- [ ] Windows 行为有集成测试或明确手工验收步骤
- [ ] 结果使用统一封套
- [ ] 错误码稳定
- [ ] 日志不含敏感内容
- [ ] 文档/示例更新
- [ ] 没有引入审批、Tauri、LLM、工作流依赖

每个里程碑完成必须满足：

- [ ] 对应 AC 用例通过
- [ ] 已知限制记录
- [ ] 性能/资源基线记录
- [ ] 下一个里程碑的前置条件满足
- [ ] 发布/回滚路径没有明显阻塞

## 14. 首批执行清单

在创建代码前：

- [ ] 确认项目正式名称 `deskctl`
- [ ] 确认 MCP 协议版本
- [ ] 确认模型许可证
- [ ] 确认代码复制策略
- [ ] 确认默认 profile

创建代码后第一周：

- [ ] workspace
- [ ] protocol 类型
- [ ] CLI help/doctor
- [ ] screenshot
- [ ] windows_list/window_info
- [ ] mouse
- [ ] 契约测试
- [ ] Windows 真机 smoke

## 15. 实施状态

实时状态以 [implementation-status.md](implementation-status.md) 为准（本节保留计划视角的摘要）：

- M0–M7 完成（M6 部分：`agent_step` 待增强决策 provider）；
- M8 部分完成（安装脚本/模板/性能与安全基线就绪；代码签名与干净机安装待发布流水线）；
- I1 输入模拟增强已实施（2026-10-19）：`desktop_mouse` 相对移动/按住释放/拟人轨迹、
  `desktop_input` 键码三写法/长按/重复/拟人打字；工具数保持 36（见
  [design/input-simulation.md](design/input-simulation.md)）。
- D1 审计与历史已实施（2026-10-19）：daemon/oneshot/MCP 全链路 JSONL 审计（脱敏、轮转、保留、
  降级）+ `deskctl history` 过滤/聚合；工具数与协议不变（见
  [design/audit-history.md](design/audit-history.md)、[implementation-status.md](implementation-status.md)）。
- D2 Agent 身份已实施（2026-10-19）：`session create`/`deskctl tool` 的 `--owner`（小写规范化、
  字符集 `[a-zA-Z0-9._:@-]`、1..64、保留名拒绝）、MCP `clientInfo.name`→`mcp:<name>`、
  `session list --owner` 过滤、`DESKCTL_OWNER_MAX_SESSIONS` per-owner 限额（超限 `resource_busy`）、
  审计归因与 `history --owner`（`prefix*` 通配）/`by_owner` 聚合、`doctor.owner_limits`；
  工具数与协议不变（见 [design/agent-identity.md](design/agent-identity.md)、
  [implementation-status.md](implementation-status.md)）。
- D3 录制与回放已实施（2026-10-19）：`desktop_recorder_start/stop`（会话内单录制器、
  include_reads、max_steps/256 KiB 上限、参数化 hwnd/candidate_id/capture_id）、
  `desktop_script_run`（解析/静态校验/preflight/dry-run/逐步执行/重试/目标重绑/两级核验/
  run report + outcome + handoff）、只读 `desktop_file_hash`；目标描述+指纹（strict/balanced/loose）
  与 `launch_ref` 受限重启；审计 `run_id` 与 `history --run`；CLI `record start|stop --out`、
  `script validate|run`；工具数 36 → 40（见 [design/record-replay.md](design/record-replay.md)、
  [implementation-status.md](implementation-status.md)）。
- D4 检查点与补偿已实施（2026-10-19）：`desktop_checkpoint_save/verify`（会话内 8 个上限、
  同名覆盖、四类差异 `value`/`checked`/`exists`/`rect`、导出 `deskctl-checkpoint/1` 并复用
  允许根策略）、`desktop_script_run` 的 `checkpoint`（运行前保存 + 收尾校验进 report）与
  `compensation`（默认 dry-run 计划、`apply` 才执行、固定 `bounded_write` + `on_error=stop`）、
  脚本 `purpose=compensation` 默认 dry-run；CLI `script run --checkpoint/--compensation/
  --apply-compensation`；工具数 40 → 42（见 [design/checkpoint-transaction.md](design/checkpoint-transaction.md)、
  [implementation-status.md](implementation-status.md)；矩形差异路径已真机回归，见
  [test-plan.md](test-plan.md) §8 E2E-013 回归记录）。
- S1 屏幕文本注入防御已实施（2026-10-19）：`perceive`/`read_kv`/`find_text` 的 OCR/视觉文本与
  `history --json` 中 `error.message`/`error.details` 字符串以
  `<untrusted-screen-content>…</untrusted-screen-content>` 包裹并声明"数据非指令"，
  `DESKCTL_UNTRUSTED_WRAP=off|0|false|no|disabled` 关闭（默认开启）；工具描述同步声明；
  工具数与协议不变（见 [acceptance-criteria.md](acceptance-criteria.md) §15.5、
  [implementation-status.md](implementation-status.md)）。
