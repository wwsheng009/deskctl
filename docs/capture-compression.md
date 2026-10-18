# 截图压缩与传输策略

- 文档版本：0.1
- 日期：2026-10-18
- 适用：`desktop_screenshot`、`desktop_window_screenshot`
- 代码：`crates/deskctl-core/src/capture_policy.rs`

## 1. 设计目标

截图是桌面工具中最大的数据对象。必须同时满足：

1. 默认输出可逆、清晰（PNG）
2. 模型需要时可选择有损压缩（JPEG/WebP）
3. 支持按尺寸和字节上限压缩
4. 压缩不能破坏 `capture_id` 与 `desktop_perceive` 的坐标映射
5. 压缩参数可被 LLM 通过工具参数控制
6. 服务端可设置全局默认值和硬上限

## 2. 输出格式

| 格式 | 编码 | 默认质量 | 适用 |
|---|---|---|---|
| PNG | 无损 | 不适用 | 默认，文字清晰，可逆 |
| JPEG | 有损 | 85 | 快速传输、视觉模型 |
| WebP | 无损（image 0.25） | 不适用 | 比 PNG 更小的无损输出 |

> `image 0.25` 只提供无损 WebP 编码。`quality` 参数对 WebP 不生效，返回结果中 `quality=0` 表示“不适用”。如需有损 WebP，需要额外引入 `libwebp`。

## 3. 工具参数

`desktop_screenshot` / `desktop_window_screenshot` 新增：

| 参数 | 类型 | 说明 |
|---|---|---|
| `format` | `png` / `jpeg` / `webp` | 输出格式；缺省使用服务端默认 |
| `quality` | 1–100 | JPEG 质量；PNG/WebP 忽略 |
| `max_width` | 1–16384 | 最大宽度，超出则等比缩小 |
| `max_height` | 1–16384 | 最大高度，超出则等比缩小 |
| `max_bytes` | 1024–64MB | 硬性文件大小上限 |

优先级：

1. 显式 `format` 参数
2. 输出路径扩展名（`.png` / `.jpg` / `.jpeg` / `.webp`）
3. 服务端默认格式

如果路径没有可识别的图片扩展名，自动补所选格式的扩展名（缺省为 `.png`）。
`.jpeg` 输入会规范化为 `.jpg` 输出。

## 4. 压缩算法

### 4.1 PNG

- 使用 `PngEncoder::new_with_quality`
- 压缩级别：
  - 0–2 → `Fast`
  - 3–6 → `Default`
  - 7–9 → `Best`
- 滤波器：`Adaptive`
- 适合文字和 UI 截图

### 4.2 JPEG

- 使用 `JpegEncoder::new_with_quality`
- 先转为 RGB
- 适合视觉模型和大面积渐变
- 文字边缘会有振铃，不建议用于 OCR

### 4.3 WebP

- 当前为无损编码
- 文件通常小于 PNG
- 不适用于需要 JPEG 级有损压缩的场景

### 4.4 尺寸缩放

使用 `Lanczos3`：

```text
scale = min(1, max_width / width, max_height / height)
new_width  = round(width  * scale)
new_height = round(height * scale)
```

- 只缩小，不放大
- 至少保留 1px
- 保持宽高比
- 返回 `resized: true` 和源尺寸

### 4.5 字节上限

`max_bytes` 是有界压缩循环，最多 6 次：

- JPEG：先降低质量（每次 -15，最低 40），再缩小尺寸（每次 ×0.75）
- PNG/WebP：直接缩小尺寸（每次减半）
- 仍超限时返回 `invalid_params`，附带实际字节数
- 不会无限循环

## 5. 服务端默认值

环境变量：

| 变量 | 默认 | 说明 |
|---|---|---|
| `DESKCTL_CAPTURE_FORMAT` | `png` | 默认格式 |
| `DESKCTL_CAPTURE_QUALITY` | `85` | JPEG 质量 |
| `DESKCTL_CAPTURE_PNG_COMPRESSION` | `6` | PNG 压缩级别 |
| `DESKCTL_CAPTURE_MAX_WIDTH` | 无 | 全局最大宽度 |
| `DESKCTL_CAPTURE_MAX_HEIGHT` | 无 | 全局最大高度 |
| `DESKCTL_CAPTURE_MAX_BYTES` | 无 | 全局字节上限 |

工具参数可覆盖这些默认值，但 `max_bytes` 的硬上限仍由服务端校验范围。

## 6. 返回字段

```json
{
  "path": "...",
  "format": "jpeg",
  "width": 1280,
  "height": 800,
  "source_width": 1600,
  "source_height": 1000,
  "bytes": 102223,
  "resized": true,
  "quality": 60
}
```

调用方可以据此判断：

- 是否发生了缩放
- 实际格式
- 实际质量
- 实际文件大小

## 7. 与 capture_id 的关系

`capture_id` 仍然关联原始截图几何：

```text
capture_id -> screen_rect (原始屏幕坐标)
           -> elements (perceive 生成的元素坐标)
           -> path (压缩后的文件)
```

缩放只影响输出图片，不影响 `screen_rect`。因此：

- `desktop_mouse` 使用 `capture_id + element_id` 仍然正确
- `desktop_perceive` 返回的 `screen_center` 仍然基于屏幕坐标
- 压缩后的图片只用于传输或视觉模型，不用于坐标换算

## 8. 已知限制

- WebP 目前只有无损编码
- JPEG 不适合 OCR 精确识别
- `max_bytes` 触发缩放后，元素坐标仍按原图计算
- 压缩不改变 `capture_id` 的 TTL 和缓存策略
- 多显示器截图仍以主屏为基准

## 9. 验证

单元测试：

```text
cargo test -p deskctl-core capture_policy
```

真机对比（1600×1000）：

| 输出 | 格式 | 尺寸 | 字节 |
|---|---|---|---|
| full.png | PNG | 1600×1000 | 121249 |
| compressed.jpg | JPEG q60 | 1280×800 | 102223 |
| tiny.webp | WebP 无损 | 640×400 | 130446 |

结论：

- JPEG 在缩放后显著降低字节数
- WebP 无损在小尺寸下不一定优于 JPEG
- 压缩参数生效，返回字段与实际文件一致

## 10. 后续优化

- 接入 `libwebp` 实现有损 WebP
- 增加 `dpi_scale` 参数
- 增加多显示器选择
- 增加截图内容哈希去重
- 增加按会话的临时文件自动清理
- 对 OCR 输入固定使用 PNG，避免有损压缩影响识别
