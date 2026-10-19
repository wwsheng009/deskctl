# Deskctl 已知限制

- 仅 Windows 10/11 x64 提供功能实现；其他平台返回
  `platform_not_supported`。
- 无审批不是安全沙箱。任何持有本地 access token 的调用方都能执行已启用工具。
- `desktop_vision` 需要外部视觉 provider；未配置时返回
  `model_unavailable`。
- `desktop_agent_step` 需要增强决策 provider；未配置时不暴露或返回
  `model_unavailable`。
- UIA 树是有界观察：单个窗口最多返回 200 个节点，候选分页每页 20 条。
- 深层虚拟化列表可能只暴露可见区域；需要滚动后重新观察。
- UIA 值只返回指纹，不返回原始文本；需要原文时应使用截图/OCR 或应用自身接口。
- 受保护内容、DRM 窗口、部分高权限窗口可能无法截图或自动化。
- 窗口激活依赖 attach 前台/目标输入队列 + `SetActiveWindow` + `SwitchToThisWindow`
  的 Win32 手法；在 UAC/安全桌面、前台进程拒绝或窗口线程无响应时仍可能失败，
  返回可重试的 `target_unavailable`。
- Windows 11 单实例 Notepad 等标签页应用会复用已有进程的窗口：新启动的进程
  可能没有自己的窗口，按进程过滤会查不到；此时应按标题或 hwnd 选择目标。
- 屏幕捕获使用主显示器；多显示器区域截图仅支持主屏范围。
- 进程启动时声明 Per-Monitor-V2 DPI 感知（Windows 10 1703+）：鼠标输入、窗口矩形、
  UIA 边界与截图统一为物理像素坐标；150% 等缩放比例下不再需要调用方手动换算
  （旧行为为鼠标逻辑坐标与截图物理像素混用，150% 缩放下有 1.5 倍偏移）。
- 受限启动只覆盖 Start Menu（用户/公共）与桌面目录下的 `.lnk`；`launch_ref` 只能使用
  `desktop_targets_list` 返回的目录引用，等待窗口上限 10 秒，不请求提权（runas=false）。
- `delivery_mode=auto/background` 依赖控件 UIA provider 的后台能力：部分 WebView2/打包
  应用只支持前台执行，auto 会自动回退前台；provider 把成功当错误上报（0x00000000）时
  不重复派发，避免双执行。
- 语义树单次观察最多 2000 节点（8 秒预算）；更大的树需要更窄的 `scope` 或使用
  `tree_cursor` 续读。`scroll` / `set_range_value` 仅在控件暴露对应 UIA 模式时可用。
- 鼠标点击在携带 `hwnd` 或 capture 引用时默认做遮挡检测：目标点被其它进程窗口覆盖会
  返回 `target_unavailable` + `occluded_by`（`allow_occluded=true` 可跳过；任务栏按钮
  不视为遮挡）。遮挡检测是"防误点"，不是安全边界。
- 输入类工具（`desktop_mouse`/`desktop_mouse_drag`/`desktop_input`）经 Input 资源门串行，
  拟人长序列（如 `path_points` 上千的移动、大批量拟人打字）会拉长门的持有时间，多 Agent
  并发输入时互相等待；长序列建议合并为单次调用并保留取消通道（取消每 16 个轨迹点/每个
  按键间隔检查一次，目标 ≤1s 生效）。
- `humanize` 默认 `off`（行为与旧版完全一致）；开启后所有随机采样来自 PCG32(`seed`)，
  响应回传 `seed` 以复现；`path_points` 为实际发送点数，取消时如实返回已发送量。
- SendInput 在少数游戏/反作弊环境会被拦截，本工具不承诺绕过；对只认扫描码的经典应用/
  远程桌面，可用 `key: "scan:0x1E"`（扩展键 `scan:0xE01D` 或 `extended:true`）通道。
- `down` / `key_down` 未配对释放时：同一会话内下一次冲突调用或会话关闭时会尽力自动
  释放（防粘键，`desktop_session` 摘要含 `pressed_buttons`/`pressed_keys`）；daemon
  进程退出由系统清理键状态，跨会话不做全局清理。
- IME 组合输入（中文选字等）建议使用 `mode=type`（UNICODE 注入，不依赖键盘布局）；
  `mode=key` 面向功能键/快捷键/游戏场景，不承诺与输入法组合行为一致。
- `move_relative` 以物理像素为基准（多显示器虚拟桌面内可跨屏）；实现使用多段绝对坐标
  模拟，不使用受鼠标加速度影响的 `MOUSEEVENTF_MOVE` 相对事件。
- 语义动作 `delivery_mode` 默认 `auto`（后台优先、失败回退前台）；`restore_foreground=true`
  时动作后尽力还原原前台窗口，受 Windows 前台锁影响可能失败，结果字段如实返回。
- 目标窗口被模态对话框（原生 Save/Open 等）禁用时，`desktop_input` 与鼠标置前会自动
  重定向到该对话框窗口（响应字段 `target_hwnd` 为实际目标）；对话框关闭后自动恢复。
- 语义动作仍以绑定的窗口为目标：需要操作原生对话框时，应先用 `desktop_targets_list`
  找到对话框窗口并显式绑定（类 `#32770`），或在 `delivery_mode=background` 下执行。
- `desktop_image_crop` 的 `image_path` 输入受允许图像根约束（与 perceive/find_image 一致）；
  使用 `image_path` 的裁剪不会登记新 capture（无屏幕坐标映射），需要链式点击/OCR 时请用
  `capture_id` 作为来源。
- `desktop_perceive`/`desktop_read_kv` 可用 `capture_id` 直接读取会话内截图/裁剪（不落盘、
  不受允许图像根限制）；`image_path` 越界时 `policy_denied` 会列出允许根目录与
  `DESKCTL_ALLOWED_INPUT_ROOT`/`capture_id` 两条出路（`doctor` 输出 `allowed_input_roots`）。
- `desktop_read_kv` 是几何启发式：按行分组后优先配"同行右侧最近、间距 ≤ `value_gap`"的元素，
  其次配"正下方水平重叠"的元素；同侧候选多于一个时结果带 `ambiguous=true`（仍取最近者）。
  表头/多列表格会产生伪配对，建议先用 `region` 收窄到数值区，并按屏幕调整 `value_gap`；
  数值正确性取决于 OCR 质量（本项目实测 SAP 监控页可读，但小字号数字可能误识）。
- UIA 名称的双层编码乱码（UTF-8↔GBK）会自动修复，但**末端残字节不可恢复**：例如
  `鎸夊崌搴忔帓搴?` 修复为 `按升序排`（原文 `按升序排序` 的最后一个字丢失）。
  正常中文/英文/日文保持原样（有回归用例覆盖）；若 provider 使用其它编码组合仍可能残留。
- `desktop_window_activate` 最多尝试 3 轮（第 2 轮起先发送 Alt 键取得"最近输入"资格），
  并在窗口被隐藏时先 `SW_RESTORE`；目标进程主动拒绝置前或前台锁极端场景下仍可能失败，
  此时按返回的 `attempts`/`method` 诊断改用 `delivery_mode=background` 路径。
- `desktop_window_screenshot` 对 `xcap` 窗口枚举滞后做了 3 次重试；若窗口被 cloaked
  （其它虚拟桌面/隐藏）仍会返回 `target_unavailable`，提示先调用 `desktop_window_activate`。
- 办公文档工具为最简 OOXML 实现：`desktop_docx_save` 支持标题/小节/正文/表格/PNG 图片与
  图注，以及页眉/页脚/页码（`header`/`footer`/`page_numbers`；页码为 PAGE 域占位 1，
  打开时由 Word 更新；`desktop_docx_read` 以 `{PAGE}` 回读该字段；暂不支持样式模板，
  大图按 15cm 默认宽度等比缩放）；`desktop_xlsx_save` 支持内联字符串/数值/布尔、多工作表、
  表头加粗（`header_row`）、冻结首行（`freeze_header`）、按列宽度与 Excel 数字格式
  （`columns[].width`/`number_format`，自定义格式写入 numFmtId≥164；读回上限 500 行×100 列
  并返回 `truncated`，写出上限 20,000 行×500 列；无公式/合并单元格/条件格式）；
  `desktop_pptx_save` 支持标题/要点/内嵌 PNG 图片与版式选择（auto/title_bullets/title_image/
  text_image/title_only），暂不支持演讲者备注、图表与多图页。
- `desktop_docx_read`/`desktop_xlsx_read`/`desktop_pptx_read` 的输入路径遵循与图片相同的
  允许根策略，越界返回 `policy_denied`（错误消息含越界路径、允许根列表与两条出路提示）。
- 自定义截图路径不受沙箱限制，调用方负责路径与覆盖策略。
- 会话关闭只清理 `%TEMP%` 下的会话截图；默认 `%LOCALAPPDATA%\deskctl\captures`
  视为持久产物，不随会话关闭删除。
- 模板/视觉输入图片限制在配置的图片根目录；字库名只允许 basename。
- 模型/运行时缺失时视觉工具不可用；非视觉工具不受影响。
- 当前未提供代码签名、自动更新和干净机安装验证；这些属于发布流水线工作。
- macOS/Linux 仅有条件编译占位，未做真机验收。

