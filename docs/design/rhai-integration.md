# Rhai 集成架构（深化设计，L3）

- 文档版本：0.1
- 日期：2026-10-19
- 状态：Draft（待评审）
- 关联：[adr-001-script-engine.md](adr-001-script-engine.md)（决策）、[record-replay.md](record-replay.md)、[data-formats.md](data-formats.md)、[audit-history.md](audit-history.md)
- 事实基线（2026-10-19 实测）：`rhai 1.26.1`，仓库 5,739★（2026-10-17 活跃），crates 下载 13.1M，MIT OR Apache-2.0，MSRV 1.66；默认 features `["std", "ahash/runtime-rng"]`。

## 0. 修正记录（对 ADR-001 的校正）

| 项 | ADR-001 原文 | 实测结论 | 影响 |
|---|---|---|---|
| Rhai async | "有 `async` feature（tokio 兼容）" | **错误**：crates.io 1.26.1 features 表**不存在 async/tokio feature** | 执行模型改为**同步桥接**（独立线程 + 命令通道），不依赖引擎异步能力 |
| 限额 API | 泛述 | 已确认：`set_max_operations`（0=不限，默认不限）、`set_max_call_levels`（默认 64，debug 8）、`set_max_string_size`（0=不限）、`set_max_variables`（0=禁止定义）、`on_progress(Fn(u64)->Option<Dynamic>)` 可强制终止（`EvalAltResult::ErrorTerminated`） | 限额表按实测 API 落定 |
| 线程建议 | — | 官方建议：**在独立线程 + 更大栈**运行引擎以放宽栈限制 | 采用专用线程（显式 8 MiB 栈） |

## 1. 目标与范围

在保持"JSON 是唯一交换/审计格式、副作用只经工具白名单"两条不变量下，把 Rhai 作为**可选编排层**接入：

- 模式 A（Generator，低权限）：Rhai 只做计算与"生成步骤"，产物是 L1 JSON 脚本；引擎不触碰桌面。
- 模式 B（Orchestrator，opt-in 高权限）：Rhai 通过宿主函数 `call()` 直接驱动工具；运行产出**展开调用序列**（expanded transcript），可回填为 L1 JSON 复现。

不在范围：把 Rhai 变成默认脚本形态；Rhai 脚本作为审计/回放的第一类产物（产物始终是调用序列/JSON）。

## 2. 组件与依赖图

```text
                        deskctl-cli (daemon / mcp / cli)
                                     │
                      ┌──────────────▼───────────────┐
                      │  deskctl-core                │
                      │  ├─ script.rs（L1 解释器，默认）│
                      │  └─ script_rhai.rs（feature="rhai-script"）   ← 本文件范围
                      │        ├─ RhaiHost（引擎装配/限额/能力）        │
                      │        ├─ Dispatcher（命令通道→工具执行）      │
                      │        └─ Transcript（展开序列/审计关联）      │
                      └──────────────┬───────────────┘
                                     │ Engine::execute_with_session（既有唯一入口）
                                     ▼
                     会话隔离 / 资源门 / 取消 / 审计 / dispatch_state 语义（全部继承，不旁路）
```

- **feature gate**：`deskctl-core` 新增可选 feature `rhai-script`（默认关闭）；默认构建不含 `rhai` 依赖。
- **Cargo（建议）**：`rhai = { version = "~1.26", default-features = false, features = ["std", "sync", "serde", "no_module", "no_time", "no_custom_syntax"] }`
  - `sync`：Engine/AST/Scope 可跨线程（专用线程 + 缓存 AST 需要）；
  - `no_module`：**编译期移除**模块/文件加载能力（沙箱第一层）；
  - `no_time`：移除时间戳类型（确定性 + 去掉一类侧信道）；
  - `no_custom_syntax`：不接受自定义语法（减小攻击面）；
  - `serde`：JSON ↔ Dynamic 桥；
  - 明确**不启用** `unchecked`、`internals`、`debugging`、`metadata`、`decimal`、`grain`（实验性）、`no_std`。

## 3. 执行模型（同步桥接）

**前提**：Rhai 1.26 无 async feature；deskctl 工具是 async。因此采用"**脚本线程同步、调用异步**"的桥接：

```text
[run 开始]
  1) 取/编译 AST（LRU 缓存，key = script sha256 + feature 指纹）
  2) 起专用 OS 线程（std::thread::Builder::stack_size(8 MiB)）
       └─ 该线程内：Engine + Scope + call_script()
  3) 起 tokio Dispatcher 任务（同一 run_id）
       └─ 顺序消费命令通道，逐条走 Engine::execute_with_session

[脚本线程内 call(tool, params)]
  host_fn:
    a) 取消/超时预检（cancel flag / deadline）
    b) 组装 Command{tool, params, step_index} → blocking_send(cmd_tx)
    c) reply_rx.recv_timeout(剩余预算) —— 阻塞的是脚本线程，不是 runtime worker
    d) 结果 JSON → Dynamic（serde）返回给脚本；失败以 `ok=false` map 返回（脚本可分支处理）

[run 结束]
  4) 线程 join（或超时强杀：仅标记 + 关闭通道，线程在下一个 progress 检查点自行退出）
  5) Dispatcher 汇聚 transcript → run report（outcome/rebinds/stats）+ 审计
```

### 3.1 为什么这样安全（无死锁）

- `tokio::main` 默认 **multi-thread** runtime（`crates/deskctl-cli/src/main.rs:148`）→ runtime worker 永不被脚本线程阻塞占用；
- 脚本线程是**独立 OS 线程**，它对 `recv_timeout` 的阻塞不影响 tokio；
- Dispatcher 在 runtime 内**串行**执行本 run 的工具调用 → 与 L1 解释器行为一致（顺序、逐步审计、逐步取消检查）；
- 禁止：在 runtime worker 内直接跑 Rhai 引擎（阻塞 worker）+ 在引擎内 `block_on` 同一 runtime（自锁）。二者均被架构排除。

### 3.2 取消与超时

| 触发 | 机制 | 结果 |
|---|---|---|
| 调用方 `request/cancel`（run_id） | 共享 `AtomicBool`；Dispatcher 在下一次工具调用入口检查；`on_progress` 每 N 次操作检查 | 脚本线程被 `Some(token)` 终止 → `EvalAltResult::ErrorTerminated` → 映射 `cancelled` |
| 运行超时（`wall_clock` 默认 120s） | `on_progress` 内比较单调时钟 + `recv_timeout` 剩余预算 | 同上，`termination=deadline` |
| 单次工具调用超时 | `recv_timeout` 超时 → host 函数返回 `{ok:false,error:{code:"timeout"}}`（脚本可处理）；连续超时按 `on_error` | 不静默丢调用 |

## 4. 宿主 API 表面（唯一副作用路径）

| 宿主函数 | 权限 | 语义 |
|---|---|---|
| `call(tool, params)` → map | **写入级**（经白名单 + 资源门 + 审计） | 唯一触达工具的方式；返回结果封套（`ok/data/error/meta`），脚本据 `meta.dispatch_state` 分支 |
| `read(tool, params)` → map | 只读级 | 仅允许 `read_only=true` 的工具（`perceive/read_kv/screenshot/xlsx_read/…`）；不进入资源门的写路径 |
| `param(name)` / `params()` | — | 脚本参数（`--param` 覆盖） |
| `set_var/get_var` | — | run 内变量（替代 `$var.*`；与 L1 变量空间打通） |
| `log(level, msg)` / `emit(obj)` | — | 结构化输出 → run report `logs[]`（上限 1 KiB/条、100 条） |
| `assert(cond, msg)` / `fail(msg)` | — | 主动失败 → 该步失败语义（`outcome` 按失败点计算） |
| `sleep_ms(ms)` | 受限 | 上限 ≤ `wait_for` 允许值；休眠期间可取消 |
| （不提供）`now/rand/env/fs/net` | — | 编译期即不存在（`no_time` + 不注册）；需要时由调用方经 `param` 注入 |

**JSON ↔ Dynamic 映射**（`rhai::serde`）：
- `null` → `()`；bool → bool；整数 → `INT(i64)`（超出 i64 范围的 `u64` 拒绝，预检报错）；浮点 → `FLOAT(f64)`（NaN/Inf 拒绝）；
- array → `Array`；object → `Map`（键必须为字符串）；
- 回程 `from_dynamic::<serde_json::Value>`；工具结果封套原样入库（`meta` 保留）。
- 规则：**不自动裁剪**（避免静默丢数据）；超限（见 §5）由限额触发终止，而不是截断。

## 5. 能力与限额（Engine 装配默认值）

| 配置 | 默认 | 说明（API 实测） |
|---|---|---|
| `set_max_operations` | `1_000_000` | 0=不限；按"约 1 表达式/操作"计（官方口径 5–10 ops ≈ 1 条简单表达式） |
| `set_max_call_levels` | `32` | 官方默认 64（debug 8）；脚本编排场景 32 足够 |
| `set_max_expr_depths` | 默认（不收窄） | 解析期防深层表达式炸栈；如需再调 |
| `set_max_string_size` | `64 KiB` | 0=不限；注意"拼接可瞬时超限"，取值偏保守 |
| `set_max_array_size` / `set_max_map_size` | `10_000` / `1_000` | 防内存炸弹（按同名族 API 设置，实施时以 docs.rs 1.26 校名） |
| `set_max_variables` | `256` | 0=禁止定义（非不限） |
| `set_max_modules` | `0` | 配合 `no_module` 双保险 |
| `on_progress` | 每 4096 次操作 | 检查 cancel/deadline；返回 `Some(token)` 立即终止 |
| 线程栈 | `8 MiB` | 官方建议独立线程+大栈；由 `std::thread::Builder` 显式设置 |
| wall-clock | `120s` | run 级预算，可经 `params` 覆盖（上限 600s） |
| 脚本大小 | `256 KiB` | 编译前检查（防解析期资源） |
| 输出 | `logs ≤ 100 条 / 1 KiB`，`transcript ≤ 10_000 调用` | 超限终止并如实报告 |

> 内存：Rhai 无硬性内存上限（官方 safety 章节亦如此说明）→ 采"字符串/数组/Map/变量/操作数/墙钟"五重间接约束，并在文档标注残余风险。

## 6. 审计与可追溯（与 D1 对齐）

一次 Rhai run 产生**两类**审计记录（均为既有 `tool_call` 记录，字段按 audit-history v1 增量扩展）：

| 记录 | 说明 |
|---|---|
| `desktop_script_run`（1 条） | `args`: `{engine:"rhai", script_sha256, mode:"generator"|"orchestrator", limits:{...}, caps:[...]}`；`data`: run report |
| 每条 `call()`（N 条） | 常规工具记录 + 新增可选字段：`run_id`、`step_index`、`engine:"rhai"`；`request_id = <run_id>:<step>`（可据此在 `history` 中一键还原整条序列） |

附加产物（run 目录，与 report 同级）：
- `expanded.jsonl`：展开的调用序列（tool/params/结果摘要），用于对账与"转 L1 脚本"；
- `script.rhai`（可选）：脚本副本 + sha256，便于复现。

**终止原因**（`termination`）枚举：`script_end` / `progress_terminated`(cancel) / `deadline` / `limit_exceeded` / `error`。写入 run report 与审计。

## 7. 两种模式与产物治理

| | Generator（默认可用） | Orchestrator（opt-in） |
|---|---|---|
| 能力 | 计算 + `emit_step()` 生成 JSON 步骤 | `call()/read()` 直接驱动 |
| 引擎触碰桌面 | 否 | 是（经白名单） |
| 产物 | **L1 JSON 脚本**（可静态校验、可 diff、可审计、可回放） | `expanded.jsonl` + run report |
| 可回放性 | 天然（JSON） | 由 expanded 转 L1 JSON（best-effort；含 `fragile/volatile` 标记） |
| 开关 | `DESKCTL_SCRIPT_ENGINE=rhai` | 另需 `--allow-orchestrator` / MCP 配置显式开启 |
| 审计 | 1 条 run 记录 + 生成的 JSON 存证 | run 记录 + 每条 call() |

取舍说明：Generator 把"脚本能力"与"执行风险"解耦——**复杂计算用它，副作用仍由 L1 解释器按 JSON 执行**；Orchestrator 只为少数确实需要"边算边做"的场景保留，且必须留痕。

## 8. 错误模型（映射到既有 ErrorCode）

| 来源 | Rhai 表现 | 映射 |
|---|---|---|
| 语法/编译错 | `ParseError`（含行列） | `invalid_params`，`details:{line, col, script_sha256}` |
| 脚本运行时错 | `EvalAltResult::ErrorRuntime` / `ErrorFunctionNotFound` 等 | `internal`（脚本缺陷），`details:{position}` |
| 主动失败 `fail()` / `assert()` | 自定义错误 | 该步失败 → 参与 `outcome` 计算，不改变错误码语义 |
| 取消/超时终止 | `ErrorTerminated(token)` | `cancelled` / `timeout`（按 token 区分），`retryable=false` |
| 限额触发 | `ErrorTooManyOperations` / `ErrorStackOverflow` / `ErrorDataTooLarge`… | `policy_denied`（本地策略），`details:{limit}` |
| 宿主 `call()` 工具失败 | 不抛错，返回 `{ok:false,…}` | 交由脚本处理；未处理则按 `on_error` |

## 9. 并发与公平

- 每个 run：1 专用线程 + 1 Dispatcher 任务；**run 内串行**（与 L1 一致）；
- 跨 run/跨 session：各 `call()` 走既有资源门（FIFO）→ 与其它 agent 公平交错，无全局锁；
- AST 缓存：`LRU(32)`，key=`sha256(script)+engine_feature_fingerprint`，与 `no_time` 等特性绑定，避免误用；
- Engine 实例：每 run 新建（装配廉价），AST 复用（编译贵）。

## 10. 依赖、体积与供应链

- 拉取：`rhai` + `thin-vec` + `smartstring`（本机缓存已含 `smallvec/num-traits/once_cell/ahash/bitflags/hashbrown`）；
- 体积：待测（D3 Spike 产出 release 二进制增量，目标 **≤ +2 MiB**；超限则评估 `no_object/no_float` 等裁剪）；
- 许可：MIT OR Apache-2.0 → 加入 `THIRD_PARTY_NOTICES` 与安装包校验清单；
- 版本：`~1.26` 锁定 minor，升级需跑沙箱与限额回归（Rhai minor 可能含行为变化）。

## 11. 威胁模型（Rhai 特有）

| 威胁 | 缓解 |
|---|---|
| 无限循环/CPU 炸弹 | `max_operations` + `on_progress` 墙钟检查 → 终止 |
| 内存炸弹（字符串/数组/Map/变量） | 四个 `set_max_*` + 操作数上限 + 线程 join 超时 |
| 深递归/深表达式炸栈 | `max_call_levels` + `max_expr_depths` + 8 MiB 专栈 + 解析期深度检查 |
| 文件/网络访问 | `no_module` + 不注册任何 IO 函数 + `set_max_modules(0)` |
| 时间/随机侧信道 | `no_time` + 不注册 `rand`；`ahash/runtime-rng` 仅影响哈希种子（不暴露给脚本） |
| 审计伪造/绕过 | 引擎无审计写权；审计由 Dispatcher 记录真实调用；`call()` 无法伪造 `request_id` |
| 提示注入驱动的脚本 | 脚本必须由调用方显式提供文件/参数（R6 包裹屏幕文本）；脚本 sha256 入库，可疑脚本可拉黑 |
| 供应链 | 版本锁定 + 许可证清单 + 回归沙箱用例（恶意脚本语料） |

## 12. 测试策略

1. **沙箱用例**：`file/import/sleep/now/rand` 不存在；`import` 报错；`no_time` 下无时间类型。
2. **限额用例**：`loop{}` 在 X 秒内被终止且 `termination=progress_terminated/limit_exceeded`；字符串/数组/Map 炸弹命中各自限额；深递归触发 `max_call_levels`。
3. **桥接用例**：host `call()` 结果 JSON 往返无损（含 `meta`）；工具失败以 `ok=false` 返回；`recv_timeout` 与取消生效。
4. **确定性用例**：同一脚本 + **录制好的工具 transcript（stub dispatcher）** → 两次运行 expanded 序列逐字节一致（`no_time` 保证无时间源）。
5. **审计对账**：`history --json` 中 run 记录 + N 条 step 记录；`expanded.jsonl` 与审计逐条一致。
6. **端到端**：SAP ST06 巡检（orchestrator 版）→ 展开序列与 L1 版脚本（E2E-012）步骤对齐；Generator 版产出 JSON 后由 L1 执行，结果一致。
7. **性能**：空转 1e6 ops <1s（对照官方基准 1e6 迭代 0.14s@2.6GHz）；每步桥接开销目标 <3ms。

## 13. 分阶段落地（R0–R3）

| 阶段 | 内容 | 出口标准 |
|---|---|---|
| **R0 Spike**（feature-gated，1–2 天） | 引擎装配 + 限额 + progress 取消 + 一个只读 `read()` | 沙箱/限额用例通过；release 体积增量实测；无 daemon 集成 |
| **R1 Generator** | `emit_step` + 生成 L1 JSON + 校验链路 | AC-RPL-012 前两条；生成物可直接 `script validate/run` |
| **R2 Orchestrator**（opt-in） | `call()` 桥 + transcript + 审计扩展 + 转 L1 | AC-RPL-011/012 全量；E2E-012 对齐 |
| **R3 加固与文档** | 恶意脚本语料回归、体积/性能门禁、llm-skills 配方、known-limitations | 门禁纳入 `scripts/security-check.ps1` 与 perf 基线 |

## 14. 风险与开放问题

1. **体积**：+2 MiB 目标需 R0 实测；若超，评估 `no_object`（放弃 object map，改用数组）或降级为"仅 Generator 模式"。
2. **Rhai minor 行为变化**：锁定 `~1.26` 并把沙箱/限额用例纳入 CI 回归。
3. **`max_operations` 粒度**："外部 Rust 函数调用也计 1 次操作"，因此 host `call()` 的**真实耗时**由 `recv_timeout` 与墙钟独立兜底（不依赖 ops 计数）。
4. **栈限制与脚本复杂度**：8 MiB 专栈 + 32 层调用对编排脚本足够；若出现深递归需求，优先改写成循环。
5. **是否允许 orchestrator 直接 `read()` 只读工具**：ADR-001 §10 建议允许（减少往返）；待 R2 评审确认命名与审计呈现。