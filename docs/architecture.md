# Deskctl 架构方案

- 文档版本：0.2
- 日期：2026-10-18
- 状态：Draft（已按产品决策收敛）
- 项目目录：`E:\projects\ai\deskctl`
- 参考实现：`E:\projects\ai\nuphus`
- 架构评审：[architecture-review.md](architecture-review.md)
- 验收标准：[acceptance-criteria.md](acceptance-criteria.md)
- 测试计划：[test-plan.md](test-plan.md)
- 实施文档：[implementation-plan.md](implementation-plan.md)
- MCP 服务：[mcp-service.md](mcp-service.md)
- LLM Skills：[llm-skills.md](llm-skills.md)
- 扩展性：[extensibility.md](extensibility.md)

## 1. 项目定位

**Deskctl** 是一个独立的、Windows 优先的桌面操作工具服务，供所有 LLM / Agent 使用。

它只提供桌面能力：

- 鼠标、键盘、拖拽
- 截图、窗口管理
- OCR、YOLO、视觉理解
- 找图、找色、找字、等待
- Windows UIA 语义观察与执行
- 执行后状态验证

不包含：

- LLM 对话和 Agent 循环
- 工作流、记忆、知识库、插件市场
- Tauri/Web UI
- 审批 UI 或 `approve/reject` 流程
- 具体模型供应商绑定

## 2. 已确认的产品决策

| 决策 | 结论 |
|---|---|
| 项目名称 | `deskctl` |
| 实现语言 | Rust |
| 首期平台 | Windows 10/11 |
| 其他平台 | 保留 trait、模块和 feature，首期不做真机验收 |
| 工具范围 | 只做 `desktop_*`，不混入 Agent/工作流 |
| 审批 | 不实现 `approve/reject`；风险只作为元数据返回 |
| 截图输出路径 | 允许调用方自定义路径 |
| 多会话 | 同一 daemon 支持多个并发 session |
| 桌面写操作 | 多会话可并发，但输入动作通过资源门串行 |
| 模型分发 | OCR/YOLO 模型随安装包分发；保留在线下载/离线安装选项 |
| 参考代码 | 尽可能抽取 Nuphus 现有 Rust 实现，不重写平台层 |
| 对外协议 | MCP stdio + CLI + 本地 JSON-RPC |
| 首期 crate | `deskctl-protocol`、`deskctl-core`、`deskctl-cli` |

### 2.1 无审批的边界

本项目明确取消人工审批。关键动作的风险等级、动作类型和建议复观察信息会出现在
结果 `meta` 中，但不会阻塞执行。

这意味着：

- 任何能连接 daemon 并持有 access token 的调用方，都可以执行已启用工具。
- daemon 默认只监听本地 Named Pipe / Unix Domain Socket。
- 默认工具 profile 不应把全部高权限工具暴露给不可信 Agent。
- `readonly` profile 作为只读场景的替代方案。
- 如需更强隔离，应使用独立用户、独立机器或系统级策略，而不是依赖本项目审批。

## 3. 系统上下文

```text
┌──────────────────────────────────────────────────────────────┐
│                        LLM / Agent 客户端                     │
│  Claude Desktop / Codex / Cline / IDE / 自研 Agent / CLI      │
└───────────────┬──────────────────────────────┬───────────────┘
                │ MCP stdio                    │ CLI / JSON-RPC
                ▼                              ▼
┌──────────────────────────────┐   ┌───────────────────────────┐
│  deskctl mcp                 │   │  deskctl call             │
│  （stdio MCP server）         │   │  （daemon 客户端）         │
└───────────────┬──────────────┘   └─────────────┬─────────────┘
                │                                │
                └──────────────┬─────────────────┘
                               ▼
                 ┌──────────────────────────────┐
                 │  deskctl serve               │
                 │  常驻 daemon                  │
                 │  多 session / 工具注册表       │
                 └──────────────┬───────────────┘
                                │
        ┌───────────────────────┼───────────────────────┐
        ▼                       ▼                       ▼
┌───────────────┐       ┌───────────────┐       ┌───────────────┐
│ deskctl-core  │       │ deskctl-vision*│      │ deskctl-runtime*│
│ registry/risk │       │ OCR / YOLO    │       │ session / IPC  │
└───────┬───────┘       └───────┬───────┘       └───────┬───────┘
        │                       │                       │
        └───────────────────────┼───────────────────────┘
                                ▼
                 ┌──────────────────────────────┐
                 │ deskctl-platform             │
                 │ Windows 首期 / macOS/Linux 预留│
                 └──────────────────────────────┘
```

`*` 首期放在 `deskctl-core` 内实现，边界稳定后再拆成独立 crate。

## 4. 运行模式

单二进制 `deskctl`，通过子命令进入不同模式：

| 模式 | 命令 | 用途 |
|---|---|---|
| MCP | `deskctl mcp` | stdio MCP server，供 MCP 客户端直接启动 |
| Daemon | `deskctl serve` | 常驻服务，持有工具注册表和多个 session |
| Client | `deskctl call ...` | 向 daemon 发送工具调用 |
| One-shot | `deskctl tool ...` | 无状态工具的单次执行 |
| Session | `deskctl session ...` | 创建、查看、关闭 session |
| Cancel | `deskctl cancel ...` | 取消指定 request |
| Doctor | `deskctl doctor` | 平台、权限、模型、运行时自检 |

不提供 `approve` / `reject` 子命令。

### 4.1 为什么需要 daemon

有状态工具依赖进程内缓存：

- `capture_id` 和截图元素映射
- `observation_token` 和候选动作
- target token
- 最近动作和增强模式状态
- ONNX 引擎
- 资源门和跨进程自动化锁

如果每次 `call` 都启动新进程，这些状态会丢失。因此：

- **完整能力**：`serve` daemon + `call` 客户端
- **无状态工具**：可以直接 `tool` 一次性执行
- **MCP 模式**：MCP server 可以自己持有 session，也可以代理到 daemon

### 4.2 默认部署

```text
LLM 客户端
   │
   ├─ MCP stdio ──> deskctl mcp
   │                     │
   │                     └─ 连接本地 daemon
   │
   └─ CLI ────────> deskctl call
                         │
                         └─ 本地 daemon
```

默认传输：

- Windows：Named Pipe，例如 `\\.\pipe\deskctl`
- macOS/Linux：Unix Domain Socket，例如 `$XDG_RUNTIME_DIR/deskctl.sock`
- 可选的本地 HTTP：仅绑定 `127.0.0.1`，默认关闭

## 5. 进程、会话与资源

### 5.1 Daemon 启动

```text
deskctl serve --socket <path> --token-file <path>
```

启动时完成：

1. 解析配置
2. 平台能力探测
3. 初始化 ToolRegistry
4. 加载随包模型或配置的模型目录
5. 打开 socket
6. 输出就绪信号

不安装审批 host，不创建 pending approval 队列。

### 5.2 会话模型

同一 daemon 支持多个并发 session。每个 session 独立拥有：

- capture cache
- target bindings
- semantic action space
- 最近动作和增强状态
- request 生命周期
- 会话配置和工具 profile

session 之间不能读取彼此的 capture/token/locator。

### 5.3 并发与资源门

允许多个 session 并发执行，但必须区分读操作和桌面写操作：

| 操作类型 | 并发策略 |
|---|---|
| 截图、窗口枚举、信息读取 | 可并行 |
| OCR/YOLO 推理 | 共享引擎，可并行或按引擎限制 |
| 鼠标点击、拖拽、键盘输入 | 全局串行 |
| 语义执行 | 对目标窗口加锁，必要时串行 |
| 等待类工具 | 独立等待，不阻塞其他 session 的读操作 |
| 长时间 OCR/视觉请求 | 不持有桌面输入锁 |

资源门建议按资源类划分：

- `DesktopInput`：鼠标/键盘/拖拽
- `DesktopSemantic`：UIA/AX 写操作
- `Capture`：截图和感知
- `VisionModel`：OCR/YOLO/视觉推理

### 5.4 单实例、版本与升级

- 每个用户会话只允许一个 daemon；使用 Windows named mutex 或 Unix lock file。
- socket 已存在但 daemon 不健康时，删除 stale socket 后重新启动。
- `initialize` 返回 `protocol_version`、`server_version`、`platform` 和 `capabilities`。
- daemon 与 client 主协议版本不匹配时拒绝连接，不做隐式降级。
- daemon 升级或重启后：
  - 所有旧 session 失效
  - 所有 capture/observation/target token 失效
  - 不自动重放已发送动作
  - 客户端收到 `server_restarted`，必须重新观察
- daemon 崩溃不恢复 session 状态；未完成请求统一标记为 `unknown`，等待人工确认或重新观察。

## 6. 协议设计

### 6.1 对 LLM 暴露的协议

首期支持：

1. **MCP stdio**：供 Claude Desktop、Codex、Cline、IDE 插件等使用。
2. **CLI / 本地 JSON-RPC**：供不支持 MCP 的 Agent 或脚本使用。

MCP 模式下：

- `tools/list` 按 profile 返回 26 个核心工具或 28 个完整工具，再按平台过滤。
- `tools/call` 接收工具名和 JSON 参数。
- MCP server 进程自身持有 session，或代理到 daemon 的隐式 session。
- 同一 MCP 连接内的 `screenshot → perceive → mouse` 可以直接工作。

### 6.2 MCP 与 daemon 的会话绑定

`session_id` 不出现在 MCP 工具 Schema 中，模型不可见。

```text
MCP 客户端启动
  -> 启动 deskctl mcp
  -> 连接 daemon
  -> session/create（owner = `mcp:<client-info>`，见 design/agent-identity.md）
  -> tools/list
  -> tools/call ...
  -> MCP 客户端退出 / stdin EOF
  -> daemon 检测断连，关闭 session
  -> capture_id、observation_token、target token 全部失效
```

规则：

- 一个 MCP server 进程对应一个隐式 session。
- 显式 session 只用于 CLI `call` 或管理接口。
- MCP 进程崩溃后，daemon 回收 session，不恢复旧 token。
- 重连后必须重新截图、绑定或观察。
- 同一 daemon 可以同时服务多个 MCP server / CLI 客户端。
- 多个 session 的桌面写操作共享资源门，但 session 状态彼此隔离。

### 6.3 内部 JSON-RPC

```json
{
  "jsonrpc": "2.0",
  "id": "req-1",
  "method": "tools/call",
  "params": {
    "session_id": "sess-...",
    "tool": "desktop_screenshot",
    "arguments": {
      "path": "C:\\temp\\shot.png",
      "region": { "x": 0, "y": 0, "width": 800, "height": 600 }
    }
  }
}
```

响应：

```json
{
  "jsonrpc": "2.0",
  "id": "req-1",
  "result": {
    "ok": true,
    "data": {
      "capture_id": "cap-...",
      "path": "C:\\temp\\shot.png",
      "width": 800,
      "height": 600
    },
    "error": null,
    "meta": {
      "tool": "desktop_screenshot",
      "request_id": "req-1",
      "session_id": "sess-...",
      "dispatch_state": "not_sent",
      "effect": "unverifiable",
      "verification": "unknown",
      "retryable": false,
      "requires_observation": false
    }
  }
}
```

### 6.4 方法列表

| 方法 | 说明 |
|---|---|
| `initialize` | 协商版本、平台、能力 |
| `tools/list` | 返回当前 profile 的工具 Schema |
| `tools/call` | 执行工具 |
| `session/create` | 创建会话 |
| `session/list` | 列出会话 |
| `session/close` | 关闭会话 |
| `session/inspect` | 查看会话状态摘要 |
| `request/cancel` | 取消指定请求 |
| `doctor` | 平台/权限/模型检查 |
| `shutdown` | 关闭 daemon |

没有 `approval/*` 方法。

### 6.5 CLI 约定

```text
deskctl tool <tool_name> --json '<json>'
deskctl tool <tool_name> --json @args.json
echo '<json>' | deskctl tool <tool_name> --stdin
deskctl call <session_id> <tool_name> --json '<json>'
```

输出约定：

- stdout：机器可读 JSON；`--format text` 时输出人类可读文本
- stderr：日志、进度、诊断
- 退出码：
  - `0`：成功
  - `2`：参数错误
  - `4`：状态过期
  - `5`：资源忙
  - `6`：平台不支持
  - `7`：策略拒绝
  - `1`：其他内部错误

### 6.6 错误码

- `invalid_params`
- `unknown_tool`
- `platform_not_supported`
- `permission_denied`
- `policy_denied`
- `stale_capture`
- `stale_observation`
- `target_unavailable`
- `resource_busy`
- `cancelled`
- `timeout`
- `model_unavailable`
- `dispatch_unknown`
- `internal`

### 6.7 结果封套、幂等与重试

统一结果封套：

```json
{
  "ok": true,
  "data": {},
  "error": null,
  "meta": {
    "tool": "desktop_mouse",
    "request_id": "req-...",
    "session_id": "sess-...",
    "dispatch_state": "sent",
    "effect": "unverifiable",
    "verification": "unknown",
    "retryable": false,
    "requires_observation": true,
    "risk": "bounded_write"
  }
}
```

失败：

```json
{
  "ok": false,
  "data": null,
  "error": {
    "code": "dispatch_unknown",
    "message": "动作可能已发送，但未收到确认",
    "retryable": false,
    "details": {}
  },
  "meta": {
    "dispatch_state": "unknown",
    "effect": "unverifiable",
    "retryable": false,
    "requires_observation": true,
    "risk": "external_commit"
  }
}
```

重试纪律：

- `dispatch_state=not_sent` 且 `retryable=true` 才允许自动重试。
- `dispatch_state=sent`、`partial`、`unknown` 时禁止自动重试。
- 超时只表示调用方停止等待，不表示动作未发送。
- 取消请求必须带 `request_id`；取消不能触发后续动作。
- 等待类工具的 `met:false` 是数据结果，不是动作失败。
- 没有审批门；`risk` 字段仅用于调用方提示、日志和策略过滤。

## 7. 工具清单与 profile

### 7.1 传统桌面工具（20）

| 分类 | 工具 |
|---|---|
| 鼠标/键盘 | `desktop_mouse`、`desktop_mouse_drag`、`desktop_input` |
| 截图/屏幕 | `desktop_screenshot`、`desktop_screen_size`、`desktop_window_screenshot` |
| 窗口 | `desktop_windows_list`、`desktop_window_activate`、`desktop_window_move`、`desktop_window_resize`、`desktop_window_info` |
| 视觉/定位 | `desktop_vision`、`desktop_perceive`、`desktop_find_image`、`desktop_find_color`、`desktop_find_multi_color`、`desktop_find_text` |
| 等待 | `desktop_wait_for` |
| 剪贴板 | `desktop_clipboard_clean`、`desktop_clipboard_write` |

`desktop_vision` 依赖外部视觉模型，属于可选能力。

### 7.2 语义桌面工具（8）

| 工具 | 作用 | 首期平台 |
|---|---|---|
| `desktop_targets_list` | 列出应用/窗口目标 | Windows |
| `desktop_target_bind` | 绑定目标并返回 token | Windows |
| `desktop_semantic_observe` | 读取 UIA 语义树和候选动作 | Windows |
| `desktop_semantic_candidate` | 查询候选详情和 workflow step | Windows |
| `desktop_semantic_execute` | 执行观察绑定的候选动作 | Windows |
| `desktop_semantic_action` | 执行持久化语义 locator | Windows |
| `desktop_verify_state` | 验证目标状态 | Windows |
| `desktop_agent_step` | 增强模式单步决策 | Windows，可选 |

`desktop_agent_step` 依赖增强判断模型，属于可选能力。

### 7.3 工具 profile

| Profile | 内容 | 用途 |
|---|---|---|
| `minimal` | 截图、窗口列表、窗口激活、鼠标、键盘、窗口截图 | 简单自动化 |
| `standard` | 26 个核心工具：19 个传统工具 + 7 个语义工具 | 默认 LLM 工具集 |
| `full` | 28 个工具，包括 `desktop_vision` 和 `desktop_agent_step` | 完整能力 |
| `readonly` | 截图、窗口信息、感知、找图/找色/找字 | 只读 Agent |
| `platform` | 当前平台实际可用工具 | 自动过滤 |

内部工具元数据：

```text
requires_session
requires_observation
single_use_token
requires_vision_model
requires_decision_model
```

没有 `requires_approval` 字段。

### 7.4 状态性分类

| 类型 | 工具示例 |
|---|---|
| 明确无状态 | `desktop_screenshot`、`desktop_screen_size`、`desktop_windows_list`、`desktop_window_info`、`desktop_clipboard_clean`、`desktop_clipboard_write`、`desktop_find_image`、`desktop_find_color`、`desktop_find_multi_color`、`desktop_find_text`、`desktop_wait_for` |
| 有条件无状态 | `desktop_window_screenshot`、`desktop_window_activate`、`desktop_window_move`、`desktop_window_resize` |
| 需要捕获状态 | `desktop_perceive`、使用 `capture_id` 的 `desktop_mouse` / `desktop_mouse_drag` / `desktop_input` |
| 需要语义会话 | `desktop_targets_list` cursor 翻页、`desktop_target_bind`、`desktop_semantic_*`、`desktop_agent_step` |
| 外部模型依赖 | `desktop_vision`、`desktop_agent_step` |

一次性 `tool` 模式只允许明确无状态或可独立完成的工具。以下操作强制使用 daemon/session：

- 使用 `capture_id` / `element_id` 的鼠标、拖拽、输入
- `desktop_targets_list` 的 cursor 翻页
- `desktop_target_bind` 之后的任何 token 操作
- `desktop_semantic_observe` → `desktop_semantic_candidate` → `desktop_semantic_execute`
- `desktop_semantic_action` 的持久化 locator 执行
- `desktop_agent_step`

### 7.5 平台矩阵

| 工具组 | Windows（首期） | macOS（预留） | Linux X11（预留） |
|---|---|---|---|
| 鼠标/键盘/剪贴板 | 支持 | 后续 | 后续 |
| 全屏/区域截图 | 支持 | 后续 | 后续 |
| 窗口截图 | 支持 | 后续 | 后续 |
| 窗口枚举/激活/移动/缩放 | 支持 | 后续 | 后续 |
| 找图/找色/找字 | 支持 | 后续 | 后续 |
| OCR/YOLO | 支持，模型随包 | 后续 | 后续 |
| UIA/AX 语义工具 | 支持 | 后续 | 不支持 |

Wayland 不纳入首期。

## 8. 状态模型

### 8.1 会话状态

```text
Session
├─ id
├─ owner
├─ created_at / last_active_at
├─ capture_cache
│  ├─ capture_id -> image path + geometry + window snapshot
│  └─ capture_id -> element_id -> screen coordinates
├─ target_bindings
│  └─ target_token -> app/window descriptor + native window info
├─ semantic_space
│  └─ observation_token -> observation + candidates + locator map
└─ enhanced_loop_state
   └─ goal_key + recent actions + stall count
```

没有 approvals 字段。

### 8.2 全局状态

- 平台适配器实例
- 自动化资源门
- OCR/YOLO 引擎
- 配置
- 日志与诊断
- session 注册表

### 8.3 TTL 与上限

| 状态 | 建议 TTL/上限 |
|---|---|
| 截图 capture | 进程退出或显式失效；可按数量淘汰 |
| observation token | 120 秒 |
| target binding | 会话结束或上限 256 条 |
| 空闲 session | 可配置，默认 30 分钟 |
| 单 daemon session 数 | 可配置，默认 8，支持并发 |
| 单次工具执行 | 按工具设置超时；等待类工具由参数限制 |

### 8.4 持久化原则

默认不持久化：

- capture_id
- observation_token
- target token
- 文本输入内容
- 窗口截图

daemon 重启后旧 token 全部失效，客户端必须重新观察和绑定。

### 8.5 会话所有权

- session 绑定到创建它的连接或显式客户端。
- 普通工具调用只能访问当前 session 的 capture、target、observation。
- 管理接口可以列出/关闭 session，但不能读取其他 session 的敏感内容。
- access token 只解决“能否连接 daemon”，不提供跨用户隔离。
- 多 session 共享资源门，但不共享 capture/target/observation 状态。

## 9. 平台层设计

平台层统一暴露 trait，上层不直接接触 Win32、AX 或 X11。首期只实现 Windows，
macOS/Linux 保留模块边界和 feature，避免以后重写核心状态机。

```rust
pub trait ScreenCapture {
    async fn capture_screen(&self, region: Option<Rect>) -> Result<Frame>;
    async fn capture_window(&self, window: WindowRef) -> Result<Frame>;
}

pub trait InputBackend {
    async fn mouse(&self, action: MouseAction) -> Result<()>;
    async fn keyboard(&self, action: KeyboardAction) -> Result<()>;
}

pub trait WindowBackend {
    async fn list(&self) -> Result<Vec<WindowInfo>>;
    async fn activate(&self, window: WindowRef) -> Result<()>;
    async fn move_resize(&self, window: WindowRef, bounds: Rect) -> Result<()>;
}

pub trait SemanticBackend {
    async fn observe(&self, scope: ObserveScope) -> Result<Observation>;
    async fn execute(&self, candidate: Candidate, input: ExecutionInput) -> Result<Receipt>;
}
```

实现约定：

- 所有错误统一转换为 `DesktopError`。
- 平台不支持时返回 `platform_not_supported`，不返回空数组伪装成功。
- 需要权限的调用在入口处检查，并给出可操作的错误信息。
- 原生句柄不进入模型可见的 Schema；模型只看到 token 或候选 ID。
- 自定义截图输出路径由调用方提供；平台层只负责创建父目录、按所选格式写入并规范化扩展名（格式策略见 [capture-compression.md](capture-compression.md)）。

### 9.1 Windows（首期）

- 输入：SendInput，处理 Unicode、组合键和按键释放。
- 窗口：Win32 枚举、激活、移动、缩放、前台检测。
- 截图：GDI/DXGI；窗口截图需要验证目标窗口未变化。
- 语义：UI Automation COM；使用专用 STA worker 和线程固定。
- 剪贴板：Win32 Clipboard；文件路径读取使用 CF_HDROP。
- 自定义截图路径：接受绝对/相对路径，父目录不存在时创建；扩展名与所选格式一致（显式 `format` > 扩展名 > 默认，见 [capture-compression.md](capture-compression.md)）。
- 普通用户权限即可；操作高权限窗口可能被系统拒绝，必须返回真实错误。

### 9.2 macOS（预留）

- 输入：CoreGraphics / enigo；需要辅助功能权限。
- 窗口：Accessibility API；numeric handle 使用进程内别名。
- 截图：CoreGraphics 或 ScreenCaptureKit；需要录屏权限。
- 语义：Accessibility tree；AX 操作在专用 worker 上执行。
- 首期不构建、不做真机验收；只保留模块和 trait 位置。

### 9.3 Linux（预留）

- 窗口：X11/EWMH。
- 输入：XTest 或 enigo。
- 截图：X11；Wayland 不纳入首期。
- 语义：首期不实现；未来可基于 AT-SPI/D-Bus。
- 首期不构建、不做真机验收。

## 10. 视觉与模型层

### 10.1 本地模型

首批需要：

- `ch_PP-OCRv4_det.onnx`
- `ch_PP-OCRv4_rec.onnx`
- `ch_PP-OCR_keys_v1.txt`
- 可选 `icon_detect.onnx`
- ONNX Runtime 动态库

分发策略：

- **默认随 Windows 安装包分发**，保证安装后 OCR/YOLO 可用。
- **保留在线下载/更新**：模型缺失、损坏或需要升级时可从配置源下载。
- **支持离线安装**：从本地模型目录或压缩包安装。
- 模型文件不提交到源码仓库；发布流水线负责打包。
- 模型文件提供 SHA-256 校验；下载失败不破坏现有安装。
- `deskctl doctor` 检查模型、ONNX Runtime、路径和权限。

### 10.2 远端视觉模型

`desktop_vision` 不作为核心默认工具。启用时需要配置通用视觉 provider：

- OpenAI-compatible 图像接口，或
- 调用方直接把截图交给自己的多模态模型

不在核心中绑定 Nuphus 的模型注册表。

### 10.3 降级策略

| 缺失资源 | 行为 |
|---|---|
| PaddleOCR 模型 | OCR/找字/感知相关工具返回 `model_unavailable` |
| YOLO 模型 | `desktop_perceive` 只返回 OCR 结果，或按配置报错 |
| 视觉 provider | `desktop_vision` 不在工具列表中出现，或调用返回 `model_unavailable` |
| ONNX Runtime | 本地模型能力不可用，传统输入/窗口工具不受影响 |
| 增强判断模型 | `desktop_agent_step` 不暴露，普通语义工具仍可用 |

## 11. 安全设计

### 11.1 本地访问控制

- 默认只监听 Named Pipe 或 Unix Domain Socket。
- socket 文件权限限制为当前用户。
- access token 只用于连接 daemon。
- HTTP 适配默认关闭；开启时只绑定 `127.0.0.1`。
- 同一用户下的进程可能读取 token；本项目不把它宣传为强沙箱。

### 11.2 无审批策略

- 不实现 `approve` / `reject`。
- 不创建 pending approval 队列。
- 不把 `approve` 暴露为 MCP 工具。
- 风险等级写入结果 `meta.risk`，不阻塞执行。
- 如需限制能力，使用工具 profile、`readonly` 模式或配置化拒绝规则；不做逐动作人工确认。
- 抽取 `semantic_desktop.rs` 时必须移除 `desktop_approval::authorize` 调用；
  `NeedsConfirmation` 不再等待用户，而是转为带风险标记的直接执行。
- 原 `desktop_approval.rs` 不进入 Deskctl；只保留 `risk.rs` 的风险分类结果。

### 11.3 资源互斥

- 多 session 可以并发。
- 截图、窗口枚举、OCR 和视觉请求可并行。
- 鼠标、键盘、拖拽和语义写操作通过全局资源门串行。
- 资源门按 `DesktopInput`、`DesktopSemantic`、`Capture`、`VisionModel` 分资源类。
- 等待类工具不持有桌面输入锁。
- 跨进程锁保留，防止多个 daemon 同时控制桌面。

### 11.4 文件路径

- `desktop_screenshot` / `desktop_window_screenshot` 允许调用方自定义输出路径。
- 输出路径的父目录自动创建，扩展名与所选格式一致。
- 自定义路径不提供沙箱；调用方需自行保证路径安全。
- `desktop_find_text` 的 `dict_name` 只允许安全 basename，禁止 `..`、路径分隔符和绝对路径。
- `desktop_find_image`、`desktop_vision` 的输入图片限制在允许读取目录，或由调用方显式配置。
- 不允许通过工具参数执行脚本、启动任意可执行文件或传任意 shell 命令。
- `target_locator` 的 `launch_ref` 必须来自本地目标目录。

### 11.5 日志与隐私

默认不记录：

- 输入的完整文本
- 剪贴板内容
- 窗口标题全文
- 语义候选中的敏感文本
- 截图内容

诊断日志只记录：

- 工具名
- 耗时
- 错误类别
- 平台
- 请求 ID
- 风险等级
- `dispatch_state`

### 11.6 事件

建议定义稳定事件：

- `session.created`
- `session.closed`
- `tool.started`
- `tool.progress`
- `tool.finished`
- `request.cancelled`
- `daemon.stopping`

事件只包含 `request_id`、工具名、耗时、错误类别、风险等级和状态，
不包含完整输入文本、剪贴板内容、窗口标题或截图。

### 11.7 前台/后台能力矩阵

| 能力 | 前台要求 | 说明 |
|---|---|---|
| 截图、窗口枚举/信息、UIA 观察、OCR/找图找色/键值读取、`verify_state` | 无 | 纯读取；被遮挡窗口可截图（最小化窗口除外）。`desktop_perceive`/`desktop_read_kv` 支持 `capture_id`（会话内截图，不受允许根限制）；UIA 名称的 UTF-8↔GBK 双层编码乱码在读取层自动修复 |
| 窗口 `move`/`resize` | 无 | `SetWindowPos` 直接操作 |
| UIA 语义动作（invoke/toggle/set_checked/select/expand/collapse/focus/set_value/scroll/scroll_into_view/set_range_value） | 可选 | `delivery_mode`：`auto`（默认，后台优先、失败回退前台）/`background`（不抢前台）/`foreground`（必须前台）；后台可用性由控件 provider 决定 |
| 鼠标点击/拖拽 | 可见即可，且需未被遮挡 | 合成输入落在屏幕坐标最上层窗口；携带 `hwnd` 或 capture 引用时强制置前并做遮挡检测（`allow_occluded=true` 跳过） |
| 键盘输入/快捷键 | 必须前台 | `SendInput` 只进焦点窗口；deskctl 先激活目标或返回结构化错误 |
| `restore_foreground=true` | — | 语义动作结束后尽力还原动作前的前台窗口（受前台锁影响可能失败，结果如实返回） |
| 模态弹窗跟随 | — | 目标窗口被禁用时：`target_bind follow_dialog=true` 绑定活动弹窗（响应 `parent_hwnd`）；`desktop_input` 输入重定向并返回 `target_hwnd`；`semantic_observe`/`semantic_action` 自动跟随并报告 `redirected_from` |
| 图像裁剪 / 办公文档 | 无 | `desktop_image_crop`、`desktop_docx_read/save`、`desktop_xlsx_read/save`、`desktop_pptx_read/save` 为纯本地文件处理（zip + OOXML），不触碰输入队列，完全后台执行；文档读取输入受允许图像根约束。文档写出支持常用排版参数：docx 页眉/页脚/页码与表格，xlsx 表头加粗/列宽/数字格式/冻结首行与多工作表，pptx 内嵌 PNG 图片与版式选择（auto/title_bullets/title_image/text_image/title_only）；xlsx 读取可用 `range` 限定区域 |

## 12. 代码组织

### 12.1 首期 workspace

```text
deskctl/
├─ Cargo.toml
├─ crates/
│  ├─ deskctl-protocol/       # 工具 Schema、JSON-RPC、结果封套、错误码
│  ├─ deskctl-core/           # registry、session、platform、risk、verification
│  └─ deskctl-cli/            # 二进制：mcp/serve/call/tool/session/doctor
├─ tests/
│  ├─ contract/
│  ├─ integration/
│  └─ fixtures/
├─ docs/
└─ scripts/
```

首期不拆 `platform`、`vision`、`runtime` crate；等边界稳定后再拆：

- `deskctl-platform`：Win32/macOS/Linux 原生适配器
- `deskctl-vision`：OCR/YOLO/视觉模型
- `deskctl-runtime`：daemon、session、IPC

### 12.2 Cargo features

- `mcp`
- `http`
- `platform-windows`（首期默认）
- `platform-macos`（预留）
- `platform-linux-x11`（预留）
- `vision-paddle`
- `vision-yolo`
- `vision-provider`
- `enhanced-decision`

平台特性必须可叠加；Windows 构建不能要求 macOS/Linux 链接库。

### 12.3 从 Nuphus 抽取

| 参考代码 | 新项目目标 |
|---|---|
| `src/tools/desktop_schemas.rs` | `deskctl-protocol` 的工具 Schema |
| `src/tools/desktop_executors.rs` | `deskctl-core` 的传统工具执行器 |
| `src/tools/semantic_desktop.rs` | `deskctl-core` 的语义工具与会话状态 |
| `src/tools/desktop_verification.rs` | `deskctl-core` 的状态验证 |
| `src/desktop_automation/risk.rs` | `deskctl-core` 的风险元数据 |
| `src/desktop/client.rs` | `deskctl-core` 的 Windows 桌面客户端 |
| `src/desktop/targets*` | `deskctl-core` 的 target/session 服务 |
| `src/desktop_automation/*` | `deskctl-core` 的自动化状态机与平台适配器 |
| `src-tauri/crates/desktop-api/src/vision/*` | 首期放入 `deskctl-core`，后续拆 `deskctl-vision` |
| `src-tauri/crates/desktop-api/src/input/*` | `deskctl-core` 的输入后端 |

明确不抽取：

- `src/tools/desktop_approval.rs`：本项目取消审批
- Tauri 命令、GUI 生命周期、前端资源
- LLM Agent、聊天、工作流、记忆、插件市场
- 与 Nuphus 产品配置强绑定的逻辑

抽取原则：

1. 先复制 Windows 路径和通用状态机，保持行为一致。
2. 把 `crate::` 依赖改为 `deskctl-core` 内部接口。
3. 保留 macOS/Linux 模块源码位置，但用 feature/cfg 排除首期构建。
4. 先不引入 Tauri、LLM SDK、工作流引擎。
5. 保留 Apache-2.0 许可证和原始版权声明。

## 13. 测试策略

### 13.1 单元测试

- Schema 校验
- 参数验证
- 状态机和错误分支
- 会话隔离
- token 失效
- 多 session 并发
- `dict_name` 路径穿越拒绝
- 自定义截图输出路径
- 输入读取路径限制
- 取消、超时、已发送但未知的状态区分

### 13.2 契约测试

- `tools/list` 的工具名和 profile 稳定
- 每个 Schema 都有执行器
- 错误码稳定
- JSON 字段兼容
- MCP 请求/响应格式

### 13.3 Windows 集成测试

- 鼠标、键盘、拖拽
- 窗口枚举、激活、移动、缩放
- 全屏/区域/窗口截图，自定义输出路径
- OCR/YOLO 模型随包可用
- UIA 语义观察、执行、验证
- 多 session 并发下的输入串行化

### 13.4 端到端测试

使用真实应用（如文本编辑器）验证：

- 截图 → 感知 → 点击
- 绑定 → 观察 → 执行 → 验证
- 多个 session 同时截图/观察，输入动作不交叉
- 取消后不产生后续动作
- 自定义截图路径可写入

### 13.5 CI

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- 单元测试和契约测试
- Windows E2E 使用专用 runner 或人工回归清单
- macOS/Linux 首期只做编译占位检查，不做功能验收
- 模型文件使用构建缓存，不在每次 CI 重新下载

## 14. 部署与打包

### 14.1 二进制

首期：

- `deskctl.exe`（Windows x64）

后续：

- `deskctl`（macOS arm64/x64）
- `deskctl`（Linux x64）

平台原生 API 决定了必须分别构建，不承诺单一跨平台二进制。

### 14.2 安装包与模型

- Windows 安装包默认包含 PaddleOCR、YOLO 和 ONNX Runtime。
- 安装包体积较大是预期成本。
- 安装后 `deskctl doctor` 验证模型哈希和运行时。
- 在线下载作为可选修复/升级路径，不作为首次安装的必要条件。
- 离线安装支持本地模型目录或压缩包。
- 模型许可证和再分发条款必须在发布前确认。

### 14.3 配置

建议位置：

- Windows：`%APPDATA%\deskctl\config.toml`
- macOS：`~/Library/Application Support/deskctl/config.toml`
- Linux：`$XDG_CONFIG_HOME/deskctl/config.toml`

环境变量：

- `DESKCTL_SOCKET`
- `DESKCTL_TOKEN`
- `DESKCTL_MODELS_DIR`
- `DESKCTL_CONFIG`
- `DESKCTL_LOG`

## 15. 实施路线

### Phase 0：文档与骨架

- 确认名称 `deskctl`
- 完成架构评审
- 创建三 crate workspace
- 定义协议和结果封套

### Phase 1：Windows 无状态工具

- `tool` 一次性命令
- 截图、窗口、剪贴板、输入
- 自定义截图路径
- 工具 Schema 和错误码
- Windows 真机测试

### Phase 2：Daemon + 多会话

- `serve` / `call`
- session 生命周期
- capture cache、observation token
- 多 session 并发
- 资源门与串行化
- MCP stdio 适配

### Phase 3：Windows 完整能力

- PaddleOCR/YOLO 随包模型
- UIA 语义工具
- 执行后验证
- 多会话压力测试
- Windows 安装包

### Phase 4：macOS（后续）

- AX、截图、输入
- 权限自检
- 真机回归

### Phase 5：Linux X11（后续）

- 窗口、输入、截图
- 明确 Wayland 限制

### Phase 6：发布与运维

- 在线模型更新
- 离线安装包
- 日志/诊断
- 性能与并发调优

## 16. 待确认事项

1. 代码策略：直接复制并解耦，还是先 path 依赖 `nuphus` 做 spike。
2. MCP 是否作为首期唯一对外协议，还是同时提供 HTTP。
3. 默认工具 profile 是否使用 `standard`（26 个）。
4. 默认 session 并发上限和资源门队列策略。
5. PaddleOCR/YOLO 模型的许可证与安装包分发方式。
6. 是否在首期实现 `readonly` profile 和配置化拒绝规则。
7. 项目是否正式定名 `deskctl`，还是保留改名空间。

## 17. 架构决策摘要

- 选择 **Rust**，复用 Nuphus 已经验证的桌面平台代码。
- 选择 **Windows 首期**，macOS/Linux 保留 trait 和模块。
- 选择 **独立 daemon**，因为工具存在大量进程内状态。
- 选择 **多 session 并发**，通过资源门串行化桌面写操作。
- 选择 **无审批**，风险只作为元数据返回。
- 选择 **自定义截图输出路径**，由调用方负责路径安全。
- 选择 **模型随安装包分发**，在线下载作为可选项。
- 选择 **单二进制多子命令**。
- 选择 **MCP stdio + CLI + JSON-RPC**。
- 选择 **平台分层**，Windows 首期落地，其他平台预留。
