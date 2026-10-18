# 操作录制与回放（Record & Replay）——P1

- 文档版本：0.1
- 日期：2026-10-19
- 状态：Draft（待评审）
- 关联：[README.md](README.md)、[data-formats.md](data-formats.md)、[audit-history.md](audit-history.md)、[../acceptance-criteria.md](../acceptance-criteria.md)

## 1. 目标与范围

- 把一次成功的操作序列**录制**成带版本的可读脚本（`deskctl-script/1`）。
- **重放**脚本：逐步执行、每步校验、失败可定位到具体步骤，输出结构化 run report。
- 重放前 **preflight**（目标/变量/风险/脆弱性检查），默认拒绝高风险步骤。
- 不做：录屏/视频、坐标级"像素回放"、审批流、自动补偿（补偿见 [checkpoint-transaction.md](checkpoint-transaction.md)）。

现状可复用基础（核查证据）：

| 基础 | 位置/验收 |
|---|---|
| 语义动作 `{locator, action}` 可跨进程重放，不依赖临时 candidate | `desktop_semantic_action`；AC-SEM-006 |
| 候选响应已自带"一步脚本"格式 `workflow_step:{tool, params:{locator, action}}` | `crates/deskctl-core/src/tools/semantic.rs:131-137` |
| `dispatch_state` 明确"是否已发出"，仅 `not_sent` 可安全重试 | `DispatchState::is_retry_safe`（`result.rs`） |
| 后置校验：`expectation` + `stable_samples` | `desktop_verify_state`；AC-SEM-014/015 |
| 会话内目标绑定与 capture 引用 | `session.rs`（targets/captures） |

## 2. 工具面

| 工具 | 类型 | 作用 |
|---|---|---|
| `desktop_recorder_start` | 会话内、写 | 开始录制；`{include_reads?: "essential"｜"all"｜"none"(默认 essential), max_steps?: int(默认 200)}` |
| `desktop_recorder_stop` | 会话内、写 | 停止并导出：`{format?: "script"(默认), path?: string}` → 返回 `{script, stats, warnings}`；`path` 写出 `.dscript.json`（路径遵循允许根策略） |
| `desktop_script_run` | 会话内、写 | 校验 + 预检 + 执行；`{script?: object, path?: string, dry_run?: bool, params?: object, on_error?: "stop"｜"continue", from_step?: string, to_step?: string, risk_max?: string, allow_external_commit?: bool}` |

配套 CLI：

```text
deskctl record  --session <id> start [--include-reads essential|all|none] [--max-steps N]
deskctl record  --session <id> stop  --out plan.dscript.json
deskctl script validate --file plan.dscript.json
deskctl script run      --file plan.dscript.json [--dry-run] [--param k=v] [--from s2] [--to s5] [--on-error stop|continue]
```

（`deskctl script run` 走 daemon 会话执行；`validate` 纯本地，不需要 daemon。）

## 3. 录制规则

### 3.1 记录集合

- 默认（`essential`）：`risk != read_only` 的写入类工具（Input/Mouse/MouseDrag/SemanticExecute/SemanticAction/WindowActivate/WindowMove/WindowResize/ClipboardWrite/图像裁剪与新文档工具中会写文件的步骤），加上**对复现必要的读**：`desktop_wait_for`、`desktop_verify_state`、`desktop_window_screenshot`（产生 capture 引用）。
- `all`：额外记录 `perceive/find_*/read_kv/xlsx_read/…` 等纯读（脚本更"厚"，一般用于取证）。
- `none`：只录写入类。
- 不记录：`recorder_*` 自身、`script_run`、`session/*`、`doctor/schema`。

### 3.2 参数化（录制时完成，重放时无需人工改脚本）

| 录到的内容 | 转换 | 原因 |
|---|---|---|
| `observation_token + candidate_id`（semantic_execute） | 就地换成 `desktop_semantic_action` 的 `locator`（从观察缓存反查；失败则记录 warning 并保留 token 步骤，标记 `volatile`） | locator 可跨进程重放（AC-SEM-006） |
| `hwnd` 且属于已绑定目标 | 换为 `$target.<名>.hwnd`，并要求 preflight 重新绑定 | 窗口重建后 hwnd 变化 |
| 裸 `hwnd`（未绑定） | 保留，标记 `fragile:true` | 无法保证稳定 |
| `capture_id` | 保留，标记 `volatile:true`；重放 preflight 校验该 capture 是否在会话内 | capture 是会话态 |
| 坐标（mouse/裁剪 `x,y,width,height`） | 保留，标记 `fragile:true` | DPI/布局变化 |
| `text`（键盘输入） | 保留原文进入脚本（这是复现所必需）；**与审计无关**（审计仍脱敏） | 脚本是调用方主动导出的产物 |

### 3.3 边界与上限

- 单会话同时最多 1 个录制器；`max_steps` 默认 200，超出后**停止记录**并在 stop 的 `warnings` 中报告（不静默截断）。
- 录制只捕捉**成功进入执行入口**的调用（含失败调用与结果元数据），供"失败步骤也回放/复盘"使用；`optional` 由录制器按结果自动置位：结果 `ok=false` 且 `dispatch_state=not_sent` 的步骤标 `optional:true`（谨慎回放）。
- 录制内容只存会话内存（≤256 KiB）；`stop` 后清空；daemon 重启即丢（语义与 capture 一致）。

## 4. 重放执行模型

### 4.1 Preflight（任何执行前，`dry_run` 也执行）

1. **格式**：`v/format` 校验（未知版本报 `invalid_params`）。
2. **工具可用性**：每一步 `tool` 必须存在于当前 profile；缺失列出步骤 id。
3. **目标解析**：`targets.*.match` 在当前窗口列表解析出唯一 hwnd；多命中/零命中 → 报错并提示用 `--param <name>=<hwnd>` 显式指定。
4. **变量解析**：所有 `$…` 引用可解析（含 `$steps.*` 前序结果）；未解析 → 报错。
5. **风险**：每步 `risk ≤ risk_max`（默认 `bounded_write`）；`external_commit` 需 `allow_external_commit=true`；`destructive_critical` **一律拒绝**。
6. **脆弱性**：`fragile` 步骤计数并警告（`--no-fragile` 可跳过执行）。

预检失败 = 不执行任何步骤，返回结构化错误（含失败清单与建议）。

### 4.2 执行

- **严格串行**：步骤逐个经 Engine 执行（与现有资源门/会话语义一致），不并行、不合并。
- **每步记录**：`{id, tool, ok, dispatch_state, effect, verification, duration_ms, error}`（见 [data-formats.md](data-formats.md#25-重放报告deskctl-script-run1)）。
- **断言**：`expect`（`ok/path/equals/matches`）在步骤返回后求值；失败按 `on_error` 处理。
- **内联校验**：`verify` 转成 `desktop_verify_state` 语义执行（可含 `stable_samples`）。
- **重试**：仅当步骤失败且 `dispatch_state == not_sent` 时按 `retry` 重试（≤3）；`partial/unknown` 绝不自动重试（防双执行）。
- **失败策略**：`stop`（默认）立即停止并返回已执行步骤报告；`continue` 跳过失败步继续（末尾 `result=partial`）。
- **前后台边界**：脚本不改变调用方会话的 `delivery_mode` 默认值；需要后台的步骤由脚本参数显式声明。

### 4.3 安全基线

- `risk_max` 默认 `bounded_write`；`DestructiveCritical` 不可通过脚本执行。
- 脚本可写盘（`path` 输出）但**读取**脚本遵循允许根策略（与图像读取一致），避免任意路径读取。
- `desktop_script_run` 自身的 meta：`requires_session=true`、`read_only=false`、`risk=external_commit`（因为可能触发外部提交），但**默认拒绝**高风险步骤（见 §4.1.5）。
- 回放不产生审批交互（延续无审批原则）。

## 5. 验收标准（AC-RPL-001..006）

| 编号 | 通过标准 | 验证方法 |
|---|---|---|
| AC-RPL-001 | 录制写入内存；`recorder_stop` 返回合法 `deskctl-script/1`（含 targets/steps/stats）；超过 `max_steps` 停止记录并警告 | 集成：录制 ≥3 步真实调用后校验脚本结构 |
| AC-RPL-002 | `candidate_id` 步骤被参数化为 `locator`；绑定过的 `hwnd` 变为 `$target.*`；`capture_id` 标 `volatile`；坐标步骤标 `fragile` | 单测：构造调用序列断言转换结果 |
| AC-RPL-003 | `validate`/preflight 拒绝：未知工具、未解析变量、非法格式/版本、超过 `risk_max`、多命中目标 | 单测 + CLI 集成 |
| AC-RPL-004 | `dry_run` 不执行任何工具（审计里无对应调用记录），输出每步判定（执行/跳过/高风险/fragile） | 集成：dry-run 前后 `history` 与屏幕状态不变 |
| AC-RPL-005 | 重放返回 run report（逐步结果齐全），失败可定位到步骤 id；`on_error=stop/continue` 行为正确 | E2E：真实脚本含 1 个故意失败步骤，两种策略各跑一次 |
| AC-RPL-006 | 仅 `not_sent` 步骤允许自动重试；`partial/unknown` 不重试（审计可见重试次数与最终状态） | 单测 + 集成（故障注入：工具返回 partial） |

## 6. 测试计划

- 单测（`crates/deskctl-core/src/script.rs`）：解析/校验/变量/风险判定/断言求值/重试规则。
- 集成（`crates/deskctl-cli/tests/script_contract.rs`）：`record → stop → validate → run --dry-run → run` 全链路。
- E2E-012（新）：SAP ST06 巡检录制 → 重放（含 `verify` 断言"操作系统监控器"文本）→ 与 E2E-011 手工流程比对步骤数。
- 安全：高风险步骤拒绝、脚本路径越界拒绝、`volatile` capture 缺失时的报错文案。
- 性能：`script run` 的额外开销（校验 + 报告）目标 <2ms/步。

## 7. 实施任务分解（D3）

| # | 任务 | 文件 | 预估 |
|---|---|---|---|
| 1 | 脚本模型：解析/序列化/校验/变量解析 | `crates/deskctl-core/src/script.rs`（新增） | 1d |
| 2 | 录制器：会话态 + 参数化转换 | `session.rs`、`tools/mod.rs` | 1d |
| 3 | 重放执行器：preflight + 逐步执行 + run report | `script.rs` + `tools/mod.rs` | 1d |
| 4 | 三个新工具注册（schema/meta/profile/契约） | `deskctl-protocol`、`tools/mod.rs` | 0.5d |
| 5 | CLI：`record`/`script validate|run` | `main.rs` + 新 `script_cmd.rs` | 0.5d |
| 6 | 测试（单测/集成/E2E-012/安全） | tests + scripts | 1.5d |
| 7 | 文档与 AC 登记 | acceptance/test-plan/status/llm-skills 配方 | 0.5d |

依赖：D1（审计）完成后开始（步骤可对照审计核对）；工具数将从 36 增至 39。

## 8. 开放问题

1. 录制是否默认包含截图步骤（当前 `essential` 包含）——截图较多会放大脚本体积与重放耗时。
2. `volatile` capture 的自动替换策略：本期要求调用方用 `--param`，是否提供"重放时重拍截图"的自动回退（建议后续）。
3. 是否支持脚本内条件分支（`if verify fails then …`）：本期不做，保持线性 + `optional/on_error` 两档。