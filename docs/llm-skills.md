# Deskctl LLM Skills 定义

- 文档版本：0.1
- 日期：2026-10-18
- 状态：Draft
- 关联文档：[mcp-service.md](mcp-service.md)、[architecture.md](architecture.md)

## 1. 什么是 Skill

在 Deskctl 中，Skill 不是可执行插件，也不是新的工具。

Skill 是给 LLM/Agent 的**使用说明包**，定义：

- 什么场景使用哪些工具
- 多步操作的顺序
- 哪些 token 不能跨会话
- 失败后如何恢复
- 哪些行为禁止
- 如何验证结果
- 示例调用序列

Skill 通过以下方式分发：

- `skills/<id>/SKILL.md`
- `skills/<id>/skill.json`
- 可选映射为 MCP prompts
- 可选作为 Agent 的 system prompt 片段

## 2. 目录结构

```text
skills/
├─ desktop-basic/
│  ├─ skill.json
│  └─ SKILL.md
├─ desktop-window/
│  ├─ skill.json
│  └─ SKILL.md
├─ desktop-text/
│  ├─ skill.json
│  └─ SKILL.md
├─ desktop-semantic/
│  ├─ skill.json
│  └─ SKILL.md
└─ desktop-recovery/
   ├─ skill.json
   └─ SKILL.md
```

## 3. skill.json 规范

```json
{
  "id": "desktop-basic",
  "version": "0.1.0",
  "name": "Desktop Basic",
  "description": "截图、窗口、鼠标和键盘的基础桌面操作。",
  "platforms": ["windows"],
  "profiles": ["minimal", "standard"],
  "tools": [
    "desktop_screenshot",
    "desktop_window_screenshot",
    "desktop_windows_list",
    "desktop_window_activate",
    "desktop_window_info",
    "desktop_perceive",
    "desktop_wait_for",
    "desktop_mouse",
    "desktop_input"
  ],
  "requires": ["session"],
  "risk": "input",
  "entry_prompt": "SKILL.md",
  "examples": [
    "打开记事本并输入一段文字",
    "点击当前窗口中的按钮"
  ]
}
```

字段说明：

| 字段 | 必填 | 说明 |
|---|---|---|
| `id` | 是 | 稳定标识，建议 kebab-case |
| `version` | 是 | Skill 版本，遵循 semver |
| `name` | 是 | 展示名称 |
| `description` | 是 | 一句话说明 |
| `platforms` | 是 | `windows` / `macos` / `linux-x11` |
| `profiles` | 是 | 适用工具 profile |
| `tools` | 是 | 允许使用的工具名 |
| `requires` | 否 | `session`、`ocr`、`vision`、`uia` |
| `risk` | 否 | `readonly` / `input` / `semantic` |
| `entry_prompt` | 是 | 提示词文件 |
| `examples` | 否 | 典型任务 |

## 4. Skill 列表

### 4.1 desktop-basic

用途：

- 截图
- 窗口列表/信息/激活
- 鼠标点击、移动、滚轮、拖拽
- 键盘输入

适用 profile：`minimal`、`standard`、`full`

推荐流程：

```text
1. desktop_windows_list / desktop_window_info
2. desktop_window_activate（如需要）
3. desktop_screenshot
4. desktop_perceive
5. desktop_mouse(capture_id, element_id)
6. desktop_screenshot 或 desktop_wait_for 验证
```

规则：

- 优先使用 `capture_id + element_id`，不要直接猜屏幕坐标。
- 每次动作后重新截图或感知，不要复用旧元素 ID。
- 不把 `hwnd` 写入长期工作流；使用窗口列表重新获取。
- 点击/输入返回 `dispatch_state`，不代表业务成功。

### 4.2 desktop-window

用途：

- 窗口枚举
- 窗口激活
- 窗口移动/缩放
- 窗口截图

推荐流程：

```text
desktop_windows_list -> desktop_window_info -> desktop_window_activate
-> desktop_window_move / resize -> desktop_window_info 验证
```

规则：

- 同名窗口必须显式选择，不可猜测。
- 移动/缩放后必须重新读取窗口信息。
- 窗口截图前确认目标窗口仍存在。
- 不假设 `hwnd` 跨进程/跨会话稳定。

### 4.3 desktop-text

用途：

- 文本输入
- 快捷键
- 长文本剪贴板
- 剪贴板清理

推荐流程：

```text
1. 语义定位输入控件，或截图 + perceive 定位
2. desktop_input(mode=type, text=...)
3. 需要时 desktop_input(mode=hotkey, keys=[...])
4. 长文本先 desktop_clipboard_write，再粘贴
5. 粘贴后 desktop_clipboard_clean
```

规则：

- 输入前必须确保目标窗口在前台。
- 不要把密码、token、密钥写入剪贴板。
- 输入后重新观察结果，而不是只相信 `sent`。
- 中文/emoji 使用 Unicode 输入路径。
- `target_locator` 优先于临时 hwnd。

### 4.4 desktop-semantic

用途：

- Windows UIA 目标发现与绑定
- 语义观察
- 候选动作执行
- 持久化语义动作
- 状态验证

推荐流程：

```text
desktop_targets_list
-> desktop_target_bind
-> desktop_semantic_observe
-> desktop_semantic_candidate（需要详情时）
-> desktop_semantic_execute
-> desktop_verify_state
```

规则：

- 优先语义工具，而不是坐标点击。
- `observation_token` 单次有效，执行后失效。
- 执行前不要切换窗口；前台变化会导致拒绝。
- 候选 ID 只属于当前观察。
- 持久化动作使用 `workflow_step.params.locator`，不要保存临时 token。
- `desktop_agent_step` 只在启用增强判断时使用；未启用时使用普通语义流程。

### 4.5 desktop-recovery

用途：

- 处理 token 过期
- 处理窗口变化
- 处理 `dispatch_unknown`
- 处理资源忙

恢复规则：

| 错误 | 处理 |
|---|---|
| `stale_capture` | 重新截图/感知 |
| `stale_observation` | 重新绑定/观察 |
| `target_unavailable` | 重新列窗口/绑定 |
| `resource_busy` | 退避后重试读取；不要连续点击 |
| `dispatch_unknown` | 不重试动作；重新截图/观察确认结果 |
| `dispatch_state=sent` | 不重试；等待/观察业务结果 |
| `model_unavailable` | 检查 `deskctl doctor` / 模型配置 |

禁止：

- 对 `dispatch_unknown` 自动重放点击或输入。
- 在窗口变化后继续使用旧 candidate_id。
- 为绕过错误而猜测坐标。

## 5. 工具选择决策树

```text
需要看屏幕？
  -> desktop_screenshot
  -> 需要元素定位：desktop_perceive
  -> 需要文字理解：desktop_vision（可选）

需要操作窗口？
  -> desktop_windows_list
  -> desktop_window_info
  -> desktop_window_activate

需要点击/输入？
  -> Windows 语义可用：desktop_targets_list -> bind -> observe -> execute
  -> 语义不可用：screenshot -> perceive -> mouse/input

需要等待？
  -> desktop_wait_for(change/stable/color)
  -> 需要后置条件：desktop_verify_state

需要长文本？
  -> desktop_clipboard_write -> 粘贴 -> desktop_clipboard_clean
```

### 5.1 老式界面巡检配方（SAP / ERP / 终端监控页）

```text
1. desktop_targets_list（或用 desktop_windows_list 取 hwnd）
2. desktop_target_bind {hwnd}                  # 免查 catalog；也可 {app_ref, window_ref}
3. desktop_semantic_observe {goal:"命令字段 okcode text", target_token}
4. desktop_semantic_action {locator:{app_id, role:"text_field", ordinal_hint:0}, action:"focus"}
5. desktop_input {mode:"type", hwnd, text:"/nST06", send:"enter"}   # SAP 只接受真实键入
6. desktop_window_screenshot {hwnd} -> capture_id                   # 后台抓窗，不抢焦点
7. desktop_read_kv {capture_id, region:{...数值区}, value_gap:700}  # 标签→数值结构化
8. desktop_perceive {capture_id}                                    # 需要元素坐标时
9. desktop_docx_save / desktop_xlsx_save / desktop_pptx_save        # 生成报告（可内嵌 capture）
10. 读回校验：docx_read / xlsx_read(range) / pptx_read
```

要点：`capture_id` 读取不受允许图像根限制（无需把截图写进允许目录）；`read_kv` 的
`ambiguous=true` 表示同行有多个候选值，需人工/LLM 复核；`window_activate` 返回
`attempts/method` 便于判断是否触发 Alt 解锁兜底。

PowerShell 传参建议：`@{...} | ConvertTo-Json -Depth 10 -Compress | deskctl call <sess> <tool> --stdin`
（`--stdin` 可避免手工拼接 JSON 时的反斜杠转义错误）。

## 6. 多会话规则

- 一个 MCP 连接对应一个 session。
- 不要在多个 session 之间复用 capture_id、observation_token、target token。
- 不同 Agent 可以并发截图和观察，但输入动作会串行。
- 如果任务是同一窗口的多步操作，保持在同一个 session。
- 如果 session 关闭/daemon 重启，必须重新开始观察。

## 7. 无审批规则

Deskctl 不提供审批。Skill 必须：

- 不要求用户“批准”某个工具调用。
- 不声称动作已被人工确认。
- 在执行破坏性动作前自行判断任务目标是否明确。
- 在目标不明确时停止并询问用户，而不是盲目操作。
- 返回结果中说明 `risk` 和 `dispatch_state`。

## 8. MCP Prompt 映射

| Skill | MCP prompt | 说明 |
|---|---|---|
| desktop-basic | `desktop-basic` | 基础截图/鼠标/键盘 |
| desktop-window | `desktop-window` | 窗口操作 |
| desktop-text | `desktop-text` | 文本/剪贴板 |
| desktop-semantic | `desktop-semantic` | UIA 语义流程 |
| desktop-recovery | `desktop-recovery` | 失败恢复 |

`prompts/get` 返回 `SKILL.md` 的精简版本；完整版本留在 `skills/` 目录。

## 9. Skill 评估

每个 Skill 至少需要：

- 1 个成功场景
- 1 个 token 过期场景
- 1 个窗口变化场景
- 1 个错误参数场景
- 期望工具调用序列
- 禁止出现的调用（例如错误重试、猜坐标）

示例：

```json
{
  "skill": "desktop-semantic",
  "task": "在记事本中输入文本并保存",
  "expected_tools": [
    "desktop_targets_list",
    "desktop_target_bind",
    "desktop_semantic_observe",
    "desktop_semantic_execute",
    "desktop_verify_state"
  ],
  "forbidden_tools": ["desktop_mouse"],
  "forbidden_patterns": [
    "retry after dispatch_unknown",
    "reuse candidate_id"
  ]
}
```

## 10. 版本与兼容

- Skill 版本独立于工具 Schema。
- Skill 引用的工具名必须存在于对应 profile。
- 工具重命名时需要发布新 Skill 版本。
- MCP prompt 内容必须与 `SKILL.md` 同步。
- 弃用 Skill 时保留一个版本的兼容说明。

## 11. 验收

- 每个 Skill 的 `skill.json` 可解析。
- 引用的工具都存在。
- 平台和 profile 匹配。
- `SKILL.md` 包含流程、失败恢复和禁止事项。
- MCP prompts 与 Skill 内容一致。
- 评估用例可重复执行。
