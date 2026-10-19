# Agent 身份与多调用方归因（Identity & Attribution）——P1

- 文档版本：0.1
- 日期：2026-10-19
- 状态：已实施 2026-10-19（AC-AGT-001..004；证据见 [../implementation-status.md](../implementation-status.md)）
- 关联：[README.md](README.md)、[audit-history.md](audit-history.md)、[../architecture.md](../architecture.md)

## 1. 目标

- 多 Agent / 多调用方并发时，能回答"**是谁**在什么时候做了这件事"（当前只能回答"哪个 session"）。
- 提供按身份的可选限额与诊断，不改变现有隔离与串行模型。
- 明确边界：**owner 是运维标签，不是鉴权凭据**。

现状证据：

- `Session.owner` 字段已存在并出现在 `session list/inspect`（`crates/deskctl-core/src/session.rs:116,202`），但 daemon 固定写 `"daemon-client"`（`crates/deskctl-cli/src/daemon.rs:185`）、MCP 固定写 `"mcp"`（`crates/deskctl-cli/src/mcp.rs:28`）。
- `session create` 请求参数只带 `profile`（`crates/deskctl-cli/src/main.rs:270-276`）。

## 2. 身份来源与映射

| 调用方 | 来源 | owner 取值 | 缺省 |
|---|---|---|---|
| CLI `session create` | `--owner <agent-id>`（新增参数） | 原样（经 §3 规范化） | `daemon-client` |
| MCP server | `initialize.params.clientInfo.name`（+`version` 记入审计不做身份） | `mcp:<name>`（name 缺失时 `mcp:unknown`） | `mcp` |
| `deskctl tool`（一次性） | `--owner`（新增，可选） | 原样 | `oneshot` |
| bench / 内部 | 固定 | `bench` | — |

MCP 的 `clientInfo.name` 在映射前统一小写并按 §3 字符集清洗（非法字符替换为 `-`、截断到 60 字符、
空名回退 `unknown`）；`clientInfo.version` 只写入 MCP 服务日志（stderr，受 `DESKCTL_LOG` 控制），
不参与身份。

owner 只在**创建会话时**确定；同一会话内所有调用的审计记录都使用该 owner（由 `Session.owner` 带出，调用方无需每次传参）。

## 3. 规范化（所有入口统一）

- 字符集：`[a-zA-Z0-9._:@-]`；长度 1..64；统一小写（保留 `:` 分层语义，如 `agent:sap-inspector`）。
- 非法字符：拒绝并返回 `invalid_params`（`session/create` 或 CLI 参数解析阶段），不静默清洗。
- 保留前缀（禁止伪造）：`daemon-client`、`mcp`、`mcp:*`、`oneshot`、`bench`、`system*` —— 调用方显式传入时报 `invalid_params`（`owner is reserved`）。
- 语义建议（非强制）：`agent:<name>`（LLM/自动化）、`user:<name>`（人工）。

## 4. 传播与会话行为

- `session/create` 参数新增 `owner`（可选）；返回体不变（`session_id/session_token/profile/owner`，owner 已在 `SessionInfo` 中）。
- 会话隔离与 token 鉴权**完全不变**：owner 不参与 `require_session` 校验；不同 owner 的会话仍互不可见 token（AC-SESS-002 不变）。
- `session list` 输出（JSON 数组）每行已含 owner；新增 `--owner <label>` 精确过滤（JSON 契约不变；未做表格输出）。
- `doctor`（daemon 对象）新增 `owner_limits`：`max_sessions_per_owner`/`fairness` 与"标签（非鉴权）"提示语。
- 审计记录 `owner` 字段来自会话（见 [data-formats.md](data-formats.md#1-审计记录deskctl-audit1)）。

## 5. 限额与公平（可选，默认关闭）

| 配置 | 默认 | 行为 |
|---|---|---|
| `DESKCTL_OWNER_MAX_SESSIONS` | `0`（不额外限制） | >0 时按 owner 限制活跃会话数；超限 `resource_busy`，消息含 owner 与现有限额 |
| `DESKCTL_OWNER_FAIRNESS` | `on` | 资源门保持 FIFO（现有 `tokio::Semaphore` 公平语义）；仅作为配置上报（`doctor.owner_limits`） |

要点：资源门是**全局**（Vision=1/Capture=4/Semantic=1/Input=1），不做 per-owner 拆分，避免小 agent 互相饿死；公平性依赖 FIFO + 现有 AC-SESS-009 无饥饿验证。
审计中的 per-acquire `wait_ms` 记录**本期未实现**（审计记录格式 v1 已冻结，新增字段需走格式版本流程）。

## 6. 观测

- `deskctl history --owner agent:sap-*`（精确或 `prefix*` 通配），`--summary` 输出 `by_owner[]`（calls/ok/failed/not_sent/unknown/p50/p95）。
- `session list --owner <id>`（精确过滤）。
- `doctor` 打印 `owner_limits`（当前配置 + 标签说明）便于排查。

## 7. 验收标准（AC-AGT-001..004）

> 已实施并验证 2026-10-19：判据测试在 `crates/deskctl-cli/tests/agent_identity.rs`（4 项契约）
> 与 `crates/deskctl-core/src/session.rs` 单测（`validate_owner` 规范化/保留名）；
> 证据汇总见 [../implementation-status.md](../implementation-status.md) §2。

| 编号 | 通过标准 | 验证方法 |
|---|---|---|
| AC-AGT-001 | `session create --owner agent:x` 返回/`session list` 显示该 owner；非法字符与保留前缀被拒（`invalid_params`） | 集成：合法/非法样例各若干 |
| AC-AGT-002 | MCP `initialize` 的 `clientInfo.name` 映射为 owner（`mcp:<name>`）；缺失时 `mcp:unknown` | 集成：MCP smoke 增加 clientInfo 断言 |
| AC-AGT-003 | 审计与 `history --owner` 能按身份归因；同一会话内跨调用 owner 一致 | 集成：两个 owner 并发跑调用后过滤断言 |
| AC-AGT-004 | `DESKCTL_OWNER_MAX_SESSIONS=2` 时同一 owner 第 3 个会话返回 `resource_busy`，其他 owner 不受影响 | 集成 + 安全脚本 |

## 8. 实施任务分解（D2）

| # | 任务 | 文件 | 预估 |
|---|---|---|---|
| 1 | `session/create` 接受/校验 owner；保留前缀表 | `daemon.rs`、`session.rs`（校验函数）、`main.rs`（`--owner`） | 0.5d |
| 2 | MCP clientInfo 提取与映射 | `mcp.rs` | 0.25d |
| 3 | `session list` 过滤 + CLI OWNER 列 | `engine.rs`、`main.rs` | 0.25d |
| 4 | 可选 owner 限额 | `engine.rs`、`config.rs` | 0.5d |
| 5 | 审计 `owner` 打通 + `history --owner`/`by_owner` | 随 D1 实现自然覆盖 | 0.25d |
| 6 | 测试与文档 | tests + acceptance/status | 0.75d |

依赖：D1 完成（审计字段与 `history` 就绪）后开始，避免重复改同一批文件。

## 9. 风险

| 风险 | 说明 | 缓解 |
|---|---|---|
| owner 被当作安全边界 | 同机任意进程都能用任意（未保留）owner 创建会话 | 文档与 `doctor.owner_limits` 标注"标签（非鉴权）"；安全模型仍以 token + 本机 ACL 为准 |
| 保留前缀伪造 | 第三方冒充 `daemon-client` 污染审计 | §3 保留前缀拒绝 |
| 限额误伤 | 单 Agent 起多会话的工作流被 `OWNER_MAX_SESSIONS` 阻断 | 默认关闭；开启时错误消息给出当前值与建议 |