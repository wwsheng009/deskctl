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
- 自定义截图路径不受沙箱限制，调用方负责路径与覆盖策略。
- 会话关闭只清理 `%TEMP%` 下的会话截图；默认 `%LOCALAPPDATA%\deskctl\captures`
  视为持久产物，不随会话关闭删除。
- 模板/视觉输入图片限制在配置的图片根目录；字库名只允许 basename。
- 模型/运行时缺失时视觉工具不可用；非视觉工具不受影响。
- 当前未提供代码签名、自动更新和干净机安装验证；这些属于发布流水线工作。
- macOS/Linux 仅有条件编译占位，未做真机验收。

