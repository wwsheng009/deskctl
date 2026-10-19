# 实施状态

- 更新日期：2026-10-19
- 对应计划：[implementation-plan.md](implementation-plan.md)
- 当前版本：0.1.0（Windows 首个可运行版本）

## 1. 总体状态

| 里程碑 | 状态 | 说明 |
|---|---|---|
| M0 文档冻结 | 完成 | 架构、验收、测试、MCP、skills 文档齐备 |
| M1 Workspace 与协议骨架 | 完成 | 3 个 crate，统一结果封套，CLI/doctor/schema |
| M2 Windows 无状态工具 | 完成 | 截图、窗口、鼠标、键盘、剪贴板、找图/找色/找字、键值读取（read_kv）、等待、图像裁剪、Word/Excel/PPT 读写（页眉页脚/样式/图片版式） |
| M3 Daemon 与多 session | 完成 | Named Pipe daemon、session 管理、资源门 |
| M4 MCP 服务 | 完成 | initialize / tools/list / tools/call / prompts |
| M5 视觉与模型 | 完成 | PaddleOCR 与 YOLO 均已接入并真机验证 |
| M6 Windows UIA 语义工具 | 部分完成 | 观察/候选/执行/验证、扩展动作（set_checked/scroll/scroll_into_view/set_range_value）、内联验证与稳定采样、树分页、catalog 受限启动均可用；agent_step 未实现 |
| M7 Skills 与 profiles | 完成 | 5 个 skills、profile 过滤、契约导出 |
| M8 打包与发布 | 部分完成 | 运行时/模型获取、哈希校验、安装脚本与 Inno Setup 模板完成；性能基线与安全/会话矩阵（12/12，含输入串行与公平性）已实测（`scripts/perf-baseline.ps1`、`scripts/security-check.ps1`）；干净机安装、代码签名与 Windows 10 回归待发布流水线 |
| I1 输入模拟增强 | 完成（待真机回归） | `desktop_mouse` 新增 `move_relative`/`down`/`up` 与 `hold_ms`/`jitter_px`，`desktop_mouse_drag` 支持 `path`/`speed`/`humanize`，`desktop_input` 新增 `key`/`key_down`/`key_up`（名字/VK/扫描码、长按与重复）与拟人打字；PCG32 轨迹/节奏采样（`humanize` 默认 off，seed 可复现）；工具数保持 36；单测覆盖轨迹/键码/限额；真机回归清单见 test-plan §7.2 |
| D1 审计与历史 | 完成 | daemon/oneshot/MCP 全链路审计（JSONL v1：basic/full、文本 digest、键名单脱敏、轮转/保留/总量清理、写失败降级 disabled）；`deskctl history`（session/owner/tool/run/since/until/failed-only/kind/limit/json/summary/dir）；协议与工具数不变；契约测试 7 + 单测 15；开销 A/B p95 ≤0.5%（见 performance-baseline §5） |
| D2 Agent 身份 | 完成 | `session create --owner`（小写规范化、`[a-zA-Z0-9._:@-]`、1..64、保留名拒绝）；MCP `clientInfo.name`→`mcp:<name>`（缺失 `mcp:unknown`）；`session list --owner` 精确过滤；`DESKCTL_OWNER_MAX_SESSIONS` per-owner 限额（超限 `resource_busy`）且不影响其他 owner；审计归因与 `history --owner`（`prefix*` 通配）/`by_owner`（p50/p95）；`doctor.owner_limits`；协议与工具数不变；契约测试 4 + 单测 3 |
| D3 录制与回放 | 完成（E2E-012 真机 SAP 流程待回归） | `desktop_recorder_start/stop`（会话内单录制器、include_reads、max_steps/256KiB 上限、`allow_relaunch` 覆盖）、`desktop_script_run`（解析/静态校验/preflight/dry-run/逐步执行/重试/重绑/核验/run report/outcome/handoff）、`desktop_file_hash`（readback 强核验）；目标描述+指纹解析（strict/balanced/loose、歧义/指纹不符/stale 分码、`launch_ref` 受限重启、5 次重绑预算）；审计 `run_id` + `history --run`；CLI `record start/stop --out`、`script validate|run`；工具数 36 → 40（standard 34 → 38）；契约测试 3 + 单测 19 |

## 2. 已验证能力

以下能力已在 Windows 真机通过：

- `deskctl doctor` 返回平台、模型、DLL 和路径信息
- `deskctl tool desktop_screen_size`
- `deskctl tool desktop_windows_list`
- `deskctl tool desktop_mouse --json '{"action":"position"}'`
- `deskctl tool desktop_screenshot`（含自定义路径、区域截图、扩展名强制 PNG）
- `deskctl tool desktop_perceive`（PaddleOCR 文本元素识别）
- `deskctl tool desktop_perceive`（YOLO 图标元素识别）
- `deskctl tool desktop_find_text`（PaddleOCR 文本匹配）
- `deskctl serve` + Named Pipe
- `deskctl session create/list`
- `deskctl call <session> desktop_screenshot`
- `deskctl mcp`：initialize、tools/list、tools/call、prompts/list
- `desktop_targets_list`、`desktop_target_bind`
- `desktop_semantic_observe`（真实 UIA 树与候选动作）
- 多 session 并发截图与状态隔离
- 8 并发 session 截图（pipe 实例池 + 客户端 231 重试，0 失败、每轮 8 个唯一 capture_id）
- `scripts/perf-baseline.ps1`：AC-PERF-001..009 本机 release 实测（见 [performance-baseline.md](performance-baseline.md)）
- `scripts/security-check.ps1`：AC-SEC-001/002/003/004/005/006/009/010、AC-SESS-003/005/007/009 全部通过（12/12）
- `deskctl call` / `session inspect|close` 强制 session token：缺失返回 `auth_required`，CLI 退出码 7；错误 token 同样拒绝
- daemon 级错误按 `code: message` 前缀映射退出码（invalid_params/unknown_tool=2、stale/server_restarted=4、resource_busy=5、platform_not_supported=6、policy/permission/auth=7），见 AC-CLI-007
- `desktop_screenshot --json '{"path":"x.jpg","format":"png"}'` 显式 format 覆盖扩展名，写出 `x.png`；省略 format 时保持 `.jpg` → jpeg
- `desktop_semantic_execute` → `desktop_verify_state value_equals` 真机闭环：目标处于非前台时激活成功后写入并 `satisfied=true`
- `desktop_window_activate` 在后台 daemon 中可置前目标窗口（attach 前台/目标输入队列 + `SetActiveWindow` + `SwitchToThisWindow` 兜底）
- DPI 缩放坐标统一（Per-Monitor-V2，`doctor` 报告 dpi 感知与系统 DPI）：150% 缩放下以
  物理像素坐标直接双击桌面图标命中；窗口枚举/截图/UIA 与鼠标共用同一坐标系
- 语义扩展动作（AC-SEM-013）：`set_checked` 幂等实测（WebView2 开关 toggled=false 时不点击）；
  `scroll`（document，direction/amount）派发成功；`scroll_into_view`/`set_range_value` 已实现
- 内联验证与提交策略（AC-SEM-014/015）：`completion_policy=verified` + `expectation`
  （`expected` 双向语义）实测 satisfied；应用未反映变更时返回 unsatisfied 不冒充成功；
  `stable_samples=2` 连续采样生效；超时未达稳定次数返回 unknown
- 树分页（AC-SEM-016）：Clash Verge 首页 `tree_count=236`；第一页 200 节点 +
  `next_tree_cursor=t:200`，第二页 36 节点、同一 `observation_token`、两页无重叠
- 受限启动（AC-SEM-017）：`desktop_targets_list` 查询命中本地 catalog（`app:x:<hash>`，
  仅 Start Menu/桌面 `.lnk`）；`desktop_target_bind` 对未运行的 WezTerm 启动（pid/hwnd）
  并完成绑定，Alt+F4 关闭验证；`desktop_input`/`desktop_semantic_action` 的 `launch_ref` 就位
- 投递模式（AC-SEM-018，默认 auto）：实测后台优先、失败自动回退前台且执行成功；provider
  报 0x00000000（成功却报错）时不重复派发；前后台能力矩阵见 architecture.md §11.7
- 前台还原（AC-SEM-019）：`restore_foreground=true` 实测——动作后前台由目标窗口还原为
  Windows Terminal（显式 `foreground` 与默认 `auto` 两种模式均通过）
- 鼠标遮挡保护（AC-MOUSE-011）：capture 元素点击在目标点被 TOPMOST 终端窗口覆盖时返回
  `target_unavailable` + `occluded_by=WindowsTerminal.exe`；`allow_occluded=true` 强制执行成功
- 模态弹窗输入重定向（AC-INPUT-008）：VS Code 全链路实测——catalog 受限启动 → Ctrl+N 新建
  → 输入测试文本 → Ctrl+S 打开原生 Save As（输入 `target_hwnd` 自动重定向到对话框）→
  覆盖确认（UIA invoke 是(Y)）→ 文件落盘内容完全一致（exact_match=true，52 bytes）
- 对话框跟随产品化（AC-WIN-007/AC-SEM-020）：VS Code Ctrl+O 打开原生 Open File 后——
  `semantic_observe`（主窗口 token）自动跟随：window=[Open File]、`redirected_from`=主窗口；
  `target_bind follow_dialog=true` 返回 `followed_dialog=true`、`parent_hwnd`=主窗口、
  hwnd=对话框；Escape 同样被重定向并关闭对话框
- 图像裁剪（AC-IMG-001/002）：后台截取 SAP 会话窗口 2175×1202 → `desktop_image_crop`
  两块区域（780×300）均成功并登记为可链式 capture
- Word 报告（AC-DOC-001/002）：`desktop_docx_save`（原 `desktop_document_save`）生成 71,797 bytes `.docx`
  （6 个包条目、2 张内嵌 PNG、document.xml 含标题/图注/正文）；全程前台保持
  Windows Terminal 不变（零抢焦点）
- 办公文档读写（AC-DOC-003/004/005）：后台生成并读回——docx（38,499 bytes：6 段落/1 表格/
  1 图片）、xlsx（2,164 bytes：ST06 表 5 行类型化值）、pptx（5,677 bytes：2 页标题+要点）；
  输入路径策略实测（允许根之外 `policy_denied`，改存 `artifacts/` 后读回通过）
- 办公文档通用化（AC-DOC-006/007/008）：后台截 SAP 窗口 → docx 页眉/页脚/页码（读回
  `headers=[SAP 巡检报告]`、`footers=[第 {PAGE}]`，5 段落/1 表格/1 图片）、xlsx 表头加粗+
  冻结首行+列宽+数字格式（styles.xml 含 `<b/>`、`numFmtId="164"`/`"165"` 与
  `applyNumberFormat`，sheet1 含 `customWidth`/`state="frozen"`/`A1 s="1"`；2 工作表）、
  pptx 三页含 2 张内嵌 PNG（`images=[1,1,0]`，来源分别为 `capture_id` 与 `image_path`）；
  `desktop_xlsx_read` 的 `range=A2:B3` 精确返回 2 行类型化值；前台窗口全程未变（零抢焦点）
- 操作体验优化（AC-PER-007/008/009、AC-WIN-008/009、AC-SEM-021/022/023）：真机实测
  `desktop_perceive {capture_id}` 返回 162 个元素（不落盘、绕开允许根）；`desktop_read_kv`
  用 `capture_id + region` 在 ST06 屏幕提取 15 组配对（如 `平均进程等待（5分钟）→0.36`、
  `CPU数量→16`、`用户利用率→2%`），全图模式在旧快照上得到 32 组；
  `desktop_window_activate` 返回 `attempts/method` 诊断（实测 `attempts=1 method=set_foreground`）
  并可恢复被隐藏（`visible=false`）的窗口；`desktop_target_bind {hwnd}` 直接绑定成功
  （自动合成 `app:16432:saplogon.exe`）；UIA 名称乱码修复后同一观察返回
   `按降序排 / 设置过滤 / 最终用户文 / 查找 / 系统`（此前为 `鎸夐檷搴忔帓搴?` 等）；
   越界读取的 `policy_denied` 现在内联允许根目录与两条出路
- D1 审计与历史（AC-AUD-001..008）：三个调用入口（daemon `call`、`deskctl tool` oneshot、MCP）
  结束即写入当日 JSONL；契约测试用私有 daemon E2E 验证「每次调用恰好一条」（成功 + 失败各一），
  且 daemon 退出前 flush；oneshot 真机记录可查询，字段与 `design/audit-history.md` §4 一致，
  `args_digest` 可复算
- 文本与键名单脱敏（AC-AUD-003/007）：`DESKCTL_AUDIT_TEXT=digest` 只落 `text_len`/`text_sha256`；
  `api_key`/`apikey`/`token`/`authorization`/`password`/`passwd`/`secret` 命中值替换为
  `<redacted>` 并置 `args_redacted=true`（单测写入样例敏感值后断言原文不出现）
- 轮转与保留（AC-AUD-005）：超 `MAX_FILE_MB` 切分片；`RETAIN_DAYS`/`MAX_MB` 清理
  「先删旧文件、再从头截断最旧文件」，最新记录不丢（单测覆盖 3 类清理）
- 降级（AC-AUD-006）：审计目录不可建/不可写时工具照常成功，stderr 仅告警一次
  （`audit disabled: …`），进程内 `audit_state=disabled`；`DESKCTL_AUDIT=off` 不创建任何文件
- `deskctl history`（AC-AUD-004）：过滤（session/owner/tool/run/since/until/failed-only/kind）、
  `--limit`、`--json`、`--summary`（含 p50/p95）、`--dir` 覆盖均通过契约测试；daemon 不在也可查
- 审计开销 A/B（`scripts/audit-perf-ab.ps1`，release）：screenshot 类操作 p95 变化 ≤0.5%；
  `screen_size` 微操作 +30–40 µs/call（绝对量），见 performance-baseline §5
- D2 身份与归因（AC-AGT-001）：`session create --owner agent:x` 返回并显示于 `session list`；
  `Agent:Alpha-1` 规范化为 `agent:alpha-1`；`session list --owner <label>` 精确过滤；
  非法字符（空格/非 ASCII/超长 65）与保留名（`daemon-client`/`mcp`/`mcp:*`/`oneshot`/`bench`/`system*`）
  返回 `invalid_params`（exit 2，CLI 与 daemon 两侧校验，单测 + 契约测试覆盖）
- D2 MCP 映射（AC-AGT-002）：`initialize.clientInfo.name` → `mcp:<name>`（小写、非法字符替换 `-`、
  截断 60 字符；`Claude Desktop`→`mcp:claude-desktop`）；无 clientInfo → `mcp:unknown`；
  `version` 写服务日志不做身份（契约测试经审计记录断言）
- D2 限额（AC-AGT-004）：`DESKCTL_OWNER_MAX_SESSIONS=2` 时同 owner 第 3 个会话 `resource_busy`
  （exit 5，消息含 owner 与现值），其他 owner 与默认 `daemon-client` 不受影响；默认 0 不启用
- D2 归因查询（AC-AGT-003）：同会话跨调用审计 `owner` 一致；`history --owner`（精确 + `prefix*`）
  过滤与 `--summary.by_owner[]`（calls/ok/failed/not_sent/unknown/p50/p95）聚合通过契约测试
- D3 录制（AC-RPL-001/002）：`desktop_recorder_start`（`include_reads` 三档、`max_steps` 1..1000、
  单会话单录制器 → 重复 start `resource_busy`）、`recorder_stop` 返回合法 `deskctl-script/1`
  （`v/format/name/created/targets/params/steps/notes`）；`max_steps` 与 256 KiB 上限触发时停止记录
  并写 `warnings`；已绑定 `hwnd` → `$target.<名>.hwnd`、`app_id` → `$target.<名>.token`、
  `candidate_id` → `locator`、坐标步骤 `fragile`、`capture_id` `volatile`、`not_sent` 失败步骤 `optional`
- D3 回放（AC-RPL-003/004/005/006）：`script validate` 纯本地校验；`script_run` 预检拒绝返回
  `policy_denied`（结构错误 `invalid_params`）且 details 为 `REJECTED_POLICY` 形态的 run report；
  `dry_run` 不执行工具、不产生步骤审计，逐步输出 `execute|skip_fragile|reject_risk|reject_unknown_tool`；
  真正执行逐步写入 `request_id=<run_id>:<step>`，`history --run <run_id>` 可分步复盘；
  仅 `not_sent` 可自动重试/重绑（`partial/unknown` 不重试）
- D3 目标与核验（AC-RPL-007/008）：描述+指纹解析（`strictness` strict/balanced/loose），
  歧义 `ambiguous_target`、指纹不符 `identity_mismatch`、句柄失效 `stale_target` 分码；
  `launch_ref` 受限重启并记 `relaunched`；单 run 重绑预算 5 次；`screen`/`readback` 两级核验，
  `file_sha256` 经 `desktop_file_hash` 读回比对
- D3 结果契约（AC-RPL-009/013/014）：run-level `outcome`（`VERIFIED`/`HALTED_BEFORE_EFFECT`/
  `RECONCILIATION_REQUIRED`/`COMPLETED_UNVERIFIED`/`REJECTED_POLICY`/`CANCELED`）；
  `handoff` 含 `run_id/failed_step/reason/effect_may_have_occurred/safe_to_retry/resume_hint/
  suggested_actions`；`RECONCILIATION_REQUIRED` 后 `--from` 默认拒绝，需
  `allow_resume_after_unknown=true`；同会话并发 run 返回 `resource_busy` + 当前 `run_id`

## 3. 测试状态

`cargo test --workspace` 通过：

```text
deskctl-cli:      30 passed（agent_identity 4、audit_history 7、cli_contract 3、mcp_smoke 4、script_contract 3、skills_contract 1、tool_contract 6、main 单测 2）
deskctl-core:     98 passed（lib 92、path_policy 2、session_isolation 4）
deskctl-protocol:  3 passed
合计:            131 passed, 0 failed
```

`cargo fmt --all -- --check` 与 `cargo clippy --workspace --all-targets -- -D warnings` 均无告警
（clippy 增量缓存偶尔导致 clippy-driver 崩溃时，可加 `CARGO_INCREMENTAL=0` 重跑）。

覆盖：

- 40 工具 Schema 合法性
- 38/40 profile 工具数量
- readonly 不暴露输入工具
- 无 approve/reject/approval 工具
- skills manifest 与工具名一致性
- CLI help、doctor、schema 导出
- MCP initialize/list/call
- 截图路径强制 PNG
- 颜色解析、图像匹配、按键校验
- OCR 模型缺失时安全报错
- dict_name 路径穿越拒绝（path_policy）
- session 隔离、关闭后 token 失效、会话上限 resource_busy（session_isolation）
- MCP 未初始化拒绝 tools/list、协议版本校验、notifications/cancelled 取消等待
- 扩展动作/树游标/delivery 解析（tools::semantic 单测）
- 应用 catalog：仅收 .lnk、`app_ref` 稳定且不可伪造、未知 ref 拒绝、扫描上限（apps 单测）
- docx 包结构（[Content_Types]/rels/document/media）与 XML 转义（docx 单测）
- 办公三件套往返：docx 段落/表格/图片与页眉/页脚/页码（`{PAGE}` 回读）、xlsx 值类型/样式/
  列宽/冻结/数字格式与 `range` 过滤、pptx 标题/要点/图片页与版式校验（docx/xlsx/pptx 单测）
- I1 输入模拟：PCG32 确定性（同 seed 序列一致）、轨迹采样（非直线/点数上限/末端减速）、
  打字延迟范围与固定值、参数校验与限额、可取消睡眠（humanize 单测）
- I1 工具层：`delay_ms` 固定/范围解析与上限拒绝、键码标识（vk/scan/扩展位）、
  落点抖动边界（元素内缩 2px、微元素回退中心、±px 边界与 seed 确定性）、按钮参数校验
  （tools 单测）
- D1 审计：记录 JSON 形状与 digest 可复算、basic/full 模式、文本策略（none/digest/full）、
  键名单脱敏、超长截断与三级降级、轮转（小阈值）、清理（过期/总量/单文件截断）、
  写失败降级、off 不落盘（audit 单测）
- D1 契约：oneshot 审计与查询字段、失败入账、`--summary` 聚合、`--dir` 覆盖与非法 `--kind`、
  off 模式提示、daemon E2E 恰好一条（含失败）、审计目录不可用降级（audit_history 契约测试）
- D2 身份：`validate_owner` 小写规范化、非法字符/长度/保留名拒绝、owner 可重设（MCP initialize）
  （session 单测）
- D2 契约：owner 创建/规范化/列表过滤/非法拒绝、per-owner 限额（`resource_busy` exit 5）与跨 owner
  不受影响、审计归因与 `history --owner`（精确 + `prefix*`）/`by_owner` 聚合、MCP clientInfo 映射
  （含含空格名清洗）与缺失回退 `mcp:unknown`（agent_identity 契约测试）
- D3 脚本模型：解析/版本与步骤 ID 校验、`$param`/`$var`/`$target`/`$steps`/`$env` 替换、
  `expect` 断言、`risk_max` 默认值与 `external_commit` 显式放行、静态校验（未知工具/风险/引用）、
  outcome 分类与 handoff 推导（script 单测 10）
- D3 录制器：`include_reads` 过滤、重复 start 拒绝、`max_steps` 截断、回放步骤不二次录制、
  坐标/capture 标记、`not_sent` 转 `optional`、脚本形状（recorder 单测 9）
- D3 契约：录制 → 导出 → `script validate` → `script run --dry-run` → 真跑（`run_id` 审计分步）→
  同会话并发 `resource_busy`；预检拒绝（未知工具/风险超限/未解析变量）；`not_sent` 失败 handoff 与
  `--from` 续跑；`file_sha256` readback 通过/失败与续跑门禁（script_contract 3）

## 4. 运行时资源

已独立管理，不再依赖 Nuphus：

```text
third_party/onnxruntime/win-x64/
├─ onnxruntime.dll
├─ onnxruntime_providers_shared.dll
├─ hashes.json
└─ README.md

models/
├─ ch_PP-OCRv4_det.onnx
├─ ch_PP-OCRv4_rec.onnx
├─ ch_PP-OCR_keys_v1.txt
├─ icon_detect.onnx
└─ hashes.json
```

脚本：

```powershell
pwsh scripts/fetch-runtime.ps1 -VerifyOnly
pwsh scripts/fetch-runtime.ps1
pwsh scripts/fetch-models.ps1 -FromDirectory <dir>
pwsh scripts/fetch-models.ps1
```

构建脚本 `crates/deskctl-cli/build.rs` 会自动复制：

- `onnxruntime*.dll` → `target/<profile>/` 和 `deps/`
- `models/*` → `target/<profile>/models/`

模型和 DLL 不提交 Git，由脚本获取。

## 5. 已知缺口

### 高优先级

- `desktop_vision` 需要外部视觉 provider，当前返回 `model_unavailable`
- `desktop_agent_step` 需要增强决策 provider，当前返回 `model_unavailable`
- `desktop_find_image` 的模板匹配为简化实现，超大模板或强缩放场景精度有限

### 中优先级

- 安装包脚本已提供；干净 Windows 机器安装与签名未执行
- Windows 10 未做真机回归
- 性能基线为单机、单屏幕内容的一次测量；固定参考机器与跨版本回退对比未建立
- 取消已覆盖等待类工具；其他长任务的协作式取消仍需逐一补齐
- session 空闲回收：daemon 每 30 秒扫描一次，另在新会话创建时触发
- 审计为本地证据（明文 JSONL、无签名/防篡改），定位是复核与归因，不是安全边界
- D3 续跑仅支持由 `params` 显式回填被跳过步骤的 `$var`/`$steps` 值（未接入
  `history --run` 自动回填）；`strict` 的 `process_start_time` 指纹在平台层不可得时退化为
  pid+class+title；`dry_run` 仍会解析/绑定目标（不执行工具、不写步骤审计）

### 低优先级

- macOS / Linux 仅保留 trait 与占位
- HTTP 适配层未实现
- 在线模型更新未实现
- 审计队列满时新记录被丢弃（`audit_dropped` 计数行周期性提示）；run 产物目录策略已就绪，写入随 D3

## 6. 与验收标准的差距

已满足：

- AC-SCOPE
- AC-CAP-001/002/003/004/005/006
- AC-WIN-001/002/003/004
- AC-MOUSE-001/002/004/005
- AC-CLIP
- AC-SEM-001/002/003/004/005/007/008/009/010/011
- AC-SEM-006（真机 smoke：保存 locator 后 `desktop_semantic_action` 重放）
- AC-SEM-013/014/015/016/017/018/019、AC-MOUSE-011（真机 smoke：幂等 set_checked、expected
  双向语义+稳定采样+unsatisfied 不冒充成功、tree_cursor 两页续读无重叠、catalog 受限启动
  并绑定/关闭、delivery_mode 后台优先+前台回退、restore_foreground 还原、点击遮挡阻断/强制）
- AC-INPUT-008（真机 smoke：VS Code 新建/输入/保存全链路，保存与覆盖确认对话框输入重定向）
- AC-WIN-007/AC-SEM-020（真机 smoke：Open File 对话框观察自动跟随、follow_dialog 绑定、
  Escape 重定向关闭）
- AC-IMG-001/002、AC-DOC-001/002（真机 smoke：后台截图 SAP 窗口 → 双区域裁剪 → docx
  汇总（含内嵌图）→ 包结构校验；前台全程未被抢占）
- AC-DOC-003/004/005（真机 smoke：docx/xlsx/pptx 生成并读回、含表格与类型化值；
  输入路径策略与后台执行验证）
- AC-DOC-006/007/008（真机 smoke：docx 页眉/页脚/页码读回、xlsx 样式/冻结/列宽/数字格式与
  `range` 读取、pptx 内嵌 PNG 与版式校验；前台窗口全程未变）
- AC-PER-003（OCR/YOLO 合并去重 smoke）
- AC-SESS-001/002/004/005/006
- AC-SESS-003/009（安全脚本实测：8 并发输入串行、0 失败、p50=708ms/max=1129ms 无饥饿）
- AC-SESS-007（安全脚本实测：会话上限 resource_busy）
- AC-SESS-008（真机实测：`%TEMP%` 下的会话截图随关闭删除、capture cache 清空；默认 captures 目录为持久目录，保留）
- AC-MCP-001/002/003/004/005/010/012
- AC-CLI-001/002/003/005/007/008/010（007：退出码 2/4/5/7 已真机实测；6 仅在非 Windows 平台出现）
- AC-ERR-001/002/004/005/007/009
- AC-PKG-002/003/007
- AC-DOC
- AC-PERF-001/002/003/004/005/007/008/009（本机 release 实测，见 [performance-baseline.md](performance-baseline.md)）
- AC-SEC-001/002/003/004/005/006/009/010（安全脚本 12/12：本地监听、token 校验、无审批字段、风险元数据、自定义路径、字库穿越、会话隔离、无脚本执行）
- AC-AUD-001..008（D1：daemon E2E 恰好一条含失败、oneshot 字段与 digest 复算、文本/键名单脱敏、
  history 过滤与聚合、轮转保留、写失败降级、日志默认不含明文与 `--run` 过滤就绪；开销 A/B ≤0.5%）

未满足或未验证：

- AC-SEM-012（agent_step）
- AC-PERF-006（已有 warm 基线记录，跨版本回退对比未建立）
- AC-SEC-007/008（读取限制/日志脱敏未逐条实测）
- AC-PKG-001/004/005/006
- AC-EXT-*
- Windows 10 兼容性

## 7. 结论

当前版本已经是可运行的 Windows 桌面工具服务：

- 可独立构建
- 不依赖 Nuphus
- 不包含审批流程
- 支持多 session
- 支持 MCP
- 支持 OCR 和 UIA
- 支持审计与历史查询（D1）

但尚未达到 `acceptance-criteria.md` 的完整发布门禁。性能与安全/会话矩阵已完成本机实测；
主要剩余工作是干净机安装、代码签名、固定参考机器基线、Windows 10 回归与 AC-EXT-*。
