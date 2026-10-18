# 方案完整性审查 01（跨文档一致性 / 端到端逻辑）

- 审查日期：2026-10-19
- 审查对象：`docs/design/` 全部 7 份文档 + `docs/acceptance-criteria.md` §15 登记
- 方法：跨文档检索取证（编号/计数/隔离/日志/恢复语义）+ 逐域逻辑推演（正常路径、失败路径、跨会话路径）
- 结论：**主体逻辑自洽**；发现 **9 项问题**（3 高 / 3 中 / 3 低-信息），其中本轮修复 7 项、2 项留待实现期确认（见 §4）

## 1. 逐域核对（用户点名六域）

| 域 | 结论 | 证据 | 缺口 |
|---|---|---|---|
| **会话隔离** | 覆盖但不集中 | audit-history §4（审计带 session/owner）；agent-identity §4（token 隔离不变）；record-replay §4.4.0（引用作用域）；checkpoint §2（会话态存储）；rhai §3（经唯一入口） | recorder/run/checkpoint 的**会话域边界**与**同会话并发**行为未集中声明（R-04） |
| **安全** | 覆盖较好 | 无审批 + `risk_max` 默认 bounded_write（record-replay §4.1.5/§4.3）；外部提交需显式；destructive 拒绝；Rhai 沙箱限额（rhai §5/§11）；审计脱敏（audit §5.1） | ①`AC-SEC-011` 编号与既有项冲突（R-01）②屏幕文本进入**写入参数**的污染路径未约束（R-08）③tracing 日志脱敏未定义（R-05） |
| **录制/回放** | 完整 | record-replay §3 录制规则、§4 执行模型、§4.4 目标重绑、§4.5 outcome、data-formats §2 格式 | Rhai orchestrator 直连引擎、绕过步骤包装器（R-03）；run 产物保留策略缺失（R-06） |
| **日志处理** | 部分 | audit-history §4/§6/§7（审计流水、轮转、history 查询）；§8 含 `DESKCTL_LOG` | tracing 与审计的**脱敏一致性**未声明；`history` 缺 `--run` 便捷过滤（R-05/R-06） |
| **hwnd 下次失效** | 完整 | record-replay §4.4.0（作用域）+ §4.4.1（描述/指纹/句柄三层）+ §4.4.2（解析算法+错误分码）+ §4.4.4（运行时重绑+rebinds）+ AC-RPL-007 | 无（已闭环；仅需 Rhai 侧继承，见 R-03） |
| **LLM 失败后接手** | **缺失** | 现仅 outcome（§4.5）与错误分码给出"暗示"；`--from` 参数存在但未定义语义 | **无 handoff 契约**：LLM 拿不到"停在哪/能否安全重跑/下一步做什么"的显式字段与续跑语义（R-02） |

## 2. 发现项清单

| # | 严重度 | 问题 | 取证 | 处置 |
|---|---|---|---|---|
| **R-01** | 高 | 新增 `AC-SEC-011`（注入防御）与既有 `AC-SEC-011`（端口暴露）编号冲突；`AC-SEC` 已用至 012 | `acceptance-criteria.md:278` vs `:398` | 本轮改号为 **AC-SEC-013** |
| **R-02** | 高 | 回放失败后缺少 LLM 接手契约：无 `handoff` 字段、无 `safe_to_retry`、无 `resume_hint`、无建议动作列表；`--from` 未定义"变量/目标如何重建" | 检索 `handoff/resume/接手` 全库无命中 | 本轮新增 record-replay **§4.6** + data-formats `handoff` 字段 + **AC-RPL-013** |
| **R-03** | 高 | Rhai orchestrator 的 Dispatcher"逐条走 `Engine::execute_with_session`"，绕过 L1 的**步骤包装器**（目标重绑/核验分级/风险与预检），导致同一脚本两套语义 | `rhai-integration.md:41,3 节代码注释` vs `record-replay.md §4.2/§4.4` | 本轮改：Dispatcher 必须经 **StepExecutor**（与 L1 同一实现） |
| **R-04** | 中 | 会话域与并发未集中声明：同会话同时两个 `script_run` / 两个 recorder 的行为未定义（变量、取消、checkpoint 语义可能互污） | record-replay/checkpoint/rhai 各域分散 | 本轮新增 record-replay **§4.7**（会话与并发约束） |
| **R-05** | 中 | tracing 日志脱敏未定义：`DESKCTL_LOG=debug` 可能把 args/文本明文写入日志文件 | audit-history §8 仅列开关 | 本轮补日志脱敏条款（沿用审计规则、默认不含明文） |
| **R-06** | 中 | run 产物（`expanded.jsonl`/run 目录）位置与保留策略未定 | rhai §6 提到 run 目录但无规范 | 本轮定路径与保留 + `history --run` 便捷过滤 + **AC-AUD-008** |
| **R-07** | 低 | `design/README.md` D3 里程碑 AC 范围过期（`001..009`，实际已到 012） | `design/README.md:81` | 本轮更新 |
| **R-08** | 低 | R6 只包裹"输出"；屏幕文本可经 LLM 拼进**写入参数**（注入的写路径） | design/README 跨切面原则 | 本轮补策略：读入允许、写入需标记 `tainted` 并在审计留痕（实现期加 `allow_untrusted`） |
| **R-09** | 信息 | 核对通过项：工具计数（36→40→42）、新错误码与退出码映射、AC-AUD/AGT/TXN 范围、三层句柄模型、安全默认拒绝、审计字段↔ResultMeta 映射、格式版本兼容策略 | 见 §1 证据列 | 无需动作 |

## 3. 端到端逻辑推演（三个关键路径）

### 3.1 正常路径（录制→回放→产出）

recorder 录制（会话内）→ `stop` 产出 L1 JSON（candidate_id→locator、hwnd→`$target.*`）→ `validate`（离线静态校验）→ `run`（preflight：目标解析+指纹校验+变量+风险+readback 能力）→ 逐步骤执行（StepExecutor：重绑/核验/审计）→ run report（outcome=VERIFIED）→ 审计：1 条 run + N 条步骤，`run_id` 可串。**闭环 ✓**

### 3.2 失败路径（目标失效 / 部分发送）

步骤失败 → 自动重绑一次（`rebinds` 记录）→ 仍失败：若 `not_sent` → `HALTED_BEFORE_EFFECT`（可直接重跑）；若 `partial/unknown` → `RECONCILIATION_REQUIRED`（**禁止自动重试**，先核对）。**修复后闭环 ✓**（本轮补 handoff 字段后，LLM 能据此决定重跑/核对/接手）

### 3.3 跨会话/跨 Agent 路径

A 录制 → B 尝试复用 A 的 `capture_id`/`target_token`/session → 全部失效（会话域），preflight 拒绝并在错误文案给出替代（`--param`/重新观察）。审计按 owner/session 归因，资源门保证公平。**闭环 ✓**（本轮补 §4.7 后语义集中）

## 4. 本轮修复（已并入）与遗留

已修复：R-01（改号 AC-SEC-013）、R-02（§4.6 + handoff 字段 + AC-RPL-013）、R-03（StepExecutor）、R-04（§4.7）、R-05（日志脱敏）、R-06（run 产物 + AC-AUD-008 + `history --run`）、R-07（README 更新）。

遗留（实现期确认，不影响设计成立）：
1. R-08 的 `allow_untrusted`/`tainted` 具体字段名与默认值（D3 实现时定稿）；
2. 「同一会话单 run」上限是否随 `DESKCTL_MAX_SESSIONS` 提供管理员开关（D3 评审）。