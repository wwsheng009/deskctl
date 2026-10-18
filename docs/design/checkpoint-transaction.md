# 检查点与补偿式事务（Checkpoint & Compensation）——P2

- 文档版本：0.1
- 日期：2026-10-19
- 状态：Draft（待评审）
- 关联：[README.md](README.md)、[data-formats.md](data-formats.md)、[record-replay.md](record-replay.md)

## 1. 定位：为什么不做真事务

桌面与外部系统的副作用**不可回滚**：已发送的键鼠、已提交到 SAP/ERP 的单据、已写入的文件，都无法原子撤销。因此本方案不提供 `begin/commit/rollback`，而是：

1. **检查点（checkpoint）**：关键步骤前保存"现场"（窗口/目标/关键值/capture），事后可**校验差异**。
2. **补偿（compensation）**：失败后给出可执行的**恢复脚本**，默认只做 dry-run 建议，显式确认才执行。
3. **批次语义**由 `desktop_script_run` 覆盖（顺序执行 + 遇错策略 + 逐步报告），不新增 `desktop_batch`，避免概念重复。

这与现有能力衔接：`dispatch_state` 已解决"是否已发出"（防双发），`expectation`/`stable_samples` 解决"是否达到预期"，本层补齐"**事后能否解释差异并恢复现场**"。

## 2. 检查点模型

保存内容（`deskctl-checkpoint/1`，见 [data-formats.md](data-formats.md#3-检查点deskctl-checkpoint1)）：

| 类别 | 内容 | 用途 |
|---|---|---|
| 窗口 | hwnd、标题、矩形、是否前台、是否最小化 | 布局变化检测（矩形差异是最常见的重放失败原因） |
| 目标 | 会话内 target token 对应关系（名称→hwnd/token） | 回放/补偿时重新绑定 |
| 关键值 | `{name, locator, kind}`：`value`（UIA 文本）、`checked`（布尔）、`exists`（元素存在） | 业务级断言（如金额字段、单据编号） |
| 画面 | capture_id 列表（会话内） | 人工复核与报告插图 |
| 外部引用（可选） | 文件路径 + `size/sha256`（如导出文件） | 校验产物是否变化 |

限制：单会话最多 8 个检查点，名字 `[a-z0-9._-]{1,32}`；存会话内存（与 capture 同生命周期），可显式导出文件长期留存。

## 3. 工具面

| 工具 | 类型 | 入参 | 返回 |
|---|---|---|---|
| `desktop_checkpoint_save` | 会话内、读+写（写会话态） | `{name, targets?: [{name, hwnd?, match?}], values?: [{name, locator, kind}], capture_ids?: [..], path?}` | `{checkpoint, saved:true, path?}` |
| `desktop_checkpoint_verify` | 会话内、读 | `{name?, checkpoint?, path?, values_only?: bool}` | `{matched: bool, diffs: [{name, kind, expected, actual}], checked: n}` |

补偿走脚本（不新增工具）：`deskctl-script/1` 增加可选字段 `"purpose": "normal" | "compensation"`（同版本内新增，向后兼容）。
- `desktop_script_run` 增加可选入参：`checkpoint?: string`（运行前保存/运行后校验）、`compensation?: {script?: object, path?: string, apply?: bool}`（失败时给出或执行补偿）。
- 规则：`purpose=compensation` 的脚本默认 `dry_run=true`；只有显式 `apply=true`（CLI `--apply-compensation`）才真正执行；补偿脚本的风险上限固定为 `bounded_write`，且执行中再次失败立即停止（不做二级补偿）。

## 4. 执行流程（脚本 + 检查点 + 补偿）

```text
script run（带 checkpoint/compensation）
  1) preflight（同 record-replay §4.1）
  2) checkpoint_save（若指定）→ 记录现场；审计记录 kind=tool_call(checkpoint_save)
  3) 逐步执行（同 record-replay §4.2）
  4) 全部成功：可选 checkpoint_verify 作为收尾断言
  5) 失败：
     a) on_error=stop → 立即停止；
     b) 若有 compensation：先 dry_run 生成补偿计划（列出将执行的步骤与风险），
        返回给调用方；`apply=true` 时按计划执行并返回第二步报告
  6) 输出 run report（含 checkpoint 差异与补偿结果字段）
```

补偿脚本的编写建议（文档层面）：

- 只做"恢复现场"最小动作：关闭错误弹窗、返回/刷新、把字段改回检查点值、删除半成品文件。
- 不做"再试一次主流程"（重试逻辑由 `retry`/外部编排负责）。
- 补偿也要写 `verify`，确保恢复动作本身生效。

## 5. 与审计/身份的关系

- `checkpoint_save/verify` 与 `script_run` 均为普通工具调用，自动进入审计（含 owner、dispatch_state、耗时）。
- 审计记录中 `args` 对 `checkpoint.values[].value` 按文本脱敏规则处理（默认摘要），避免把业务数据全文写进审计。
- 检查点文件本身是调用方产物，不受审计保留策略约束。

## 6. 验收标准（AC-TXN-001..003）

| 编号 | 通过标准 | 验证方法 |
|---|---|---|
| AC-TXN-001 | `checkpoint_save` 保存窗口/目标/关键值/capture 到会话并可导出为 `deskctl-checkpoint/1`；超过 8 个或非法命名被拒 | 集成 + 单测 |
| AC-TXN-002 | `checkpoint_verify` 返回 `matched/diffs`（值、checked、exists、窗口矩形四类差异至少各覆盖一例）；无差异时 `matched=true` | 集成：真实窗口改动前后各校验一次 |
| AC-TXN-003 | `purpose=compensation` 脚本默认 dry-run（无实际执行，审计可证）；`apply=true` 才执行；执行失败立即停止并如实报告 | 集成 + 安全脚本 |

## 7. 测试计划

- 单测：检查点序列化/差异计算（值/布尔/存在/矩形）。
- 集成：`checkpoint_save → 修改窗口 → checkpoint_verify`（差异命中）；`script run --compensation`（dry-run 与 apply 两态）。
- E2E-013（新，可选）：SAP 场景"改错字段 → 检查点发现差异 → 补偿脚本恢复原值"。

## 8. 实施任务分解（D4）

| # | 任务 | 文件 | 预估 |
|---|---|---|---|
| 1 | 检查点模型 + 差异计算 | `crates/deskctl-core/src/checkpoint.rs`（新增） | 0.75d |
| 2 | 会话态存储（上限 8）+ 两个工具 | `session.rs`、`tools/mod.rs` | 0.75d |
| 3 | 脚本 `purpose` 字段 + `checkpoint/compensation` 入参 | `script.rs`、`tools/mod.rs` | 0.75d |
| 4 | CLI：`--checkpoint`、`--apply-compensation` | `main.rs` | 0.25d |
| 5 | 测试与文档 | tests + acceptance/status | 0.75d |

依赖：D3（脚本执行器）完成后开始；工具数 39 → 41。

## 9. 开放问题

1. 检查点是否支持"外部文件哈希"作为一等字段（当前为可选）——涉及大文件读取性能。
2. 补偿是否允许"半自动"（dry-run 结果由 LLM 复核后再 `apply=true`）——建议保持由调用方编排，工具不做决策。
3. 检查点跨会话复用（导出/导入）是否需要——本期只支持同会话，导出文件用于留档。