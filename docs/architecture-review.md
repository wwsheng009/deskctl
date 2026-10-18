# Deskctl 架构评审

- 评审版本：0.2
- 日期：2026-10-18
- 评审对象：`docs/architecture.md`、`docs/feasibility-analysis.md`
- 目标项目：`E:\projects\ai\deskctl`
- 参考实现：`E:\projects\ai\nuphus`
- 状态：主要产品决策已确认，可进入骨架实现

## 1. 评审结论

初版架构的方向是正确的：

- Rust
- 独立 daemon
- 单二进制多子命令
- MCP stdio + CLI
- 平台层隔离
- 复用 Nuphus 现有桌面代码

但初版存在三个必须先解决的 P0 问题：

1. MCP 与 daemon 的 session 绑定不明确
2. 无 GUI 审批流程不明确
3. 工具结果、错误和重试语义不统一

经过产品决策，当前版本已经收敛：

- 取消审批流程
- Windows 首期
- 多 session 并发
- 自定义截图输出路径
- 模型随安装包分发
- 项目改名 `deskctl`
- 优先参考并抽取 Nuphus

## 2. 最终产品决策

| 项目 | 决策 |
|---|---|
| 项目名称 | `deskctl` |
| 语言 | Rust |
| 首期平台 | Windows 10/11 |
| 其他平台 | 保留 trait、模块和 feature，不做首期承诺 |
| 工具范围 | 只提供 `desktop_*` |
| 审批 | 不实现 `approve/reject` |
| 截图路径 | 允许调用方自定义输出路径 |
| 多会话 | 同一 daemon 支持多个并发 session |
| 桌面写操作 | 多 session 可并发，但输入/语义写操作串行 |
| 模型 | OCR/YOLO/ONNX Runtime 随安装包分发；保留在线下载和离线安装 |
| 默认工具集 | 核心 26 个；完整 28 个 |
| 可选工具 | `desktop_vision`、`desktop_agent_step` |
| 参考策略 | 优先抽取 Nuphus 现有 Rust 实现 |

## 3. 问题与解决状态

| 编号 | 原问题 | 状态 | 当前方案 |
|---|---|---|---|
| A1 | MCP 与 daemon session 绑定未定义 | 已解决 | MCP 进程隐式绑定一个 session，`session_id` 不暴露给 LLM |
| A2 | 审批权限与无 GUI 审批未定义 | 已取消 | 不实现审批；风险作为 `meta.risk` 返回 |
| A3 | 结果/错误/重试契约未统一 | 已解决 | 统一 `ok/data/error/meta` 封套和 `dispatch_state` |
| A4 | 无状态/有状态工具未区分 | 已解决 | 工具元数据 + 一次性 `tool` 白名单 |
| A5 | 28 工具上下文压力 | 已解决 | `minimal/standard/full/readonly/platform` profile |
| A6 | 文件路径安全边界 | 部分接受 | 截图输出路径允许自定义；输入读取和 `dict_name` 仍受限 |
| A7 | 版本/单实例/升级未定义 | 已解决 | 单实例锁、协议版本、重启失效语义 |
| A8 | 取消与超时语义未定义 | 已解决 | 超时 ≠ 取消；已发送/未知禁止自动重试 |
| A9 | 模型工具与通用定位冲突 | 已解决 | `desktop_vision`、`desktop_agent_step` 标为可选 |
| A10 | crate 拆分过早 | 已解决 | 首期 3 个 crate |
| A11 | 平台能力矩阵过于乐观 | 已解决 | 首期只承诺 Windows；macOS/Linux 预留 |
| A12 | 模型分发与许可证 | 部分解决 | 模型随包分发；许可证仍需发布前确认 |
| A13 | 缺少事件模型 | 已解决 | 定义 session/tool/request/daemon 事件 |

## 4. 无审批的残余风险

这是本项目最重要的安全取舍。

取消审批后：

- 模型可以直接执行鼠标、键盘、窗口和语义操作。
- 关键动作不会被人工阻塞。
- access token 只解决“能否连接 daemon”，不是动作级授权。
- 同用户下的其他进程可能读取 token 或直接控制桌面。

必须保留的缓解措施：

- daemon 默认只监听本地 Named Pipe / Unix Domain Socket。
- 默认工具 profile 不暴露全部高权限工具。
- 提供 `readonly` profile。
- 日志不记录输入文本、剪贴板内容、窗口标题和截图。
- 结果中保留 `risk` 和 `dispatch_state`，让调用方可视化风险。
- 文档明确：本项目不是安全沙箱，强隔离需要独立用户或系统级策略。
- 不建议把 daemon 暴露到局域网或公网。

## 5. 多会话并发约束

允许同一 daemon 多 session 并发，但必须满足：

- session 之间不共享 capture/target/observation 状态。
- 截图、窗口枚举、OCR、视觉请求可以并行。
- 鼠标、键盘、拖拽和语义写操作通过资源门串行。
- 等待类工具不持有输入锁。
- 输入动作前必须验证前台窗口和目标窗口没有变化。
- 资源门需要公平队列，避免一个 session 长期饿死其他 session。
- daemon 需要可配置 session 上限，默认建议 8。
- daemon 必须有 `session/list`、`session/inspect`、`session/close` 等管理能力。

## 6. 自定义截图路径约束

已确认允许自定义输出路径：

- `desktop_screenshot.path` 和 `desktop_window_screenshot.path` 可接受绝对/相对路径。
- 父目录不存在时自动创建。
- 扩展名强制为 `.png`。
- 不提供输出路径沙箱。
- 文档需明确调用方对路径安全负责。

仍需限制的输入路径：

- `desktop_find_text.dict_name` 只允许安全 basename。
- `desktop_find_image.template_path` 和 `desktop_vision.image_path` 限制在允许读取目录，或要求显式配置。
- `target_locator.launch_ref` 必须来自本地目标目录。

## 7. 模型分发

已确认：

- PaddleOCR、YOLO、ONNX Runtime 默认随 Windows 安装包分发。
- 保留在线下载/更新。
- 支持离线安装。
- 模型文件不提交源码仓库。
- 安装后通过 `deskctl doctor` 检查模型和 ONNX Runtime。
- 模型文件需要 SHA-256 校验。
- 发布前必须确认 PaddleOCR、YOLO、ONNX Runtime 的许可证和再分发条款。

## 8. 首期范围

### 必须实现

- Windows 10/11 核心 26 工具
- 单二进制 `deskctl`
- `mcp` stdio server
- `serve` daemon
- `call` 客户端
- 多 session 并发
- 自定义截图输出路径
- OCR/YOLO 随包模型
- 工具 profile
- 统一结果封套
- Windows 真机回归

### 可选实现

- `desktop_vision`
- `desktop_agent_step`
- `readonly` profile
- HTTP 适配
- 在线模型更新

### 明确不做

- 审批流程
- Tauri GUI
- LLM Agent 循环
- 工作流、记忆、插件市场
- macOS/Linux 首期真机验收
- Wayland

## 9. 进入实现前的剩余决策

1. 代码策略：直接复制并解耦，还是先 path 依赖 `nuphus` 做 spike。
2. MCP 是否作为首期唯一对外协议，还是同时提供 HTTP。
3. 默认工具 profile 是否使用 `standard`（26 个）。
4. 默认 session 并发上限和资源门队列策略。
5. PaddleOCR/YOLO 模型的许可证与安装包分发方式。
6. 是否在首期实现 `readonly` profile 和配置化拒绝规则。
7. 项目是否正式定名 `deskctl`，还是保留改名空间。

## 10. 实施建议

先做一个 Windows spike：

1. 创建 `deskctl-protocol`、`deskctl-core`、`deskctl-cli` 三个 crate。
2. 从 Nuphus 复制 Windows 桌面代码和 Schema。
3. 去掉 Tauri、LLM、工作流依赖。
4. 实现 `tool` 一次性命令和统一结果封套。
5. 实现 `serve` + 多 session + 资源门。
6. 接入 MCP stdio。
7. 接入 OCR/YOLO 随包模型。
8. 做 Windows 真机回归。

## 11. 评审结论

初版的 P0 阻塞项已经通过产品决策解决：

- session 绑定已经明确
- 审批已取消
- 结果封套和重试语义已经明确

当前架构可以进入骨架实现阶段。

**评审状态：通过，允许开始 Windows MVP 骨架。**
