# 数据格式规范（审计记录 / 回放脚本 / 检查点）——P0 + P1 + P2 共用

- 文档版本：0.1
- 日期：2026-10-19
- 状态：Draft（待评审）
- 关联：[README.md](README.md)、[audit-history.md](audit-history.md)、[record-replay.md](record-replay.md)、[checkpoint-transaction.md](checkpoint-transaction.md)

本文件是三个新格式的**唯一权威定义**。实现与测试均以本文件为准。

## 0. 通用约定

- 编码 UTF-8（不含 BOM），行尾 `\n`，时间一律 RFC3339 含毫秒与本地时区（如 `2026-10-19T01:23:45.123+08:00`）。
- 每个格式带整型 `v` 与字符串 `format`（形如 `deskctl-<name>/<major>`）。
- **兼容策略**：
  - 同一 `v` 内只允许**新增可选字段**；读取方忽略未知字段。
  - 破坏性变更必须提升 `v`；读取方遇到更高 `v` 必须报错（`invalid_params`），不得猜测。
  - 写入方只写当前 `v`；不提供向下转换。
- 数字：毫秒为 int；尺寸为 int；哈希统一 `sha256`（小写 hex，带 `sha256:` 前缀）。
- 变量/引用（仅脚本）：`$` 开头，未解析引用视为脚本错误。

## 1. 审计记录（`deskctl-audit/1`）

JSONL：一行一条。`format` 不在每行重复，由文件头约定（文件名 + `v` 字段）；如需自描述，可用首行 `{"v":1,"kind":"audit_header","format":"deskctl-audit/1",...}`（可选）。

字段定义（`?` 表示可空/可缺省）：

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `v` | int | ✓ | =1 |
| `ts` | string | ✓ | 记录时刻（调用完成时） |
| `kind` | string | ✓ | `tool_call` / `session` / `audit_health` |
| `request_id` | string? | — | `req:<hex>` / `oneshot-<uuid>` |
| `session_id` | string? | — | 会话 id |
| `owner` | string | ✓ | 身份标签（daemon 默认 `daemon-client`；MCP `mcp:<client-info>`，缺失 clientInfo 时 `mcp:unknown`；oneshot `oneshot`；`--owner` 可自定义，见 [agent-identity.md](agent-identity.md)） |
| `source` | string | ✓ | `daemon` / `mcp` / `oneshot` / `bench` |
| `tool` | string | ✓* | `kind=tool_call` 必填 |
| `risk` | string | ✓* | `read_only`/`reversible`/`bounded_write`/`external_commit`/`destructive_critical` |
| `args` | object? | — | 脱敏后入参（`basic` 模式为 null） |
| `args_digest` | string | ✓ | 完整入参序列化的 sha256 |
| `args_truncated` | bool | ✓ | 参数是否被体积/长度规则截断 |
| `duration_ms` | int | ✓ | 执行耗时 |
| `ok` | bool | ✓ | 结果封套 `ok` |
| `dispatch_state` | string | ✓ | `not_sent`/`sent`/`partial`/`unknown` |
| `effect` | string | ✓ | `read`/`unverifiable`/`external_commit`/`confirmed`/`refused`/`partial` |
| `verification` | string | ✓ | `not_applicable`/`unknown`/`satisfied`/`progress`/`no_change` |
| `retryable` | bool | ✓ | 结果封套字段 |
| `error` | object? | — | `{code, message, retryable}`；`message` ≤512 字符 |
| `result_digest` | string? | — | `data` 序列化 sha256（默认记录；`basic` 模式也保留） |
| `audit_dropped` | int? | — | 仅 `kind=audit_health`：此前累计丢弃条数 |

`kind=session`（可选）额外字段：`event`（`create`/`close`/`gc`）、`profile`、`session_id`。

## 2. 回放脚本（`deskctl-script/1`）——文件扩展名 `.dscript.json`

```json
{
  "v": 1,
  "format": "deskctl-script/1",
  "name": "sap-st06-inspection",
  "created": "2026-10-19T01:23:45.123+08:00",
  "owner": "agent:sap-inspector",
  "source_session": "sess-9a3c…",
  "params": { "tcode": "/nST06" },
  "targets": {
    "sap": {
      "match": { "title_contains": "S4H sap1809demo" },
      "bind": { "delivery_mode": "auto", "follow_dialog": true }
    }
  },
  "steps": [
    { "id": "s1", "tool": "desktop_target_bind",
      "params": { "app_ref": "$target.sap.app_ref", "delivery_mode": "auto" },
      "set": { "sap_token": "$result.data.target_token" },
      "expect": { "ok": true } },
    { "id": "s2", "tool": "desktop_semantic_action",
      "params": { "locator": { "app_id": "$var.sap_token", "role": "text_field", "ordinal_hint": 0 }, "action": "focus" } },
    { "id": "s3", "tool": "desktop_input",
      "params": { "mode": "type", "hwnd": "$target.sap.hwnd", "text": "$param.tcode", "send": "enter" } },
    { "id": "s4", "tool": "desktop_verify_state",
      "params": { "expectation": { "kind": "text_present", "value": "操作系统监控器" } },
      "optional": true, "note": "屏幕就绪校验，失败不终止" }
  ],
  "on_error": "stop",
  "risk_max": "bounded_write"
}
```

### 2.1 顶层字段

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `v` / `format` | int / string | ✓ | =1 / `deskctl-script/1` |
| `name` | string | ✓ | 脚本名（`[a-z0-9._-]{1,64}`） |
| `created` / `owner` / `source_session` | string | — | 溯源信息 |
| `params` | object | — | 可覆盖参数（`--param k=v`） |
| `targets` | object | — | 目标声明；重放前 preflight 用 `match` 找窗口并 `bind` |
| `steps` | array | ✓ | ≥1 步 |
| `on_error` | string | — | `stop`（默认）/`continue`（单步失败后继续） |
| `risk_max` | string | — | 允许的最大风险（默认 `bounded_write`；`external_commit` 需显式提升） |

### 2.2 目标声明（`targets.<name>`）

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `match.title_contains` | string | — | 标题包含（可配置为正则，见 `strictness`）；与其它 match 条件为 AND |
| `match.process_name` | string | — | 进程名（如 `saplogon.exe`），精确匹配 |
| `match.class_name` | string | — | 窗口类名（如 `SAP_FRONTEND_SESSION`） |
| `match.app_ref` | string | — | catalog 引用（`desktop_targets_list`）；最稳的匹配方式 |
| `match.window_ref` | string | — | `win:<hwnd>`，仅用于消歧（不单独依赖） |
| `fingerprint` | object? | — | 录制时快照：`{pid, class_name, title, process_start_time?}`，重放时校验"还是不是那个窗口" |
| `strictness` | string? | — | `strict` / `balanced`(默认) / `loose`，见 [record-replay.md §4.4.3](record-replay.md#443-严格度-strictness) |
| `launch_ref` | string? | — | catalog app_ref；目标未运行时允许受限启动（≤10s） |
| `allow_relaunch` | bool? | — | 执行中目标消失时是否允许重新拉起（有 `launch_ref` 时默认 true） |
| `bind.delivery_mode` | string? | — | 默认 `auto` |
| `bind.follow_dialog` | bool? | — | 默认 false |

绑定结果进入变量：`$target.<name>.hwnd|token|pid|title|class_name|app_ref`。
解析算法与失效处理详见 [record-replay.md §4.4](record-replay.md#44-目标解析与-hwnd-失效处理目标重绑定)。

### 2.3 步骤字段

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | string | 唯一（`s1,s2,…` 或语义名） |
| `tool` | string | 工具名（必须存在于当前 profile） |
| `params` | object | 工具入参；字符串值可含 `$…` 引用 |
| `expect` | object? | 结果断言：`{ok?, path?, equals?, matches?, not_matches?}`（`path` 为对 `data` 的点路径） |
| `set` | object? | 变量绑定：`{"<name>": "$result.data.<path>"}`（或 `$result.<path>`） |
| `verify` | object? | 内联校验：`{tier:"screen"(默认)｜"readback", ...}`，两种 tier 的字段见 §2.6 |
| `require_readback` | bool? | true 时该步必须具备 readback 级核验（本步 `verify.tier=readback` 或 run 级声明），否则 preflight 拒绝 |
| `optional` | bool? | true 时失败不停（默认 false） |
| `fragile` | bool? | 坐标类步骤（mouse/坐标裁剪）标记；`script run --no-fragile` 可跳过 |
| `retry` | int? | 自动重试次数（0..3）；**仅当 `dispatch_state=not_sent`** 才允许（利用 `DispatchState::is_retry_safe`） |
| `note` | string? | 备注（不参与执行） |

### 2.4 变量解析

| 语法 | 来源 | 示例 |
|---|---|---|
| `$target.<名>.<字段>` | preflight/重绑结果（`hwnd/token/pid/title/class_name/app_ref`） | `$target.sap.hwnd`、`$target.sap.token` |
| `$param.<键>` | `params` + `--param` 覆盖 | `$param.tcode` |
| `$var.<名>` | 步骤 `set` 写入 | `$var.sap_token` |
| `$steps.<id>.result.<路径>` | 历史步骤结果（只读） | `$steps.s5.result.data.capture_id` |
| `$session.id` | 当前会话 | — |
| `$env.<名>` | 环境变量（白名单见实现；默认仅 `DESKCTL_*` 前缀可读） | `$env.DESKCTL_AUDIT` |

规则：未解析引用 = 脚本校验失败；`params` 中非字符串值不做替换。

### 2.5 重放报告（`deskctl-script-run/1`，`desktop_script_run` 返回与 CLI 输出共用）

```json
{"v":1,"format":"deskctl-script-run/1","script":"sap-st06-inspection","dry_run":false,
 "run_id":"run-9a3c8f2b",
 "started":"...","finished":"...","result":"ok|failed|partial",
 "outcome":"VERIFIED|HALTED_BEFORE_EFFECT|RECONCILIATION_REQUIRED|COMPLETED_UNVERIFIED|REJECTED_POLICY|FAILED_PLATFORM|CANCELED",
 "handoff":{"failed_step":null,"reason":null,"effect_may_have_occurred":false,"safe_to_retry":true,
            "resume_hint":null,"suggested_actions":[]},
 "steps":[{"id":"s1","tool":"desktop_target_bind","ok":true,"dispatch_state":"sent","verification":"unknown",
           "verify_tier":"readback|screen|null","rebind":null,"retries":0,"duration_ms":38,"error":null}],
 "rebinds":[{"step":"s4","reason":"stale_target","from_hwnd":3345330,"to_hwnd":5702916,"relaunched":true}],
 "stats":{"total":4,"ok":4,"failed":0,"skipped":0,"readback_verified":2}}
```

`result` 与 `outcome` 并存：`result` 是粗粒度（ok/failed/partial），`outcome` 是决策级分类（语义与触发
条件见 [record-replay.md §4.5](record-replay.md#45-运行结果分类run-level-outcomer4)）。**调用方应以
`outcome` 决定是否重跑**：`HALTED_BEFORE_EFFECT` 可直接重跑，`RECONCILIATION_REQUIRED` 必须先核对。

`run_id` 与 `handoff` 是失败接手的关键字段：

- `run_id`：审计记录的 `request_id` 前缀（`<run_id>:<step_index>`），配合 `history --run <run_id>` 回看已完成步骤；
- `handoff`：`failed_step` / `reason` / `effect_may_have_occurred` / `safe_to_retry` / `resume_hint{from_step,params_patch}` / `suggested_actions[]`，
  推导规则与续跑语义见 [record-replay.md §4.6](record-replay.md#46-失败后的接手契约handoff--resumellm人如何继续)。

### 2.6 核验定义（`verify`，R3）

```json
"verify": { "tier": "screen", "kind": "text_present", "value": "操作系统监控器", "stable_samples": 2 }
"verify": { "tier": "readback", "kind": "xlsx_cell", "target": "$steps.s5.result.data.path",
            "sheet": "巡检指标", "cell": "C5", "equals": 4105 }
```

| tier | kind | 参数 | 说明 |
|---|---|---|---|
| `screen`（弱） | `text_present` / `value_equals` / `element_exists` / `checked` | `locator`(可选)、`value`、`stable_samples`(1..5) | 复用 `desktop_verify_state` 语义；只证明"界面看起来对" |
| `readback`（强） | `file_exists` | `target`（前序步骤结果/参数里的路径） | 只读工具 `desktop_file_hash` 提供 |
| | `file_sha256` | `target`、`sha256` | 同上；比对内容指纹 |
| | `docx_contains` | `target`、`contains` | 调 `desktop_docx_read` 读回 |
| | `xlsx_cell` | `target`、`sheet?`、`cell`(如 `C5`)、`equals`/`matches` | 调 `desktop_xlsx_read`（`region=A1:C5` 等）读回 |
| | `pptx_contains` | `target`、`contains` | 调 `desktop_pptx_read` 读回 |
| | `json_path` | `target`（前序步骤结果）、`path`(点路径)、`equals`/`contains` | 对 `$steps.*.result` 做结构化断言 |

规则：

1. `readback` 的 `target` 只能引用**前序步骤结果**或脚本参数（不新增任意路径读取能力；文件路径仍走允许根策略）。
2. 校验失败 → 该步 `verification=failed`，run 的 `outcome` 至少为 `RECONCILIATION_REQUIRED`（不得判 `VERIFIED`）。
3. run report 的 `stats.readback_verified` 统计强核验通过步数；`VERIFIED` 要求"所有声明 `readback` 的步骤通过"。

## 3. 检查点（`deskctl-checkpoint/1`）

```json
{"v":1,"format":"deskctl-checkpoint/1","name":"before-commit","created":"...","session_id":"sess-…",
 "targets":[{"name":"sap","hwnd":5702916,"title":"S4H sap1809demo_S4H_00 …","foreground":false,
             "rect":{"x":65,"y":99,"width":2175,"height":1245}}],
 "values":[{"name":"amount","locator":{"app_id":"$var.sap_token","role":"text_field","accessible_name":"金额"},
            "kind":"value","value":"1000.00"}],
 "captures":["cap-…"]}
```

- `values[].kind`：`value`（UIA 文本）/`checked`（布尔）/`exists`（元素存在）。
- `desktop_checkpoint_verify` 返回差异：`{matched:false, diffs:[{name, expected, actual, kind}]}`；无差异 `matched:true`。

## 4. 版本演进示例（供参考）

| 变更 | 处理 |
|---|---|
| 审计新增 `result_digest` | 同 v 内新增可选字段（读取方忽略未知字段） |
| 脚本新增 `preflight` 块 | 同 v 内新增；老读取方忽略 |
| 脚本参数语义改变（如 `risk_max` 默认值提升） | 提升 `format` 主版本到 `deskctl-script/2` |