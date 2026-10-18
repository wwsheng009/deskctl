# Deskctl 可扩展性设计

- 文档版本：0.1
- 日期：2026-10-18
- 状态：Draft
- 关联文档：[architecture.md](architecture.md)、[mcp-service.md](mcp-service.md)、[llm-skills.md](llm-skills.md)

## 1. 目标

Deskctl 首期只做 Windows 桌面工具，但必须避免把 Windows 细节写死到协议和状态机中。

扩展目标：

- 新增工具：只改 Schema、执行器、元数据和测试
- 新增平台：只实现平台 trait 和 feature
- 新增传输：MCP/HTTP 作为 adapter，不改核心
- 新增视觉/决策 provider：通过接口接入
- 新增 Skill：只增加指令包和评估
- 新增工具 profile：配置化，不改代码

## 2. 分层

```text
┌─────────────────────────────────────────────┐
│ 传输适配层：mcp / cli / json-rpc / http      │
├─────────────────────────────────────────────┤
│ 工具协议层：Schema、结果封套、错误码          │
├─────────────────────────────────────────────┤
│ 核心状态层：session、capture、target、observe │
├─────────────────────────────────────────────┤
│ 自动化层：候选、执行、验证、风险              │
├─────────────────────────────────────────────┤
│ 平台层：Windows / macOS / Linux              │
├─────────────────────────────────────────────┤
│ 资源层：OCR / YOLO / 视觉 / 模型             │
└─────────────────────────────────────────────┘
```

依赖方向只能从上到下。平台层不得依赖 MCP 或 CLI。

## 3. 扩展点

### 3.1 新增工具

步骤：

1. 在 `deskctl-protocol` 增加工具名和 inputSchema。
2. 在 `deskctl-core` 增加执行器。
3. 增加工具元数据：
   - `requires_session`
   - `requires_observation`
   - `single_use_token`
   - `requires_vision_model`
   - `requires_decision_model`
4. 指定 profile 和平台。
5. 添加契约测试：Schema ↔ executor 一一对应。
6. 添加功能测试和失败测试。
7. 更新 MCP 工具列表和文档。

禁止：

- 在 MCP 层直接实现桌面逻辑。
- 在 Schema 中暴露原生指针/句柄。
- 新增工具绕过统一结果封套。
- 新增工具默认暴露给所有 profile。

### 3.2 新增平台

步骤：

1. 实现平台 trait：
   - `ScreenCapture`
   - `InputBackend`
   - `WindowBackend`
   - `SemanticBackend`（如有）
2. 使用 Cargo feature 和 `cfg` 隔离。
3. 声明能力矩阵。
4. 增加权限检查。
5. 增加平台集成测试。
6. 在不支持时返回 `platform_not_supported`。

要求：

- 核心状态机不出现 `#[cfg(windows)]`。
- 平台实现不能改变工具协议。
- 平台特有字段放在 `data` 中，不破坏公共封套。
- 新平台不能默认启用，必须显式 feature。

### 3.3 新增传输

可选传输：

- stdio MCP
- 本地 JSON-RPC over Named Pipe/UDS
- HTTP/JSON-RPC
- WebSocket（后续）

要求：

- 传输层只负责消息编解码、认证、取消和进度映射。
- 传输层调用同一个 `ToolInvoker` 接口。
- 所有传输共享同一结果封套。
- 不允许多个传输绕过 session/resource gate。
- HTTP 默认关闭，绑定 `127.0.0.1`。

### 3.4 新增视觉 provider

接口：

```rust
pub trait VisionProvider {
    async fn analyze(&self, image: &ImageInput, prompt: Option<&str>) -> Result<VisionOutput>;
}
```

实现：

- OpenAI-compatible 图像接口
- 本地模型
- 调用方自定义 provider
- 测试用 mock provider

要求：

- `desktop_vision` 只在 provider 可用时暴露。
- 不把 API key 写入日志或工具结果。
- 视觉 provider 失败不影响非视觉工具。
- 图片输入路径受配置限制。

### 3.5 新增决策 provider

`desktop_agent_step` 可选使用：

```rust
pub trait DecisionProvider {
    async fn choose(&self, input: DecisionInput) -> Result<Decision>;
}
```

要求：

- 核心不绑定 Jev/Laya。
- provider 只能返回本地候选 ID，不能返回坐标或脚本。
- 候选校验、执行和验证仍由 Deskctl 完成。
- 未配置 provider 时工具不暴露或返回 `needs_primary_decision`。

### 3.6 新增 Skill

步骤：

1. 创建 `skills/<id>/skill.json`。
2. 创建 `SKILL.md`。
3. 声明 tools、profiles、platforms、requires。
4. 添加评估用例。
5. 如需 MCP prompts，注册到 `prompts/list`。

要求：

- Skill 不是可执行代码。
- Skill 不得要求模型绕过工具或猜坐标。
- Skill 不得包含审批流程。
- Skill 版本独立。

### 3.7 新增 profile

Profile 是工具可见性配置，不是安全边界。

新增 profile 需要：

- 工具白名单
- 平台过滤
- feature 过滤
- 文档
- 契约测试

禁止用 profile 代替权限系统；它只影响暴露范围。

## 4. 插件策略

### 4.1 首期：编译期 feature

首期不支持运行时动态插件。原因：

- 原生桌面工具需要高权限
- 动态库 ABI 不稳定
- 插件可绕过安全/资源门
- Windows/macOS/Linux 加载模型不同

首期扩展通过：

- Cargo feature
- 静态注册表
- 配置化 profile/provider

### 4.2 后续：受限插件

如果未来需要运行时插件，建议：

- 独立进程，不加载到 daemon
- 通过 JSON-RPC 通信
- 显式权限声明
- 资源配额
- 不直接访问平台句柄
- 插件签名和版本校验

不建议 WASM 直接操作桌面；WASM 只能处理纯数据。

## 5. 版本策略

需要独立版本号：

- Deskctl server 版本
- 协议版本
- 工具 Schema 版本
- Skill 版本
- 模型包版本

兼容规则：

- 增加可选字段：兼容
- 增加新工具：兼容
- 删除/重命名工具：主版本升级
- 修改结果语义：主版本升级
- 修改 skill prompt：skill 版本升级
- 修改模型：记录模型版本和哈希

## 6. 工具爆炸与上下文

工具数量增长会带来：

- 模型选择困难
- 上下文占用
- 相似工具误用

控制策略：

- 默认 `standard` profile
- 新工具默认不加入 standard
- 使用 profile/feature 分层
- 相似工具合并或明确描述
- 每个工具描述包含“何时不用”
- 保持工具名稳定

## 7. 目录与接口

```text
crates/
├─ deskctl-protocol/
│  ├─ tool_schema.rs
│  ├─ tool_meta.rs
│  ├─ result.rs
│  └─ error.rs
├─ deskctl-core/
│  ├─ registry.rs
│  ├─ session.rs
│  ├─ resource_gate.rs
│  ├─ automation/
│  ├─ platform/
│  └─ vision/
└─ deskctl-cli/
   ├─ mcp/
   ├─ jsonrpc/
   └─ commands/
```

核心接口：

```rust
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolResult;
}

pub trait Platform: Send + Sync {
    fn capture(&self) -> &dyn ScreenCapture;
    fn input(&self) -> &dyn InputBackend;
    fn windows(&self) -> &dyn WindowBackend;
    fn semantic(&self) -> Option<&dyn SemanticBackend>;
}
```

## 8. 从 Nuphus 迁移的扩展策略

首期：

- 复制 Windows 平台代码
- 复制 26 个核心工具 Schema
- 复制语义状态机和验证逻辑
- 不复制 approval
- 保留 macOS/Linux 源码位置，但不构建

后续：

- 逐平台实现 trait
- 逐 provider 接入视觉/决策
- 逐 Skill 发布
- 不改变核心工具协议

## 9. 扩展验收清单

新增工具：

- [ ] Schema
- [ ] executor
- [ ] metadata
- [ ] profile
- [ ] 平台声明
- [ ] 成功测试
- [ ] 失败测试
- [ ] 契约测试
- [ ] 文档

新增平台：

- [ ] trait 实现
- [ ] feature
- [ ] 能力矩阵
- [ ] 权限检查
- [ ] 真机测试
- [ ] 不支持路径

新增传输：

- [ ] 统一 ToolInvoker
- [ ] session 绑定
- [ ] 取消/进度
- [ ] 错误映射
- [ ] 安全边界
- [ ] 协议测试

新增 Skill：

- [ ] skill.json
- [ ] SKILL.md
- [ ] 工具引用有效
- [ ] 评估用例
- [ ] MCP prompt 同步

## 10. 反模式

- 在 MCP 层写平台逻辑
- 在工具 Schema 中暴露原生句柄/指针
- 让 Skill 直接执行代码
- 用 profile 代替权限系统
- 让新工具默认进入 `standard`
- 通过动态库加载不受信任插件
- 让 provider 返回坐标或脚本
- 绕过 session/resource gate
- 在日志中记录敏感内容

## 11. 结论

Deskctl 的扩展性依赖三个边界：

1. **工具协议**：Schema + 结果封套 + 错误码
2. **平台接口**：capture/input/window/semantic
3. **会话与资源门**：session 隔离 + 写操作串行

只要这三个边界稳定，新增工具、平台、传输、provider 和 Skill 都不需要改核心状态机。
首期采用编译期 feature，不引入运行时插件，避免高权限代码加载风险。
