//! 检查点与补偿支持（D4，AC-TXN-001..003）：保存现场、校验差异、导出/读取文档。
//!
//! 检查点存于会话内存（与 capture 同生命周期，daemon 重启即失效），可显式导出为
//! `deskctl-checkpoint/1` 文件留档。单会话上限 8 个，名字 `[a-z0-9._-]{1,32}`。
//! 差异只有四类（design §2）：`value` / `checked` / `exists` / `rect`。

use std::path::PathBuf;

use deskctl_protocol::error::{ErrorCode, ToolError};
use serde_json::{json, Value};

use crate::engine::Engine;
use crate::platform;
use crate::session::Session;
use crate::tools::{self, ToolExecution};

/// 检查点文档格式（data-formats §3）。
pub const CHECKPOINT_FORMAT: &str = "deskctl-checkpoint/1";
/// 单会话检查点数量上限（design §2）。
pub const MAX_CHECKPOINTS: usize = 8;
/// 单个检查点内的目标/关键值上限。
pub const MAX_TARGETS: usize = 16;
pub const MAX_VALUES: usize = 32;
pub const MAX_CAPTURES: usize = 16;

/// 校验检查点名字：`[a-z0-9._-]{1,32}`（design §2）。
pub fn validate_name(name: &str) -> Result<(), ToolError> {
    let ok = (1..=32).contains(&name.chars().count())
        && name.chars().all(|value| {
            value.is_ascii_lowercase() || value.is_ascii_digit() || matches!(value, '.' | '_' | '-')
        });
    if ok {
        Ok(())
    } else {
        Err(tools::invalid(format!(
            "checkpoint name must match [a-z0-9._-]{{1,32}}, got '{name}'"
        )))
    }
}

/// `desktop_checkpoint_save`：采样指定现场并存入会话（同名覆盖），可导出文件。
pub fn save(engine: &Engine, session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
    let name = tools::required_str(args, "name")?.to_string();
    validate_name(&name)?;
    let targets = sample_targets(session, args.get("targets"))?;
    let values = sample_values(session, args.get("values"))?;
    let captures = sample_captures(session, args.get("capture_ids"))?;
    let document = json!({
        "v": 1,
        "format": CHECKPOINT_FORMAT,
        "name": name,
        "created": chrono::Local::now().to_rfc3339(),
        "session_id": session.id,
        "targets": targets,
        "values": values,
        "captures": captures
    });
    session.store_checkpoint(&name, document.clone())?;
    let mut data = json!({ "checkpoint": document, "saved": true });
    if let Some(raw) = args.get("path").and_then(Value::as_str) {
        let path = write_document(engine, raw, &document)?;
        data["path"] = json!(path);
    }
    Ok(ToolExecution::read(data))
}

/// `desktop_checkpoint_verify`：对当前状态重新采样并计算差异。
///
/// 文档来源三选一：会话内 `name`、内联 `checkpoint`、导出文件 `path`。
pub fn verify(
    engine: &Engine,
    session: &Session,
    args: &Value,
) -> Result<ToolExecution, ToolError> {
    let values_only = args
        .get("values_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let document = load_document(engine, session, args)?;
    let expected_values = document
        .get("values")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let expected_targets = document
        .get("targets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut diffs = Vec::new();
    let mut checked = 0usize;
    for entry in &expected_values {
        checked += 1;
        let actual = sample_value(session, entry)?;
        if let Some(diff) = diff_value(entry, &actual) {
            diffs.push(diff);
        }
    }
    if !values_only {
        for entry in &expected_targets {
            checked += 1;
            let current = sample_target_current(entry);
            if let Some(diff) = diff_target(entry, &current) {
                diffs.push(diff);
            }
        }
    }
    Ok(ToolExecution::read(json!({
        "matched": diffs.is_empty(),
        "diffs": diffs,
        "checked": checked,
        "values_only": values_only,
        "name": document.get("name").cloned().unwrap_or(Value::Null)
    })))
}

/// 计算一个关键值条目的差异（无差异返回 `None`）。纯函数，便于单测。
pub fn diff_value(entry: &Value, actual: &Value) -> Option<Value> {
    let name = entry.get("name").and_then(Value::as_str).unwrap_or("value");
    let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("value");
    let expected = entry.get("value").cloned().unwrap_or(Value::Null);
    let got = actual.get("value").cloned().unwrap_or(Value::Null);
    (expected != got).then(|| {
        json!({
            "name": name,
            "kind": kind,
            "expected": expected,
            "actual": got
        })
    })
}

/// 计算一个目标（窗口）条目的差异：仅比较矩形（design §2/§6）。纯函数。
pub fn diff_target(entry: &Value, current: &Value) -> Option<Value> {
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("target");
    let expected = normalize_rect(entry);
    let actual = normalize_rect(current);
    let expected = expected?;
    if actual.as_ref() == Some(&expected) {
        return None;
    }
    Some(json!({
        "name": name,
        "kind": "rect",
        "expected": expected,
        "actual": actual,
        "missing": normalize_rect(current).is_none()
    }))
}

/// 归一化矩形：接受 `{rect:{x,y,width,height}}` 与扁平 `{x,y,width,height}`。
pub fn normalize_rect(value: &Value) -> Option<Value> {
    let source = value.get("rect").filter(|rect| rect.is_object());
    let source = source.unwrap_or(value);
    let x = source.get("x").and_then(Value::as_i64)?;
    let y = source.get("y").and_then(Value::as_i64)?;
    let width = source.get("width").and_then(Value::as_i64)?;
    let height = source.get("height").and_then(Value::as_i64)?;
    Some(json!({ "x": x, "y": y, "width": width, "height": height }))
}

fn sample_targets(session: &Session, raw: Option<&Value>) -> Result<Vec<Value>, ToolError> {
    let Some(value) = raw else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| tools::invalid("targets must be an array"))?;
    if array.len() > MAX_TARGETS {
        return Err(tools::invalid(format!(
            "too many targets (max {MAX_TARGETS})"
        )));
    }
    array
        .iter()
        .map(|item| sample_target(session, item))
        .collect()
}

fn sample_target(session: &Session, item: &Value) -> Result<Value, ToolError> {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| tools::invalid("targets[].name is required"))?;
    let hwnd = resolve_target_hwnd(session, item)?;
    let info = platform::window_info(hwnd)?;
    let rect = normalize_rect(&info).unwrap_or(Value::Null);
    Ok(json!({
        "name": name,
        "hwnd": hwnd,
        "title": info.get("title").cloned().unwrap_or(Value::Null),
        "foreground": platform::foreground_hwnd()? == Some(hwnd),
        "rect": rect
    }))
}

fn resolve_target_hwnd(session: &Session, item: &Value) -> Result<i64, ToolError> {
    if let Some(hwnd) = item.get("hwnd").and_then(Value::as_i64) {
        platform::window_info(hwnd)?;
        return Ok(hwnd);
    }
    if let Some(spec) = item.get("match").filter(|value| value.is_object()) {
        let rows = platform::windows_list()?;
        let matches: Vec<&Value> = rows
            .iter()
            .filter(|row| window_matches(row, spec))
            .collect();
        let label = item.get("name").and_then(Value::as_str).unwrap_or("target");
        return match matches.as_slice() {
            [one] => Ok(one["hwnd"].as_i64().unwrap_or_default()),
            [] => Err(ToolError::new(
                ErrorCode::TargetUnavailable,
                format!("no window matches checkpoint target '{label}'"),
            )
            .retryable(true)),
            _ => Err(ToolError::new(
                ErrorCode::AmbiguousTarget,
                format!("multiple windows match checkpoint target '{label}'; use hwnd"),
            )
            .retryable(true)),
        };
    }
    let name = item.get("name").and_then(Value::as_str).unwrap_or("");
    if !name.is_empty() {
        if let Ok(target) = session.target(name) {
            return Ok(target.hwnd);
        }
    }
    Err(tools::invalid(
        "targets[] needs hwnd, match, or a bound session target with the same name",
    ))
}

fn window_matches(row: &Value, spec: &Value) -> bool {
    let mut checked_any = false;
    if let Some(process_name) = spec.get("process_name").and_then(Value::as_str) {
        checked_any = true;
        let actual = row["process_name"].as_str().unwrap_or_default();
        if !actual
            .trim_end_matches(".exe")
            .eq_ignore_ascii_case(process_name.trim_end_matches(".exe"))
        {
            return false;
        }
    }
    if let Some(class_name) = spec.get("class_name").and_then(Value::as_str) {
        checked_any = true;
        if row["class_name"].as_str() != Some(class_name) {
            return false;
        }
    }
    if let Some(needle) = spec.get("title_contains").and_then(Value::as_str) {
        checked_any = true;
        let haystack = row["title"].as_str().unwrap_or_default().to_lowercase();
        if !haystack.contains(&needle.to_lowercase()) {
            return false;
        }
    }
    checked_any
}

fn sample_values(session: &Session, raw: Option<&Value>) -> Result<Vec<Value>, ToolError> {
    let Some(value) = raw else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| tools::invalid("values must be an array"))?;
    if array.len() > MAX_VALUES {
        return Err(tools::invalid(format!(
            "too many values (max {MAX_VALUES})"
        )));
    }
    let mut out = Vec::new();
    for entry in array {
        let actual = sample_value(session, entry)?;
        out.push(json!({
            "name": actual.get("name").cloned().unwrap_or(Value::Null),
            "locator": entry.get("locator").cloned().unwrap_or(Value::Null),
            "kind": actual.get("kind").cloned().unwrap_or(json!("value")),
            "value": actual.get("value").cloned().unwrap_or(Value::Null)
        }));
    }
    Ok(out)
}

fn sample_value(session: &Session, entry: &Value) -> Result<Value, ToolError> {
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| tools::invalid("values[].name is required"))?;
    let locator = entry
        .get("locator")
        .filter(|value| value.is_object())
        .ok_or_else(|| tools::invalid("values[].locator must be an object"))?;
    let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("value");
    if !matches!(kind, "value" | "checked" | "exists") {
        return Err(tools::invalid(format!(
            "values[].kind must be value/checked/exists, got '{kind}'"
        )));
    }
    let hwnd = tools::semantic::resolve_locator_window_strict(session, locator)?;
    let matched: Vec<Value> = match hwnd {
        Some(hwnd) => platform::observe_window(hwnd, 1000)?
            .into_iter()
            .map(|node| node.value)
            .filter(|node| tools::semantic::locator_matches(node, locator))
            .collect(),
        None => Vec::new(),
    };
    let actual = match kind {
        "exists" => json!(!matched.is_empty()),
        "checked" => matched
            .first()
            .and_then(|node| node["toggled"].as_bool())
            .map_or(Value::Null, Value::Bool),
        _ => match (hwnd, matched.first()) {
            (Some(hwnd), Some(node)) => match node.get("opaque_id").and_then(Value::as_str) {
                Some(opaque) => platform::read_node_value(hwnd, opaque)?.map_or_else(
                    || node.get("name").cloned().unwrap_or(Value::Null),
                    Value::String,
                ),
                None => node.get("name").cloned().unwrap_or(Value::Null),
            },
            (None, Some(node)) => node.get("name").cloned().unwrap_or(Value::Null),
            (_, None) => Value::Null,
        },
    };
    Ok(json!({
        "name": name,
        "kind": kind,
        "value": actual,
        "found": !matched.is_empty()
    }))
}

fn sample_target_current(entry: &Value) -> Value {
    let name = entry.get("name").cloned().unwrap_or(Value::Null);
    let hwnd = entry.get("hwnd").and_then(Value::as_i64);
    let Some(hwnd) = hwnd else {
        return json!({ "name": name, "hwnd": Value::Null, "rect": Value::Null });
    };
    match platform::window_info(hwnd) {
        Ok(info) => json!({
            "name": name,
            "hwnd": hwnd,
            "rect": normalize_rect(&info).unwrap_or(Value::Null)
        }),
        Err(_) => json!({ "name": name, "hwnd": hwnd, "rect": Value::Null }),
    }
}

fn sample_captures(session: &Session, raw: Option<&Value>) -> Result<Vec<Value>, ToolError> {
    let Some(value) = raw else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| tools::invalid("capture_ids must be an array"))?;
    if array.len() > MAX_CAPTURES {
        return Err(tools::invalid(format!(
            "too many capture_ids (max {MAX_CAPTURES})"
        )));
    }
    let mut out = Vec::new();
    for item in array {
        let id = item
            .as_str()
            .ok_or_else(|| tools::invalid("capture_ids must be strings"))?;
        // 校验 capture 仍可用（过期/未知 capture 直接拒绝，避免存下死引用）。
        session.capture(id)?;
        out.push(json!(id));
    }
    Ok(out)
}

fn load_document(engine: &Engine, session: &Session, args: &Value) -> Result<Value, ToolError> {
    let sources = [
        args.get("name").and_then(Value::as_str).is_some(),
        args.get("checkpoint").is_some(),
        args.get("path").and_then(Value::as_str).is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if sources == 0 {
        return Err(tools::invalid(
            "provide one of name / checkpoint / path to verify",
        ));
    }
    if sources > 1 {
        return Err(tools::invalid(
            "name / checkpoint / path are mutually exclusive",
        ));
    }
    let document = if let Some(name) = args.get("name").and_then(Value::as_str) {
        session.checkpoint(name)?
    } else if let Some(inline) = args.get("checkpoint") {
        inline.clone()
    } else {
        let raw = args.get("path").and_then(Value::as_str).unwrap_or_default();
        let path = normalize_input(engine, raw)?;
        let content = std::fs::read_to_string(&path)
            .map_err(|error| tools::invalid(format!("read {}: {error}", path.display())))?;
        serde_json::from_str(&content)
            .map_err(|error| tools::invalid(format!("parse {}: {error}", path.display())))?
    };
    validate_document(&document)?;
    Ok(document)
}

/// 校验 checkpoint 文档的基本形态（v/format/数组字段）。
pub fn validate_document(document: &Value) -> Result<(), ToolError> {
    let object = document
        .as_object()
        .ok_or_else(|| tools::invalid("checkpoint must be a JSON object"))?;
    let format = object.get("format").and_then(Value::as_str);
    let version = object.get("v").and_then(Value::as_i64);
    if format != Some(CHECKPOINT_FORMAT) || version != Some(1) {
        return Err(tools::invalid(format!(
            "unsupported checkpoint format (expected {CHECKPOINT_FORMAT} with v=1)"
        )));
    }
    for key in ["targets", "values", "captures"] {
        if let Some(value) = object.get(key) {
            if !value.is_array() && !value.is_null() {
                return Err(tools::invalid(format!("checkpoint.{key} must be an array")));
            }
        }
    }
    Ok(())
}

fn normalize_input(engine: &Engine, raw: &str) -> Result<PathBuf, ToolError> {
    tools::normalize_image_path(engine, raw)
}

fn write_document(engine: &Engine, raw: &str, document: &Value) -> Result<String, ToolError> {
    let path = tools::normalize_image_path(engine, raw)?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|error| {
                ToolError::new(
                    ErrorCode::Internal,
                    format!("create {}: {error}", parent.display()),
                )
            })?;
        }
    }
    let content = serde_json::to_string_pretty(document)
        .map_err(|error| ToolError::new(ErrorCode::Internal, error.to_string()))?;
    std::fs::write(&path, format!("{content}\n")).map_err(|error| {
        ToolError::new(
            ErrorCode::Internal,
            format!("write {}: {error}", path.display()),
        )
    })?;
    Ok(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value_entry(kind: &str, value: Value) -> Value {
        json!({ "name": "field", "kind": kind, "value": value })
    }

    #[test]
    fn name_validation_accepts_and_rejects() {
        for good in ["a", "before-commit", "sap.st06_v2", "0123456789"] {
            assert!(validate_name(good).is_ok(), "{good}");
        }
        for bad in ["", "Upper", "with space", "中文", "a/b"] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
        let long = "a".repeat(33);
        assert!(validate_name(&long).is_err());
    }

    #[test]
    fn diff_covers_value_checked_exists_and_rect() {
        // value: 文本差异
        let diff = diff_value(
            &value_entry("value", json!("1000.00")),
            &json!({ "value": "1200.00" }),
        )
        .expect("value diff");
        assert_eq!(diff["kind"], "value");
        assert_eq!(diff["expected"], "1000.00");
        assert_eq!(diff["actual"], "1200.00");

        // checked: 布尔差异
        let diff = diff_value(
            &value_entry("checked", json!(true)),
            &json!({ "value": false }),
        )
        .expect("checked diff");
        assert_eq!(diff["kind"], "checked");

        // exists: 存在性差异
        let diff = diff_value(
            &value_entry("exists", json!(true)),
            &json!({ "value": false }),
        )
        .expect("exists diff");
        assert_eq!(diff["kind"], "exists");

        // 无差异 → None
        assert!(diff_value(
            &value_entry("value", json!("same")),
            &json!({ "value": "same" })
        )
        .is_none());

        // rect: 窗口矩形差异（嵌套 rect）
        let expected = json!({
            "name": "sap",
            "hwnd": 42,
            "rect": { "x": 65, "y": 99, "width": 2175, "height": 1245 }
        });
        let current = json!({
            "name": "sap",
            "hwnd": 42,
            "rect": { "x": 0, "y": 0, "width": 800, "height": 600 }
        });
        let diff = diff_target(&expected, &current).expect("rect diff");
        assert_eq!(diff["kind"], "rect");
        assert_eq!(diff["missing"], false);

        // 窗口关闭 → rect 差异且 missing=true
        let gone = json!({ "name": "sap", "hwnd": 42, "rect": Value::Null });
        let diff = diff_target(&expected, &gone).expect("missing diff");
        assert_eq!(diff["missing"], true);
        assert_eq!(diff["actual"], Value::Null);

        // 完全一致 → None
        assert!(diff_target(&expected, &expected).is_none());
    }

    #[test]
    fn document_validation_enforces_format() {
        let good = json!({
            "v": 1,
            "format": CHECKPOINT_FORMAT,
            "name": "x",
            "targets": [],
            "values": [],
            "captures": []
        });
        assert!(validate_document(&good).is_ok());
        let bad_version = json!({ "v": 2, "format": CHECKPOINT_FORMAT });
        assert!(validate_document(&bad_version).is_err());
        let bad_format = json!({ "v": 1, "format": "deskctl-checkpoint/2" });
        assert!(validate_document(&bad_format).is_err());
        let bad_field = json!({ "v": 1, "format": CHECKPOINT_FORMAT, "values": {} });
        assert!(validate_document(&bad_field).is_err());
    }

    #[test]
    fn rect_normalization_accepts_nested_and_flat() {
        let nested = json!({ "rect": { "x": 1, "y": 2, "width": 3, "height": 4 } });
        let flat = json!({ "x": 1, "y": 2, "width": 3, "height": 4 });
        assert_eq!(normalize_rect(&nested), normalize_rect(&flat));
        assert!(normalize_rect(&json!({ "x": 1 })).is_none());
    }
}
