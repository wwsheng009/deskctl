# Deskctl 文档索引

- 项目：`E:\projects\ai\deskctl`
- 状态：Windows 首个可运行版本已实现
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
| [implementation-status.md](implementation-status.md) | 当前实施进度、已验证能力与剩余缺口 |
| [capture-compression.md](capture-compression.md) | 截图格式、压缩、缩放与字节上限策略 |
| [known-limitations.md](known-limitations.md) | 已知限制与发布前注意事项 |
| [performance-baseline.md](performance-baseline.md) | 性能基线与待测项 |
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
| 实施状态 | implementation-status | 已覆盖 |
| 截图压缩 | capture-compression | 已覆盖 |
| MCP 服务 | mcp-service | 已覆盖 |
| LLM Skills | llm-skills | 已覆盖 |
| 可扩展性 | extensibility | 已覆盖 |
| 安全边界 | architecture、acceptance、test-plan | 已覆盖 |
| 发布/安装包 | architecture、acceptance、test-plan | 已覆盖 |

## 尚未完成的部分

Windows 首期功能已落地，仍待发布流水线完成：

- 干净 Windows 机器安装与升级/卸载验证
- 代码签名
- 固定参考机器上的性能基线
- Windows 10 兼容性回归
- 受保护窗口、多 DPI、多显示器矩阵
- 模型与运行时的最终许可证/法务签署

## 文档维护规则

- 架构变更先更新 `architecture.md` 和 `architecture-review.md`。
- 验收项变更同步 `acceptance-criteria.md` 和 `test-plan.md`。
- MCP 协议变更同步 `mcp-service.md`。
- Skill 变更同步 `llm-skills.md` 和 `skills/`。
- 工具新增/删除同步 `extensibility.md`、工具 profile 和契约测试。
- 文档版本与协议版本独立管理。
