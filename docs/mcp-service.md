# Deskctl MCP 服务设计

- 文档版本：0.1
- 日期：2026-10-18
- 状态：Draft
- 关联文档：[architecture.md](architecture.md)、[llm-skills.md](llm-skills.md)、[acceptance-criteria.md](acceptance-criteria.md)

## 1. 目标

`deskctl mcp` 提供一个标准 MCP stdio server，让支持 MCP 的 LLM 客户端直接使用
Deskctl 的桌面工具，不需要了解 daemon、session、capture_id 或 JSON-RPC 内部细节。

首期目标：

- MCP stdio 可用
- 工具列表按 profile / 平台过滤
- 工具调用支持结构化错误
- 支持取消和长任务进度
- 同一 MCP 连接隐式绑定一个 daemon session
- 不暴露审批、session 管理或内部管理方法给模型

## 2. 部署形态

```text
MCP 客户端
  │ stdio (JSON-RPC)
  ▼
deskctl mcp
  │ Named Pipe / Unix Domain Socket
  ▼
deskctl serve
  │
  ├─ ToolRegistry
  ├─ 多 session
  ├─ OCR/YOLO
  └─ Windows 平台层
```

默认：

- `deskctl mcp` 连接已有 daemon。
- 如果没有可用 daemon，默认自动启动一个（`--auto-start` 默认开启）。
- MCP 进程退出只关闭自己的 session，不关闭共享 daemon。
- daemon 空闲超时或显式 `deskctl shutdown` 才退出。

可配置：

```text
deskctl mcp \
  --socket <path> \
  --token-file <path> \
  --profile standard \
  --auto-start \
  --no-auto-start
```

## 3. MCP 生命周期

### 3.1 启动

1. 读取配置和 profile。
2. 连接 daemon；必要时自动启动。
3. 创建隐式 session。
4. 进入 MCP stdio 循环。
5. 不向 stdout 输出任何非协议内容。

### 3.2 initialize

请求：

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "<client-supported-version>",
    "capabilities": {},
    "clientInfo": { "name": "example", "version": "1.0" }
  }
}
```

响应：

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": "<negotiated-version>",
    "capabilities": {
      "tools": { "listChanged": true },
      "prompts": { "listChanged": false },
      "logging": {}
    },
    "serverInfo": {
      "name": "deskctl",
      "version": "0.1.0"
    }
  }
}
```

规则：

- 协议版本以客户端和 server 共同支持的版本为准。
- 主版本不兼容时返回错误并退出。
- `initialize` 之后才接受 `tools/list` / `tools/call`。
- 不把 `session_id` 放进 MCP 初始化结果。

### 3.3 连接关闭

- stdin EOF、客户端退出或 MCP 进程被终止时，关闭 session。
- session 关闭后所有 capture/observation/target token 失效。
- 如果 daemon 是 MCP 进程自动启动的，可按 idle timeout 回收；默认不立即关闭共享 daemon。

## 4. 工具暴露

### 4.1 tools/list

按以下顺序过滤：

1. 平台可用性（首期 Windows）
2. feature（`vision-provider`、`enhanced-decision`）
3. profile（`minimal` / `standard` / `full` / `readonly`）
4. 安全策略（例如禁用某些工具）

默认 `standard` 暴露 26 个核心工具；`full` 暴露 28 个。

`tools/list` 中每个工具包含：

- `name`
- `description`
- `inputSchema`
- 可选 `outputSchema`（如果客户端支持）
- 可选 annotations（只读、破坏性等）

工具名必须保持 `desktop_*` 前缀。

### 4.2 不暴露的内容

MCP `tools/list` 不包含：

- `approve` / `reject`
- `session/create` / `session/close`
- `shutdown`
- `cancel`（取消通过 MCP cancellation 协议）
- 任意 shell、脚本、文件系统或网络工具

管理接口只通过 `deskctl` CLI 提供。

### 4.3 工具调用

请求：

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "desktop_screenshot",
    "arguments": {
      "path": "C:\\temp\\shot.png"
    }
  }
}
```

成功响应：

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "{\"ok\":true,\"data\":{\"capture_id\":\"cap-...\",\"path\":\"C:\\\\temp\\\\shot.png\"},\"error\":null,\"meta\":{\"dispatch_state\":\"not_sent\",\"retryable\":false}}"
      }
    ],
    "isError": false
  }
}
```

工具错误：

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "{\"ok\":false,\"error\":{\"code\":\"stale_observation\",\"message\":\"observation expired\",\"retryable\":true},\"meta\":{\"requires_observation\":true}}"
      }
    ],
    "isError": true
  }
}
```

约定：

- 协议级错误（解析失败、未知方法、参数结构错误）用 JSON-RPC error。
- 工具业务错误用 `result.isError=true` + 统一 JSON 封套。
- 文本 JSON 是规范形态；支持 structuredContent 的客户端可以额外提供结构化字段。
- 截图等大对象只返回路径，不返回 base64。
- 单次响应有大小上限；语义候选使用 cursor 分页。

## 5. 错误映射

| 内部错误 | MCP 表现 |
|---|---|
| `invalid_params` | JSON-RPC `-32602` 或 `isError` + code |
| `unknown_tool` | JSON-RPC `-32601` 或 `isError` |
| `platform_not_supported` | `isError=true`，code 保留 |
| `permission_denied` | `isError=true` |
| `policy_denied` | `isError=true` |
| `stale_capture` | `isError=true`，`retryable=true` |
| `stale_observation` | `isError=true`，`requires_observation=true` |
| `resource_busy` | `isError=true`，`retryable=true` |
| `cancelled` | JSON-RPC cancellation 或 `isError=true` |
| `timeout` | `isError=true`；如已发送，`dispatch_state=unknown` |
| `model_unavailable` | `isError=true` |
| `dispatch_unknown` | `isError=true`，`retryable=false` |
| `internal` | JSON-RPC `-32603` 或 `isError=true` |

## 6. 取消与进度

### 6.1 取消

支持 MCP cancellation：

- 客户端发送取消通知，携带原 request id。
- server 将取消传播到 daemon 的 `request/cancel`。
- 已发送动作不能回滚；返回 `dispatch_state=sent/unknown`。
- 取消等待类工具时返回 `cancelled`。
- 取消不能产生后续动作。

### 6.2 进度

长任务可以发送 progress：

- OCR/YOLO
- 长等待
- 语义观察分页
- 大窗口枚举

进度事件包含：

- `progressToken`
- `progress`
- `total`（可选）
- 简短消息

进度事件不得包含窗口标题、输入文本或截图内容。

## 7. Notifications

建议支持：

- `notifications/message`：日志/诊断（默认关闭或低级别）
- `notifications/progress`：长任务进度
- `notifications/tools/list_changed`：profile/feature 变化时
- `notifications/cancelled`：请求取消

不发送审批通知，因为本项目没有审批。

## 8. Prompts 与 Skills

MCP server 可以把 Deskctl 的 LLM skills 暴露为 MCP prompts：

| Prompt 名称 | 对应 skill |
|---|---|
| `desktop-basic` | 截图、窗口、鼠标、键盘 |
| `desktop-semantic` | UIA 目标绑定、观察、执行、验证 |
| `desktop-recovery` | 过期 token、窗口变化、dispatch unknown |
| `desktop-text` | 文本输入、剪贴板、长文本 |

`prompts/get` 返回 skill 的 `SKILL.md` 内容或精选版本。

规则：

- prompt 只是使用说明，不是工具。
- prompt 不得要求模型自行批准动作。
- prompt 不得要求模型猜坐标或持久化 hwnd。
- prompt 内容有版本号，与工具 Schema 同步。

## 9. 安全

- MCP stdio 继承当前用户权限。
- 默认只连接本地 daemon，不监听公网。
- MCP server 不读取/打印 access token。
- 工具参数不包含密码、API key 或任意脚本。
- 日志只写 stderr，stdout 保持协议纯净。
- 允许自定义截图输出路径；调用方负责路径安全。
- 输入图片/字库路径仍受限制。
- 无审批：任何已暴露工具都可直接执行；依赖 profile 和本地访问控制。

## 10. 版本与兼容

- MCP 协议版本、Deskctl server 版本、工具 Schema 版本分别声明。
- 工具新增字段为兼容变更；删除/重命名字段需要新主版本。
- `tools/list` 的工具名稳定；工具移除需要弃用期。
- `desktop_vision` 和 `desktop_agent_step` 的出现由 feature 决定。
- 客户端应容忍未知字段。

## 11. 实现结构

```text
deskctl-cli/src/mcp/
├─ mod.rs          # stdio 循环
├─ protocol.rs     # JSON-RPC 消息
├─ tools.rs        # tools/list、tools/call 映射
├─ prompts.rs      # skills 暴露
├─ progress.rs     # 进度/取消
└─ errors.rs       # MCP 错误映射
```

MCP 层只做协议适配，不直接操作 Win32/UIA。

## 12. MCP 验收

必须覆盖：

- `initialize` 成功和版本不匹配
- `tools/list` 的 26/28 工具差异
- `tools/call` 成功/失败/isError
- 未知工具
- 参数错误
- 取消
- 进度
- stdout 纯净
- 两个 MCP 客户端并发
- 退出后 session 清理
- 无 approval 方法
- prompts 与 skills 一致
- 大结果分页

详细测试见 [test-plan.md](test-plan.md) 第 13 节。
