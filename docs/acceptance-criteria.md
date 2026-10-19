# Deskctl 验收标准

- 文档版本：0.2
- 日期：2026-10-18
- 状态：Draft
- 修订：0.2 将 AC-CAP-004/005、AC-SEC-005 的扩展名规则对齐 [capture-compression.md](capture-compression.md)（显式 `format` > 可识别扩展名 > 服务端默认）
- 适用版本：Windows 首个可用版本
- 关联文档：[architecture.md](architecture.md)、[test-plan.md](test-plan.md)、[mcp-service.md](mcp-service.md)

## 1. 验收原则

1. **可重复**：每条验收项必须能在固定环境、固定测试数据下重复执行。
2. **可观测**：结果必须能从 JSON 输出、退出码、日志或截图/文件产物中判断。
3. **安全失败**：不支持、参数错误、平台限制必须返回结构化错误，不能 panic 或假成功。
4. **状态明确**：所有动作必须区分 `not_sent`、`sent`、`partial`、`unknown`。
5. **无审批**：不验收 `approve/reject`；风险只作为 `meta.risk` 返回。
6. **Windows 优先**：首期只对 Windows 10/11 做功能验收；macOS/Linux 只检查可编译占位。

## 2. 范围验收

### AC-SCOPE-001 独立项目

- `deskctl` 不依赖 Tauri、前端资源、LLM SDK、工作流引擎。
- `cargo tree` 中不存在 `tauri`、`nuphus` 作为运行时依赖。
- 代码中不包含 `src-tauri` GUI 生命周期。

### AC-SCOPE-002 工具范围

- 默认 `standard` profile 暴露 38 个核心工具。
- `full` profile 暴露 40 个工具（D3 新增 `desktop_recorder_start/stop`、
  `desktop_script_run` 与只读 `desktop_file_hash`）。
- 只有 `desktop_*`，不出现 `browser_*`、`workflow_*`、`agent_*` 工具。
- `desktop_vision` 和 `desktop_agent_step` 仅在对应可选功能启用时出现。

### AC-SCOPE-003 平台范围

- Windows 10/11 x64 可构建、可运行、可通过全部 Windows 验收项。
- macOS/Linux 首期只要求代码路径存在、feature 可配置；不要求运行验收。
- 不支持平台返回 `platform_not_supported`。

## 3. 功能验收

### 3.1 截图

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-CAP-001 | 全屏截图 | 返回 `capture_id`、路径、宽高；PNG 文件可打开 |
| AC-CAP-002 | 区域截图 | 区域宽高与输出图片一致；坐标从屏幕坐标正确裁剪 |
| AC-CAP-003 | 窗口截图 | 指定 `hwnd` 可生成窗口 PNG；窗口不存在返回结构化错误 |
| AC-CAP-004 | 自定义路径 | 接受绝对/相对路径；父目录自动创建；输出扩展名与所选格式一致（`.png`/`.jpg`/`.webp`） |
| AC-CAP-005 | 格式选择 | 优先级：显式 `format` > 可识别扩展名（`.png`/`.jpg`/`.jpeg`/`.webp`）> 服务端默认；`foo.jpg` 输出 JPEG `foo.jpg`，无法识别的 `foo.bmp` 回退默认 PNG `foo.png` |
| AC-CAP-006 | 屏幕尺寸 | `desktop_screen_size` 返回主屏宽高，数值为正 |
| AC-CAP-007 | 截图失效 | 截图后窗口变化时，窗口截图返回明确错误，不返回旧图 |
| AC-CAP-008 | 压缩参数 | `quality`/`max_width`/`max_height`/`max_bytes` 生效；返回 `format`/`bytes`/`resized` 与实际文件一致 |

> 截图格式、质量、缩放与字节上限的完整策略见 [capture-compression.md](capture-compression.md)。

### 3.2 窗口

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-WIN-001 | 窗口列表 | 返回至少一个可见窗口，含窗口标识、标题、位置、进程信息 |
| AC-WIN-002 | 窗口激活 | 目标窗口成为前台，返回 `foreground=true` |
| AC-WIN-003 | 窗口移动 | 移动后 `window_info` 坐标发生变化 |
| AC-WIN-004 | 窗口缩放 | 缩放后 `window_info` 尺寸发生变化 |
| AC-WIN-005 | 无效句柄 | 返回 `target_unavailable` 或等价结构化错误 |
| AC-WIN-006 | 多窗口选择 | 同名窗口不被错误猜测，必须显式选择 |
| AC-WIN-007 | 对话框跟随绑定 | `target_bind follow_dialog=true` 时，若目标窗口被模态弹窗禁用则绑定该弹窗，响应 `followed_dialog=true` 与 `parent_hwnd`；多窗口场景优先选择活动弹窗 |
| AC-WIN-008 | 激活自愈 | `desktop_window_activate` 内部最多 3 轮置前（第 2 轮起先发送 Alt 键解锁前台锁），隐藏窗口先恢复显示；响应返回 `attempts` 与 `method`，失败错误同样附带该诊断 |
| AC-WIN-009 | 截图枚举重试 | `desktop_window_screenshot` 对窗口枚举滞后自动重试 3 次；仍不可枚举时提示 "call desktop_window_activate first" |

### 3.3 鼠标

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-MOUSE-001 | 鼠标位置 | `position` 返回当前屏幕坐标 |
| AC-MOUSE-002 | 单击 | 在测试按钮上单击触发一次 UI 事件 |
| AC-MOUSE-003 | 双击 | 触发双击事件，不误判为两次单击 |
| AC-MOUSE-004 | 悬停/移动 | 鼠标移动到目标坐标，位置可读回 |
| AC-MOUSE-005 | 滚轮 | 方向与数量正确，负数/非法方向被拒绝 |
| AC-MOUSE-006 | 拖拽 | 从起点拖到终点，UI 显示拖拽结果 |
| AC-MOUSE-007 | capture 引用 | `capture_id + element_id` 可解析为屏幕坐标 |
| AC-MOUSE-008 | 参数互斥 | `capture_id` 与裸坐标混用被拒绝 |
| AC-MOUSE-009 | 目标窗口 | 指定 `hwnd` 时窗口未前台则激活或失败，不误点其他窗口 |
| AC-MOUSE-010 | 派发状态 | 点击返回 `dispatch_state`，不把“已发送”伪装为“业务成功” |

### 3.4 键盘与输入

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-INPUT-001 | 文本输入 | 英文、中文、符号可输入；不丢字符 |
| AC-INPUT-002 | 快捷键 | `ctrl+c`、`alt+tab` 等组合键生效 |
| AC-INPUT-003 | 回车策略 | `send=enter` 追加回车，`send=none` 不追加 |
| AC-INPUT-004 | 组合键校验 | 未知键名、空数组被拒绝，不能发送部分按键 |
| AC-INPUT-005 | target_locator | 可使用稳定 locator 定位目标输入控件 |
| AC-INPUT-006 | 前台失败 | 目标无法置前时中止，不把文本输入其他窗口 |
| AC-INPUT-007 | 部分发送 | 文本已输入但后续按键失败时返回 `partial`，不冒充成功 |
| AC-INPUT-008 | 模态弹窗重定向 | 目标窗口被模态对话框禁用时，输入自动重定向到其当前活动弹窗（如原生 Save As），响应返回实际 `target_hwnd`；窗口正常时行为不变 |

### 3.5 剪贴板

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-CLIP-001 | 写入 | 写入文本后可在其他应用粘贴 |
| AC-CLIP-002 | 清理 | `clean` 后剪贴板内容为空 |
| AC-CLIP-003 | 不读取 | 默认工具集中没有读取剪贴板的工具 |
| AC-CLIP-004 | 日志脱敏 | 写入内容不出现在日志中 |

### 3.6 找图、找色、找字、等待

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-FIND-001 | 找图命中 | 已知模板在测试画面中返回正确位置和置信度 |
| AC-FIND-002 | 找图未命中 | 返回 `found=false` 和诊断信息，不返回错误 |
| AC-FIND-003 | 多模板 | `|` 分隔模板可逐个尝试 |
| AC-FIND-004 | 找色 | 已知颜色在区域内返回正确坐标 |
| AC-FIND-005 | 多颜色 | 锚点 + 偏移点匹配正确 |
| AC-FIND-006 | 找字 | 已知字库和画面可返回匹配文字与坐标 |
| AC-FIND-007 | 字库路径安全 | `dict_name` 含 `..`、路径分隔符或绝对路径时被拒绝 |
| AC-FIND-008 | 字库缺失 | 返回可操作的 `model_unavailable` 或字库缺失错误 |
| AC-WAIT-001 | change | 区域变化后返回 `met=true` |
| AC-WAIT-002 | stable | 画面静止达到 `stable_ms` 后返回 `met=true` |
| AC-WAIT-003 | color | 指定颜色出现后返回 `met=true` |
| AC-WAIT-004 | 超时 | 超时返回 `met=false`，不是工具错误 |
| AC-WAIT-005 | 取消 | 取消后停止等待，不返回假阳性 |

### 3.7 感知与 OCR

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-PER-001 | OCR | 已知截图返回文字和坐标块 |
| AC-PER-002 | YOLO | 已知图标截图返回元素框；模型缺失时按配置降级或报错 |
| AC-PER-003 | 合并结果 | OCR/YOLO 结果去重、合并，元素 ID 稳定 |
| AC-PER-007 | capture_id 输入 | `desktop_perceive` 接受 `capture_id`（与 `image_path` 互斥）；会话内截图/裁剪可直接识别，不受允许图像根限制 |
| AC-PER-008 | 键值读取 | `desktop_read_kv` 按行分组并把标签与右侧（或正下方）取值配对，返回 `pairs`（含 `label/value/rect/row/ambiguous`）与 `unpaired`；支持 `region`/`value_gap`/`row_tolerance`/`min_confidence`/`max_pairs`；`capture_id` 与 `image_path` 二选一 |
| AC-PER-009 | 越界提示 | 读取图像越界时 `policy_denied` 消息列出允许根目录、`DESKCTL_ALLOWED_INPUT_ROOT` 与 `capture_id` 三条出路；`doctor` 输出 `allowed_input_roots` |
| AC-PER-004 | capture 映射 | 已登记截图返回 `screen_center` 和 `capture_id` |
| AC-PER-005 | 未登记截图 | 未登记图片返回图像坐标，`screen_center=null` |
| AC-PER-006 | 模型校验 | 模型缺失/损坏时 `doctor` 和工具错误信息一致 |

### 3.8 语义桌面（Windows UIA）

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-SEM-001 | 目标列表 | 返回 app/window 引用，不暴露任意启动路径 |
| AC-SEM-002 | 目标绑定 | 返回 target token；重复窗口要求显式选择 |
| AC-SEM-003 | 语义观察 | 返回观察 token、候选动作和可读元素 |
| AC-SEM-004 | 候选查询 | candidate_id 只能属于当前观察 |
| AC-SEM-005 | 候选执行 | 执行成功返回 receipt、验证结果和 workflow step |
| AC-SEM-006 | 持久化动作 | locator 可跨进程重放，不依赖临时 candidate_id |
| AC-SEM-007 | 状态验证 | `desktop_verify_state` 返回 satisfied/unsatisfied/unknown |
| AC-SEM-008 | 过期观察 | 120 秒后或 session 关闭后 token 失效 |
| AC-SEM-009 | 前台变化 | 执行前窗口变化时拒绝动作，不误操作 |
| AC-SEM-010 | 跨 session | 一个 session 的 token 不能用于另一个 session |
| AC-SEM-011 | 无审批 | 关键动作不弹出批准请求；结果带风险元数据 |
| AC-SEM-012 | 可选增强 | `desktop_agent_step` 未启用时不出现在工具列表 |
| AC-SEM-013 | 扩展动作 | `set_checked` 幂等（目标状态已满足时不点击）；`scroll`（`direction` + `amount=small/page`）、`scroll_into_view`、`set_range_value` 在支持对应 UIA 模式的控件上生效 |
| AC-SEM-014 | 提交策略与内联验证 | `completion_policy=auto/verified/dispatched` 生效；携带 `expectation` 时动作后做有界验证并返回 `verification/satisfied`；`expected=false` 反向语义生效；`verified` 缺少 `expectation` 被拒绝；`unsatisfied` 不冒充成功 |
| AC-SEM-015 | 稳定采样 | `expectation.stable_samples`（1..5，默认 2）生效：连续采样一致才返回 `satisfied`；超时未达到稳定次数返回 `unknown` |
| AC-SEM-016 | 树分页 | observe 返回 `next_tree_cursor`（格式 `t:<n>`）；携带 `tree_cursor` 与同一 `observation_token` 可续读原生树，不重新采集 |
| AC-SEM-017 | 受限启动 | `app_ref` 只能来自本地快捷方式目录（Start Menu/桌面）生成的 catalog 引用；`target_bind` 对未运行的应用可启动并等待窗口（≤10s）后绑定；`desktop_input`/`desktop_semantic_action` 支持 `launch_ref` 重启缺失窗口；拒绝任意可执行路径/脚本 |
| AC-SEM-018 | 投递模式 | 语义动作 `delivery_mode` 默认 `auto`：优先后台执行、必要时回退前台且不重复派发；`background` 不抢前台，失败即返回结构化错误；`foreground` 仍可显式要求 |
| AC-SEM-019 | 前台还原 | `restore_foreground=true` 时语义动作结束后尽力还原动作前的前台窗口，结果字段如实返回（受前台锁影响允许失败） |
| AC-SEM-020 | 对话框自动跟随 | `semantic_observe`/`semantic_action` 在目标窗口被模态弹窗禁用时自动跟随其活动弹窗（响应报告 `redirected_from`），弹窗关闭后自动恢复原目标 |
| AC-SEM-021 | 按 hwnd 绑定 | `target_bind` 支持 `hwnd`（与 `app_ref`/`window_ref` 互斥），无需先查 `targets_list`；响应与绑定记录自动合成 `app:<pid>:<process>` |
| AC-SEM-022 | 绑定失败提示 | `app_ref` 无匹配时报错含正确格式示例与 `targets_list`/`hwnd` 两条出路；`app_ref` 命中多窗口时报错内联列出候选 `window_ref` |
| AC-SEM-023 | UIA 名称修复 | UIA 名称/描述的双层编码乱码（UTF-8↔GBK 两个方向）自动修复；正常中文/英文/日文恒等；末端残字节允许丢失（结果保留可读前缀） |
| AC-MOUSE-011 | 遮挡保护 | 携带 `hwnd`/capture 引用的点击在目标点被其它进程窗口遮挡时返回 `target_unavailable` + `occluded_by`，不误点；`allow_occluded=true` 可强制 |

### 3.9 图像处理与文档

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-IMG-001 | 图像裁剪 | `desktop_image_crop` 以 `capture_id` 或允许目录内的 `image_path` 为源，按区域裁剪；越界返回 `invalid_params` 并附源图尺寸；输出遵循截图编码策略（`format`/`quality`/`max_*`） |
| AC-IMG-002 | 裁剪链 | `capture_id` 来源的裁剪结果登记为新 capture（`capture_id` 非空），元素坐标映射按区域平移保持，可继续用于点击/OCR/二次裁剪 |
| AC-DOC-001 | Word 文档 | `desktop_docx_save` 生成可被 Word/WPS/LibreOffice 打开的 `.docx`（标题、小节、正文、表格、内嵌图片与图注）；缺省输出到 data 目录 |
| AC-DOC-002 | 后台执行 | 截图 → 裁剪 → 文档（docx/xlsx/pptx）汇总全流程不要求前台、不抢焦点（执行前后前台窗口保持不变） |
| AC-DOC-003 | docx 读取 | `desktop_docx_read` 返回段落、表格、内嵌图片数量与页眉/页脚文本（页码字段回读为 `{PAGE}` 占位）；非 docx 包返回结构化错误 |
| AC-DOC-004 | xlsx 读写 | `desktop_xlsx_save` 写出可被 Excel/WPS 打开的工作簿（可选表头加粗、列宽、数字格式、冻结首行、多工作表）；`desktop_xlsx_read` 读回工作表与类型化单元格值，可用 `range`（如 `B2:D10`）限定区域（行/列有上限并返回 `truncated`） |
| AC-DOC-005 | pptx 读写 | `desktop_pptx_save` 生成可打开的标题/要点/图片幻灯片，`layout` 可选 auto/title_bullets/title_image/text_image/title_only（版式与内容冲突返回 `invalid_params`）；`desktop_pptx_read` 返回每页文本与图片数量 |
| AC-DOC-006 | docx 排版 | `desktop_docx_save` 的 `header`/`footer`/`page_numbers` 生成 `word/header1.xml`/`word/footer1.xml` 与 sectPr 引用；`desktop_docx_read` 可读回页眉/页脚文本 |
| AC-DOC-007 | xlsx 样式 | `header_row`（首行加粗字型）、`freeze_header`（冻结首行窗格）、`columns[].width`（`<col customWidth>`）与 `columns[].number_format`（自定义 numFmt + `applyNumberFormat`）写入 styles/worksheet XML |
| AC-DOC-008 | pptx 图片 | `desktop_pptx_save` 每页可内嵌来自 `capture_id` 或允许根内 `image_path` 的 PNG（等比居中）；`desktop_pptx_read` 读回该页 `images` 计数 |

## 4. 多会话与状态验收

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-SESS-001 | 多会话并发 | 同一 daemon 同时存在 ≥2 个 session，均可截图/观察 |
| AC-SESS-002 | 状态隔离 | A 的 capture/target/observation token 在 B 中无效 |
| AC-SESS-003 | 输入串行 | 两个 session 同时请求点击时，动作不交叉；按资源门顺序执行 |
| AC-SESS-004 | 读并行 | 两个 session 可同时截图/窗口枚举，不互相阻塞 |
| AC-SESS-005 | 会话关闭 | 关闭 session 后其 token 全部失效 |
| AC-SESS-006 | daemon 重启 | 重启后旧 token 全部失效，不恢复已发送动作 |
| AC-SESS-007 | 会话上限 | 超过配置上限时返回 `resource_busy`，不崩溃 |
| AC-SESS-008 | 资源泄漏 | 关闭会话后 capture cache、锁和临时文件被清理 |
| AC-SESS-009 | 公平性 | 长时间 session 不永久饿死新 session |

## 5. 协议与 MCP 验收

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-MCP-001 | initialize | 返回协议版本、server 版本、平台、capabilities |
| AC-MCP-002 | tools/list | 返回当前 profile 的 26/28 工具，名称和 Schema 合法 |
| AC-MCP-003 | tools/call | 同步调用返回 JSON 内容；错误映射为 `isError` |
| AC-MCP-004 | stdout 纯净 | MCP 模式下 stdout 只有协议 JSON，日志全部走 stderr |
| AC-MCP-005 | 隐式 session | MCP 进程退出后 session 关闭，token 失效 |
| AC-MCP-006 | 取消 | MCP cancellation 能取消等待中的请求 |
| AC-MCP-007 | 进度 | 长任务支持 progress token 事件 |
| AC-MCP-008 | 工具列表变化 | profile/feature 变化时发送 list_changed（如支持） |
| AC-MCP-009 | 版本不匹配 | daemon/client 主版本不匹配时拒绝连接 |
| AC-MCP-010 | 无审批方法 | `tools/list` 和 `prompts/list` 中不存在 approve/reject |
| AC-MCP-011 | 大结果 | 语义候选分页，单次响应不超过配置上限 |
| AC-MCP-012 | 技能 prompts | 可选暴露 `desktop-basic` 等 skills，内容与 docs 一致 |

## 6. CLI 验收

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-CLI-001 | help | `deskctl --help` 列出 `mcp/serve/call/tool/session/doctor` |
| AC-CLI-002 | doctor | 输出平台、权限、模型、ONNX Runtime、socket 状态 |
| AC-CLI-003 | tool | `--json`、`--json @file`、`--stdin` 均可工作 |
| AC-CLI-004 | call | 可指定 session 调用有状态工具 |
| AC-CLI-005 | session | create/list/inspect/close 可用 |
| AC-CLI-006 | cancel | 指定 request_id 可取消请求 |
| AC-CLI-007 | 退出码 | 成功/参数错误/状态过期/资源忙/平台不支持/策略拒绝区分 |
| AC-CLI-008 | stdout/stderr | 机器 JSON 与日志分离 |
| AC-CLI-009 | 单实例 | 重复启动 daemon 不产生第二个实例 |
| AC-CLI-010 | token | 无 token 或错误 token 被拒绝 |

## 7. 错误、取消与重试验收

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-ERR-001 | 参数错误 | 返回 `invalid_params`，不执行动作 |
| AC-ERR-002 | 未知工具 | 返回 `unknown_tool` |
| AC-ERR-003 | 平台不支持 | 返回 `platform_not_supported` |
| AC-ERR-004 | 状态过期 | 返回 `stale_capture` / `stale_observation` |
| AC-ERR-005 | 已发送未知 | 返回 `dispatch_unknown`，`retryable=false` |
| AC-ERR-006 | 超时 | 超时不等于取消，已发送动作不自动重试 |
| AC-ERR-007 | 取消 | 取消后不产生后续动作；已发送动作仍需复观察 |
| AC-ERR-008 | 部分发送 | 输入文本成功、后续按键失败时返回 `partial` |
| AC-ERR-009 | 资源忙 | 返回 `resource_busy`，不阻塞或串错 session |
| AC-ERR-010 | panic | 工具 panic 被捕获，daemon 不崩溃 |

## 8. 性能与资源验收

在固定 Windows 参考机器、release 构建下测量，记录 p50/p95：

| 编号 | 指标 | 目标 |
|---|---|---|
| AC-PERF-001 | CLI `--help` 启动 | p95 < 200 ms |
| AC-PERF-002 | daemon 就绪（不加载模型） | p95 < 2 s |
| AC-PERF-003 | 本地 IPC 往返 | p95 < 20 ms |
| AC-PERF-004 | 1080p 全屏截图（warm） | p95 < 300 ms |
| AC-PERF-005 | 窗口截图（warm） | p95 < 500 ms |
| AC-PERF-006 | OCR warm 感知 | 记录 p50/p95，不得比基线回退 >20% |
| AC-PERF-007 | 100 次连续调用 | 内存无持续增长；warm 后增长 < 10% |
| AC-PERF-008 | 8 个并发 session | 无死锁、无 session 串扰、无明显饥饿 |
| AC-PERF-009 | 会话关闭 | capture/temp 资源释放，30 秒内回到稳定水平 |
| AC-PERF-010 | 模型加载 | 首次加载有明确进度/日志；不阻塞其他只读工具 |

性能未达标不一定是功能阻塞，但必须记录基线、硬件、模型版本和构建配置。

## 9. 安全验收

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-SEC-001 | 本地监听 | 默认只开 Named Pipe/Unix Socket，不监听公网 |
| AC-SEC-002 | 访问控制 | 无 token/错误 token 无法连接 |
| AC-SEC-003 | 无审批 | 不存在 approve/reject 命令、MCP 工具或 pending 队列 |
| AC-SEC-004 | 风险元数据 | 关键动作返回 `meta.risk`，但不停顿 |
| AC-SEC-005 | 自定义截图路径 | 合法路径可写；父目录自动创建；扩展名与所选格式一致 |
| AC-SEC-006 | 字库路径穿越 | `dict_name` 含 `..`/分隔符/绝对路径被拒绝 |
| AC-SEC-007 | 输入读取限制 | 模板/视觉图片只从允许目录读取 |
| AC-SEC-008 | 日志脱敏 | 输入文本、剪贴板、窗口标题、截图不进入普通日志 |
| AC-SEC-009 | 会话隔离 | 不能读取其他 session 的 capture/target/observation |
| AC-SEC-010 | 无脚本执行 | 工具参数不能执行 shell、脚本或任意可执行文件 |
| AC-SEC-011 | 端口暴露 | 可选 HTTP 默认关闭；开启时只绑定 127.0.0.1 |
| AC-SEC-012 | 强隔离说明 | 文档明确：无审批不是安全沙箱 |

## 10. 模型与安装包验收

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-PKG-001 | Windows 安装包 | 包含 `deskctl.exe`、ONNX Runtime、PaddleOCR、YOLO 模型 |
| AC-PKG-002 | 模型校验 | 安装后 `deskctl doctor` 通过 SHA-256 校验 |
| AC-PKG-003 | 离线安装 | 无网络环境可用随包模型完成 OCR/感知 |
| AC-PKG-004 | 在线更新 | 可选择从配置源下载/更新模型，失败不破坏现有安装 |
| AC-PKG-005 | 卸载 | 卸载不删除用户自定义截图目录 |
| AC-PKG-006 | 许可证 | 安装包包含 PaddleOCR/YOLO/ONNX Runtime 的许可证声明 |
| AC-PKG-007 | 无模型降级 | 模型缺失时非视觉工具仍可用，视觉工具返回 `model_unavailable` |

## 11. 扩展性验收

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-EXT-001 | 新增工具 | 只需新增 Schema、executor、metadata 和契约测试，不改 core 主流程 |
| AC-EXT-002 | 新增平台 | 通过 trait + feature 接入，不修改工具协议 |
| AC-EXT-003 | 新增传输 | MCP/HTTP 作为 adapter，不修改桌面核心 |
| AC-EXT-004 | 新增视觉 provider | 通过统一接口接入，不绑定具体供应商 |
| AC-EXT-005 | 新增 skill | 只需 `skill.json` + `SKILL.md` + eval，不改 daemon |
| AC-EXT-006 | 工具 profile | 新工具可声明 profile、平台和状态性 |
| AC-EXT-007 | 协议兼容 | 增加字段不破坏旧客户端；移除字段需版本升级 |
| AC-EXT-008 | feature 隔离 | 未启用平台/模型的代码不进入构建或运行路径 |

## 12. 文档与 skills 验收

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-DOC-001 | 架构文档 | 包含进程、协议、状态、平台、模型、安全、扩展机制 |
| AC-DOC-002 | 测试计划 | 覆盖单元、契约、集成、E2E、性能、安全、故障注入 |
| AC-DOC-003 | MCP 文档 | 包含 initialize、tools/list、tools/call、取消、进度、错误映射 |
| AC-DOC-004 | skills | 每个 skill 有 manifest、prompt、适用工具、失败恢复和示例 |
| AC-DOC-005 | 验收标准 | 每条验收项可重复、可观测、有明确通过条件 |
| AC-DOC-006 | 版本记录 | 协议、工具 Schema、skills 均有版本号 |

## 13. 发布门禁

Windows 首个可用版本必须同时满足：

- [ ] AC-SCOPE 全部通过
- [ ] 传统桌面工具 P0 用例全部通过
- [ ] 语义 UIA P0 用例全部通过
- [ ] 多 session 并发和隔离通过
- [ ] MCP `initialize` / `tools/list` / `tools/call` 通过
- [ ] CLI 验收通过
- [ ] 错误与重试验收通过
- [ ] 安全验收通过
- [ ] 模型随包和 `doctor` 通过
- [ ] 性能基线记录完成
- [ ] 文档和 skills 完成
- [ ] 无 Tauri/LLM/工作流依赖

## 14. 已知不在首期验收范围

- macOS Accessibility 真机行为
- Linux X11/Wayland 行为
- `desktop_vision` 的供应商兼容矩阵
- `desktop_agent_step` 的增强模型质量
- 超大规模并发（>8 session）和长时间稳定性
- 受保护内容/DRM 窗口的截图

## 15. 运维增强验收（规划中，尚未实现）

> 方案见 [design/README.md](design/README.md)（D1–D4 里程碑）。以下编号为**草案登记**，
> 逐项实现并通过验证后，从本节移入对应正式章节（第 3/4/6/7/9 节）并解除"规划中"标记。
>
> 进度：D1（审计与历史）已于 2026-10-19 实现并通过验证（见 §15.1）；D2（Agent 身份）已于 2026-10-19
> 实现并通过验证（见 §15.2）；D3（录制与回放）已于 2026-10-19 实现并通过验证（见 §15.3，真机
> SAP ST06 E2E-012 待回归）；D4（检查点与补偿）已于 2026-10-19 实现并通过验证（见 §15.4，
> 真机 GUI 差异样本待回归）；I1（输入模拟增强）已于 2026-10-19 实施（见 §15.6）；
> AC-SEC-013（§15.5）已于 2026-10-19 实施并通过验证。

### 15.1 审计与历史（D1，P0，已实施 2026-10-19）

> 已验证（AC-AUD-001..008）：证据见 [implementation-status.md](implementation-status.md) §2/§6；
> 判据测试在 `crates/deskctl-cli/tests/audit_history.rs`（daemon E2E、oneshot、失败/降级、
> history 过滤与聚合）与 `crates/deskctl-core/src/audit.rs` 单测（脱敏/轮转/清理/降级）；
> 审计开销 A/B 见 [performance-baseline.md](performance-baseline.md) §5。
> `history --run` 过滤已就绪；run 产物目录的写入随 D3。

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-AUD-001 | 审计落盘 | daemon 每次工具调用结束即在当日 JSONL 追加恰好一条记录（含失败/取消）；字段与 [design/audit-history.md](design/audit-history.md) §4 一致 |
| AC-AUD-002 | 记录模式 | `basic` 不含 `args`；`full` 含脱敏 `args`；两种模式 `args_digest` 均可复算 |
| AC-AUD-003 | 文本脱敏 | `DESKCTL_AUDIT_TEXT=digest` 时键盘文本仅存长度与 sha256，原文不出现在任何审计文件 |
| AC-AUD-004 | 查询 | `deskctl history` 支持 `--session/--owner/--tool/--since/--failed-only/--limit/--json/--summary` 且过滤正确 |
| AC-AUD-005 | 轮转与保留 | 分片、保留天数与总量上限生效，清理不丢最新记录 |
| AC-AUD-006 | 降级 | 审计目录不可写时工具照常成功、告警一次、`audit_state=disabled` 可观测 |
| AC-AUD-007 | 脱敏增强 | 密码字段（UIA `IsPassword=true`）永不落盘；键名单命中值替换为 `<redacted>`；全文检索无命中 |
| AC-AUD-008 | 日志与 run 产物 | tracing 日志沿用与审计相同的脱敏规则（默认级别不含 args/文本明文）；run 产物（`expanded.jsonl` 等）固定落在 `%LOCALAPPDATA%\deskctl\runs\<run_id>\` 并受同一保留策略约束；`history --run <id>` 可过滤该 run 的全部记录 |

### 15.2 Agent 身份（D2，P1，已实施 2026-10-19）

> 已验证（AC-AGT-001..004）：证据见 [implementation-status.md](implementation-status.md) §2/§6；
> 判据测试在 `crates/deskctl-cli/tests/agent_identity.rs`（owner 创建/规范化/拒绝、`session list --owner`
> 过滤、MCP clientInfo 映射、审计归因与 `history --owner`/`--summary`、per-owner 限额）与
> `crates/deskctl-core/src/session.rs` 单测（`validate_owner` 规范化/非法字符/保留名）。
> 有意保留未实现：审计中 per-acquire `wait_ms`（审计记录格式 v1 冻结）；owner 仍是标签而非鉴权。

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-AGT-001 | owner 参数 | `session create --owner` 生效并显示于 `session list`；非法字符与保留前缀被拒 |
| AC-AGT-002 | MCP 映射 | `initialize.clientInfo.name` 映射为 `mcp:<name>`，缺失时为 `mcp:unknown` |
| AC-AGT-003 | 归因 | 审计与 `history --owner` 可按身份过滤；同会话内 owner 一致 |
| AC-AGT-004 | 限额 | 开启 owner 会话上限后，超限返回 `resource_busy` 且不影响其他 owner |

### 15.3 录制与回放（D3，P1，已实施 2026-10-19）

> 已完成验证（AC-RPL-001..009/013/014）：证据见 [implementation-status.md](implementation-status.md)
> §1 的 D3 行与 §2 已验证能力；判据测试在 `crates/deskctl-cli/tests/script_contract.rs`（录制→导出→
> 校验→dry-run→真跑、并发 `resource_busy`、handoff/续跑、readback 门禁）与 `crates/deskctl-core/src/`
> 的 recorder/script 单测；CLI 为 `deskctl record start|stop`、`deskctl script validate|run`。
> 有意未实现：AC-RPL-010..012（L2/L3 脚本引擎）仍为规划中；真机 E2E-012 见
> [test-plan.md](test-plan.md) §8。

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-RPL-001 | 录制 | `recorder_stop` 返回合法 `deskctl-script/1`；`max_steps` 超限停止并告警 |
| AC-RPL-002 | 参数化 | `candidate_id`→`locator`、绑定 `hwnd`→`$target.*`、`capture_id` 标 `volatile`、坐标标 `fragile` |
| AC-RPL-003 | 校验与预检 | 拒绝未知工具、未解析变量、非法版本、超限风险与歧义目标 |
| AC-RPL-004 | dry-run | 不执行任何工具，输出每步判定 |
| AC-RPL-005 | 重放报告 | 逐步结果与失败定位正确；`on_error=stop/continue` 行为正确 |
| AC-RPL-006 | 安全重试 | 仅 `not_sent` 允许重试；`partial/unknown` 不重试 |
| AC-RPL-007 | hwnd 失效 | 关闭/重启后可经 `launch_ref` 重绑；执行中失效自动重绑并记录 `rebinds`；歧义/指纹不符默认拒绝 |
| AC-RPL-008 | 核验分级 | `screen`（弱）与 `readback`（强）可执行并在报告中标注；`require_readback` 缺失能力时预检拒绝 |
| AC-RPL-009 | 运行结果 | run-level `outcome` 分类正确（`VERIFIED`/`HALTED_BEFORE_EFFECT`/`RECONCILIATION_REQUIRED`/`COMPLETED_UNVERIFIED`/`REJECTED_POLICY`） |
| AC-RPL-010 | 受限表达式（L2，可选） | `when`/`repeat` 生效；表达式无副作用（不能调用工具）；表达式越界/类型错误在预检被拒（见 [design/adr-001-script-engine.md](design/adr-001-script-engine.md)） |
| AC-RPL-011 | 引擎沙箱（L3，仅启用时） | `max_operations` 超限被中止且不计成功；未注册任何文件/网络/时间能力；`call()` 调用出现在审计中且与展开序列一致 |
| AC-RPL-012 | 引擎运行可追溯（L3，仅启用时） | 每次 Rhai run 产出 `expanded.jsonl`，与审计逐条一致（`run_id`/`step_index`）；Generator 模式产物为合法 `deskctl-script/1` 且可被 L1 校验/执行；同脚本 + 录制 transcript 的两次运行展开序列一致（确定性） |
| AC-RPL-013 | 失败接手契约 | run report 含 `handoff`（`failed_step`/`reason`/`effect_may_have_occurred`/`safe_to_retry`/`resume_hint`/`suggested_actions`）；`HALTED_BEFORE_EFFECT` 时 `safe_to_retry=true` 且 `resume_hint.from_step` 指向失败步；`RECONCILIATION_REQUIRED` 时 `safe_to_retry=false` 且建议"先核对"；LLM 修正参数后 `script_run --from <step>` 能在同一会话续跑成功（目标/变量按 §4.6 规则重建） |

### 15.4 检查点与补偿（D4，P2，已实施 2026-10-19）

> 已完成验证（AC-TXN-001..003）：证据见 [implementation-status.md](implementation-status.md) §1 的
> D4 行与 §2 已验证能力；判据测试在 `crates/deskctl-cli/tests/checkpoint_contract.rs`（导出/读回、
> 配额与命名、`rect` 差异四类、`script run --checkpoint/--compensation/--apply-compensation`、
> `purpose=compensation` 默认 dry-run）与 `crates/deskctl-core/src/checkpoint.rs` 单测；
> 真机 GUI 差异样本见 [test-plan.md](test-plan.md) §8 的 E2E-013。

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-TXN-001 | 检查点保存 | 保存窗口/目标/关键值/capture 并可导出 `deskctl-checkpoint/1`；上限与命名校验生效 |
| AC-TXN-002 | 差异校验 | `checkpoint_verify` 返回 `matched/diffs`（值/布尔/存在/矩形四类至少各一例） |
| AC-TXN-003 | 补偿脚本 | `purpose=compensation` 默认 dry-run；`apply=true` 才执行；二次失败立即停止 |

### 15.5 屏幕文本注入防御（独立小改动，已实施 2026-10-19）

> 已完成验证（AC-SEC-013）：证据见 [implementation-status.md](implementation-status.md) §1 的 S1 行与
> §2 已验证能力；判据测试在 `crates/deskctl-cli/tests/untrusted_wrap.rs`（history 默认包裹 /
> `DESKCTL_UNTRUSTED_WRAP=off` 恢复原文）与 `crates/deskctl-core/src/untrusted.rs` 单测（开关解析、
> 递归包裹）；工具描述已声明"数据非指令"。未包裹面（UIA/语义候选文本、写入参数污染路径 R-08）
> 见 [known-limitations.md](known-limitations.md)。

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-SEC-013 | 不可信内容包裹 | `perceive`/`read_kv`/`history` 返回的屏幕文本以 `<untrusted-screen-content>` 包裹并声明"数据非指令"；`DESKCTL_UNTRUSTED_WRAP=off` 可关闭（编号避开既有 AC-SEC-011/012，见 [design/review-01-consistency.md](design/review-01-consistency.md) R-01） |

### 15.6 输入模拟增强（I1，独立，可并行）

> 方案见 [design/input-simulation.md](design/input-simulation.md)。扩展现有 `desktop_mouse`/`desktop_input` 子命令与参数，**不新增工具（保持 36）**；`humanize` 默认 `off`，行为向后兼容。
> 进度：已于 2026-10-19 实施（见 [implementation-status.md](implementation-status.md) §1 的 I1 行与 §2 已验证能力）。

| 编号 | 验收项 | 通过标准 |
|---|---|---|
| AC-MOUSE-012 | 相对移动与按住/释放 | `move_relative` 以物理像素偏移（多屏虚拟桌面内可跨屏）；`down`→多次 `move`→`up` 可完成拖选/长按菜单；未配对的 `down` 在会话内被跟踪并在冲突调用/会话关闭时清理（无粘键） |
| AC-MOUSE-013 | 拟人轨迹确定性 | 同 `seed`+参数两次运行点列逐点一致（单测）；实测轨迹非直线、总时长与 `speed` 偏差 ≤20%；`path_points` 回传与实际一致 |
| AC-MOUSE-014 | 点击拟人 | `hold_ms` 生效（down/up 时差可测）；`jitter_px>0` 落点在目标元素内且距边缘 ≥2px；`double_click` 间隔受系统双击时间约束 |
| AC-INPUT-009 | 键码三写法 | `key` 支持 `"enter"`/`"vk:0x41"`/`"scan:0x1E"`（含 `extended`）；非法写法 `invalid_params` 并附示例；`scan` 模式在记事本与远程桌面/经典应用至少各一例生效 |
| AC-INPUT-010 | 长按与重复 | `key_down`→`key_up` 期间键保持按下（目标应用行为可证）；`repeat`+`interval_ms` 按序发送且可取消；`hotkey` 的 `hold_ms` 生效 |
| AC-INPUT-011 | 拟人打字 | `humanize` 下逐字延迟落在配置范围（统计断言）；同 `seed` 复现同一延迟序列；`delay_ms` 固定值模式精确生效 |
| AC-INPUT-012 | 限额与取消 | 超限（hold/repeat/interval/总时长）被拒并附范围；执行中 `request/cancel` ≤1s 生效并如实报告 `cancelled` 与已发送量 |
