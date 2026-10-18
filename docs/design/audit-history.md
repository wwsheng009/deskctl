# 审计与历史查询（Audit & History）——P0

- 文档版本：0.1
- 日期：2026-10-19
- 状态：Draft（待评审）
- 关联：[README.md](README.md)、[data-formats.md](data-formats.md)、[prior-art.md](prior-art.md)、[../architecture.md](../architecture.md)

## 1. 目标

让每一次工具调用都留下**可查询、可归因、可复核**的记录：

- 谁（owner / session）在什么时候调用了什么工具、参数摘要、结果如何（ok/dispatch_state/effect/verification/错误码/耗时）。
- 失败与"不确定"同样入账（`dispatch_state=unknown` 不美化）。
- 对键盘输入、剪贴板等敏感文本默认只留摘要（长度 + sha256），可配置。
- 提供 `deskctl history` 做过滤、聚合与导出；不引入新工具、不改变现有调用协议。

## 2. 设计原则

1. **唯一汇聚点**：所有调用都经过 `Engine::execute_with_session`（daemon `call`、MCP、`deskctl tool` oneshot、bench 均汇入，见 `crates/deskctl-core/src/engine.rs:263`、`:228`）。审计只在这里挂钩一次。
2. **异步、可丢弃**：审计写入绝不阻塞工具执行；队列满时丢弃并计数（`audit_dropped`），宁可少几条记录也不拖慢操作。
3. **失败也记**：包括参数校验失败（`invalid_params`）、策略拒绝（`policy_denied`）、取消、超时、内部错误。
4. **脱敏优先**：默认不落敏感文本全文；`args` 只存白名单字段 + 体积上限。
5. **本地证据，不是安全边界**：文件可被本机用户修改；文档明确其定位。
6. **协议零变更**：审计是 daemon 侧行为，工具 Schema、结果封套、MCP 协议都不变。
7. **日志与审计同源脱敏**：`tracing` 日志（`DESKCTL_LOG`）与审计共用同一套脱敏规则（密码字段、键名单、文本 digest 策略）；默认 `info` 级别不输出 args/文本明文——避免"审计干净但日志泄漏"。

## 3. 架构与数据流

```text
                     ┌─ daemon call ──┐
CLI / MCP / oneshot ─┤                ├─► Engine::execute_with_session ─► execute_inner
                     └─ request/cancel┘             │                        │
                                                    │ 完成后                  │ 执行中（现状不变）
                                                    ▼                        ▼
                                   AuditSink.record(AuditRecord)      会话隔离 / 资源门 / 取消
                                                    │
                                    mpsc(4096, 有界) │  （daemon 内 tokio 任务）
                                                    ▼
                                       单写者任务：序列化 JSONL → 追加写
                                                    │
                        %LOCALAPPDATA%\deskctl\audit\audit-YYYY-MM-DD.jsonl
                                                    ▲
                                                    │ 只读扫描（不经 daemon）
                                    deskctl history --session/--owner/--tool/--since/--json
```

要点：

- **挂钩位置**：`execute_with_session` 在拿到 `ToolResult` 后（含 spawn 失败/panic 分支）调用 `audit.record(...)`；`execute_oneshot` 自动获得覆盖（owner="oneshot"）。
- **不记录**：`history` 自身、`session/*` 管理方法与 `doctor`/`schema`（它们不经工具执行入口，无需入账；`session/create` 可选择记一条 `kind="session"`，见 §4）。
- **写者模型**：daemon 进程内单写者任务串行追加；oneshot 进程（`deskctl tool`）独立写同一目录——采用"单文件追加 + 每行完整 JSON"（行级原子性由 O_APPEND 保证），跨进程不交错破坏行结构（超长行除外，见 §5 体积上限）。

## 4. 审计记录模型（v1）

一行 = 一条记录，UTF-8，`\n` 结尾。规范见 [data-formats.md](data-formats.md#1-审计记录deskctl-audit1)。

| 字段 | 类型 | 来源 | 说明 |
|---|---|---|---|
| `v` | int | 常量 | 记录版本（=1），用于兼容升级 |
| `ts` | string | `chrono::Local` | RFC3339 含毫秒与本地时区 |
| `kind` | string | 常量 | `tool_call` / `session`（可选） |
| `request_id` | string? | `ResultMeta.request_id` | daemon 请求 id（`req:` 前缀）或 `oneshot-<uuid>` |
| `session_id` | string? | `ResultMeta.session_id` | 会话 id |
| `owner` | string | `Session.owner` | 身份标签（P1 起来自 `--owner`/clientInfo） |
| `source` | string | 采集点 | `daemon` / `mcp` / `oneshot` |
| `tool` | string | `ToolName.as_str()` | 工具名 |
| `risk` | string | `ToolMeta.risk` | `read_only`…（与 `RiskClass` 一致） |
| `args` | object? | 入参 | 经白名单/截断/脱敏（§5）；`basic` 模式为 null |
| `args_digest` | string | sha256 | 完整入参的摘要（始终记录，便于比对） |
| `args_truncated` | bool | 常量 | 参数超限被截断 |
| `duration_ms` | int | 计时 | 从进入 execute 到返回 |
| `ok` | bool | `ToolResult.ok` | — |
| `dispatch_state` | string | `ResultMeta` | `not_sent/sent/partial/unknown` |
| `effect` | string | `ResultMeta` | `read/unverifiable/external_commit/confirmed/refused/partial` |
| `verification` | string | `ResultMeta` | `not_applicable/unknown/satisfied/progress/no_change` |
| `retryable` | bool | `ResultMeta` | — |
| `error` | object? | `ToolResult.error` | `{code, message(≤512), retryable}` |
| `audit_dropped` | int? | 计数器 | 仅在被丢弃记录后出现的周期性提示行 |

示例（`full` 模式，节选自真实字段结构）：

```json
{"v":1,"ts":"2026-10-19T01:23:45.123+08:00","kind":"tool_call","request_id":"req:1f2e...","session_id":"sess-9a3c...","owner":"agent:sap-inspector","source":"daemon","tool":"desktop_input","risk":"bounded_write","args":{"mode":"type","hwnd":5702916,"send":"enter","text_len":6,"text_sha256":"9f86d081..."},"args_digest":"sha256:71c0...","args_truncated":false,"duration_ms":42,"ok":true,"dispatch_state":"sent","effect":"confirmed","verification":"unknown","retryable":false,"error":null}
```

## 5. 脱敏与体积控制

### 5.1 字段白名单（`full` 模式仍适用）

| 工具/字段 | 处理 |
|---|---|
| `desktop_input.text`（键盘文本） | 按 `DESKCTL_AUDIT_TEXT` 策略：`digest`（默认）= `text_len` + `text_sha256`；`none` = 两者都不记；`full` = 原文（仅建议在隔离环境） |
| `desktop_clipboard_write.text` | 同上 |
| UIA 密码字段（`IsPassword=true` / `AXSecureTextField`） | **永不记录**（在 digest/full 策略之前先屏蔽，连长度与摘要都不落） |
| 参数键命中名单（`api_key`/`apikey`/`token`/`authorization`/`password`/`passwd`/`secret`，大小写不敏感） | 值替换为 `"<redacted>"`（不落明文、不落长度/摘要），并在记录中置 `args_redacted:true` |
| `path` / `image_path` / `template_path` | 记录完整路径（运维需要定位产物） |
| `screenshot` 等图像内容 | 不记录二进制；只记路径/尺寸/字节数（如已在结果中） |
| `args` 中其它字符串 | 值长度 ≤256 原样；>256 截断并置 `args_truncated=true` |
| `args` 总体积 | 序列化后 ≤4 KiB；超出按"先截断长字符串、再省略列表尾部"降级 |
| `error.message` | ≤512 字符 |
| 未知字段 | `full` 模式下列名保留、值按字符串规则处理 |

### 5.2 模式

`DESKCTL_AUDIT`：

- `off`：完全关闭（不创建目录、不写文件；`history` 返回空并提示）。
- `basic`（默认）：不记 `args`（只记 `args_digest` + 工具名 + 结果元数据）。
- `full`：按 §5.1 记录脱敏后的 `args`。

### 5.3 日志（tracing）规则

| 规则 | 说明 |
|---|---|
| 脱敏同源 | 日志字段复用 §5.1 的处理函数；禁止绕过（如 `debug!("args={:?}", args)` 之类直接打印原始入参） |
| 默认级别 | `info`；`debug`/`trace` 需显式开启，且仍不含 `text`/密码/键名单明文 |
| 文本策略 | `DESKCTL_AUDIT_TEXT=full` 只影响**审计文件**，日志仍按 `digest` 处理（`DESKCTL_LOG_TEXT` 单独开关，默认 `digest`） |
| 内容边界 | 日志用于诊断（启动、目录、错误、限额、审计丢弃计数）；**不承担审计职责**（对账以 §4 记录为准） |

## 6. 存储、轮转与保留

- 目录：`%LOCALAPPDATA%\deskctl\audit\`（沿用 `config::data_dir()`；`DESKCTL_AUDIT_DIR` 可覆盖）。
- 文件：`audit-YYYY-MM-DD.jsonl`（本地日期分片）。单文件超过 `DESKCTL_AUDIT_MAX_FILE_MB`（默认 64）后切换到 `audit-YYYY-MM-DD.N.jsonl`。
- 保留：`DESKCTL_AUDIT_RETAIN_DAYS`（默认 30）+ `DESKCTL_AUDIT_MAX_MB`（默认 256）。
  - daemon 启动时执行一次清理；之后每天首次写入前再检查。
  - 清理策略：先删最旧的完整分片文件；仍在超限则从最旧文件头部截断（重写该文件，保留尾部）。
- 磁盘失败降级：审计目录不可写 → 工具照常执行；stderr 打一次告警（`audit disabled: <reason>`），进程内 `audit_state=disabled`，`history` 提示。
- 不加密、不签名（见 §2.5）；文件权限继承用户目录默认 ACL。
- **run 产物**（脚本执行相关）：`%LOCALAPPDATA%\deskctl\runs\<run_id>\`（含 `report.json`、`expanded.jsonl`、可选的 `script.rhai`/脚本副本）；
  与审计共用保留语义：`DESKCTL_RUNS_RETAIN_DAYS`（默认 30）、`DESKCTL_RUNS_MAX_MB`（默认 256），清理最旧 run 目录；
  产物中的文本同样遵守 §5.1 脱敏（`args` 快照按审计策略处理）。

## 7. 查询接口：`deskctl history`

CLI 直读审计文件（不经 daemon，daemon 不在也能查），只读扫描，流式过滤。

```text
deskctl history [--session <id>] [--owner <id>] [--tool <name>] [--run <run_id>] [--since <rfc3339|1h|today>]
                [--until <rfc3339|now>] [--failed-only] [--kind tool_call|session]
                [--limit 50] [--json] [--dir <path>] [--summary]
```

| 选项 | 语义 |
|---|---|
| `--session` / `--owner` / `--tool` | 精确匹配（owner/tool 支持前缀 `agent:sap-*`） |
| `--since` / `--until` | 相对时间（`30m/2h/7d`）、`today`、RFC3339；默认最近 24h |
| `--failed-only` | 仅 `ok=false` 或 `dispatch_state in (partial, unknown)` |
| `--run <run_id>` | 列出某次脚本 run 的全部记录（等价于 `request_id` 前缀 `<run_id>:` 匹配；用于失败接手时回看已完成步骤） |
| `--limit` | 最新 N 条（默认 50；`0`=全部，流式输出） |
| `--summary` | 聚合视图：按 tool/owner 计数、失败率、P50/P95 耗时（见下） |
| `--json` | 输出原始 JSONL 行（可管道给 jq） |

人类可读输出示例：

```text
2026-10-19 01:23:45 +08:00  agent:sap-inspector  sess-9a3c…  desktop_input            sent/confirmed  42ms   ok
2026-10-19 01:23:47 +08:00  agent:sap-inspector  sess-9a3c…  desktop_read_kv          read/unknown    118ms  ok  (32 pairs)
2026-10-19 01:23:49 +08:00  agent:sap-inspector  sess-9a3c…  desktop_mouse            not_sent/refused  3ms  FAIL target_unavailable
```

`--summary` 输出字段：`by_tool[]{tool,calls,ok,failed,not_sent,unknown,p50_ms,p95_ms}`、`by_owner[]` 同构、`window{since,until,total_calls,failure_rate}`。

退出码：正常 0；无记录 0（输出 "no records"）；`--since` 解析失败 2（`invalid_params` 语义）；审计目录不可读 7（`policy/permission`）。

## 8. 配置汇总

| 环境变量 | 默认 | 说明 |
|---|---|---|
| `DESKCTL_AUDIT` | `basic` | `off`/`basic`/`full` |
| `DESKCTL_AUDIT_DIR` | `%LOCALAPPDATA%\deskctl\audit` | 目录覆盖 |
| `DESKCTL_AUDIT_TEXT` | `digest` | `none`/`digest`/`full`（键盘/剪贴板文本） |
| `DESKCTL_AUDIT_QUEUE` | `4096` | 队列深度（记录数） |
| `DESKCTL_AUDIT_MAX_FILE_MB` | `64` | 单分片上限 |
| `DESKCTL_AUDIT_MAX_MB` | `256` | 目录总量上限 |
| `DESKCTL_AUDIT_RETAIN_DAYS` | `30` | 保留天数 |
| `DESKCTL_LOG` | 空 | 初始化 `tracing`（`error`/`warn`/`info`/`debug`），修复"依赖未初始化"现状 |
| `DESKCTL_LOG_TEXT` | `digest` | 日志中的文本策略（`none`/`digest`/`full`）；独立于 `DESKCTL_AUDIT_TEXT` |
| `DESKCTL_RUNS_RETAIN_DAYS` | `30` | run 产物目录保留天数 |
| `DESKCTL_RUNS_MAX_MB` | `256` | run 产物目录总量上限 |

## 9. 性能影响与测量

- 记录构造：微秒级（sha256 只对 text 字段做一次；`digest` 模式 O(len) 一次性）。
- 落盘：异步单写者；正常负载（<100 qps）队列水位接近 0。
- 门禁：对比 `scripts/perf-baseline.ps1` 基线（AC-PERF-001..009），**工具调用 p95 回退 <5%**；审计开启 vs 关闭各测一次，结果写入 `performance-baseline.md`。
- 丢弃语义：队列满 → 丢弃当前记录、`audit_dropped+=1`；下一次成功写入时附加 `audit_dropped` 计数行（`kind="audit_health"`）。

## 10. 验收标准（AC-AUD-001..008）

| 编号 | 通过标准 | 验证方法 |
|---|---|---|
| AC-AUD-001 | daemon 每次工具调用结束即在当日 JSONL 追加**恰好一条**记录（含失败/取消）；字段与 §4 表一致 | 集成：daemon 跑 N 次成功 + 2 次失败 + 1 次取消，`history --json` 计数符合 |
| AC-AUD-002 | `basic` 模式不含 `args`；`full` 模式含脱敏 `args`；两种模式 `args_digest` 均可复算一致 | 单测 + 手工复算 sha256 |
| AC-AUD-003 | `DESKCTL_AUDIT_TEXT=digest` 时键盘文本仅存 `text_len`/`text_sha256`，原文不出现在文件中 | 安全：写入敏感样例后全文检索审计文件应无命中 |
| AC-AUD-004 | `history` 支持 `--session/--owner/--tool/--since/--failed-only/--limit/--json/--summary`，过滤结果与手工比对一致 | 集成：预置记录集，逐项断言 |
| AC-AUD-005 | 轮转与保留生效：超过 `MAX_FILE_MB` 切分片；超过 `RETAIN_DAYS`/`MAX_MB` 的老文件被清理且最新记录不丢 | 单测：小阈值 + 伪造时间戳文件 |
| AC-AUD-006 | 审计目录不可写时工具照常成功，仅告警一次；`audit_state=disabled` 可观测 | 集成：只读目录模拟 |
| AC-AUD-007 | 脱敏增强：密码字段（UIA `IsPassword=true`）值永不落盘；键名单（api_key/token/authorization/password/secret）命中的参数值替换为 `<redacted>` 并置 `args_redacted=true`；审计文件全文检索无命中 | 安全脚本：写入样例敏感值后全文检索 |
| AC-AUD-008 | 日志与 run 产物：tracing 默认级别不含 args/文本明文（`DESKCTL_AUDIT_TEXT=full` 不影响日志）；run 目录位于 `%LOCALAPPDATA%\deskctl\runs\<run_id>\` 且受保留策略清理；`history --run <id>` 能取回该 run 的全部步骤记录 | 集成：开启 debug 日志后检索样例敏感值应为空；`history --run` 计数与 report 步骤数一致 |

## 11. 测试计划

- 单测（`crates/deskctl-core/src/audit.rs`）：记录构造/脱敏/截断/轮转/清理/丢弃计数。
- 集成（`crates/deskctl-cli/tests/audit_history.rs`）：daemon 起停 + `history` 全选项；`--json` 与 jq 兼容。
- 安全（并入 `scripts/security-check.ps1`）：AC-AUD-003 全文检索断言。
- 性能：`perf-baseline.ps1` 增加 `--audit on/off` 两轮对比。
- E2E-011 扩展：巡检流程结束后 `history --session` 能回放出全部步骤（与录制脚本步骤数比对）。

## 12. 实施任务分解（D1）

| # | 任务 | 文件 | 预估 |
|---|---|---|---|
| 1 | `AuditRecord` 模型 + 脱敏/摘要/截断 | `crates/deskctl-core/src/audit.rs`（新增） | 0.5d |
| 2 | 异步写者 + 轮转/清理 + 降级 | 同文件 + `config.rs`（新增 env） | 0.5d |
| 3 | Engine 挂钩（execute_with_session 完成后记录） | `engine.rs` | 0.25d |
| 4 | `deskctl history` 子命令（过滤/汇总/JSON） | `crates/deskctl-cli/src/main.rs` + 新 `history.rs` | 0.5d |
| 5 | tracing 初始化（`DESKCTL_LOG`） | `main.rs`（`tracing-subscriber` 已声明） | 0.25d |
| 6 | 单测/集成/安全/性能 | tests + scripts + docs | 1d |
| 7 | 文档与 AC 登记 | acceptance-criteria / test-plan / implementation-status / known-limitations | 0.5d |

## 13. 开放问题

1. `DESKCTL_AUDIT` 默认值：`basic`（推荐）是否足够，还是默认 `off` 由部署方开启？
2. 队列满策略：丢弃+计数（当前设计）vs 背压阻塞（更完整但影响延迟）——建议维持丢弃。
3. `session` 事件（create/close）是否入账：建议入账（`kind="session"`），便于归因窗口存活期。
4. 是否需要 `history --follow`（tail -f 语义）：建议 D1 不做，留待 D3 与录制器联动。
5. 脱敏增强已并入（R5）：密码字段与键名单见 §5.1，验收见 AC-AUD-007。仍待定：可选 OTLP 导出（后续迭代），以及**可选**哈希链 tamper-evident 档位（参考 agentlens，与 §2.5"本地证据、非防篡改"定位相关，需单独评审）。
