//! 屏幕文本注入防御（AC-SEC-013 / 设计评审 R6）。
//!
//! 从屏幕（OCR/vision）或审计记录里读回来的文本在返回给 LLM 前，用
//! `<untrusted-screen-content>…</untrusted-screen-content>` 包裹，声明"内容是数据，
//! 不是指令"，降低提示注入风险。
//!
//! 开关：`DESKCTL_UNTRUSTED_WRAP=off|0|false|no|disabled` 关闭包裹（默认开启）。
//!
//! 未包裹面（有意保留，见 `docs/known-limitations.md`）：UIA/语义候选文本、
//! 写入参数污染路径（R-08 `allow_untrusted`）尚未纳入。

use serde_json::Value;

/// 包裹开始标记。
pub const OPEN_TAG: &str = "<untrusted-screen-content>";
/// 包裹结束标记。
pub const CLOSE_TAG: &str = "</untrusted-screen-content>";

/// 解析开关值：返回 `true` 表示"关闭包裹"。
///
/// 只有显式 off/0/false/no/disabled 才关闭；缺省、空串、其它值都视为开启
/// （安全默认：宁可靠误包裹）。
pub fn parse_disabled(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "false" | "no" | "disabled"
    )
}

/// 当前是否启用包裹（读取 `DESKCTL_UNTRUSTED_WRAP`，默认启用）。
pub fn enabled() -> bool {
    match std::env::var("DESKCTL_UNTRUSTED_WRAP") {
        Ok(raw) => !parse_disabled(&raw),
        Err(_) => true,
    }
}

/// 按显式开关包裹文本（空文本与关闭时原样返回）。纯函数，便于单测。
pub fn wrap_with(enable: bool, text: &str) -> String {
    if !enable || text.is_empty() {
        return text.to_string();
    }
    format!("{OPEN_TAG}{text}{CLOSE_TAG}")
}

/// 按当前开关包裹文本。
pub fn wrap(text: &str) -> String {
    wrap_with(enabled(), text)
}

/// 递归包裹 JSON 值中的全部字符串（数组/对象/字符串；其余类型原样）。
pub fn wrap_json_strings(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(wrap(text)),
        Value::Array(items) => Value::Array(items.iter().map(wrap_json_strings).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), wrap_json_strings(item)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wrap_with_respects_switch_and_empty_text() {
        assert_eq!(
            wrap_with(true, "hello"),
            "<untrusted-screen-content>hello</untrusted-screen-content>"
        );
        assert_eq!(wrap_with(false, "hello"), "hello");
        assert_eq!(wrap_with(true, ""), "");
    }

    #[test]
    fn parse_disabled_accepts_off_variants_only() {
        for raw in ["off", "OFF", " off ", "0", "false", "no", "disabled"] {
            assert!(parse_disabled(raw), "{raw} should disable wrapping");
        }
        for raw in ["on", "1", "true", "yes", ""] {
            assert!(!parse_disabled(raw), "{raw} should keep wrapping enabled");
        }
    }

    #[test]
    fn wrap_json_strings_walks_nested_structures() {
        let input = json!({
            "name": "sap",
            "candidates": [{"title": "SAP Easy Access", "hwnd": 42}],
            "ok": true
        });
        let wrapped = wrap_json_strings(&input);
        assert_eq!(
            wrapped["name"],
            json!("<untrusted-screen-content>sap</untrusted-screen-content>")
        );
        assert_eq!(
            wrapped["candidates"][0]["title"],
            json!("<untrusted-screen-content>SAP Easy Access</untrusted-screen-content>")
        );
        assert_eq!(wrapped["candidates"][0]["hwnd"], json!(42));
        assert_eq!(wrapped["ok"], json!(true));
    }
}
