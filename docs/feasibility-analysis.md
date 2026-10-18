# 可行性分析

- 文档版本：0.2
- 日期：2026-10-18
- 状态：Draft（已按产品决策收敛）
- 目标项目：`E:\projects\ai\deskctl`
- 参考实现：`E:\projects\ai\nuphus`
- 架构评审：[architecture-review.md](architecture-review.md)

## 1. 结论摘要

**结论：可行，推荐以独立 Rust 项目抽取 Nuphus 现有桌面能力。**

Deskctl 的定位是：

- 只提供 `desktop_*` 工具
- Windows 首期落地
- 保留 macOS/Linux 平台抽象
- 不包含 Tauri、LLM Agent、工作流、记忆、插件市场
- 不实现审批流程
- 允许同一 daemon 多 session 并发
- 允许自定义截图输出路径
- OCR/YOLO 模型随安装包分发，同时保留在线下载

现有 Nuphus 中已经有一套经过验证的桌面实现：

- 28 个 `desktop_*` 工具
- 传统桌面工具执行器
- Windows UIA / macOS Accessibility 语义自动化
- 截图、OCR、YOLO、视觉模型调用
- 风险分类、执行后验证
- 跨进程自动化锁和资源门

因此不需要从零重写核心能力，重点是：

1. 抽取并解耦现有 Rust 代码
2. 去掉 Tauri、LLM、工作流依赖
3. 增加 daemon/session/MCP/CLI 层
4. 把 Windows 路径先做完整
5. 保留 macOS/Linux 的 trait 和模块位置

## 2. 参考实现基线

### 2.1 工具数量

参考项目当前定义了 28 个 `desktop_*` 工具：

- 20 个传统桌面工具：鼠标、键盘、截图、窗口、剪贴板、找图/找色/找字、OCR、感知、等待
- 8 个语义桌面工具：目标发现/绑定、语义观察、候选查询、候选执行、持久化语义动作、
  状态验证、增强模式单步决策

Schema 与执行分支已经一一对应：

- `src/tools/desktop_schemas.rs`
- `src/tools/desktop_executors.rs`
- `src/tools/semantic_desktop.rs`
- `src/tools/registry.rs`

### 2.2 平台实现

| 平台 | 传统桌面工具 | 语义桌面工具 |
|---|---|---|
| Windows | Win32、SendInput、xcap、UIA | Windows UIA |
| macOS | CoreGraphics、AX、xcap、enigo | macOS Accessibility |
| Linux X11 | X11 窗口管理、enigo、xcap | 无 |
| Linux Wayland | 窗口管理未支持 | 无 |
| 其他平台 | 返回 `PlatformNotSupported` | 无 |

关键代码：

- `src/desktop/client.rs`
- `src/desktop/targets.rs`
- `src/desktop/targets/windows.rs`
- `src/desktop/targets/macos.rs`
- `src/desktop/linux_window.rs`
- `src/desktop/macos_window.rs`
- `src/desktop_automation/windows_uia.rs`
- `src/desktop_automation/macos_accessibility.rs`
- `src-tauri/crates/desktop-api/`

### 2.3 有状态能力

以下状态保存在进程内：

- 截图捕获缓存与 `capture_id`
- `desktop_perceive` 生成的元素到屏幕坐标映射
- `desktop_target_bind` 返回的 target token
- `desktop_semantic_observe` 返回的 observation token 和候选动作空间
- 语义观察的 owner、TTL、最近动作和停滞计数
- ONNX 引擎
- 自动化锁和资源门

因此：

- `desktop_screenshot` → `desktop_perceive` → `desktop_mouse`
- `desktop_target_bind` → `desktop_semantic_observe` → `desktop_semantic_execute`

必须在同一个会话进程中完成，或者由常驻 daemon 维护会话。

### 2.4 已有入口参考

1. `src/main.rs`
   - 已经是 Rust CLI。
   - 当前使用 `ToolRegistry::builtin()`，没有挂载 desktop 工具。
   - 可改为 `ToolRegistry::builtin_with_desktop()` 或 `work_agent()`。

2. `src-tauri/src/main.rs`
   - 已在 Tauri 初始化前处理 `task` 子命令。
   - 证明“单二进制 + 子命令 + 非 GUI 模式”可行。

3. `src-tauri/crates/desktop-api/src/main.rs`
   - 已经是独立 HTTP 服务入口。
   - `api/http.rs` 有 SessionManager 和 `see/find/do/say` 会话接口。
   - 可作为 daemon/session 设计的参考，但不是完整的 28 工具注册表。

## 3. 已确认的产品约束

| 约束 | 影响 |
|---|---|
| 只做 desktop 工具 | 不引入 Agent、工作流、记忆、GUI |
| 无审批 | 去掉 `desktop_approval`；风险只作为元数据 |
| Windows 首期 | 只对 Windows 做完整实现和验收 |
| 其他平台预留 | 保留 trait、模块和 feature，不伪称已支持 |
| 多会话并发 | 同一 daemon 支持多个 session，桌面写操作串行 |
| 自定义截图路径 | 输出路径由调用方提供，不做输出沙箱 |
| 模型随包分发 | 安装包包含 OCR/YOLO/ONNX Runtime，保留下载选项 |
| 复用 Nuphus | 优先抽取现有 Rust 实现，不重写平台层 |

## 4. 可行性矩阵

| 能力 | 可行性 | 说明 |
|---|---|---|
| 工具 Schema、注册、分发 | 高 | 纯 Rust 结构，易抽离 |
| 无状态工具 CLI | 高 | 截图、窗口列表、剪贴板等可一次性执行 |
| 有状态工具 CLI | 中高 | 需要 daemon 或 session |
| 多会话并发 | 高 | session 隔离 + 全局资源门 |
| 无审批执行 | 高 | 实现简单，但安全风险由调用方承担 |
| MCP / JSON-RPC 接入 | 高 | 工具列表和调用协议容易映射 |
| Windows 传统桌面工具 | 高 | 现有 Win32 代码可直接抽取 |
| Windows UIA 语义工具 | 中高 | 需要 COM/STA 和 UIA 线程模型 |
| macOS AX 语义工具 | 预留 | 后续平台工作 |
| Linux X11 | 预留 | 后续平台工作 |
| Linux Wayland | 低 | 不纳入首期 |
| PaddleOCR / YOLO | 中高 | 现有 Rust 实现可复用，模型随包分发 |
| 视觉模型调用 | 高 | 普通 HTTP + base64 图片，作为可选能力 |
| 真实桌面端到端测试 | 中 | 需要 Windows 真机和应用样本 |

## 5. 主要技术风险

### 5.1 进程内状态丢失

最大风险不是工具函数本身，而是多步操作的状态链。

如果每个工具做成独立进程：

- `capture_id` 在下一次调用时不存在
- `observation_token` 无效
- target binding 失效
- 模型每次重新加载

**对策：**

- 有状态能力统一走 `serve` daemon。
- 无状态工具保留 `tool` 一次性模式。
- CLI `call` 作为 daemon 客户端，不直接执行有状态逻辑。

### 5.2 无审批带来的安全风险

本项目明确取消审批。这意味着：

- 模型可以直接执行鼠标点击、键盘输入、窗口操作。
- 关键动作不会被人工阻塞。
- access token 只限制连接，不限制具体动作。
- 同用户下的其他进程可能读取 token。

**对策：**

- daemon 默认只监听本地 Named Pipe / Unix Domain Socket。
- 默认使用 `standard` 或 `minimal` profile，不默认暴露全部高权限工具。
- 提供 `readonly` profile。
- 日志脱敏，不记录输入文本、剪贴板和窗口标题。
- 文档明确：审批不是本项目的能力，强隔离需要独立用户或系统级策略。

### 5.3 多会话并发冲突

同一 daemon 多 session 并发时，可能出现：

- 两个 session 同时移动鼠标
- 一个 session 正在语义执行，另一个 session 切换前台窗口
- 截图和输入交错，导致观察失效

**对策：**

- 每个 session 独立 capture/target/observation 状态。
- 读操作可并行；桌面写操作通过资源门串行。
- 输入动作使用 `DesktopInput` 资源类。
- 语义动作使用 `DesktopSemantic` 资源类。
- 等待类工具不持有输入锁。
- 对目标窗口做前后快照和变更校验。

### 5.4 平台 API 与权限

- Windows UIA 需要 COM apartment 和正确的线程模型。
- Windows 高权限窗口可能拒绝操作。
- macOS 辅助功能/录屏权限是硬性前提，但首期不做。
- Linux Wayland 不支持现有窗口管理路径，首期不做。

**对策：**

- 首期只对 Windows 做功能验收。
- macOS/Linux 保留 trait 和模块，不做虚假承诺。
- 不支持的能力返回 `platform_not_supported`。
- Windows 普通权限失败必须返回真实错误，不伪造成功。

### 5.5 模型分发

当前 PaddleOCR 需要：

- `ch_PP-OCRv4_det.onnx`
- `ch_PP-OCRv4_rec.onnx`
- `ch_PP-OCR_keys_v1.txt`
- ONNX Runtime 动态库

YOLO 还需要 `icon_detect.onnx`。

**决策：**

- 模型默认随 Windows 安装包分发。
- 保留在线下载/更新和离线安装。
- 模型文件不提交源码仓库。
- 发布前确认 PaddleOCR、YOLO、ONNX Runtime 的许可证和再分发条款。
- 模型提供 SHA-256 校验。
- `deskctl doctor` 检查模型和运行时。

### 5.6 Tauri / GUI 耦合

如果新项目继续依赖 Tauri：

- 二进制体积显著增大
- Windows release 无控制台输出
- 无 GUI 服务器环境难以运行
- 事件和窗口管理受 GUI 生命周期影响

**对策：**

- 新项目不依赖 Tauri。
- 不引入 WebView、前端资源和 GUI 事件循环。
- 如果未来需要 GUI，只作为独立客户端。

### 5.7 文件路径

已确认允许自定义截图输出路径。这意味着：

- 调用方可以指定任意可写路径。
- 父目录不存在时自动创建。
- 扩展名强制为 `.png`。
- 本项目不为输出路径提供沙箱。

仍需限制：

- `desktop_find_text` 的 `dict_name` 必须是安全 basename。
- `desktop_find_image`、`desktop_vision` 的输入图片应限制在允许目录。
- `target_locator.launch_ref` 不接受任意启动路径。

## 6. 实现路径选项

### 方案 A：抽离现有 Rust 模块（推荐）

把 `src/desktop`、`src/desktop_automation`、`src/tools` 中的相关部分抽取到 Deskctl：

- 去掉 Tauri、Agent、工作流、记忆
- 保留 ToolRegistry、DesktopClient、平台适配器和状态验证
- **不抽取 `desktop_approval.rs`**
- 首期只编译 Windows 路径
- 新增 daemon、session、MCP 和 CLI 层

优点：

- 复用已经验证的实现
- 风险最低
- 可以逐步解耦

注意：

- 保留原项目许可证和版权声明
- 把 `crate::` 依赖改成 `deskctl-core` 内部边界
- 不要把整个 Nuphus crate 作为长期依赖

### 方案 B：临时依赖 `nuphus` 库

新建项目先通过 path dependency 使用 `nuphus`，快速做 daemon/MCP 原型。

优点：

- 最快验证协议和工具调用
- 不需要先拆代码

缺点：

- 不是真正独立项目
- 会把 Tauri/LLM/工作流依赖带进来
- 后续仍要解耦

适合短期 spike，不适合最终形态。

### 方案 C：包装现有可执行文件

用新项目包装 Nuphus 或 `desktop-api` 的现有进程。

优点：

- 改动最小
- 可以最快给多个 LLM 提供统一接口

缺点：

- 受原进程生命周期和 API 限制
- 仍需解决状态和版本兼容
- 不是真正的独立工具

### 方案 D：完全从零重写

不建议。现有代码已经覆盖大量平台细节、错误处理和验证逻辑，从零重写会重复踩坑。

## 7. 首期范围

### 7.1 Windows MVP

- 独立 Rust workspace
- 单二进制 `deskctl`
- `mcp` stdio server
- `serve` daemon + `call` 客户端
- 无状态工具的 `tool` 一次性命令
- Windows 10/11 核心 26 工具
- 配置模型后可启用完整 28 工具
- 多 session 并发
- 自定义截图输出路径
- OCR/YOLO 模型随安装包
- 工具 Schema 与错误码
- Windows 真机回归

### 7.2 后续阶段

- macOS 传统工具 + Accessibility 语义工具
- Linux X11 传统工具
- Linux Wayland 研究
- 可选 HTTP/WebSocket 适配
- 在线模型更新
- 更完整的诊断包

## 8. 验收标准

首期达到以下条件才算“可用”：

1. `deskctl mcp` 可以被至少一个 MCP 客户端列出并调用工具。
2. `desktop_screenshot` → `desktop_perceive` → `desktop_mouse` 在同一 session 中可完成。
3. `desktop_target_bind` → `desktop_semantic_observe` → `desktop_semantic_execute` 可完成。
4. 同一 daemon 至少两个 session 可并发截图/观察，输入动作不交叉。
5. 自定义截图输出路径可写入，父目录自动创建，扩展名强制为 `.png`。
6. 安装包默认包含 OCR/YOLO 模型，`deskctl doctor` 检查通过。
7. 没有审批流程；风险等级只出现在结果元数据中。
8. 不支持的工具/平台返回结构化错误，而不是 panic 或假成功。
9. Windows 真机通过鼠标、键盘、窗口、截图、UIA 回归测试。
10. 项目中不依赖 Tauri、LLM SDK、工作流引擎和前端资源。

## 9. 工作量判断

以现有代码为基础，工作量主要不在“写 28 个函数”，而在：

- 解耦和模块边界
- daemon/session/多会话并发
- MCP 协议适配
- 取消和超时
- 自定义截图路径和文件策略
- Windows 原生资源和权限
- 模型随包分发
- Windows 真机回归
- 安装包和发布流程

按难度分级：

- 低：工具 Schema、CLI 参数、JSON 输出、无状态工具
- 中：daemon、多会话、MCP、取消、日志脱敏
- 高：Windows UIA、截图/窗口捕获、模型运行时
- 预留：macOS AX、Linux X11、Wayland

## 10. 最终判断

**建议执行方案 A：抽离现有 Rust 桌面能力，构建独立的 Deskctl 项目。**

它不是对现有代码的简单复制，而是一次边界清晰的产品化重构：

- 去掉 Tauri、LLM 编排和审批
- 保留桌面工具与 Windows 平台实现
- 增加 daemon、多 session、MCP/CLI
- 把模型随安装包分发
- 允许自定义截图路径
- 保留 macOS/Linux 的扩展位置

只要接受“有状态工具必须通过 daemon 使用”和“没有审批门”这两个约束，
整体方案在技术上可行，且明显优于从零重写。
