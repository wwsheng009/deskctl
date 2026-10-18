# Deskctl 性能基线

- 状态：已在开发机 release 构建上实测（AC-PERF-001..009）
- 构建：`cargo build --release -p deskctl-cli`
- 复现：`pwsh scripts/perf-baseline.ps1 -Binary <release-deskctl.exe>` → 原始报告 `artifacts/perf-baseline.json`
- 报告口径：p50/p95/max、样本数、硬件、Windows 版本、分辨率、DPI 与 daemon RSS

## 1. 本机 release 实测（2026-10-18）

环境：Windows 11 x64（10.0.26200），16 vCPU，主屏 2240×1400；私有 daemon
（`\\.\pipe\deskctl-perf`，`--max-sessions 8`，release 构建，模型随包加载）。

| AC | 项目 | 样本 | p50 | p95 | max | 门禁 | 结论 |
|---|---|---:|---:|---:|---:|---|---|
| AC-PERF-002 | daemon ready（不加载模型） | 1 | 282.84 ms | — | — | p95 < 2 s | ✅ |
| AC-PERF-001 | CLI `--help`（含进程启动） | 20 | 18.23 ms | 24.13 ms | — | p95 < 200 ms | ✅ |
| AC-PERF-003 | 本地 IPC 往返（`screen_size`） | 50 | 0.39 ms | 0.58 ms | — | p95 < 20 ms | ✅ |
| AC-PERF-004 | 1080p+ 全屏截图（warm） | 20 | 130.93 ms | 149.72 ms | — | p95 < 300 ms | ✅ |
| AC-PERF-005 | 窗口截图（warm，2233×1321） | 20 | 120.61 ms | 137.15 ms | — | p95 < 500 ms | ✅ |
| AC-PERF-006 | OCR warm 感知（640×400 区域） | 3 | 418.50 ms | 443.27 ms | 443.27 ms | 记录值，后续不得回退 >20% | 基线 |
| AC-PERF-007 | 100 次连续调用 RSS 增长 | 100 | — | — | — | warm 后增长 < 10% | ✅ 0.04% |
| AC-PERF-008 | 8 并发 session 截图 | 40 | 322.55 ms | 480.86 ms | 499.64 ms | 无死锁/串扰/饥饿 | ✅ 5 轮 × 8 个唯一 capture_id，0 失败 |
| AC-PERF-009 | 会话关闭资源回落 | — | — | — | — | 30 s 内回到稳定水平 | ✅ 见下 |

daemon RSS：ready 10.1 MB → load 12.0 MB → OCR 常驻后 idle 225.5 MB。

## 2. 说明与已知行为

- `daemon ready` 使用 `session list` 轮询到首个成功响应计时，包含进程启动与 pipe 就绪。
- AC-PERF-005 的目标窗口固定为「前台窗口优先、其次可见非最小化窗口」，报告会记录实际尺寸
  （`window_screenshot_target`）；不同窗口尺寸会直接改变截图耗时，跨机器对比时必须对齐尺寸。
- AC-PERF-007 在 100 次 `desktop_screen_size` 调用前后采样 daemon 工作集，实测增长 0.04%，无持续增长。
- AC-PERF-009 的 idle 值包含常驻的 OCR/YOLO ONNX 会话（约 213 MB）；会话关闭后
  capture 缓存与临时文件立即释放（见 AC-SESS-008 / 会话关闭清理逻辑），模型不卸载，
  因此 idle 高于 ready 属预期。需要模型也卸载的场景不在当前验收范围内。
- AC-PERF-006 是首个记录基线；OCR 文本内容随屏幕变化，跨版本比较应在同一屏幕内容下进行。

## 3. 已修复的性能/并发缺陷（M8 期间发现）

| 缺陷 | 现象 | 修复 |
|---|---|---|
| Named Pipe 并发容量 | 8 并发连接触发 `ERROR_PIPE_BUSY (os error 231)`，每轮唯一 capture_id 仅 2-3 个 | daemon 维护 `max_sessions + 2` 个 pipe 实例，每个实例一个 accept worker；客户端对 231 以 20 ms 退避重试，上限 100 次 |
| `session list` 退出码 | 成功也返回 exit 1，导致脚本误判 daemon 未就绪 | `session` 分支统一返回 0，daemon 级失败由客户端错误传递 |
| 模型目录优先级 | 空的 `%LOCALAPPDATA%\deskctl\models` 会遮蔽随包 `models/`，OCR 报 `model_unavailable` | `models_dir()` 优先选择真正包含模型文件的目录，空目录仅作兜底 |
| 0×0 窗口截图 | 合成渲染目标的窗口返回空图，报 `internal: PNG encode failed` | 空图改报 `target_unavailable`（可重试），bench 自动换用可截图的窗口 |

## 4. 复现与门禁

```powershell
# 完整基线（私有 daemon，不影响默认实例）
pwsh scripts/perf-baseline.ps1 -Binary E:\path\to\release\deskctl.exe

# 只看并发 session（AC-PERF-008）
deskctl bench --op parallel --count 5 --parallel 8 --socket \\.\pipe\deskctl-perf --token-file artifacts\perf-daemon.token
```

`deskctl bench` 是隐藏的开发命令（`--help` 不显示），复用普通 CLI 客户端路径，
因此测量值覆盖「进程 → Named Pipe → daemon → 工具」的完整链路。
