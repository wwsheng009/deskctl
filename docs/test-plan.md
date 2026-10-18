# Deskctl 测试计划

- 文档版本：0.1
- 日期：2026-10-18
- 状态：Draft
- 关联文档：[acceptance-criteria.md](acceptance-criteria.md)、[architecture.md](architecture.md)、[mcp-service.md](mcp-service.md)

## 1. 目标

本计划回答四个问题：

1. 每个工具和协议如何被验证。
2. 哪些测试可以自动运行，哪些必须真机执行。
3. 多 session、异常、性能和安装包如何验收。
4. 什么情况下允许发布 Windows 首个版本。

## 2. 测试分层

```text
                 ┌─────────────────────┐
                 │ Windows E2E / 真机  │  少量、真实应用
                 ├─────────────────────┤
                 │ 集成测试            │  平台 API、daemon、CLI
                 ├─────────────────────┤
                 │ 契约测试            │  Schema、MCP、结果封套
                 ├─────────────────────┤
                 │ 单元测试            │  状态机、参数、路径、错误
                 └─────────────────────┘
```

原则：

- 核心状态机尽量用 mock 平台测试，减少对真实鼠标键盘的依赖。
- 平台 API 必须有 Windows 真机集成测试。
- 多 session 行为同时用 mock 和真机验证。
- 性能和安全测试单独记录，不与功能测试混在一起。

## 3. 测试环境

### 3.1 必测环境

| 环境 | 用途 |
|---|---|
| Windows 11 x64 | 主验收环境 |
| Windows 10 x64 | 兼容性抽测 |
| Windows Server Core | CLI/daemon 无 GUI 场景（不要求桌面工具） |
| macOS/Linux CI | 编译占位和 feature 检查，不做功能验收 |

### 3.2 构建配置

- debug：单元/契约/集成调试
- release：性能、安装包、E2E
- `standard` profile：默认验收
- `full` profile：可选工具验收
- `readonly` profile：安全/只读验收
- 模型存在/缺失两种配置

### 3.3 测试账号与桌面

- 使用专用 Windows 测试账号。
- 测试桌面分辨率固定，避免坐标变化。
- 关闭屏幕保护、睡眠、通知弹窗和自动更新。
- 测试前清理剪贴板和临时截图目录。
- 不在测试机上运行其他自动化工具。

## 4. 测试数据与夹具

建议建立 `tests/fixtures/`：

```text
tests/fixtures/
├─ images/
│  ├─ template-button.png
│  ├─ screen-1080p.png
│  ├─ icon-fixture.png
│  └─ text-fixture.png
├─ models/
│  ├─ README.md
│  └─ hashes.json
├─ configs/
│  ├─ standard.toml
│  ├─ full.toml
│  └─ readonly.toml
├─ mcp/
│  ├─ initialize.json
│  ├─ tools-list.json
│  └─ tools-call.json
└─ text/
   ├─ ascii.txt
   └─ unicode.txt
```

固定测试应用：

- Notepad：文本输入、保存、窗口操作
- Explorer：窗口列表、激活
- Calculator：按钮点击、UIA 语义
- 一个简单 Win32 测试程序：固定坐标和按钮，用于鼠标/键盘/UIA
- 一个可选 Electron/WPF 应用：验证 UIA/输入兼容性

## 5. 单元测试

### 5.1 协议与参数

- Schema 合法性和必需字段
- `additionalProperties` / 未知字段策略
- 参数类型、范围、互斥关系
- `hwnd`、`capture_id`、`element_id` 校验
- 自定义截图路径解析和扩展名处理
- `dict_name` basename 校验
- 输入文本长度、NUL 字符、空字符串
- 超时、间隔、稳定时间上下限

### 5.2 状态机

- 无 session 时拒绝有状态工具
- capture token 过期/消费/跨 session
- observation token 过期/跨 session
- target token 绑定和关闭
- 多 session 状态隔离
- 资源门排队、取消和释放
- 窗口变化导致动作中止
- `dispatch_state` 和 `retryable` 组合
- `partial` 和 `unknown` 不自动重试

### 5.3 平台 mock

建立 `MockPlatform`：

- 可控截图帧
- 可控窗口列表和前台状态
- 可控输入事件记录
- 可控 UIA 观察和候选
- 可注入超时、权限错误、窗口变化
- 可记录调用顺序，验证多 session 串行化

## 6. 契约测试

### 6.1 工具契约

- `standard` 26 个工具，`full` 28 个
- 每个 Schema 都有执行器
- 工具名稳定，不出现非 `desktop_*`
- 工具元数据包含 `requires_session`、`requires_observation`
- `desktop_vision`/`desktop_agent_step` 按 feature 出现
- 结果封套字段稳定
- 错误码稳定
- 旧字段新增不破坏客户端

### 6.2 MCP 契约

- `initialize` 版本和 capabilities
- `tools/list` 与协议文档一致
- `tools/call` 成功/失败映射
- `isError` 与 JSON-RPC error 的边界
- stdout 只有协议 JSON
- 取消/进度事件
- 无 `approval/*` 方法
- 工具列表变化通知

### 6.3 CLI 契约

- `--help` 子命令完整
- JSON 输入文件/stdin
- 退出码
- stdout/stderr 分离
- `doctor` 字段
- `session` 管理
- `cancel` 行为

## 7. Windows 集成测试

### 7.1 截图

- 全屏/区域/窗口
- 自定义路径、父目录创建、扩展名强制
- 多显示器/DPI 缩放（至少 100% 和 150%）
- 窗口移动/缩放期间截图
- 最小化/遮挡窗口
- 受保护窗口（预期失败）

### 7.2 输入

- 中文/英文/emoji/组合键
- 鼠标单击/双击/右键/中键/滚轮
- 拖拽
- 前台窗口切换
- 输入过程中窗口关闭
- 目标 locator 失效
- 两个 session 同时输入（验证串行）

### 7.3 窗口

- 枚举、激活、移动、缩放、信息
- 最小化窗口激活
- 同名窗口
- 高权限窗口
- 无效/过期窗口句柄

### 7.4 OCR/YOLO

- 模型随包路径
- 模型缺失/损坏/哈希不匹配
- 1080p 截图 OCR
- YOLO 图标检测
- OCR/YOLO 合并
- `capture_id` 映射
- 未登记图片的 `screen_center=null`

### 7.5 Windows UIA

- 目标列表、绑定、观察
- 按钮点击、复选框、文本框、列表项、菜单
- 同名控件和祖先上下文
- 观察后窗口变化
- 前台切换
- 高权限窗口
- 候选过期
- 持久化 locator 重放
- 状态验证 satisfied/unsatisfied/unknown

## 8. 端到端场景

### E2E-001 截图点击

```text
screenshot -> perceive -> mouse(capture_id, element_id) -> screenshot -> assert change
```

### E2E-002 窗口操作

```text
windows_list -> window_info -> window_activate -> window_move/resize -> window_info
```

### E2E-003 文本输入

```text
target_bind/window_activate -> input -> read file -> assert text
```

### E2E-004 UIA 语义执行

```text
targets_list -> target_bind -> semantic_observe -> semantic_candidate
-> semantic_execute -> verify_state
```

### E2E-005 持久化语义动作

```text
保存 workflow_step -> 关闭 session -> 新 session -> semantic_action
-> verify_state
```

### E2E-006 多 session 隔离

```text
session A screenshot -> session B 尝试使用 A 的 capture_id -> 必须失败
session A/B 同时输入 -> 输入不交叉
```

### E2E-007 取消

```text
启动 wait_for -> cancel -> 返回 cancelled，不再产生动作
```

### E2E-008 daemon 重启

```text
创建 session/token -> 重启 daemon -> 旧 token 失效 -> 客户端重新观察
```

## 9. 并发与压力测试

- 2/4/8 个 session 同时截图
- 2/4/8 个 session 同时观察 UIA
- 多 session 同时请求输入，验证串行和顺序
- 一个 session 长等待，另一个 session 正常操作
- 高频 screenshot/perceive 100 次，检查内存和临时文件
- 随机取消请求
- daemon 运行 8 小时稳定性测试
- 达到 session 上限时返回 `resource_busy`
- 关闭 session 后资源回收

## 10. 故障注入

| 故障 | 预期 |
|---|---|
| 平台 API 超时 | 返回 timeout，生成 `dispatch_unknown`（如可能已发送） |
| 窗口在动作前变化 | 拒绝动作，返回 stale/target_unavailable |
| 窗口在动作后消失 | 返回 unknown/needs_observation，不重试 |
| ONNX Runtime 缺失 | 视觉工具 `model_unavailable`，其他工具正常 |
| 模型哈希不匹配 | doctor 失败，视觉工具拒绝 |
| socket 断开 | 请求失败；daemon 回收 session |
| daemon 崩溃 | 客户端收到连接错误；旧 token 失效 |
| 两个 daemon 竞争 | 单实例锁阻止第二个 daemon |
| 工具 panic | 捕获并返回内部错误，daemon 不崩溃 |
| 磁盘写满 | 截图返回明确写入错误，不返回假路径 |

## 11. 性能测试

测量项：

- CLI `--help` 冷启动
- daemon 启动到 ready
- 本地 IPC p50/p95
- 全屏/区域/窗口截图
- 鼠标/键盘调用开销
- OCR cold/warm
- YOLO cold/warm
- 100 次调用内存曲线
- 8 session 并发 CPU/内存/延迟
- 会话关闭后的资源回落

要求：

- 使用 release 构建。
- 固定硬件、系统版本、分辨率、DPI、模型版本。
- 每次记录 p50/p95/p99、最大值和样本数。
- 与上一版本基线比较，回退超过 20% 必须说明。
- 不以单次最好成绩作为验收依据。

## 12. 安全测试

- 无 token / 错误 token 无法连接
- 非当前用户无法访问 socket/pipe
- `dict_name` 路径穿越
- 输入图片路径逃逸
- 自定义截图路径写入合法位置
- 工具参数不能注入 shell/脚本
- 日志中不出现输入文本、剪贴板、窗口标题
- 无 approve/reject 工具或方法
- `readonly` profile 不暴露输入/点击工具
- HTTP 默认关闭；开启时只绑定 127.0.0.1
- 会话隔离和 token 伪造
- 大 JSON/深嵌套参数不会导致崩溃或长时间占用

## 13. MCP 协议测试

使用独立 mock MCP client 和真实 MCP 客户端各测一遍：

- `initialize` 握手
- `tools/list`
- `tools/call` 成功
- 工具错误
- 未知工具
- 取消
- 进度
- stdout 纯净
- 客户端退出后 session 清理
- 两个 MCP 客户端并发
- profile 变化
- 版本不匹配
- 大结果分页

详细协议见 [mcp-service.md](mcp-service.md)。

## 14. 安装包测试

- 全新 Windows 安装
- 升级安装
- 卸载
- 无网络离线安装
- 在线模型更新
- 模型哈希校验
- 安装后 `deskctl doctor` 通过
- 安装包不包含源码/测试数据
- 许可证文件存在
- 用户自定义截图目录不被卸载删除

## 15. CI 流程

```text
fmt -> clippy -> unit -> contract -> Windows integration -> E2E -> package smoke
```

- Windows runner：必跑单元、契约、集成、E2E 子集。
- macOS/Linux runner：只跑编译和 feature 检查。
- 模型相关测试使用缓存目录，不在每次 CI 重新下载。
- 真实桌面 E2E 可以拆成 nightly 或手动流水线，避免阻塞每次提交。
- 任何 P0/P1 缺陷未关闭不允许发布。

## 16. 缺陷等级

| 等级 | 定义 | 发布影响 |
|---|---|---|
| P0 | daemon 崩溃、错误操作其他窗口、数据丢失、安全绕过 | 阻塞 |
| P1 | 工具主要功能不可用、session 串扰、MCP 不可用 | 阻塞 |
| P2 | 局部平台兼容、错误信息不清晰、性能回退 | 可评估发布 |
| P3 | 文案、日志、非关键体验 | 可后续修复 |

## 17. 验收映射

本测试计划的用例必须映射到 [acceptance-criteria.md](acceptance-criteria.md) 的 AC 编号：

```text
AC-CAP-*   -> 截图集成测试
AC-WIN-*   -> 窗口集成测试
AC-MOUSE-* -> 鼠标集成测试
AC-INPUT-* -> 键盘集成测试
AC-CLIP-*  -> 剪贴板测试
AC-FIND-*  -> 找图/找色/找字测试
AC-PER-*   -> OCR/YOLO 测试
AC-SEM-*   -> UIA 集成/E2E
AC-SESS-*  -> 多 session 测试
AC-MCP-*   -> MCP 契约测试
AC-CLI-*   -> CLI 契约测试
AC-ERR-*   -> 错误/取消/重试测试
AC-PERF-*  -> 性能测试
AC-SEC-*   -> 安全测试
AC-PKG-*   -> 安装包测试
AC-EXT-*   -> 扩展性评审
AC-DOC-*   -> 文档/skills 评审
```

## 18. 退出标准

Windows 首个版本发布前：

- [ ] 所有 P0/P1 用例通过
- [ ] 26 个核心工具至少各有一个成功用例
- [ ] 7 个核心语义工具至少各有一个真机用例
- [ ] 多 session 并发和隔离通过
- [ ] MCP 核心协议通过
- [ ] CLI 核心命令通过
- [ ] 无审批相关残留
- [ ] 模型随包和 doctor 通过
- [ ] 安全测试通过
- [ ] 性能基线记录完成
- [ ] 已知限制写入发布说明
