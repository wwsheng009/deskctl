# Deskctl

一个独立的、Windows 优先、面向所有 LLM / Agent 的桌面操作工具服务。

项目名称暂定 **Deskctl**，取 “desktop control” 之意。它只提供桌面操作能力，
不包含 Nuphus 的对话、工作流、记忆、插件市场或 Tauri GUI。

> 当前阶段：设计与可行性分析阶段，尚未开始功能代码实现。

## 文档

- [文档索引](docs/README.md)
- [可行性分析](docs/feasibility-analysis.md)
- [架构方案](docs/architecture.md)
- [架构评审](docs/architecture-review.md)
- [验收标准](docs/acceptance-criteria.md)
- [测试计划](docs/test-plan.md)
- [实施文档](docs/implementation-plan.md)
- [MCP 服务设计](docs/mcp-service.md)
- [LLM Skills 定义](docs/llm-skills.md)
- [可扩展性设计](docs/extensibility.md)

## 项目定位

- 只提供 `desktop_*` 工具，不实现 LLM Agent 循环。
- 工具协议与模型供应商解耦；支持 MCP、CLI 和本地 JSON-RPC 的客户端均可接入。
- 默认以本地常驻 daemon 运行，保留观察令牌、截图缓存和会话状态。
- 同一 daemon 支持多个并发 session；桌面输入动作仍通过资源门串行化，避免互相踩键盘鼠标。
- 项目不包含审批流程；工具执行只做参数校验、风险标记、日志和结果验证。

## 工具范围

参考 Nuphus 当前的 28 个 `desktop_*` 工具：

- 默认核心 profile：26 个工具
- 完整 profile：28 个工具
- `desktop_vision`：可选，依赖外部视觉模型
- `desktop_agent_step`：可选，依赖增强判断模型

工具组：

- 鼠标、键盘、拖拽、剪贴板
- 全屏、区域、窗口截图
- 窗口枚举、激活、移动、缩放、信息
- 图像模板、颜色、字库找字
- 画面等待、OCR、YOLO 感知、视觉理解
- Windows UIA 语义观察与执行
- 执行后状态验证

完整清单和平台矩阵见架构文档。

## 平台策略

| 平台 | 首期状态 |
|---|---|
| Windows 10/11 | 首期完整实现和验收目标 |
| macOS | 保留平台抽象和适配器位置，后续实现 |
| Linux X11 | 保留平台抽象，后续实现 |
| Linux Wayland | 后续独立研究，不纳入首期 |
| 其他平台 | 返回 `platform_not_supported` |

首期只对 Windows 做功能和真机验收；macOS/Linux 的 trait、模块边界和条件编译保留，
避免以后重写核心状态机。

## 核心设计约束

1. **单二进制、多模式**：`mcp`、`serve`、`call`、`tool`、`doctor` 共享同一套工具实现。
2. **有状态工具必须走 daemon/session**：不能每次调用都启动新进程，否则观察令牌和截图缓存会丢失。
3. **多会话并发**：允许多个 session 同时存在；读操作可并行，桌面写操作按资源门串行。
4. **无审批**：不实现 `approve/reject`；风险级别作为元数据返回，不阻塞执行。
5. **自定义截图路径**：`desktop_screenshot` / `desktop_window_screenshot` 接受调用方提供的输出路径。
6. **模型随安装包分发**：PaddleOCR/YOLO 模型默认随安装包提供，同时保留在线下载和离线安装选项。
7. **平台隔离**：Windows 首期落地；macOS/Linux 预留模块，不伪造跨平台能力。
8. **尽量复用 Nuphus**：优先抽取现有 Rust 桌面实现，而不是从零重写。

## 安全说明

本项目不提供审批门。任何能连接本地 daemon 并持有 access token 的调用方，都可以执行
已启用的桌面工具。建议：

- 默认只监听 Named Pipe / Unix Domain Socket，不监听公网。
- 默认启用工具 profile，不把全部高权限工具暴露给不可信 Agent。
- 对输入文本、剪贴板和窗口内容做日志脱敏。
- 如需只读场景，使用 `readonly` profile。

## 目录约定

```text
deskctl/
├─ README.md
├─ docs/
│  ├─ feasibility-analysis.md
│  ├─ architecture.md
│  ├─ architecture-review.md
│  ├─ acceptance-criteria.md
│  ├─ test-plan.md
│  ├─ implementation-plan.md
│  ├─ mcp-service.md
│  ├─ llm-skills.md
│  └─ extensibility.md
├─ crates/             # 后续实现
├─ tests/
└─ scripts/
```
