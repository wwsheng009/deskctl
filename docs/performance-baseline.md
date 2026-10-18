# Deskctl 性能基线

- 状态：待固定参考机器测量
- 构建：`cargo build --release -p deskctl-cli`
- 测量要求：p50/p95/p99、最大值、样本数、硬件、Windows 版本、分辨率、DPI

## 本机 release smoke

当前开发机：Windows 11 x64，2240×1400 主屏，release 构建。
这些数字是功能 smoke，不是固定参考机器上的正式发布基线。

| 项目 | 样本 | p50 | p95 | max |
|---|---:|---:|---:|---:|
| CLI `--help`（含进程启动） | 10 | 约 30 ms | 约 55 ms | 55 ms |
| `tool desktop_screenshot` | 20 | 174 ms | 216 ms | 229 ms |
| `tool desktop_windows_list` | 10 | 34 ms | 67 ms | 67 ms |
| `call desktop_screen_size`（含 CLI 启动） | 20 | 27 ms | 45 ms | 45 ms |

OCR 感知（2240×1400，debug）约 2-4 s，冷启动含模型加载；UIA 观察（Notepad）< 300 ms。

## 发布前必须记录

- AC-PERF-001 CLI help 启动
- AC-PERF-002 daemon ready
- AC-PERF-003 IPC 往返
- AC-PERF-004 全屏截图 warm
- AC-PERF-005 窗口截图 warm
- AC-PERF-006 OCR warm
- AC-PERF-007 100 次调用内存
- AC-PERF-008 8 session 并发
- AC-PERF-009 会话关闭资源回落
- AC-PERF-010 模型加载进度

