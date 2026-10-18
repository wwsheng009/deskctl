# ADR-001：脚本执行层选型（自研声明式 vs 嵌入脚本引擎）

- 文档版本：0.1
- 日期：2026-10-19
- 状态：Proposed（待评审）
- 关联：[record-replay.md](record-replay.md)、[data-formats.md](data-formats.md)、[prior-art.md](prior-art.md)、[README.md](README.md)

## 1. 问题

回放脚本如何执行：

1. **自研**：把 `deskctl-script/1`（声明式 JSON）解释成工具调用序列；
2. **嵌入成熟脚本引擎**（Lua/Rhai/JS/WASM…）：用通用语言写"脚本"，由引擎执行；
3. **混合**：JSON 为交换格式，引擎只做计算/生成/包装。

## 2. 需求盘点（现在 vs 未来）

| 能力 | D3 现在需要 | 引擎才能实现？ |
|---|---|---|
| 线性步骤 + 变量（`$target/$param/$var/$steps`） | ✅ 必须 | 不需要（自研即可） |
| 结果断言（`expect`）与核验（screen/readback） | ✅ 必须 | 不需要 |
| 失败策略（`on_error`）、安全重试（`not_sent`）、每步审计 | ✅ 必须 | 不需要 |
| 预检（preflight）与静态校验、dry-run、可 diff | ✅ 必须 | **自研 JSON 更优**（引擎脚本难静态校验） |
| 条件（`when`）、有限循环（`repeat n` / `foreach`） | ✕ 暂不需要 | 不需要：受限表达式即可 |
| 数据变换（解析/拼接/计算，驱动数据驱动回放） | ✕ 暂不需要 | 受限表达式/宿主函数即可 |
| 任意控制流、函数、递归、复杂状态机 | ✕ 明确不需要（会破坏可审计性与 LLM 可生成性） | 需要引擎（opt-in 场景） |

## 3. 决策（Decision）

**分三层，默认不嵌入引擎；JSON 始终是唯一交换格式。**

| 层 | 形态 | 何时用 | 实现 |
|---|---|---|---|
| **L1（默认，D3）** | `deskctl-script/1` 声明式 JSON：线性步骤 + 变量 + 断言 + 重试 + `on_error` | 所有生产路径（巡检、回归、SOP） | 自研解释器（`crates/deskctl-core/src/script.rs`） |
| **L2（可选，后续）** | JSON 增补受限表达式：`when`（布尔）、`repeat`（次数/集合）、值计算 | 需要轻量条件/循环时 | **`evalexpr`**（表达式求值，无语句/循环）或自研 mini-expr（同 v 内新增可选字段，格式不变） |
| **L3（可选，opt-in）** | 嵌入 **Rhai**：用 Rhai 写"生成器/编排"脚本，通过 host 函数调用工具 | 需要图灵完备的复杂编排（少数用户） | `rhai` crate（feature gate `script-engine`，默认关闭） |

三条不变量（L1–L3 均适用）：

1. **JSON 是唯一交换/录制/审计格式**：recorder 产出 JSON；LLM 生成 JSON；审计按步骤关联。Rhai 脚本只能是"生成器/包装器"（生成或展开 JSON 步骤），**引擎脚本本身不作为回放产物**。
2. **副作用只经工具白名单**：引擎（L3）只暴露一个 host 函数 `call(tool, params) -> result`，路由到既有 `Engine::execute_with_session`——会话隔离、资源门、取消、审计、`dispatch_state` 语义全部继承，不因引入引擎而旁路。
3. **引擎被剥夺系统能力**：无文件/网络/时间/随机（或仅注入受控能力）；`max_operations`/调用深度/字符串上限 + 进度回调（超限→ `cancelled`）；`--allow-script` 显式开启前 MCP 不暴露。

## 4. 备选评估（2026-10-19 实测数据）

| 方案 | 语言/实现 | 依赖与构建 | 沙箱/限额 | 异步 | 成熟度（stars / crates 下载 / 最近活跃） | 体积 | 适配度 |
|---|---|---|---|---|---|---|---|
| **自研 JSON 解释器** | Rust | 零新增 | 完全可控（仅白名单语义） | 原生 tokio | 本项目 | ~0 | ★★★★★（默认） |
| **evalexpr** | Rust（表达式） | 纯 Rust，极轻 | 无 IO；无控制流 | 同步（够用） | 416★ / 11.0M / 2026-10 | 极小 | ★★★★（L2 首选） |
| **Rhai** | Rust（脚本） | 纯 Rust，7 个轻量依赖 | **强**：`max_operations`、`max_call_stack`、`max_string_size`、进度钩子可中止、可禁用循环/关键字、默认无 IO | 有 `async` feature（tokio 兼容） | **5,739★ / 13.1M / 2026-10-17**；MIT OR Apache-2.0；MSRV 1.66 | 小（单二进制内） | ★★★★★（L3 首选） |
| **mlua（Lua）** | C（vendored Lua/LuaJIT） | 需 C 工具链（lua-src）；或系统 Lua | 可移除 stdlib/IO；限额需自行加指令钩子 | 原生 async/await | 2,908★ / 7.9M / 2026-10-03 | 小 | ★★★★（生态最好，构建复杂化） |
| **rquickjs（QuickJS）** | C（QuickJS） | 需 C 工具链 | 支持中断处理器与内存上限 | 一般（非 async-first） | 1,040★ / 5.5M / 2026-10-17 | 小 | ★★★（要 JS 兼容时） |
| **Rune** | Rust | 纯 Rust | VM 限额有，文档/生态较小 | 内建 async | 2,339★ / **0.18M** / 2026-10-11 | 小 | ★★（观望） |
| **Boa（JS）** | Rust | 纯 Rust | 有限额 | 一般 | 7,612★ / — / 2026-10-17 | 较大、性能偏低 | ★★（仅 JS 兼容刚需） |
| **wasmtime / deno_core** | WASM/V8 | 重依赖（V8/cranelift） | WASI 能力模型优秀 | 一般 | 超大项目 | **+10–40MB 级** | ✕（与"单二进制/模型随包"体积目标冲突） |
| fasteval | Rust | 纯 Rust | 表达式 | — | 301★ / 2023 后停更 | 极小 | ✕（停维护，选 evalexpr） |

> 说明：本地 cargo 缓存检查——Rhai 的传递依赖 `smallvec/num-traits/once_cell/ahash/bitflags/hashbrown` **已命中**，
> 仅需拉取 `rhai`、`thin-vec`、`smartstring`（+ 可能的 `bitflags 2.x`）；crates.io 可达，拉取成本可控。
> 若必须完全离线构建，可 `cargo fetch` 预热或 `cargo vendor`。

## 5. 为什么默认不嵌入引擎

1. **可审计性**：JSON 逐步可静态校验、可 diff、可与审计逐条对应；引擎脚本的内部状态无法等价审计（与 D1 目标冲突）。
2. **安全**：引擎=任意代码执行面；即便沙箱化，也多一层需要证明的边界，而本项目的风险控制建立在"工具白名单 + 默认拒绝高风险"上。
3. **LLM 生成可靠性**：让模型产出 JSON 比产出正确程序显著更可控（schema 校验、修复循环短）。
4. **体积与依赖**：项目要求单二进制、模型随包；引入引擎（尤其 C/WASM/V8）增加构建与发布复杂度。
5. **需求未出现**：当前全部用例（巡检、回归、SOP 回放）不需要图灵完备。

## 6. 若引入引擎（L3）的安全设计要点

- **同一入口**：`call()` → `Engine::execute_with_session`（request_id 由 run 生成；审计记 `engine:"rhai"` 与步骤 id）。
- **能力关闭**：不注册任何 IO/时间/随机函数；需要"现在时间/环境变量"时经受控 host 函数显式白名单。
- **限额与取消**：`max_operations`（默认 1e6）、`max_call_stack`（64）、`max_string_size`（64KiB）、进度钩子每 N 次检查 `CancelFlag`。
- **fail-closed**：引擎错误 → 该步失败；不自动重试（除非 `dispatch_state=not_sent` 语义成立——引擎步骤统一按 `not_sent` 处理，因为它尚未发出桌面动作）。
- **开关**：`DESKCTL_SCRIPT_ENGINE=off|rhai`（默认 off）；CLI `--allow-script`；不在 MCP 暴露引擎运行入口，除非显式配置。
- **审计**：记录引擎脚本的 sha256 + 展开后的工具调用序列（可复现、可对账）。

## 7. 与既有设计的关系

- [record-replay.md](record-replay.md) §2/§4 的 JSON 步骤模型**不变**；本 ADR 只补充"执行层可选引擎"与 L2 表达式扩展位。
- [data-formats.md](data-formats.md) 无破坏性变更；L2 的 `when`/`repeat` 为同 `v` 内的**新增可选步骤字段**。
- 新增验收（可选，登记进 [../acceptance-criteria.md](../acceptance-criteria.md) §15）：
  - **AC-RPL-010**：L2 受限表达式——`when`/`repeat` 生效，表达式无副作用（不可调用工具），越界/类型错误预检拒绝。
  - **AC-RPL-011**（仅当 L3 启用时）：引擎沙箱——`max_operations` 超限被中止且不计为成功；未注册 IO 函数；`call()` 必须出现在审计中且与展开序列一致。

## 8. 决策记录

| 选项 | 结论 | 理由 |
|---|---|---|
| 自研声明式 JSON（L1） | **采纳（默认）** | 可审计、可静态校验、LLM 友好、零新增依赖 |
| 受限表达式（L2，evalexpr） | 备选，按需 | 成本极小；`when/repeat` 覆盖大多数"想加点逻辑"的需求 |
| 嵌入 Rhai（L3） | 备选，opt-in | 纯 Rust/沙箱强/限额与进度钩子齐全，是"真需要脚本"时的最优选 |
| mlua / rquickjs | 备选（有 C 工具链时可接受） | 生态与 JS 兼容更强，但构建复杂化 |
| Rune / Boa | 不推荐（观望） | 生态/性能/体积不占优 |
| wasmtime / deno_core | 拒绝 | 体积与构建复杂度违背单二进制目标 |
| fasteval | 拒绝 | 停维护 |

## 9. 验证与验收（D3 内）

1. 单测：L1 解释器（已有 AC-RPL-001..009 覆盖）；L2 表达式（若实现）：`when` 真/假、`repeat 0/1/N`、类型错误。
2. 基准：`script run` 每步开销 <2ms（L1）；若 L3：Rhai 空转 1e6 次操作 <1s 且超限可中止。
3. 安全用例（若 L3）：未注册 `file`/`net`/`time`；`max_operations` 触发中止；`call()` 审计对账。

## 10. 开放问题

1. L2 用 `evalexpr` 还是自研 mini-expr（约 300 行，零依赖、无浮点细节差异）——建议先自研、复杂化再换。
2. 是否允许 L3 脚本直接调用**只读**工具（如 perceive）以减少宿主往返——建议允许，但写入类必须经 `call()`。
3. L3 是否需要"脚本库/导入"（跨脚本复用）——默认禁止，保持单文件可审计。