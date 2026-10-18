# 先例调研：桌面自动化 / 录放 / 审计 / 多 Agent（GitHub）

- 文档版本：0.1
- 日期：2026-10-19
- 状态：Draft（调研记录，结论待评审）
- 方法：GitHub Search API（repositories，按 stars 排序）+ 关键项目 README 原文 + 网络搜索；查询串见 §6
- 关联：[README.md](README.md)、[audit-history.md](audit-history.md)、[record-replay.md](record-replay.md)、[agent-identity.md](agent-identity.md)、[checkpoint-transaction.md](checkpoint-transaction.md)

## 1. 结论速览

1. 本方案（审计/身份/录放/检查点）在开源生态中**均有先例**，机制方向一致；差异集中在两点：
   - **错误与结果的分类学**比我们细（stale/ambiguous 目标、run-level `transaction_outcome`）；
   - **效果核验的位置**比我们更严：不只验屏幕，而是**独立读回目标系统**（OpenAdapt 实测：屏幕"保存成功"提示在 72/90 的故障注入用例中骗过了检查，而数据库读回全部识破）。
2. 我们的**"无审批 + 默认拒绝高风险"**路线与主流（confirm/审批门）不同，但可保留；可用"人类可见性（指示灯/急停）"补足透明性。
3. 审计方向与 `agent_recorder` / `agentlens` 高度一致：本地、结构化、失败开放、脱敏、可导出（JSONL/HAR/OTLP）；`agentlens` 的**哈希链 tamper-evident** 值得作为可选档位。
4. `screenpipe`（22k★，"Open Computer History"）证明"连续记录屏幕→工作流挖掘"是独立品类；我们的 `history`（工具调用流水）与它互补，不必做连续录屏。
5. 值得直接吸收的 7 条修订：见 §5（R1–R7）。

## 2. 同类项目清单（按赛道）

### 2.1 桌面/计算机使用（MCP 或库）

| 项目 | ★ | 关键机制 | 与我们关系 |
|---|---|---|---|
| [bytedance/UI-TARS-desktop](https://github.com/bytedance/UI-TARS-desktop) | 39,314 | 视觉驱动 GUI Agent 栈（模型+基建） | 视觉路线对照；我们坚持 a11y/OCR 优先 |
| [microsoft/OmniParser](https://github.com/microsoft/OmniParser) | 25,530 | 截屏→结构化元素解析（纯视觉） | 与 `perceive` 的"图像→元素"同层，但纯视觉 |
| [CursorTouch/Windows-MCP](https://github.com/CursorTouch/Windows-MCP) | 9,459 | Windows MCP server（computer use） | 最直接的同平台竞品 |
| [lahfir/agent-desktop](https://github.com/lahfir/agent-desktop) | 1,806 | **快照作用域 ref**（`@snapshot:e3`）+ `STALE_REF`/`AMBIGUOUS_TARGET` 错误分类；headless 默认 | **与 hwnd 失效处理直接相关（见 R1/R2）** |
| [mrpulor-gh/nuphus-mcp](https://github.com/mrpulor-gh/nuphus-mcp) | 326 | 桌面 MCP server（stdio），含 Chrome 控制 | 名称与本项目源流（nuphus）相关，值得比对工具面 |
| [AmrDab/clawdcursor](https://github.com/AmrDab/clawdcursor) | 404 | a11y+OCR 融合为 confidence 评分 UI map（`{element_id, snapshot_id}`）；**动作后 reactive verification（DEVIATION 语义）**；单一安全闸门 allow/confirm/block；屏幕边缘指示灯+急停；`<untrusted-screen-content>` 包裹屏幕文本防注入；密码字段脱敏 | **多项可直接借鉴（R3/R5/R6/R7）** |
| [shanselman/FlaUI-MCP](https://github.com/shanselman/FlaUI-MCP) | 106 | FlaUI/UIA 驱动的 Windows MCP | UIA 定位模型参考 |

### 2.2 录制 / 回放 / 流程编译

| 项目 | ★ | 关键机制 | 与我们关系 |
|---|---|---|---|
| [OpenAdaptAI/OpenAdapt](https://github.com/OpenAdaptAI/OpenAdapt) | 1,780 | `record → compile → certify → qualify → run with checks`；**独立效果核验**（读回系统记录而非屏幕）；run-level `transaction_outcome`（VERIFIED / HALTED_BEFORE_EFFECT / RECONCILIATION_REQUIRED / COMPLETED_UNVERIFIED / REJECTED_POLICY / CANCELED / FAILED_PLATFORM）；无模型调用运行；运行回执（receipt） | **录放设计的最强参照（R3/R4）** |
| [A9T9/RPA](https://github.com/A9T9/RPA) | 2,084 | 浏览器+桌面自动化 + MCP；录制宏或写 JS | 录放与 MCP 结合的先例 |
| [beuaaa/pywinauto_recorder](https://github.com/beuaaa/pywinauto_recorder) | 200 | pywinauto 上的录放工具（控件属性定位） | 同平台录放实现参考 |
| [ugarchance/record-and-replay-skill](https://github.com/ugarchance/record-and-replay-skill) | 45 | 把"用户演示"转成可复用 Agent Skill（浏览器 Playwright / 桌面 OpenAdapt） | 与"录制成脚本供 Agent 用"同思路 |
| [oculix-org/SikuliX1](https://github.com/oculix-org/SikuliX1) | 3,257 | 图像识别定位（经典兜底手段） | 我们标 `fragile` 的坐标/图像步骤可参考其相似度阈值策略 |
| [aisingapore/TagUI](https://github.com/aisingapore/TagUI) / [tebelorg/RPA-Python](https://github.com/tebelorg/RPA-Python) | 6,338 / 5,504 | 图像+文本混合定位的 RPA | 定位策略组合参考 |

### 2.3 定位稳定性 / 自愈

| 项目 | ★ | 关键机制 |
|---|---|---|
| [microsoft/playwright-mcp](https://github.com/microsoft/playwright-mcp) | 38,339 | 浏览器 MCP；**locator 自动等待/自动重试**（网络版"失效重绑"范式） |
| [healenium/healenium-web](https://github.com/healenium/healenium-web) | 201 | **自愈定位**：失败时用备用策略/历史定位库恢复，并报告"healed" |
| [Selenium](https://github.com/SeleniumHQ/selenium) 生态 | — | `StaleElementReferenceException` 语义：**元素引用属于文档/上下文，变化即失效，必须重新查找** |

### 2.4 审计 / 可观测 / 历史

| 项目 | ★ | 关键机制 | 与我们关系 |
|---|---|---|---|
| [screenpipe/screenpipe](https://github.com/screenpipe/screenpipe) | 21,977 | "Open Computer History"：连续录屏+OCR+检索，用于工作流挖掘 | 与我们 `history`（调用级）互补；不做连续录屏 |
| [AMAP-ML/LongHorizon-Harness](https://github.com/AMAP-ML/LongHorizon-Harness) | 1,754 | 长程任务：durable verified state、independent auditing、recoverable progress | 与检查点/审计设计互相印证 |
| [agentkitai/agentlens](https://github.com/agentkitai/agentlens) | 24 | MCP-native **tamper-evident** 事件日志（实时面板） | 我们的审计可选加哈希链（R5） |
| [EdytaKucharska/agent_recorder](https://github.com/EdytaKucharska/agent_recorder) | 9 | MCP/工具调用落 SQLite；**fail-open**（记录失败不阻塞）；自动脱敏 key 名单（api_key/token/authorization/password/secret）+ 截断；导出 JSONL/HAR/**OTLP**；`tail -f` 与 TUI；层级事件树 agent→subagent→skill→tool | **与我们审计设计几乎同构（R5/R6 印证）** |
| [valtors/observer](https://github.com/valtors/observer) / [localtonet/McpNet](https://github.com/localtonet/McpNet) | 3 / 5 | MCP 代理侧透明记录、per-client 限流与审计 | 说明"代理层审计"是常见部署形态 |

## 3. 机制对照（我们 vs 先例）

| 关注点 | 我们的设计 | 先例做法 | 差距/取舍 |
|---|---|---|---|
| 目标失效 | 描述+指纹+重绑，`target_unavailable` 单一码 | agent-desktop：**快照作用域 ref**；`STALE_REF` / `AMBIGUOUS_TARGET` 分码，明确要求"重新快照" | **采纳分码**（R1/R2） |
| 元素引用 | `observation_token` + candidate → locator；AC-SEM-006 跨进程重放 | Selenium StaleElement / Playwright 自动重试 / healenium 自愈 | 我们已有"观察作用域"，需把"跨观察必须 re-observe"写死 |
| 效果核验 | `expectation`/`verify_state`：屏幕/UIA 级 | OpenAdapt：**独立读回**（第一个 run 校验记录条数）；fault 注入实验证明屏幕检查会骗人 | **补强：效果核验分级**（R3） |
| 运行结果 | `ok` + `dispatch_state` + `verification` + `on_error` | OpenAdapt：`transaction_outcome`（含 RECONCILIATION_REQUIRED = 可能已写入，需人工核对） | **采纳 run-level outcome**（R4） |
| 重放安全 | preflight + `risk_max` 默认 bounded_write；无审批 | OpenAdapt：`replay` 明示"无检查、别对生产用"；只允许认证过的 bundle 走 run；clawdcursor：confirm 门 | 保留无审批，但**显式区分 replay(检查关)/run(检查开)**，并强化文案 |
| 审计 | JSONL 按天、脱敏 text、失败也记、丢弃计数 | agent_recorder：SQLite + fail-open + key 脱敏 + OTLP/HAR 导出 + TUI/tail；agentlens：哈希链 | 补 key 名单与密码字段识别（R5）；哈希链列可选 |
| 多 Agent | 会话隔离 + owner 标签 + 全局资源门 | agent_recorder 的 agent→subagent→skill→tool 事件树；MCP 网关审计按 client | 身份/层级可后续在审计事件中体现（owner 已覆盖一层） |
| 屏幕内容防注入 | （未设计） | clawdcursor：`<untrusted-screen-content>` 包裹 | **采纳**（R6） |
| 人类可见性 | 无（专注后台） | clawdcursor：指示灯 + 急停；OpenAdapt：Desktop 审批界面 | 采纳轻量版（R7，可选） |
| 流程治理 | 脚本 + preflight + verify | OpenAdapt：certify/qualify/admission（对 exact bundle+环境发证） | 超出本期；列为长期演进 |

## 4. 强力证据：为什么"屏幕级成功"不够

OpenAdapt README 的故障注入实测（2026-10-14/15，本地 OpenEMR 测试副本）：

- 90 次故障注入运行中，**72 次在"成功提示"背后留下了错误记录**；
- 以"成功横幅"为判据的检查在其中 **54/72 误判成功**；
- **完整读回数据库**的检查 **72/72 全部识破**。

对我们的直接含义：`desktop_verify_state`/`expectation` 是**弱核验**（屏幕/UI 层），脚本对"有系统记录可读回"的场景（文件、xlsx、导出物、可读 API/DB）应支持**强核验（independent read-back）**，并在 run report 中区分二者。

## 5. 建议修订清单（待评审后并入各设计文档）

| # | 修订 | 落点 | 优先级 |
|---|---|---|---|
| R1 | 目标错误分码：`stale_target`（窗口/句柄已失效，需重解析）、`ambiguous_target`（多候选，拒猜）、`identity_mismatch`（指纹不符）替代单一 `target_unavailable`；错误文案带"下一步动作建议" | record-replay §4.4、error.rs 文案 | P1 |
| R2 | 明确"引用作用域"：`hwnd`/`target_token`/`capture_id`/candidate 均属于"观察窗口"；跨观察必须重新解析/重绑（对齐 agent-desktop 的 snapshot-scoped ref 与 Selenium 语义） | record-replay §4.4、session 文档 | P1 |
| R3 | **效果核验分级**：`verify.kind = screen`（弱，现有）\| `readback`（强：文档读回/文件哈希/行数比对）；`script_run` 允许声明多级校验；run report 标注每步核验级别 | record-replay §4.2/§5、data-formats §2.3 | **P1（最高）** |
| R4 | run-level `outcome` 枚举：`VERIFIED / HALTED_BEFORE_EFFECT / RECONCILIATION_REQUIRED / COMPLETED_UNVERIFIED / REJECTED_POLICY / FAILED_PLATFORM`，并映射到"可安全重试/需人工核对/勿重试" | data-formats §2.5、record-replay §4.2 | P1 |
| R5 | 审计脱敏增强：密码字段（UIA `IsPassword=true`）永不记录；key 名单（api_key/token/authorization/password/secret）；明示 fail-open；可选导出 OTLP；**可选**哈希链 tamper-evident 档位 | audit-history §5/§8/§13 | P1 |
| R6 | 屏幕文本注入防御：`perceive`/`read_kv`/`history` 返回的屏幕文本统一包裹 `<untrusted-screen-content>…</untrusted-screen-content>` 并附说明（数据非指令） | 工具输出规范 + llm-skills | P1 |
| R7 | 可选人类可见性/急停：运行期间窗口指示灯或任务栏提示 + "双击停止"（与无审批原则不冲突的透明机制） | 长期（D3+，默认关闭） | P2 |
| — | 不采纳：审批门（confirm/approve）——与项目"无审批"决策冲突，仍以"默认拒绝高风险 + 显式参数"替代 | — | — |
| — | 暂不采纳：certify/qualify/admission 全流程（OpenAdapt 的合规级治理），列为长期演进方向 | — | — |

## 6. 调研复现（查询与命令）

GitHub Search API（unauthenticated，注意 10 req/min 限速）：

```powershell
$q = 'desktop automation record replay'   # 依次替换为下列查询
$r = Invoke-RestMethod "https://api.github.com/search/repositories?q=$([uri]::EscapeDataString($q))&sort=stars&order=desc&per_page=6" -Headers @{ 'User-Agent'='deskctl-research' }
$r.items | Select-Object stargazers_count, full_name, description
```

已执行查询：`desktop automation record replay`、`windows uia automation library`、`mcp server desktop automation`、`computer use agent desktop`、`self-healing locator test automation`、`mcp audit log observability`、`healenium self-healing`、`selenium ide record replay`、`playwright mcp server`、`flaui windows automation`、`pywinauto`、`sikulix image automation`、`openadapt`、`Windows-MCP desktop`、`UI-TARS desktop`、`OmniParser`、`TagUI RPA`、`screenpipe`。

关键 README 已逐字阅读：`lahfir/agent-desktop`、`AmrDab/clawdcursor`、`EdytaKucharska/agent_recorder`、`OpenAdaptAI/OpenAdapt`。

## 7. 评审待定项

1. R3 的强核验范围：哪些工具提供 `readback` 实现？（`docx/xlsx/pptx_read`、`sha256` 文件比对、结构化导出；脚本可显式声明，未声明则视为弱核验）
2. R4 的 outcome 是否需要进入**工具结果封套**（对所有工具生效）还是仅脚本 run report（建议后者先行，避免协议面扩散）
3. R6 的包装是否会干扰既有 LLM 集成（需要一次兼容性评估；可用开关 `DESKCTL_UNTRUSTED_WRAP=on|off`）
4. R7 是否纳入首个发布（涉及 UI 线程与窗口叠加，评估成本后再定）