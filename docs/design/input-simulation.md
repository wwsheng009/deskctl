# 输入模拟增强（鼠标 / 键盘 / 拟人化）——设计

- 文档版本：0.1
- 日期：2026-10-19
- 状态：已实施（I1，2026-10-19；实现见 `crates/deskctl-core/src/humanize.rs`、`tools/mod.rs`、`platform/windows/input.rs`；真机回归清单见 [../test-plan.md](../test-plan.md) §7.2）
- 关联：[README.md](README.md)、[record-replay.md](record-replay.md)、[audit-history.md](audit-history.md)、[../acceptance-criteria.md](../acceptance-criteria.md)
- 目标：在**不改变工具数量（36）**的前提下，为 `desktop_mouse` / `desktop_mouse_drag` / `desktop_input` 增加更通俗的操作子命令与更拟人的行为；补上"键盘码（VK/扫描码）"与"长按/按住"能力。

## 1. 现状盘点（实测代码，2026-10-19）

| 能力 | 现状 | 缺口 |
|---|---|---|
| 鼠标点击 | `desktop_mouse {action: click\|double_click}`，`button: left/right/middle`，`clicks 1..2`，**遮挡保护**、元素/capture 引用（`resolve_element`） | 无 down/up（按住）；无按下时长；无落点抖动 |
| 鼠标移动 | `{action: move\|hover}` 绝对坐标；支持元素/capture 引用 | 无**相对移动**（dx/dy）；无轨迹/速度控制 |
| 拖拽 | `desktop_mouse_drag {start_*, end_*, duration_ms}` | 轨迹为线性（无曲线/拟人） |
| 滚轮 | `{action: scroll, direction, amount 1..100}` | 无平滑/惯性（可后置） |
| 键盘文本 | `desktop_input {mode: type, text}` → SendInput **KEYEVENTF_UNICODE**（逐字符） + `send: enter/ctrl+enter/tab/none` | 无逐字节奏（拟人打字） |
| 键盘按键 | `{mode: hotkey, keys: ["ctrl","s"]}` → `keybd_event`，按**键名**（`key_to_vk`） | 无**VK 数字码**、无**扫描码**、无 `key_down/key_up`（长按/按住组合）、无重复次数与间隔 |
| 资源/审计 | Input 资源门 =1 串行；审计记录工具与结果；文本不落明文 | 新参数（hold/repeat/humanize/seed）需纳入审计白名单 |

结论：需求关键词"鼠标移动、左右点击、键盘码、更拟人、更通俗"中，**左右键已有**（保留并强化），缺口集中在 **相对移动 / 按住与释放 / 键码三写法 / 长按重复 / 拟人节奏与轨迹**。

## 2. 设计原则

1. **零新增工具**：以 `action`/`mode` 子命令 + 可选参数扩展，避免 MCP `tools/list` 膨胀与契约 churn；既有入参语义与默认值**不变**（`humanize` 默认 `off`）。
2. **确定性优先**：拟人化的随机全部来自可播种 PRNG（自研 PCG32，零新依赖）；`seed` 默认随机生成并**回传/入审计**，同 seed 可逐字节复现（与审计/录放的"可复现"目标一致）。
3. **精确与通俗并存**：键盘支持"名字（`enter`）/ VK 码（`vk:0x41`）/ 扫描码（`scan:0x1E`）"三种写法；鼠标支持绝对/相对/元素引用三种定位。
4. **安全边界不变**：仍为 `BoundedWrite`、经 Input 资源门串行、遮挡保护沿用；长序列必须可取消、受上限约束。
5. **只加不改**：既有字段/默认行为保持（`clicks`、`allow_occluded`、`send` 等），新增字段全部可选。

## 3. API 设计（子命令 + 参数）

### 3.1 `desktop_mouse`（扩展 action）

| action | 新增/现状 | 关键参数 | 说明 |
|---|---|---|---|
| `click`（现状） | 扩展 | `button`、`clicks`、**`hold_ms`**、**`jitter_px`**、**`humanize`**、**`speed`**、**`seed`** | 拟人：先移动（曲线）→ 停顿 → 按下 `hold_ms` → 释放；`jitter_px>0` 时在元素内随机落点（避开边缘 2px） |
| `double_click`（现状） | 扩展 | 同上（间隔受系统双击时间约束） | |
| `move`（现状） | 扩展 | `x/y` 或 `capture_id+element_id`；**`humanize`/`speed`/`seed`** | 拟人轨迹（曲线+缓入缓出） |
| **`move_relative`**（新） | 新增 | `dx`、`dy`、`humanize`、`speed`、`seed` | 物理像素相对移动（多屏虚拟桌面内，可跨屏） |
| **`down` / `up`**（新） | 新增 | `button`、`x/y` 或元素引用 | 按住/释放（拖选、长按菜单、游戏走位）；`down` 后可多次移动再 `up` |
| `hover` / `scroll`（现状） | 不变 | 现有参数 | 后续可加平滑滚动（非本期） |

用法示例（通俗）：

```jsonc
{ "action": "move_relative", "dx": 240, "dy": -80, "humanize": "natural", "seed": 12345 }
{ "action": "down", "button": "right", "capture_id": "cap-…", "element_id": 42 }
{ "action": "up",   "button": "right" }
{ "action": "click", "capture_id": "cap-…", "element_id": 42, "hold_ms": 45, "jitter_px": 3, "humanize": "light" }
```

### 3.2 `desktop_input`（扩展 mode）

| mode | 现状/新增 | 参数 | 说明 |
|---|---|---|---|
| `type`（现状） | 扩展 | `text`、`send`、**`humanize`**、**`seed`**、**`delay_ms`**（范围或固定值） | UNICODE 逐字；拟人节奏见 §4.2 |
| `hotkey`（现状） | 扩展 | `keys`（名字数组）、**`hold_ms`**、**`repeat`**、**`interval_ms`** | 组合键；`hold_ms` 控制按下时长（默认 ~35ms） |
| **`key`**（新） | 新增 | **`key`**：`"enter"` \| `"vk:0x41"` \| `"scan:0x1E"`（可带 `extended:true`）、**`hold_ms`**、**`repeat`**、**`interval_ms`**、`send` | 单键（含 F1–F24、方向键、小键盘、媒体键） |
| **`key_down` / `key_up`**（新） | 新增 | 同上 `key` | 按住/释放（Shift 连点、游戏前进、远程桌面组合键） |

键码解析规则（`key` 字段）：

| 写法 | 解析 | 示例 |
|---|---|---|
| 名字 | `key_to_vk` 既有表（大小写不敏感、别名 `esc`/`escape`） | `"enter"`、`"f5"`、`"pageup"` |
| `vk:<十进制或 0x 十六进制>` | 直接作为 `wVk`（1..254） | `"vk:0x41"`（A）、`"vk:13"`（Enter） |
| `scan:<0x 十六进制>` | `wScan` + `KEYEVENTF_SCANCODE` | `"scan:0x1E"`（A）、扩展键 `"scan:0xE01D"` 或 `{"scan":"0x1D","extended":true}` |

发送通道选择（实现层）：

| 场景 | 通道 | 原因 |
|---|---|---|
| 文本（含 CJK/emoji） | `KEYEVENTF_UNICODE`（现状） | 不依赖键盘布局与 IME 组合 |
| 按键/组合键 | `wVk`（SendInput） | 与 `key_to_vk` 表一致；扩展键置 `KEYEVENTF_EXTENDEDKEY` |
| 游戏/远程桌面/经典应用（只认扫描码） | `wScan` + `KEYEVENTF_SCANCODE` | 部分程序忽略 VK 或需要扫描码区分左右 Shift/小键盘 |

### 3.3 `desktop_mouse_drag`（扩展）

- 新增 `humanize`（默认 `off`）、`speed`、`seed`、`path: "line"|"curve"`（默认 `line` 保持现状）；
- `curve` 时使用与移动相同的贝塞尔轨迹采样，拖拽中途可取消。

## 4. 拟人化算法

### 4.1 鼠标（轨迹与点击）

- **轨迹**：三次贝塞尔（控制点垂直方向偏移 ±15% 距离、带方向的轻微弧度）→ 按 **ease-in-out** 时间曲线采样 → 步长 1–3 px（`speed` 默认 1200 px/s，范围 200–5000）→ 末端 2–4 个点减速停靠；
- **过冲回修（可选）**：30% 概率先到终点外 1–3 px，再 1–2 步回调（更像人；`humanize:"natural"` 时启用）；
- **点击**：`down`→`hold_ms`（拟人默认 25–60ms 采样）→`up`；多击间隔 35–90ms（受 `SPI_GETDOUBLECLICKTIME` 约束）；
- **落点抖动**：`jitter_px`（默认 0）>0 时在目标元素内均匀取点并**内缩 2px**；坐标定位时以点为中心 ±`jitter_px`；
- **实现要点**：轨迹用**多段绝对坐标**（`MOUSEEVENTF_ABSOLUTE|MOUSEEVENTF_VIRTUALDESK` + 0..65535 归一化），避免鼠标加速度影响；不使用 `SPI_SETMOUSE` 修改系统设置；
- **遮挡保护**：仍按最终落点与 `expected_click_target` 校验（jitter 后重新校验）。

### 4.2 键盘（节奏）

- `type` + `humanize`：逐字延迟按范围采样（默认 40–120ms/字；标点后 +30%；单词间 5% 概率 200–400ms 停顿）；可显式 `delay_ms: 60` 或 `delay_ms: [30, 90]`；
- `hotkey`/`key`：`hold_ms` 默认 35ms（拟人 20–60 采样）；`repeat` 间隔默认 120ms；
- **确定性**：PCG32(seed) 驱动所有采样；响应字段回传 `seed`；同一 seed + 同一参数 → 同一延迟序列（单测断言）。

## 5. 限额与取消

| 限制 | 值 | 超限行为 |
|---|---|---|
| `hold_ms` | 5..5000 | `invalid_params`（附合法范围） |
| `repeat` | 1..100 | 同上 |
| `interval_ms` | 10..10000 | 同上 |
| 轨迹点 | ≤ 2000（超长距离自动放宽步长） | 自动降采样（不报错），并回传 `path_points` |
| 单次调用总时长 | ≤ 30s（估算） | `invalid_params` 并建议拆分 |
| 取消 | 每 16 个轨迹点/每个按键间隔检查 `CancelFlag` | `cancelled`（如实报告已发送量 `dispatched`） |

## 6. 与录放 / 审计 / 脚本的整合

- **录制**：`humanize`/`seed`/`hold_ms`/`repeat`/`key` 原样进脚本步骤；坐标类步骤继续标 `fragile`（拟人轨迹不改变"坐标脆弱"结论）；`seed` 记录使回放**可复现**（可用 `--param` 覆盖 seed 以产生不同节奏）。
- **回放**：`script_run` 无需改动（参数透传）；`speed`/`humanize` 属行为参数，preflight 不校验其"语义"。
- **审计**：新增白名单字段 `humanize`/`seed`/`hold_ms`/`repeat`/`interval_ms`/`key_mode`(name|vk|scan)/`path_points`（**不含** `text` 明文，沿用 §5.1 脱敏）。
- **Rhai（L3）**：经 `call()` 自然可用；`seed` 可作为 `param` 注入以实现确定性。

## 7. 兼容性

- 既有调用**行为不变**（`humanize` 默认 `off`：鼠标直线移动到点、点击无抖动、键盘无节奏延迟、`hold_ms` 默认沿用现状值）；
- Schema 只**新增可选字段与枚举值**（`action`/`mode` 枚举扩展属于兼容增量，既有取值语义不变）；契约重导出；
- **工具数量保持 36**；`README`/`llm-skills` 的"工具选择决策树"增补键码/长按用法。

## 8. 验收标准（§15.6 登记）

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-MOUSE-012 | 相对移动与按住/释放 | `move_relative` 以物理像素偏移（多屏虚拟桌面内可跨屏）；`down`→多次 `move`→`up` 可完成一次拖选/长按菜单；`down` 未配对 `up` 时在下一次调用或会话关闭时被清理（无"粘键"） |
| AC-MOUSE-013 | 拟人轨迹确定性 | 同 `seed`+参数两次运行的点列**逐点一致**（单测）；实测轨迹非直线、总时长与 `speed` 偏差 ≤20%；`path_points` 回传与实际发送一致 |
| AC-MOUSE-014 | 点击拟人 | `hold_ms` 生效（down/up 时差可测）；`jitter_px>0` 落点在目标元素内且距边缘 ≥2px；`double_click` 间隔在系统双击时间约束内 |
| AC-INPUT-009 | 键码三写法 | `key` 支持 `"enter"`/`"vk:0x41"`/`"scan:0x1E"`（含 `extended`）；非法写法返回 `invalid_params` 并列出示例；`scan` 模式在记事本与远程桌面/经典应用至少各一例真实生效 |
| AC-INPUT-010 | 长按与重复 | `key_down`→`key_up` 期间键保持按下（目标应用行为可证）；`repeat`+`interval_ms` 按序发送且可取消；`hotkey` 的 `hold_ms` 生效 |
| AC-INPUT-011 | 拟人打字 | `humanize` 下逐字延迟落在配置范围（统计断言）；同 `seed` 复现同一延迟序列；`delay_ms` 固定值模式精确生效 |
| AC-INPUT-012 | 限额与取消 | 超限参数（hold/repeat/interval/总时长）被拒并附范围；执行中 `request/cancel` ≤1s 生效，结果如实报告 `cancelled` 与已发送量 |

## 9. 测试计划

- **单测**（无桌面）：PCG32 确定性、轨迹采样（点列/单调性/边界）、键码解析（三写法+非法）、参数校验与限额、估算时长、jitter 内缩逻辑。
- **真机**：Notepad 相对移动+拖选；右键菜单（down/up）开关；Shift 长按连点；`scan:` 在记事本/远程桌面（或 SAP 会话）验证；多显示器绝对坐标与相对移动跨屏；拟人打字 300 字样本统计延迟分布。
- **录放回归**：录制一段含 humanize/seed 的操作 → 回放两次（同 seed）轨迹一致；`history --run` 可回看参数。
- **性能**：`path_points=2000` 的一次移动 p95 延迟与资源门持有时间记录（输入门=1，长拟人序列会拉长他人等待——写入 known-limitations）。

## 10. 实施分解

| # | 任务 | 文件 | 预估 |
|---|---|---|---|
| 1 | 平台层：SendInput 键盘（VK/扫描码/扩展键/长按）与鼠标（相对/绝对/按钮 down-up/多步轨迹） | `crates/deskctl-core/src/platform/windows/input.rs` | 1d |
| 2 | 拟人模块：PCG32 + 轨迹/节奏采样 + 参数校验 | 新 `crates/deskctl-core/src/humanize.rs` | 0.75d |
| 3 | 工具层：`desktop_mouse` 新 action、`desktop_input` 新 mode、参数校验与取消检查 | `crates/deskctl-core/src/tools/mod.rs` | 1d |
| 4 | Schema/meta/契约 | `deskctl-protocol`、contracts 重导出 | 0.25d |
| 5 | 测试（单测+真机+录放回归）与文档（AC/test-plan/llm-skills/known-limitations） | tests + docs | 1d |

依赖：无（可独立于 D1–D4 实施）；工具数不变（36）。

## 11. 风险与开放问题

1. **游戏反作弊**：注入式（SendInput）在部分游戏被拦截——本设计不承诺绕过，仅提供扫描码通道；文档标注。
2. **输入门串行 vs 拟人时长**：拟人序列会让 Input 门持有更久，可能影响多 Agent 并发预期 → 在 `known-limitations` 写明，并建议长序列用脚本一键执行（可取消）。
3. **相对移动的基准**：以**物理像素**而非 DPI 逻辑像素（与现有坐标体系一致）；跨屏时联合虚拟桌面坐标，不使用 `MOUSEEVENTF_MOVE` 相对事件（受加速度影响），保持可预测。
4. **`down` 未配对 `up` 的兜底**：会话内跟踪"按下的键/按钮"集合，会话关闭或下一次冲突调用时自动释放（防粘键）——是否也在 daemon 关闭时兜底（进程退出由系统清理）待评审。
5. **平滑滚动/惯性**：本期不做，保留 `scroll` 现状。
6. **文本输入法的组合按键**（如中文输入法选字）：仍建议走 `type`（UNICODE）而非 `key`；文档给出选择建议。