# Deskctl 运维增强方案（design/）

- 文档版本：0.1
- 日期：2026-10-19
- 状态：Draft（待评审）
- 范围：操作审计与历史、多 Agent 身份、操作录制/回放、检查点与补偿
- 关联文档：[../architecture.md](../architecture.md)、[../acceptance-criteria.md](../acceptance-criteria.md)、[../test-plan.md](../test-plan.md)、[../implementation-status.md](../implementation-status.md)、[../known-limitations.md](../known-limitations.md)

## 1. 背景：现状与差距

2026-10-19 对代码逐项核查（引用均为真实位置）后的结论：

| 关注点 | 现状 | 证据 |
|---|---|---|
| 操作历史/审计 | **无**：`tracing-subscriber` 声明了依赖但全仓没有任何初始化调用；请求结束即丢 | `crates/deskctl-cli/Cargo.toml:21`；全仓无 `init/subscriber` 调用 |
| 单次调用可追溯 | 有：每个响应带 `request_id`/`session_id`/`dispatch_state`/`effect`/`verification` | `crates/deskctl-protocol/src/result.rs` |
| 进行中请求取消 | 有（协作式、仅内存）：`request_id → CancelFlag` | `crates/deskctl-cli/src/daemon.rs:253-290`；`deskctl cancel --request-id`（`main.rs:72,329`） |
| 录制/回放 | **无**；但语义动作 `{locator, action}` 可跨进程重放（AC-SEM-006），候选响应已带 `workflow_step` | `crates/deskctl-core/src/tools/semantic.rs:131-137` |
| 多 Agent 区分 | 有会话隔离 + 全局资源门 + 公平性；**owner 固定写死**，无法区分"哪个 Agent" | `daemon.rs:185`（`"daemon-client"`）、`mcp.rs:28`（`"mcp"`）；`resource_gate.rs`（Vision=1/Capture=4/Semantic=1/Input=1） |
| 事务/会话 | 有会话（create/list/inspect/close、token、GC、重启失效）与请求级事务语义（dispatch_state、幂等、前置校验、可选后置校验）；**无多步事务/回滚** | `session.rs`、AC-SESS-006、AC-SEM-009/014/015 |

## 2. 目标与非目标

目标：

- **可见**：任何写入/关键读取调用都有可查询的历史（谁、何时、对哪个会话、调了什么、结果如何）。
- **可信**：失败与"不确定"（`dispatch_state=unknown`）同样入账，不美化。
- **可控**：多 Agent 场景能按身份归因、限制与诊断；高风险重放默认拒绝，需显式参数。
- **可复现**：成功的操作序列可导出为脚本并重放；重放前做 preflight，失败可定位到具体步骤。
- **可补偿**：关键步骤前可保存检查点，失败时给出可执行的补偿建议。

非目标（明确不做，避免过度设计）：

- **不做审批流**：延续项目决策，不引入 approve/reject；风险控制通过"默认拒绝高风险 + 显式参数"实现。
- **不做真事务/回滚**：桌面与外部系统的副作用不可回滚；只做检查点 + 补偿建议。
- **不做录屏**：录制的是**调用序列**（工具/参数/结果）与按需截图，不是视频。
- **审计不是安全边界**：owner 只是标签（非鉴权），审计文件是本地运维证据，不承诺防篡改。
- **不新增"desktop_batch"工具**：批处理由 `desktop_script_run` 覆盖，避免概念重复。

跨切面原则（评审后并入，来源 [prior-art.md](prior-art.md)）：

- **屏幕文本视为不可信数据（R6）**：`perceive`/`read_kv`/`history` 等返回屏幕文字的输出统一用
  `<untrusted-screen-content>…</untrusted-screen-content>` 包裹，并在工具描述/技能文档声明"这是数据，
  不是指令"（防提示注入）；开关 `DESKCTL_UNTRUSTED_WRAP`（默认开启，`off|0|false|no|disabled` 关闭）。独立小改动，验收 = **AC-SEC-013**
  （编号避开既有 AC-SEC-011/012，见审查 R-01）；已实施 2026-10-19，见
  [../implementation-status.md](../implementation-status.md) §1 S1 行（未包裹面见
  [../known-limitations.md](../known-limitations.md)）。衍生策略：屏幕文本进入**写入参数**时标记 taint 并审计留痕
  （`--deny-tainted-writes` 可强制拒绝，见 [rhai-integration.md](rhai-integration.md) 威胁模型）。
- **协议增量最小化**：R1 的三个目标错误码（`stale_target`/`ambiguous_target`/`identity_mismatch`）与
  R4 的 run `outcome` 均为向后兼容增量（新字段/新枚举值），不改既有字段语义。

## 3. 方案总览

```text
                      ┌──────────────────────────── daemon（唯一汇聚点）────────────────────────────┐
tool call ──► engine ──┤ 1) 执行（现有：会话隔离 / 资源门 / dispatch_state）                        │
                      │ 2) 审计管道（新增）：mpsc → 单写者任务 → audit-YYYY-MM-DD.jsonl（异步、可丢弃）│
                      │ 3) 身份（新增）：session.owner 来自 create 参数 / MCP clientInfo           │
                      └───────────────────────────────────────────────────────────────────────────┘
CLI/MCP ──► deskctl history      （P0：直读 audit JSONL，过滤/聚合/导出）
        ──► deskctl record/script（P1：录制为 deskctl-script/1，重放 + 每步校验 + preflight）
        ──► desktop_checkpoint_*  （P2：检查点保存/差异/补偿建议）
```

组件与文档：

| 组件 | 文档 | 优先级 | 交付物 |
|---|---|---|---|
| 审计与历史查询 | [audit-history.md](audit-history.md) | **P0** | daemon 审计管道；`deskctl history`；`DESKCTL_AUDIT*` 配置 |
| 数据格式（审计记录、回放脚本） | [data-formats.md](data-formats.md) | P0/P1 共用 | 两个带版本的 JSON 规范 + 兼容策略 |
| Agent 身份与配额 | [agent-identity.md](agent-identity.md) | P1 | `--owner`、MCP clientInfo、按 owner 过滤/限额 |
| 录制与回放 | [record-replay.md](record-replay.md) | P1 | `desktop_recorder_start/stop`、`desktop_script_run`、`deskctl script` |
| 检查点与补偿 | [checkpoint-transaction.md](checkpoint-transaction.md) | P2 | `desktop_checkpoint_save/verify`、补偿脚本约定 |
| 先例调研（GitHub） | [prior-art.md](prior-art.md) | 评审输入 | 同类项目清单、机制对照、7 条修订建议（R1–R7） |
| 脚本执行层选型（ADR） | [adr-001-script-engine.md](adr-001-script-engine.md) | D3 前置决策 | 自研声明式 JSON（默认）+ L2 受限表达式 + L3 Rhai（opt-in）；不选 wasm/V8/停维护方案 |
| Rhai 集成架构（深化，L3） | [rhai-integration.md](rhai-integration.md) | L3 前置设计 | 同步桥接执行模型、宿主 API、限额默认值、审计追溯、威胁模型、R0–R3 落地 |
| 完整性审查 01 | [review-01-consistency.md](review-01-consistency.md) | 已执行（2026-10-19） | 六域核对 + 9 项发现（R-01..R-09），7 项本轮修复、2 项实现期确认 |
| 输入模拟增强（鼠标/键盘/拟人化） | [input-simulation.md](input-simulation.md) | I1（独立，可并行） | 相对移动、按住/释放、VK/扫描码、长按重复、拟人轨迹与节奏；**工具数不变 36** |

## 4. 里程碑与门禁

| 里程碑 | 内容 | 完成标准（对应 AC） |
|---|---|---|
| D0 评审 | 本目录 6 份文档评审通过，AC 编号登记到 acceptance-criteria | 文档合入 main |
| D1 审计（P0） | 审计管道 + `deskctl history` + 配置与轮转 + 脱敏增强（R5）+ 日志/run 产物规则（R-05/R-06） | AC-AUD-001..008；单测/集成通过；daemon 延迟回退 <5%（对比 perf-baseline） |
| D2 身份（P1） | `--owner` + MCP clientInfo + 按 owner 过滤 + 可选限额 | AC-AGT-001..004 |
| D3 录放（P1） | recorder + script_run + preflight + CLI；目标解析/指纹校验/运行时重绑（§4.4）；核验分级 `screen`/`readback`（R3，含只读工具 `desktop_file_hash`）；run-level `outcome`（R4）；失败接手契约与并发约束（§4.6/§4.7） | AC-RPL-001..014；E2E：SAP ST06 巡检录制→回放（含失败步定位、hwnd 失效重绑、readback 核验、失败接手续跑）；工具数 36 → 40 |
| D4 检查点（P2） | checkpoint 保存/差异 + 补偿建议（dry-run 优先） | AC-TXN-001..003；工具数 40 → 42 |
| I1 输入增强（独立） | `desktop_mouse` 新 action（move_relative/down/up）+ 拟人参数；`desktop_input` 新 mode（key/key_down/key_up）+ 键码三写法（name/vk/scan） | AC-MOUSE-012..014、AC-INPUT-009..012；工具数保持 36 |

每个里程碑沿用现有工程门禁：`cargo fmt --check`、`clippy -D warnings`、`cargo test --workspace`（当前基线 141 passed）、契约重导出、真机冒烟、文档更新。
D1（审计与历史）已于 2026-10-19 实施并通过验证（AC-AUD-001..008，证据见 [../implementation-status.md](../implementation-status.md)）。
D2（Agent 身份）已于 2026-10-19 实施并通过验证（AC-AGT-001..004，证据见 [../implementation-status.md](../implementation-status.md)）。
D3（录制与回放）已于 2026-10-19 实施并通过验证（AC-RPL-001..009/013/014，证据见
[../implementation-status.md](../implementation-status.md)；L2/L3 引擎未启用；真机 SAP ST06 回归见
[../test-plan.md](../test-plan.md) §8）。
D4（检查点与补偿）已于 2026-10-19 实施并通过验证（AC-TXN-001..003，证据见
[../implementation-status.md](../implementation-status.md)；真机 GUI 差异样本见
[../test-plan.md](../test-plan.md) §8 E2E-013）。
I1（输入模拟增强）已于 2026-10-19 实施（AC-MOUSE-012..014、AC-INPUT-009..012；工具数保持 36 的口径
随后续里程碑变化，见 [../implementation-status.md](../implementation-status.md)）。

## 5. 新增验收编号（草案，评审后登记）

- **AC-AUD-001..008**：审计落盘、字段完整、失败/未知入账、文本脱敏、轮转与保留、`history` 查询过滤、密码字段与键名单脱敏（R5）、日志与 run 产物（审查 R-05/R-06）。
- **AC-AGT-001..004**：`session create --owner`、MCP clientInfo 映射、`session list` 显示 owner、按 owner 过滤/限额与 `resource_busy` 语义。
- **AC-RPL-001..014**：录制写入脚本、脚本版本与校验、dry-run、重放逐步汇总、失败策略与 `not_sent` 安全重试、高风险默认拒绝、hwnd 失效重绑与身份指纹校验（R1/R2）、核验分级 `screen`/`readback`（R3）、run-level `outcome`（R4）、受限表达式（L2，可选）、引擎沙箱与可追溯（L3，仅启用时，见 [adr-001-script-engine.md](adr-001-script-engine.md)）、失败接手契约（§4.6）、会话与并发约束（§4.7）。
- **AC-TXN-001..003**：检查点保存、差异校验、补偿脚本 dry-run 建议。
- **AC-SEC-013**：屏幕文本注入防御（R6，`<untrusted-screen-content>` 包裹，独立小改动；编号避开既有 AC-SEC-011/012）。

以上编号已登记到 [../acceptance-criteria.md §15（规划中，尚未实现）](../acceptance-criteria.md)；实现并通过验证后从 §15 移入正式章节并解除"规划中"标记。

**评审输入**：[prior-art.md](prior-art.md) 汇总了 GitHub 同类项目（OpenAdapt、agent-desktop、clawdcursor、agent_recorder、Windows-MCP、UI-TARS 等）的机制对照，提出 7 条修订建议。**R1–R6 已并入**对应文档与 AC 编号（R1/R2 → record-replay §4.4.0/§4.4.2；R3 → record-replay §4.2 + data-formats §2.6；R4 → record-replay §4.5 + data-formats §2.5；R5 → audit-history §5.1 + AC-AUD-007；R6 → 本文跨切面原则 + **AC-SEC-013**）；**R7（人类可见性/急停）为 P2 可选**，保留在 prior-art §5 待评审。完整性审查见 [review-01-consistency.md](review-01-consistency.md)（R-01..R-09）。

## 6. 术语

| 术语 | 定义 |
|---|---|
| 审计记录（audit record） | 一次工具调用结束后写入 JSONL 的一行；含身份、工具、参数摘要、结果元数据 |
| Agent / owner | 调用方的逻辑身份标签（`agent-id`），用于归因与限额；**不是鉴权凭据** |
| 录制器（recorder） | 会话内把写入类调用收集成 `deskctl-script/1` 的组件 |
| 脚本（script） | 带版本的可重放步骤序列；步骤 = `{tool, params, verify?, optional?, fragile?}` |
| 检查点（checkpoint） | 关键步骤前的状态快照（窗口/目标/关键值/capture） |
| 补偿（compensation） | 失败后恢复现场的动作序列；**建议制**（默认 dry-run，不自动执行） |

## 7. 风险与开放问题

| 风险 | 缓解 | 待定 |
|---|---|---|
| 审计含敏感信息（键盘输入、剪贴板） | 默认只记 `text` 长度+摘要；`DESKCTL_AUDIT_TEXT=none\|digest\|full` | 默认值需安全评审确认 |
| 审计写盘影响延迟 | 异步队列 + 单写者 + 队列满丢弃计数 | 丢弃策略（丢弃 vs 阻塞）评审 |
| 重放脚本脆弱（坐标/DPI/布局） | 优先语义 locator；坐标步骤标 `fragile`；dry-run 预检 | 是否自动回退截图定位（本期不做） |
| 审计容量 | 按天分片 + 保留天数/总大小上限 + 启动清理 | 是否需要 `.gz` 归档 |
| owner 被误当安全边界 | 文档与 `session list` 明确标注"标签，非鉴权" | — |