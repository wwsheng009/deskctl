# Deskctl 文档索引

- 项目：`E:\projects\ai\deskctl`
- 状态：设计阶段
- 日期：2026-10-18

## 文档列表

| 文档 | 作用 |
|---|---|
| [README.md](../README.md) | 项目定位、范围、核心决策 |
| [feasibility-analysis.md](feasibility-analysis.md) | 可行性、风险、实现路径 |
| [architecture.md](architecture.md) | 总体架构、进程、协议、状态、平台、打包 |
| [architecture-review.md](architecture-review.md) | 架构评审、决策状态、残余风险 |
| [acceptance-criteria.md](acceptance-criteria.md) | 可执行的验收标准和发布门禁 |
| [test-plan.md](test-plan.md) | 单元、契约、集成、E2E、性能、安全测试 |
| [implementation-plan.md](implementation-plan.md) | 里程碑、任务分解、代码抽取、首个迭代 |
| [mcp-service.md](mcp-service.md) | MCP server、tools、prompts、取消、进度、错误映射 |
| [llm-skills.md](llm-skills.md) | LLM Skill 定义、工具选择、失败恢复、评估 |
| [extensibility.md](extensibility.md) | 新工具/平台/传输/provider/skill 的扩展机制 |

## 覆盖矩阵

| 关注点 | 覆盖文档 | 状态 |
|---|---|---|
| 项目范围与非目标 | README、architecture | 已覆盖 |
| 可行性 | feasibility-analysis | 已覆盖 |
| 进程与 daemon | architecture | 已覆盖 |
| 多 session 并发 | architecture、acceptance、test-plan | 已覆盖 |
| 自定义截图路径 | architecture、acceptance、test-plan | 已覆盖 |
| 模型随包分发 | architecture、feasibility、acceptance | 已覆盖 |
| 无审批边界 | architecture、feasibility、review | 已覆盖 |
| 验收标准 | acceptance-criteria | 已覆盖 |
| 功能测试 | test-plan | 已覆盖 |
| 实施计划 | implementation-plan | 已覆盖 |
| MCP 服务 | mcp-service | 已覆盖 |
| LLM Skills | llm-skills | 已覆盖 |
| 可扩展性 | extensibility | 已覆盖 |
| 安全边界 | architecture、acceptance、test-plan | 已覆盖 |
| 发布/安装包 | architecture、acceptance、test-plan | 已覆盖 |

## 尚未完成的部分

当前完成的是设计与验收方案，以下内容还未进入代码：

- Cargo workspace 和三个 crate 的骨架
- 28 个工具的逐项 JSON Schema 文件
- `skills/` 下的实际 `skill.json` / `SKILL.md`
- 从 Nuphus 抽取的 Windows 平台代码
- daemon/session/资源门实现
- MCP stdio server 实现
- CLI 命令实现
- 模型随包分发的构建脚本
- PaddleOCR/YOLO/ONNX Runtime 的许可证审查
- MCP 版本兼容矩阵
- 无审批模式的威胁模型
- 安装包和签名
- Windows 真机测试结果

## 文档维护规则

- 架构变更先更新 `architecture.md` 和 `architecture-review.md`。
- 验收项变更同步 `acceptance-criteria.md` 和 `test-plan.md`。
- MCP 协议变更同步 `mcp-service.md`。
- Skill 变更同步 `llm-skills.md` 和 `skills/`。
- 工具新增/删除同步 `extensibility.md`、工具 profile 和契约测试。
- 文档版本与协议版本独立管理。
