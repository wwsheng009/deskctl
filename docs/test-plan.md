# Deskctl 测试计划

- 文档版本：0.1
- 日期：2026-10-18
- 状态：Draft
- 关联文档：[acceptance-criteria.md](acceptance-criteria.md)、[architecture.md](architecture.md)、[mcp-service.md](mcp-service.md)

## 1. 目标

本计划回答四个问题：

1. 每个工具和协议如何被验证。
2. 哪些测试可以自动运行，哪些必须真机执行。
3. 多 session、异常、性能和安装包如何验收。
4. 什么情况下允许发布 Windows 首个版本。

## 2. 测试分层

```text
                 ┌─────────────────────┐
                 │ Windows E2E / 真机  │  少量、真实应用
                 ├─────────────────────┤
                 │ 集成测试            │  平台 API、daemon、CLI
                 ├─────────────────────┤
                 │ 契约测试            │  Schema、MCP、结果封套
                 ├─────────────────────┤
                 │ 单元测试            │  状态机、参数、路径、错误
                 └─────────────────────┘
```

原则：

- 核心状态机尽量用 mock 平台测试，减少对真实鼠标键盘的依赖。
- 平台 API 必须有 Windows 真机集成测试。
- 多 session 行为同时用 mock 和真机验证。
- 性能和安全测试单独记录，不与功能测试混在一起。

## 3. 测试环境

### 3.1 必测环境

| 环境 | 用途 |
|---|---|
| Windows 11 x64 | 主验收环境 |
| Windows 10 x64 | 兼容性抽测 |
| Windows Server Core | CLI/daemon 无 GUI 场景（不要求桌面工具） |
| macOS/Linux CI | 编译占位和 feature 检查，不做功能验收 |

### 3.2 构建配置

- debug：单元/契约/集成调试
- release：性能、安装包、E2E
- `standard` profile：默认验收
- `full` profile：可选工具验收
- `readonly` profile：安全/只读验收
- 模型存在/缺失两种配置

### 3.3 测试账号与桌面

- 使用专用 Windows 测试账号。
- 测试桌面分辨率固定，避免坐标变化。
- 关闭屏幕保护、睡眠、通知弹窗和自动更新。
- 测试前清理剪贴板和临时截图目录。
- 不在测试机上运行其他自动化工具。

## 4. 测试数据与夹具

建议建立 `tests/fixtures/`：

```text
tests/fixtures/
├─ images/
│  ├─ template-button.png
│  ├─ screen-1080p.png
│  ├─ icon-fixture.png
│  └─ text-fixture.png
├─ models/
│  ├─ README.md
│  └─ hashes.json
├─ configs/
│  ├─ standard.toml
│  ├─ full.toml
│  └─ readonly.toml
├─ mcp/
│  ├─ initialize.json
│  ├─ tools-list.json
│  └─ tools-call.json
└─ text/
   ├─ ascii.txt
   └─ unicode.txt
```

固定测试应用：

- Notepad：文本输入、保存、窗口操作
- Explorer：窗口列表、激活
- Calculator：按钮点击、UIA 语义
- 一个简单 Win32 测试程序：固定坐标和按钮，用于鼠标/键盘/UIA
- 一个可选 Electron/WPF 应用：验证 UIA/输入兼容性

## 5. 单元测试

### 5.1 协议与参数

- Schema 合法性和必需字段
- `additionalProperties` / 未知字段策略
- 参数类型、范围、互斥关系
- `hwnd`、`capture_id`、`element_id` 校验
- 自定义截图路径解析和格式/扩展名选择（format > 扩展名 > 默认）
- `dict_name` basename 校验
- 输入文本长度、NUL 字符、空字符串
- 超时、间隔、稳定时间上下限

### 5.2 状态机

- 无 session 时拒绝有状态工具
- capture token 过期/消费/跨 session
- observation token 过期/跨 session
- target token 绑定和关闭
- 多 session 状态隔离
- 资源门排队、取消和释放
- 窗口变化导致动作中止
- `dispatch_state` 和 `retryable` 组合
- `partial` 和 `unknown` 不自动重试

### 5.3 平台 mock

建立 `MockPlatform`：

- 可控截图帧
- 可控窗口列表和前台状态
- 可控输入事件记录
- 可控 UIA 观察和候选
- 可注入超时、权限错误、窗口变化
- 可记录调用顺序，验证多 session 串行化

## 6. 契约测试

### 6.1 工具契约

- `standard` 40 个工具，`full` 42 个（`readonly` 25、`minimal` 6）
- 每个 Schema 都有执行器
- 工具名稳定，不出现非 `desktop_*`
- 工具元数据包含 `requires_session`、`requires_observation`
- `desktop_vision`/`desktop_agent_step` 按 feature 出现
- 结果封套字段稳定
- 错误码稳定
- 旧字段新增不破坏客户端

### 6.2 MCP 契约

- `initialize` 版本和 capabilities
- `tools/list` 与协议文档一致
- `tools/call` 成功/失败映射
- `isError` 与 JSON-RPC error 的边界
- stdout 只有协议 JSON
- 取消/进度事件
- 无 `approval/*` 方法
- 工具列表变化通知

### 6.3 CLI 契约

- `--help` 子命令完整
- JSON 输入文件/stdin
- 退出码
- stdout/stderr 分离
- `doctor` 字段
- `session` 管理
- `cancel` 行为

## 7. Windows 集成测试

### 7.1 截图

- 全屏/区域/窗口
- 自定义路径、父目录创建、扩展名与格式一致
- 多显示器/DPI 缩放（至少 100% 和 150%）
- 窗口移动/缩放期间截图
- 最小化/遮挡窗口
- 受保护窗口（预期失败）

### 7.2 输入

- 中文/英文/emoji/组合键
- 鼠标单击/双击/右键/中键/滚轮
- 拖拽
- 前台窗口切换
- 输入过程中窗口关闭
- 目标 locator 失效
- 两个 session 同时输入（验证串行）

**I1 输入模拟增强回归（AC-MOUSE-012..014、AC-INPUT-009..012）**：

- `move_relative`：dx/dy 物理像素偏移；多显示器虚拟桌面下跨屏（起点/终点位移可测）。
- `down` → 多次 `move` → `up`：完成一次拖选；右键长按菜单；`down` 未配对 `up` 时在下一次
  冲突调用或会话关闭被清理（无粘键，`pressed_buttons`/`pressed_keys` 会话摘要可查）。
- 拟人轨迹（`humanize`）：同 `seed` + 同参数两次运行点列逐点一致（单测已覆盖）；
  实测轨迹非直线、时长与 `speed` 偏差 ≤20%；`path_points` 回传与实际发送一致。
- 点击拟人：`hold_ms` 生效（down/up 时差可测）；`jitter_px>0` 落点在目标元素内且距边缘
  ≥2px；`double_click` 间隔受系统双击时间约束。
- 键码三写法：`"enter"` / `"vk:0x41"` / `"scan:0x1E"`（含 `extended`）；非法写法返回
  `invalid_params` 并列出示例；`scan` 模式在记事本与远程桌面/经典应用各验证一例。
- 长按与重复：`key_down` → `key_up` 期间键保持按下（目标应用可证）；`repeat`+`interval_ms`
  按序发送且可取消；`hotkey` 的 `hold_ms` 生效。
- 拟人打字：逐字延迟落在配置范围（统计断言）；同 `seed` 复现同一延迟序列；`delay_ms`
  固定值模式精确生效。
- 限额与取消：超限参数（hold/repeat/interval/总时长）被拒并附范围；执行中
  `request/cancel` ≤1s 生效，结果如实报告 `cancelled` 与已发送量。

### 7.3 窗口

- 枚举、激活、移动、缩放、信息
- 最小化窗口激活
- 同名窗口
- 高权限窗口
- 无效/过期窗口句柄

### 7.4 OCR/YOLO

- 模型随包路径
- 模型缺失/损坏/哈希不匹配
- 1080p 截图 OCR
- YOLO 图标检测
- OCR/YOLO 合并
- `capture_id` 映射
- 未登记图片的 `screen_center=null`

### 7.5 Windows UIA

- 目标列表、绑定、观察
- 按钮点击、复选框、文本框、列表项、菜单
- 同名控件和祖先上下文
- 观察后窗口变化
- 前台切换
- 高权限窗口
- 候选过期
- 持久化 locator 重放
- 状态验证 satisfied/unsatisfied/unknown

### 7.6 审计与历史（D1）

**AC-AUD-001 落盘与归属**：私有 daemon（独立 pipe/token/审计目录）跑 2 次成功调用 + 1 次
参数失败调用 → `shutdown` 后 `history --json` 恰好 2 条（每调用一条），`source=daemon`、
`session_id` 与调用会话一致；daemon 退出前 flush（契约测试已覆盖）。

**AC-AUD-002/003/007 脱敏**：`basic` 无 `args`；`full` 下 `desktop_input`/`desktop_clipboard_write`
文本按 `DESKCTL_AUDIT_TEXT`（`digest`=长度+sha256；`none`=删除；`full`=原文）处理；
`api_key`/`token`/`authorization`/`password`/`secret` 等键命中值替换为 `<redacted>`；
写入样例敏感值后对审计文件全文检索应无命中（单测覆盖）。

**AC-AUD-004 查询**：`--session/--owner/--tool/--run/--since/--until/--failed-only/--kind/--limit/
--json/--summary/--dir` 过滤与聚合正确；非法 `--kind` 返回 `invalid_params`（exit 2）；daemon
不在时仍可查询。

**AC-AUD-005 轮转与保留**：小阈值单测验证分片、过期删除与总量清理；单文件截断保留最新记录。

**AC-AUD-006 降级**：审计目录为不可建路径（父路径是文件）时工具照常成功，stderr 恰好一条
`audit disabled:` 告警；`DESKCTL_AUDIT=off` 不创建目录，`history` 提示已关闭（契约测试覆盖）。

**AC-AUD-008 日志与 run 规则**：`DESKCTL_LOG` 默认 `info`，日志不打印 args/文本明文；
`history --run <id>` 过滤就绪（run 产物目录随 D3 写入）。

**开销门禁**：`pwsh scripts/audit-perf-ab.ps1 -Binary <release>`——审计 off vs basic 的
`window_screenshot` p95 变化 <5%（实测 ≤0.5%，见 [performance-baseline.md](performance-baseline.md) §5）。

### 7.7 Agent 身份（D2）

**AC-AGT-001 owner 参数**：`session create --owner agent:x` 返回 owner 并显示于 `session list`；
`Agent:Alpha-1` 规范化为 `agent:alpha-1`；`session list --owner <label>` 精确过滤；非法字符
（空格/非 ASCII/超长 65）与保留名（`daemon-client`/`mcp`/`mcp:*`/`oneshot`/`bench`/`system*`）
返回 `invalid_params`（exit 2；CLI 与 daemon 双侧校验；契约测试 + 单测覆盖）。

**AC-AGT-002 MCP 映射**：`initialize.clientInfo.name` → `mcp:<name>`（小写、非法字符替换为 `-`、
截断 60 字符；`"Claude Desktop"` → `mcp:claude-desktop`）；缺失/空名 → `mcp:unknown`；
`clientInfo.version` 只写服务日志不做身份（契约测试经审计归因断言）。

**AC-AGT-003 归因**：同会话跨调用审计 `owner` 一致；`history --owner`（精确与 `prefix*` 通配）
与 `--summary.by_owner[]`（calls/ok/failed/not_sent/unknown/p50/p95）按身份过滤/聚合
（契约测试覆盖）。

**AC-AGT-004 限额**：`DESKCTL_OWNER_MAX_SESSIONS=2` 时同 owner 第 3 个会话 `resource_busy`
（exit 5，消息含 owner 与现值），其他 owner 与默认 `daemon-client` 不受影响；默认 0 不启用
（契约测试覆盖）。

**观测**：`doctor` 的 `daemon.owner_limits` 输出 `max_sessions_per_owner`/`fairness` 与
"标签（非鉴权）"说明。

### 7.8 录制与回放（D3）

**AC-RPL-001/002 录制**：会话内 `desktop_recorder_start {include_reads:"all", max_steps:N}` 后执行
≥3 次真实调用 → `desktop_recorder_stop` 返回 `deskctl-script/1`（`v/format/name/created/targets/
params/steps/notes`）；`max_steps` 与 256 KiB 上限触发后停止记录并写 `warnings`；同会话第二个
recorder_start 返回 `resource_busy`。参数化断言：绑定 `hwnd` → `$target.<名>.hwnd`、
`candidate_id` → `locator`（`app_id=$target.<名>.token`）、坐标步骤 `fragile=true`、
`capture_id` 步骤 `volatile=true`、`dispatch_state=not_sent` 的失败步骤 `optional=true`。

**AC-RPL-003/004 校验与 dry-run**：`deskctl script validate --file <path>` 纯本地执行（daemon 不在
也可用），未知工具/未解析变量/风险超限/版本错误分别给出 `issues[].code` 与退出码 2；`script_run
{dry_run:true}` 不执行工具、不产生步骤审计，逐步输出 `execute|skip_fragile|reject_risk|
reject_unknown_tool`，`outcome=null`。

**AC-RPL-005/006 执行与重试**：真实 run 返回 `deskctl-script-run/1`（`run_id/started/finished/
result/outcome/handoff/steps/rebinds/stats`）；每步审计 `request_id=<run_id>:<step>`，
`deskctl history --run <run_id>` 恰好返回该 run 的步骤；`on_error=stop|continue` 行为正确；
仅 `dispatch_state=not_sent` 的步骤按 `retry` 自动重试（`partial/unknown` 不重试）。

**AC-RPL-007 目标解析与重绑**：录制后关闭应用 → preflight 经 `launch_ref` 受限启动并绑定，
run report `relaunched=true`；执行中关闭窗口 → 单步自动重绑（预算 5 次）并在 `rebinds`
记录 `reason/from_hwnd/to_hwnd/relaunched`；同名窗口 pid 不符 → `identity_mismatch`（默认拒绝，
`strictness=loose` 放宽）；多命中 → `ambiguous_target` + `candidates`，可用 `--param <name>=<hwnd>`
覆盖。

**AC-RPL-008/009 核验与 outcome**：`verify.tier=screen` 走 `desktop_verify_state`；
`tier=readback` 用 `desktop_file_hash`/`docx_read`/`xlsx_read`/`pptx_read` 读回比对；
`require_readback=true` 但无 readback 时 preflight 拒绝；映射表全枚举：`not_sent` →
`HALTED_BEFORE_EFFECT`，`partial/unknown` 或 readback 失败 → `RECONCILIATION_REQUIRED`，
全部成功且有 readback → `VERIFIED`，无强核验 → `COMPLETED_UNVERIFIED`，预检拒绝 →
`REJECTED_POLICY`，取消 → `CANCELED`。

**AC-RPL-013/014 handoff 与并发**：三类失败（not_sent / partial / readback 失败）分别断言
`handoff.failed_step/reason/effect_may_have_occurred/safe_to_retry/resume_hint/suggested_actions`；
`RECONCILIATION_REQUIRED` 后 `--from` 默认拒绝（`resume_after_unknown`），
`allow_resume_after_unknown=true` 可续跑；同会话第二个 `script_run` 返回 `resource_busy` +
当前 `run_id`；不同会话可并发。

**AC-TXN-001/002 检查点**：`desktop_checkpoint_save` → `desktop_checkpoint_verify` 往返
`matched=true`；第 9 个新名字 `resource_busy`（exit 5）、同名可覆盖、非法名字/未知名
`invalid_params`（exit 2）；导出文件（允许根内，含尚不存在的目标文件）可原样读回；
窗口关闭后 `verify` 返回 `rect` 差异且 `missing=true`；`values_only=true` 跳过矩形比较；
四类差异（`value`/`checked`/`exists`/`rect`）各有单测样本（`checkpoint.rs` 单测）。

**AC-TXN-003 补偿**：`script run --checkpoint <name>` 的 report 含
`checkpoint{name,matched,diffs,checked}`，且该名字随后可作为普通会话检查点校验；
失败 run（`HALTED_BEFORE_EFFECT`）带 `--compensation` 时默认只产生 dry-run 补偿计划
（`compensation.applied=false`、计划步骤 `decision=execute`、零派发），
`--apply-compensation` 时真实执行（`applied=true`、内层 `dry_run=false`、`result=ok`）；
补偿脚本自身失败只记录 `compensation.error`，不改变主 run 结果；脚本
`purpose=compensation` 未显式给 `dry_run` 时为 dry-run，显式 `dry_run=false` 才执行
（契约测试 `crates/deskctl-cli/tests/checkpoint_contract.rs`）。

## 8. 端到端场景

### E2E-001 截图点击

```text
screenshot -> perceive -> mouse(capture_id, element_id) -> screenshot -> assert change
```

### E2E-002 窗口操作

```text
windows_list -> window_info -> window_activate -> window_move/resize -> window_info
```

### E2E-003 文本输入

```text
target_bind/window_activate -> input -> read file -> assert text
```

### E2E-004 UIA 语义执行

```text
targets_list -> target_bind -> semantic_observe -> semantic_candidate
-> semantic_execute -> verify_state
```

### E2E-005 持久化语义动作

```text
保存 workflow_step -> 关闭 session -> 新 session -> semantic_action
-> verify_state
```

### E2E-006 多 session 隔离

```text
session A screenshot -> session B 尝试使用 A 的 capture_id -> 必须失败
session A/B 同时输入 -> 输入不交叉
```

### E2E-007 取消

```text
启动 wait_for -> cancel -> 返回 cancelled，不再产生动作
```

### E2E-008 daemon 重启

```text
创建 session/token -> 重启 daemon -> 旧 token 失效 -> 客户端重新观察
```

### E2E-009 VS Code 新建/输入/保存（含对话框跟随）

```text
targets_list("visual studio code") -> target_bind（受限启动）-> Ctrl+N 新建未命名文件
-> 输入测试文本（send=none）-> Ctrl+S 打开原生 Save As
-> 用主窗口 hwnd 输入绝对路径：响应 target_hwnd=对话框 hwnd（自动重定向）
-> 覆盖确认弹窗：绑定对话框 -> set_value(文件名) -> invoke 保存/是(Y)
-> 读回磁盘：内容与输入完全一致；弹窗关闭后主窗口自动恢复
```

通过标准：文件内容 exact match；全程无焦点丢失；`desktop_semantic_observe`（被禁用窗口的
target token）返回 `redirected_from` 且 window.title 为对话框；`target_bind follow_dialog=true`
返回 `followed_dialog=true` 与 `parent_hwnd`。

### E2E-010 后台采集 → 三格式通用报告（含排版参数）

```text
window_screenshot（SAP 窗口，后台）-> docx_save（header/footer/page_numbers + 表格 + capture_id 图片）
-> xlsx_save（header_row + freeze_header + columns[width/number_format]，多工作表）
-> pptx_save（text_image/title_image，图片分别来自 capture_id 与 image_path）
-> docx_read / xlsx_read(range=A2:B3) / pptx_read 读回校验
```

通过标准：docx 读回 `headers`/`footers` 含设置值（页码字段为 `{PAGE}` 占位）；xlsx 的
styles.xml 含加粗字型与自定义 numFmt（≥164）、sheet XML 含 `customWidth` 与冻结窗格、
`range` 只返回区域内单元格且保持类型；pptx 每页 `images` 计数与内嵌图片数一致；生成物包
结构可被 zip 直读校验并被 deskctl 读回（人工打开以 `artifacts/` 产物为准）；全程不抢前台
（执行前后前台进程一致）。

### E2E-011 巡检键值提取与运维自愈（capture_id / read_kv / 激活重试）

```text
target_bind {hwnd}（免 targets_list）-> semantic_action focus（text_field, ordinal=0）
-> input /nST06（真实键入）-> 等待加载
-> window_screenshot（capture_id）-> read_kv {capture_id, region, value_gap}
-> perceive {capture_id}（元素坐标）-> 越界读取检查 policy_denied 文案
```

通过标准：`target_bind {hwnd}` 直接返回 token 并合成 `app:<pid>:<process>`；`read_kv` 在数值区
返回 `pairs`，其中包含 `平均进程等待（5分钟）→0.36`、`CPU数量→16`、`用户利用率→2%` 这类
标签-数值配对（伪配对带 `ambiguous=true`）；`perceive {capture_id}` 返回元素数 > 0 且不要求
图像落盘；越界 `image_path` 的 `policy_denied` 消息内联允许根目录与 `DESKCTL_ALLOWED_INPUT_ROOT`/
`capture_id` 两条出路；`window_activate` 返回 `attempts/method`，被隐藏（`visible=false`）的
窗口在激活后恢复可见并可截图。

### E2E-012 巡检录制 → 回放（D3）

```text
desktop_recorder_start {include_reads:"essential"}
-> SAP ST06 巡检步骤（target_bind / semantic_action / input / window_screenshot / read_kv / 保存报告）
-> desktop_recorder_stop -> plan.dscript.json
-> 手工补充 verify {tier:"screen", kind:"text_present", value:"操作系统监控器"} 与 readback 断言
-> deskctl script validate --file plan.dscript.json
-> deskctl script run --file plan.dscript.json --dry-run
-> deskctl script run --file plan.dscript.json --session <id>
```

通过标准：录制的 steps 数与手工 E2E-011 步骤数一致（允许差异附注原因）；`targets.*` 含
description+fingerprint 且步骤内不出现裸 hwnd；dry-run 每步 `decision=execute` 且屏幕状态不变；
真跑返回 `outcome`（声明 readback 时为 `VERIFIED`，否则 `COMPLETED_UNVERIFIED`）；
关闭 SAP 窗口后重跑触发 `relaunched=true` 并成功；故意让某步 `not_sent` 失败时，
`handoff.resume_hint.from_step` 指向失败步，修正 `--param` 后 `--from` 续跑成功。

### E2E-013 检查点 → 变更 → 补偿（D4，可选真机项）

```text
desktop_checkpoint_save {name:"before", targets:[SAP 主窗口], values:[{name:"cpu", locator:...}]}
-> 手工在 SOP 里做出可疑变更（或移动窗口）
-> desktop_checkpoint_verify {name:"before"} -> diffs 显示 rect/value 变化
-> deskctl script run --file incident.dscript.json --session <id>
   （脚本故意失败）--compensation undo.dscript.json
-> 默认 dry-run 补偿计划 -> 追加 --apply-compensation 真实回滚
-> desktop_checkpoint_verify {name:"before"} -> 值恢复
```

通过标准：`diffs` 的 `kind` 与人工改动一致（矩形移动/值变化各至少一例）；dry-run 补偿
不产生输入派发；`--apply-compensation` 后校验恢复（或如实返回残余差异）；全过程
`history --run <run_id>` 可复盘补偿步骤（`request_id=<compensation_run_id>:<step>`）。

## 9. 并发与压力测试

- 2/4/8 个 session 同时截图
- 2/4/8 个 session 同时观察 UIA
- 多 session 同时请求输入，验证串行和顺序
- 一个 session 长等待，另一个 session 正常操作
- 高频 screenshot/perceive 100 次，检查内存和临时文件
- 随机取消请求
- daemon 运行 8 小时稳定性测试
- 达到 session 上限时返回 `resource_busy`
- 关闭 session 后资源回收

## 10. 故障注入

| 故障 | 预期 |
|---|---|
| 平台 API 超时 | 返回 timeout，生成 `dispatch_unknown`（如可能已发送） |
| 窗口在动作前变化 | 拒绝动作，返回 stale/target_unavailable |
| 窗口在动作后消失 | 返回 unknown/needs_observation，不重试 |
| ONNX Runtime 缺失 | 视觉工具 `model_unavailable`，其他工具正常 |
| 模型哈希不匹配 | doctor 失败，视觉工具拒绝 |
| socket 断开 | 请求失败；daemon 回收 session |
| daemon 崩溃 | 客户端收到连接错误；旧 token 失效 |
| 两个 daemon 竞争 | 单实例锁阻止第二个 daemon |
| 工具 panic | 捕获并返回内部错误，daemon 不崩溃 |
| 磁盘写满 | 截图返回明确写入错误，不返回假路径 |

## 11. 性能测试

测量项：

- CLI `--help` 冷启动
- daemon 启动到 ready
- 本地 IPC p50/p95
- 全屏/区域/窗口截图
- 鼠标/键盘调用开销
- OCR cold/warm
- YOLO cold/warm
- 100 次调用内存曲线
- 8 session 并发 CPU/内存/延迟
- 会话关闭后的资源回落

要求：

- 使用 release 构建。
- 固定硬件、系统版本、分辨率、DPI、模型版本。
- 每次记录 p50/p95/p99、最大值和样本数。
- 与上一版本基线比较，回退超过 20% 必须说明。
- 不以单次最好成绩作为验收依据。

## 12. 安全测试

- 无 token / 错误 token 无法连接
- 非当前用户无法访问 socket/pipe
- `dict_name` 路径穿越
- 输入图片路径逃逸
- 自定义截图路径写入合法位置
- 工具参数不能注入 shell/脚本
- 日志中不出现输入文本、剪贴板、窗口标题
- 无 approve/reject 工具或方法
- `readonly` profile 不暴露输入/点击工具
- HTTP 默认关闭；开启时只绑定 127.0.0.1
- 会话隔离和 token 伪造
- 大 JSON/深嵌套参数不会导致崩溃或长时间占用

## 13. MCP 协议测试

使用独立 mock MCP client 和真实 MCP 客户端各测一遍：

- `initialize` 握手
- `tools/list`
- `tools/call` 成功
- 工具错误
- 未知工具
- 取消
- 进度
- stdout 纯净
- 客户端退出后 session 清理
- 两个 MCP 客户端并发
- profile 变化
- 版本不匹配
- 大结果分页

详细协议见 [mcp-service.md](mcp-service.md)。

## 14. 安装包测试

- 全新 Windows 安装
- 升级安装
- 卸载
- 无网络离线安装
- 在线模型更新
- 模型哈希校验
- 安装后 `deskctl doctor` 通过
- 安装包不包含源码/测试数据
- 许可证文件存在
- 用户自定义截图目录不被卸载删除

## 15. CI 流程

```text
fmt -> clippy -> unit -> contract -> Windows integration -> E2E -> package smoke
```

- Windows runner：必跑单元、契约、集成、E2E 子集。
- macOS/Linux runner：只跑编译和 feature 检查。
- 模型相关测试使用缓存目录，不在每次 CI 重新下载。
- 真实桌面 E2E 可以拆成 nightly 或手动流水线，避免阻塞每次提交。
- 任何 P0/P1 缺陷未关闭不允许发布。

## 16. 缺陷等级

| 等级 | 定义 | 发布影响 |
|---|---|---|
| P0 | daemon 崩溃、错误操作其他窗口、数据丢失、安全绕过 | 阻塞 |
| P1 | 工具主要功能不可用、session 串扰、MCP 不可用 | 阻塞 |
| P2 | 局部平台兼容、错误信息不清晰、性能回退 | 可评估发布 |
| P3 | 文案、日志、非关键体验 | 可后续修复 |

## 17. 验收映射

本测试计划的用例必须映射到 [acceptance-criteria.md](acceptance-criteria.md) 的 AC 编号：

```text
AC-CAP-*   -> 截图集成测试
AC-WIN-*   -> 窗口集成测试
AC-MOUSE-* -> 鼠标集成测试
AC-INPUT-* -> 键盘集成测试
AC-CLIP-*  -> 剪贴板测试
AC-FIND-*  -> 找图/找色/找字测试
AC-PER-*   -> OCR/YOLO 测试
AC-SEM-*   -> UIA 集成/E2E
AC-SESS-*  -> 多 session 测试
AC-MCP-*   -> MCP 契约测试
AC-CLI-*   -> CLI 契约测试
AC-ERR-*   -> 错误/取消/重试测试
AC-PERF-*  -> 性能测试
AC-SEC-*   -> 安全测试
AC-PKG-*   -> 安装包测试
AC-EXT-*   -> 扩展性评审
AC-DOC-*   -> 文档/skills 评审
```

## 18. 退出标准

Windows 首个版本发布前：

- [ ] 所有 P0/P1 用例通过
- [ ] 26 个核心工具至少各有一个成功用例
- [ ] 7 个核心语义工具至少各有一个真机用例
- [ ] 多 session 并发和隔离通过
- [ ] MCP 核心协议通过
- [ ] CLI 核心命令通过
- [ ] 无审批相关残留
- [ ] 模型随包和 doctor 通过
- [ ] 安全测试通过
- [ ] 性能基线记录完成
- [ ] 已知限制写入发布说明
