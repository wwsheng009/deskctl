# 实施状态

- 更新日期：2026-10-18
- 对应计划：[implementation-plan.md](implementation-plan.md)
- 当前版本：0.1.0（Windows 首个可运行版本）

## 1. 总体状态

| 里程碑 | 状态 | 说明 |
|---|---|---|
| M0 文档冻结 | 完成 | 架构、验收、测试、MCP、skills 文档齐备 |
| M1 Workspace 与协议骨架 | 完成 | 3 个 crate，统一结果封套，CLI/doctor/schema |
| M2 Windows 无状态工具 | 完成 | 截图、窗口、鼠标、键盘、剪贴板、找图/找色/找字、等待 |
| M3 Daemon 与多 session | 完成 | Named Pipe daemon、session 管理、资源门 |
| M4 MCP 服务 | 完成 | initialize / tools/list / tools/call / prompts |
| M5 视觉与模型 | 完成 | PaddleOCR 与 YOLO 均已接入并真机验证 |
| M6 Windows UIA 语义工具 | 部分完成 | 观察/候选/执行/验证、扩展动作（set_checked/scroll/scroll_into_view/set_range_value）、内联验证与稳定采样、树分页、catalog 受限启动均可用；agent_step 未实现 |
| M7 Skills 与 profiles | 完成 | 5 个 skills、profile 过滤、契约导出 |
| M8 打包与发布 | 部分完成 | 运行时/模型获取、哈希校验、安装脚本与 Inno Setup 模板完成；性能基线与安全/会话矩阵（12/12，含输入串行与公平性）已实测（`scripts/perf-baseline.ps1`、`scripts/security-check.ps1`）；干净机安装、代码签名与 Windows 10 回归待发布流水线 |

## 2. 已验证能力

以下能力已在 Windows 真机通过：

- `deskctl doctor` 返回平台、模型、DLL 和路径信息
- `deskctl tool desktop_screen_size`
- `deskctl tool desktop_windows_list`
- `deskctl tool desktop_mouse --json '{"action":"position"}'`
- `deskctl tool desktop_screenshot`（含自定义路径、区域截图、扩展名强制 PNG）
- `deskctl tool desktop_perceive`（PaddleOCR 文本元素识别）
- `deskctl tool desktop_perceive`（YOLO 图标元素识别）
- `deskctl tool desktop_find_text`（PaddleOCR 文本匹配）
- `deskctl serve` + Named Pipe
- `deskctl session create/list`
- `deskctl call <session> desktop_screenshot`
- `deskctl mcp`：initialize、tools/list、tools/call、prompts/list
- `desktop_targets_list`、`desktop_target_bind`
- `desktop_semantic_observe`（真实 UIA 树与候选动作）
- 多 session 并发截图与状态隔离
- 8 并发 session 截图（pipe 实例池 + 客户端 231 重试，0 失败、每轮 8 个唯一 capture_id）
- `scripts/perf-baseline.ps1`：AC-PERF-001..009 本机 release 实测（见 [performance-baseline.md](performance-baseline.md)）
- `scripts/security-check.ps1`：AC-SEC-001/002/003/004/005/006/009/010、AC-SESS-003/005/007/009 全部通过（12/12）
- `deskctl call` / `session inspect|close` 强制 session token：缺失返回 `auth_required`，CLI 退出码 7；错误 token 同样拒绝
- daemon 级错误按 `code: message` 前缀映射退出码（invalid_params/unknown_tool=2、stale/server_restarted=4、resource_busy=5、platform_not_supported=6、policy/permission/auth=7），见 AC-CLI-007
- `desktop_screenshot --json '{"path":"x.jpg","format":"png"}'` 显式 format 覆盖扩展名，写出 `x.png`；省略 format 时保持 `.jpg` → jpeg
- `desktop_semantic_execute` → `desktop_verify_state value_equals` 真机闭环：目标处于非前台时激活成功后写入并 `satisfied=true`
- `desktop_window_activate` 在后台 daemon 中可置前目标窗口（attach 前台/目标输入队列 + `SetActiveWindow` + `SwitchToThisWindow` 兜底）
- DPI 缩放坐标统一（Per-Monitor-V2，`doctor` 报告 dpi 感知与系统 DPI）：150% 缩放下以
  物理像素坐标直接双击桌面图标命中；窗口枚举/截图/UIA 与鼠标共用同一坐标系
- 语义扩展动作（AC-SEM-013）：`set_checked` 幂等实测（WebView2 开关 toggled=false 时不点击）；
  `scroll`（document，direction/amount）派发成功；`scroll_into_view`/`set_range_value` 已实现
- 内联验证与提交策略（AC-SEM-014/015）：`completion_policy=verified` + `expectation`
  （`expected` 双向语义）实测 satisfied；应用未反映变更时返回 unsatisfied 不冒充成功；
  `stable_samples=2` 连续采样生效；超时未达稳定次数返回 unknown
- 树分页（AC-SEM-016）：Clash Verge 首页 `tree_count=236`；第一页 200 节点 +
  `next_tree_cursor=t:200`，第二页 36 节点、同一 `observation_token`、两页无重叠
- 受限启动（AC-SEM-017）：`desktop_targets_list` 查询命中本地 catalog（`app:x:<hash>`，
  仅 Start Menu/桌面 `.lnk`）；`desktop_target_bind` 对未运行的 WezTerm 启动（pid/hwnd）
  并完成绑定，Alt+F4 关闭验证；`desktop_input`/`desktop_semantic_action` 的 `launch_ref` 就位
- 投递模式（AC-SEM-018）：`delivery_mode=auto` 实测后台优先、失败自动回退前台且执行成功；
  provider 报 0x00000000（成功却报错）时不重复派发

## 3. 测试状态

`cargo test --workspace` 通过：

```text
deskctl-cli:      13 passed（cli_contract 3、mcp_smoke 3、skills_contract 1、tool_contract 6）
deskctl-core:     24 passed（lib 18、path_policy 2、session_isolation 4）
deskctl-protocol:  3 passed
合计:             40 passed, 0 failed
```

`cargo fmt --all -- --check` 与 `cargo clippy --workspace --all-targets -- -D warnings` 均无告警
（clippy 增量缓存偶尔导致 clippy-driver 崩溃时，可加 `CARGO_INCREMENTAL=0` 重跑）。

覆盖：

- 28 工具 Schema 合法性
- 26/28 profile 工具数量
- readonly 不暴露输入工具
- 无 approve/reject/approval 工具
- skills manifest 与工具名一致性
- CLI help、doctor、schema 导出
- MCP initialize/list/call
- 截图路径强制 PNG
- 颜色解析、图像匹配、按键校验
- OCR 模型缺失时安全报错
- dict_name 路径穿越拒绝（path_policy）
- session 隔离、关闭后 token 失效、会话上限 resource_busy（session_isolation）
- MCP 未初始化拒绝 tools/list、协议版本校验、notifications/cancelled 取消等待
- 扩展动作/树游标/delivery 解析（tools::semantic 单测）
- 应用 catalog：仅收 .lnk、`app_ref` 稳定且不可伪造、未知 ref 拒绝、扫描上限（apps 单测）

## 4. 运行时资源

已独立管理，不再依赖 Nuphus：

```text
third_party/onnxruntime/win-x64/
├─ onnxruntime.dll
├─ onnxruntime_providers_shared.dll
├─ hashes.json
└─ README.md

models/
├─ ch_PP-OCRv4_det.onnx
├─ ch_PP-OCRv4_rec.onnx
├─ ch_PP-OCR_keys_v1.txt
├─ icon_detect.onnx
└─ hashes.json
```

脚本：

```powershell
pwsh scripts/fetch-runtime.ps1 -VerifyOnly
pwsh scripts/fetch-runtime.ps1
pwsh scripts/fetch-models.ps1 -FromDirectory <dir>
pwsh scripts/fetch-models.ps1
```

构建脚本 `crates/deskctl-cli/build.rs` 会自动复制：

- `onnxruntime*.dll` → `target/<profile>/` 和 `deps/`
- `models/*` → `target/<profile>/models/`

模型和 DLL 不提交 Git，由脚本获取。

## 5. 已知缺口

### 高优先级

- `desktop_vision` 需要外部视觉 provider，当前返回 `model_unavailable`
- `desktop_agent_step` 需要增强决策 provider，当前返回 `model_unavailable`
- `desktop_find_image` 的模板匹配为简化实现，超大模板或强缩放场景精度有限

### 中优先级

- 安装包脚本已提供；干净 Windows 机器安装与签名未执行
- Windows 10 未做真机回归
- 性能基线为单机、单屏幕内容的一次测量；固定参考机器与跨版本回退对比未建立
- 取消已覆盖等待类工具；其他长任务的协作式取消仍需逐一补齐
- session 空闲回收：daemon 每 30 秒扫描一次，另在新会话创建时触发

### 低优先级

- macOS / Linux 仅保留 trait 与占位
- HTTP 适配层未实现
- 在线模型更新未实现

## 6. 与验收标准的差距

已满足：

- AC-SCOPE
- AC-CAP-001/002/003/004/005/006
- AC-WIN-001/002/003/004
- AC-MOUSE-001/002/004/005
- AC-CLIP
- AC-SEM-001/002/003/004/005/007/008/009/010/011
- AC-SEM-006（真机 smoke：保存 locator 后 `desktop_semantic_action` 重放）
- AC-SEM-013/014/015/016/017/018（真机 smoke：幂等 set_checked、expected 双向语义+稳定采样+
  unsatisfied 不冒充成功、tree_cursor 两页续读无重叠、catalog 受限启动并绑定/关闭、
  delivery_mode=auto 后台优先+前台回退）
- AC-PER-003（OCR/YOLO 合并去重 smoke）
- AC-SESS-001/002/004/005/006
- AC-SESS-003/009（安全脚本实测：8 并发输入串行、0 失败、p50=708ms/max=1129ms 无饥饿）
- AC-SESS-007（安全脚本实测：会话上限 resource_busy）
- AC-SESS-008（真机实测：`%TEMP%` 下的会话截图随关闭删除、capture cache 清空；默认 captures 目录为持久目录，保留）
- AC-MCP-001/002/003/004/005/010/012
- AC-CLI-001/002/003/005/007/008/010（007：退出码 2/4/5/7 已真机实测；6 仅在非 Windows 平台出现）
- AC-ERR-001/002/004/005/007/009
- AC-PKG-002/003/007
- AC-DOC
- AC-PERF-001/002/003/004/005/007/008/009（本机 release 实测，见 [performance-baseline.md](performance-baseline.md)）
- AC-SEC-001/002/003/004/005/006/009/010（安全脚本 12/12：本地监听、token 校验、无审批字段、风险元数据、自定义路径、字库穿越、会话隔离、无脚本执行）

未满足或未验证：

- AC-SEM-012（agent_step）
- AC-PERF-006（已有 warm 基线记录，跨版本回退对比未建立）
- AC-SEC-007/008（读取限制/日志脱敏未逐条实测）
- AC-PKG-001/004/005/006
- AC-EXT-*
- Windows 10 兼容性

## 7. 结论

当前版本已经是可运行的 Windows 桌面工具服务：

- 可独立构建
- 不依赖 Nuphus
- 不包含审批流程
- 支持多 session
- 支持 MCP
- 支持 OCR 和 UIA

但尚未达到 `acceptance-criteria.md` 的完整发布门禁。性能与安全/会话矩阵已完成本机实测；
主要剩余工作是干净机安装、代码签名、固定参考机器基线、Windows 10 回归与 AC-EXT-*。
