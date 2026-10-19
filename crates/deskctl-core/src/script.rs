//! D3 回放脚本（`deskctl-script/1`）：模型、静态校验、变量解析、目标解析与
//! 指纹、preflight、dry-run、逐步执行、run report / outcome / handoff。
//!
//! 设计依据：[record-replay.md §4](../../../docs/design/record-replay.md)，
//! 数据格式：[data-formats.md §2](../../../docs/design/data-formats.md)。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use deskctl_protocol::{
    DispatchState, Effect, ErrorCode, RiskClass, ToolError, ToolName, ToolResult, Verification,
};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::audit::AuditSource;
use crate::engine::{CancelFlag, Engine};
use crate::platform;
use crate::session::{Session, TargetBinding};
use crate::tools::ToolExecution;

pub const SCRIPT_FORMAT: &str = "deskctl-script/1";
/// 重放报告格式（data-formats §2.5）。
pub const RUN_REPORT_FORMAT: &str = "deskctl-script-run/1";
pub const MAX_STEPS: usize = 500;
pub const DEFAULT_RISK_MAX: &str = "bounded_write";
/// 一次 run 内允许的目标重绑预算（§4.4.4：最多 5 次，防"重启-失败"循环）。
const REBIND_BUDGET: u32 = 5;

/// 解析后的脚本步骤；录制器与脚本文件共用同一模型。
#[derive(Debug, Clone)]
pub struct ScriptStep {
    pub id: String,
    pub tool: String,
    pub params: Value,
    pub expect: Option<Value>,
    pub set: Option<Value>,
    pub verify: Option<Value>,
    pub require_readback: Option<bool>,
    pub optional: Option<bool>,
    pub fragile: Option<bool>,
    pub volatile: Option<bool>,
    pub retry: Option<u32>,
    pub note: Option<String>,
}

impl ScriptStep {
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("id".to_string(), json!(self.id));
        map.insert("tool".to_string(), json!(self.tool));
        map.insert("params".to_string(), self.params.clone());
        if let Some(value) = &self.expect {
            map.insert("expect".to_string(), value.clone());
        }
        if let Some(value) = &self.set {
            map.insert("set".to_string(), value.clone());
        }
        if let Some(value) = &self.verify {
            map.insert("verify".to_string(), value.clone());
        }
        if let Some(value) = self.require_readback {
            map.insert("require_readback".to_string(), json!(value));
        }
        if let Some(value) = self.optional {
            map.insert("optional".to_string(), json!(value));
        }
        if let Some(value) = self.fragile {
            map.insert("fragile".to_string(), json!(value));
        }
        if let Some(value) = self.volatile {
            map.insert("volatile".to_string(), json!(value));
        }
        if let Some(value) = self.retry {
            map.insert("retry".to_string(), json!(value));
        }
        if let Some(value) = &self.note {
            map.insert("note".to_string(), json!(value));
        }
        Value::Object(map)
    }

    fn from_value(value: &Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "step must be an object".to_string())?;
        let id = require_string(object, "id")?;
        if id.is_empty() || id.len() > 64 || id.contains(':') {
            return Err(format!("step id '{id}' must be 1..64 chars without ':'"));
        }
        let tool = require_string(object, "tool")?;
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
        if !params.is_object() {
            return Err(format!("step {id}: params must be an object"));
        }
        let optional_object = |key: &str| -> Result<Option<Value>, String> {
            match object.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(value @ Value::Object(_)) => Ok(Some(value.clone())),
                Some(_) => Err(format!("step {id}: {key} must be an object")),
            }
        };
        let optional_bool = |key: &str| -> Result<Option<bool>, String> {
            match object.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Bool(value)) => Ok(Some(*value)),
                Some(_) => Err(format!("step {id}: {key} must be a boolean")),
            }
        };
        let retry = match object.get("retry") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let retry = value
                    .as_u64()
                    .ok_or_else(|| format!("step {id}: retry must be an integer 0..3"))?;
                if retry > 3 {
                    return Err(format!("step {id}: retry must be within 0..3"));
                }
                Some(retry as u32)
            }
        };
        let verify = optional_object("verify")?;
        if let Some(verify) = &verify {
            let tier = verify
                .get("tier")
                .and_then(Value::as_str)
                .unwrap_or("screen");
            if !matches!(tier, "screen" | "readback") {
                return Err(format!("step {id}: verify.tier must be screen or readback"));
            }
        }
        let note = match object.get("note") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) => Some(value.clone()),
            Some(_) => return Err(format!("step {id}: note must be a string")),
        };
        // 先完成全部闭包求值，再移动 `id`；避免借用与移动冲突。
        let expect = optional_object("expect")?;
        let set = optional_object("set")?;
        let require_readback = optional_bool("require_readback")?;
        let optional = optional_bool("optional")?;
        let fragile = optional_bool("fragile")?;
        let volatile = optional_bool("volatile")?;
        Ok(Self {
            id,
            tool,
            params,
            expect,
            set,
            verify,
            require_readback,
            optional,
            fragile,
            volatile,
            retry,
            note,
        })
    }
}

/// 解析后的脚本文档。
#[derive(Debug, Clone)]
pub struct Script {
    pub name: String,
    pub created: Option<String>,
    pub targets: Map<String, Value>,
    pub params: Map<String, Value>,
    pub steps: Vec<ScriptStep>,
    pub notes: Vec<String>,
    /// `normal`（默认）或 `compensation`（补偿脚本，默认 dry-run）。
    pub purpose: Option<String>,
}

/// 校验/预检问题（结构性问题 → invalid_params；策略问题 → policy_denied）。
#[derive(Debug, Clone)]
pub struct Issue {
    pub code: &'static str,
    pub step: Option<String>,
    pub message: String,
    pub details: Option<Value>,
}

impl Issue {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            step: None,
            message: message.into(),
            details: None,
        }
    }

    fn at(code: &'static str, step: &str, message: impl Into<String>) -> Self {
        Self {
            code,
            step: Some(step.to_string()),
            message: message.into(),
            details: None,
        }
    }

    fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn to_value(&self) -> Value {
        json!({
            "code": self.code,
            "step": self.step,
            "message": self.message,
            "details": self.details
        })
    }

    fn is_format(&self) -> bool {
        matches!(self.code, "invalid_format" | "invalid_step")
    }
}

fn require_string(object: &Map<String, Value>, key: &str) -> Result<String, String> {
    match object.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(value.clone()),
        _ => Err(format!("{key} must be a non-empty string")),
    }
}

/// 解析脚本文档（格式与版本）；格式错误以 Issue 列表返回。
pub fn parse_document(document: &Value) -> Result<Script, Vec<Issue>> {
    let Some(object) = document.as_object() else {
        return Err(vec![Issue::new(
            "invalid_format",
            "script document must be a JSON object",
        )]);
    };
    let version = object.get("v").and_then(Value::as_u64);
    let format = object.get("format").and_then(Value::as_str);
    if version != Some(1) || format != Some(SCRIPT_FORMAT) {
        return Err(vec![Issue::new(
            "invalid_format",
            format!(
                "expected v=1 format=\"{SCRIPT_FORMAT}\", got v={} format={}",
                version.map_or("null".to_string(), |v| v.to_string()),
                format.unwrap_or("null")
            ),
        )]);
    }
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("script")
        .to_string();
    let created = object
        .get("created")
        .and_then(Value::as_str)
        .map(str::to_string);
    let targets = match object.get("targets") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => {
            return Err(vec![Issue::new(
                "invalid_format",
                "targets must be an object",
            )])
        }
    };
    let params = match object.get("params") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => {
            return Err(vec![Issue::new(
                "invalid_format",
                "params must be an object",
            )])
        }
    };
    let notes = match object.get("notes") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(_) => {
            return Err(vec![Issue::new(
                "invalid_format",
                "notes must be an array of strings",
            )])
        }
    };
    let purpose = match object.get("purpose") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if matches!(value.as_str(), "normal" | "compensation") => {
            Some(value.clone())
        }
        Some(other) => {
            return Err(vec![Issue::new(
                "invalid_format",
                format!("purpose must be normal/compensation, got {other}"),
            )])
        }
    };
    let Some(steps_value) = object.get("steps").and_then(Value::as_array) else {
        return Err(vec![Issue::new(
            "invalid_format",
            "steps must be a non-empty array",
        )]);
    };
    if steps_value.is_empty() {
        return Err(vec![Issue::new(
            "invalid_format",
            "steps must be a non-empty array",
        )]);
    }
    if steps_value.len() > MAX_STEPS {
        return Err(vec![Issue::new(
            "invalid_format",
            format!("steps exceed the {MAX_STEPS} step limit"),
        )]);
    }
    let mut steps = Vec::with_capacity(steps_value.len());
    let mut issues = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for value in steps_value {
        match ScriptStep::from_value(value) {
            Ok(step) => {
                if !seen.insert(step.id.clone()) {
                    issues.push(Issue::at(
                        "invalid_step",
                        &step.id,
                        format!("duplicate step id '{}'", step.id),
                    ));
                }
                steps.push(step);
            }
            Err(message) => issues.push(Issue::new("invalid_step", message)),
        }
    }
    if !issues.is_empty() {
        return Err(issues);
    }
    Ok(Script {
        name,
        created,
        targets,
        params,
        steps,
        notes,
        purpose,
    })
}

/// 风险等级排序（read_only 最低）。
pub fn risk_rank(risk: RiskClass) -> u8 {
    match risk {
        RiskClass::ReadOnly => 0,
        RiskClass::Reversible => 1,
        RiskClass::BoundedWrite => 2,
        RiskClass::ExternalCommit => 3,
        RiskClass::DestructiveCritical => 4,
    }
}

/// `risk_max` 参数解析；返回允许的最高风险。
pub fn parse_risk_max(
    raw: Option<&str>,
    allow_external_commit: bool,
) -> Result<RiskClass, ToolError> {
    let allowed = match raw.unwrap_or(DEFAULT_RISK_MAX) {
        "read_only" => RiskClass::ReadOnly,
        "reversible" => RiskClass::Reversible,
        "bounded_write" => RiskClass::BoundedWrite,
        "external_commit" => RiskClass::ExternalCommit,
        other => {
            return Err(ToolError::new(
                ErrorCode::InvalidParams,
                format!("risk_max must be read_only/reversible/bounded_write/external_commit, got '{other}'"),
            ))
        }
    };
    if allowed == RiskClass::ExternalCommit
        && !allow_external_commit
        && raw != Some("external_commit")
    {
        // 显式 risk_max=external_commit 等价于允许外部提交。
        return Ok(RiskClass::BoundedWrite);
    }
    Ok(allowed)
}

/// 运行上下文：脚本参数、已绑定目标、变量与步骤结果（§2.4 变量解析）。
pub struct Ctx {
    pub params: Map<String, Value>,
    pub targets: HashMap<String, TargetRef>,
    pub vars: Map<String, Value>,
    pub steps: Map<String, Value>,
    /// 当前会话 id（`$session.id`）。
    pub session_id: String,
}

/// 会话内已解析/已绑定的目标。
#[derive(Debug, Clone)]
pub struct TargetRef {
    pub name: String,
    pub hwnd: i64,
    pub token: String,
    pub app_ref: String,
    pub title: String,
    pub pid: i64,
    pub class_name: String,
    pub spec: Value,
    /// preflight/runtime 解析期间是否通过 `launch_ref` 重启了应用。
    pub relaunched: bool,
}

impl TargetRef {
    fn field(&self, field: &str) -> Option<Value> {
        match field {
            "hwnd" => Some(json!(self.hwnd)),
            "token" => Some(json!(self.token)),
            "app_ref" => Some(json!(self.app_ref)),
            "title" => Some(json!(self.title)),
            "pid" => Some(json!(self.pid)),
            "class_name" => Some(json!(self.class_name)),
            _ => None,
        }
    }
}

fn env_allowed(key: &str) -> bool {
    key.starts_with("DESKCTL_")
}

/// 目标字段引用精确取值；其余引用沿 JSON 路径逐段导航。
fn navigate<'a>(value: &'a Value, segments: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for segment in segments {
        current = match current {
            Value::Object(map) => map.get(*segment)?,
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

/// 解析单个 `$…` 引用（去掉 `$` 后传入）。
fn resolve_ref(path: &str, ctx: &Ctx, current: Option<&Value>) -> Result<Value, String> {
    let segments: Vec<&str> = path.split('.').collect();
    let head = segments.first().copied().unwrap_or_default();
    let resolved = match head {
        "target" => {
            let name = segments
                .get(1)
                .copied()
                .ok_or_else(|| format!("$target.{path} must name a target"))?;
            let target = ctx
                .targets
                .get(name)
                .ok_or_else(|| format!("target '{name}' is not declared in this script"))?;
            let field = segments.get(2).copied().unwrap_or("hwnd");
            target
                .field(field)
                .ok_or_else(|| format!("target.{name} has no field '{field}'"))?
        }
        "param" => {
            let key = segments
                .get(1)
                .copied()
                .ok_or_else(|| "param reference is empty".to_string())?;
            let base = ctx
                .params
                .get(key)
                .ok_or_else(|| format!("script parameter '{key}' is not provided"))?;
            navigate(base, &segments[2..])
                .cloned()
                .ok_or_else(|| format!("$param.{path} does not resolve"))?
        }
        "var" => {
            let key = segments.get(1).copied().unwrap_or_default();
            let base = ctx
                .vars
                .get(key)
                .ok_or_else(|| format!("variable '{key}' is not set by an earlier step"))?;
            navigate(base, &segments[2..])
                .cloned()
                .ok_or_else(|| format!("$var.{path} does not resolve"))?
        }
        "env" => {
            let key = segments.get(1).copied().unwrap_or_default();
            if !env_allowed(key) {
                return Err(format!(
                    "$env.{key} is not allowed; only DESKCTL_* environment variables may be referenced"
                ));
            }
            std::env::var(key)
                .map(Value::String)
                .map_err(|_| format!("environment variable {key} is not set"))?
        }
        "session" => {
            let field = segments.get(1).copied().unwrap_or_default();
            match field {
                "id" => json!(ctx.session_id),
                other => return Err(format!("$session has no field '{other}'")),
            }
        }
        "steps" => {
            let id = segments
                .get(1)
                .copied()
                .ok_or_else(|| "steps reference is empty".to_string())?;
            let base = ctx
                .steps
                .get(id)
                .ok_or_else(|| format!("step '{id}' has not produced a result in this run"))?;
            navigate(base, &segments[2..])
                .cloned()
                .ok_or_else(|| format!("$steps.{path} does not resolve"))?
        }
        "result" => {
            let base = current
                .ok_or_else(|| "$result is only valid inside a step's set block".to_string())?;
            navigate(base, &segments[1..])
                .cloned()
                .ok_or_else(|| format!("$result.{path} does not resolve"))?
        }
        other => return Err(format!("unknown reference '${other}'")),
    };
    Ok(resolved)
}

/// 字符串渲染：引用嵌入字符串时转成文本（数字不带引号）。
fn render_ref(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

fn substitute_string(raw: &str, ctx: &Ctx, current: Option<&Value>) -> Result<String, String> {
    if !raw.contains('$') {
        return Ok(raw.to_string());
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(index) = rest.find('$') {
        out.push_str(&rest[..index]);
        let tail = &rest[index + 1..];
        let end = tail
            .find(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')))
            .unwrap_or(tail.len());
        let symbol = &tail[..end];
        if symbol.is_empty() {
            out.push('$');
            rest = tail;
            continue;
        }
        let value = resolve_ref(symbol, ctx, current)?;
        out.push_str(&render_ref(&value));
        rest = &tail[end..];
    }
    out.push_str(rest);
    Ok(out)
}

/// 递归替换值中的 `$…` 引用；`current` 为 `$result.*` 提供当前步骤结果。
pub fn substitute(value: &Value, ctx: &Ctx, current: Option<&Value>) -> Result<Value, String> {
    match value {
        Value::String(raw) => {
            if let Some(symbol) = raw.strip_prefix('$') {
                // 纯引用保留类型（$target.sap.hwnd 是数字）。
                if !symbol.is_empty()
                    && symbol
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
                {
                    return resolve_ref(symbol, ctx, current);
                }
            }
            Ok(Value::String(substitute_string(raw, ctx, current)?))
        }
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(substitute(item, ctx, current)?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, item) in map {
                out.insert(key.clone(), substitute(item, ctx, current)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

/// 收集值中出现过的 `$…` 引用（去重保序）。
pub fn scan_refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(raw) => {
            let mut rest = raw.as_str();
            while let Some(index) = rest.find('$') {
                let tail = &rest[index + 1..];
                let end = tail
                    .find(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')))
                    .unwrap_or(tail.len());
                let symbol = &tail[..end];
                if !symbol.is_empty() && !out.iter().any(|existing| existing == symbol) {
                    out.push(symbol.to_string());
                }
                rest = &tail[end.max(1)..];
            }
        }
        Value::Array(items) => {
            for item in items {
                scan_refs(item, out);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                scan_refs(item, out);
            }
        }
        _ => {}
    }
}

/// 步骤参数/断言里引用的目标名列表。
fn step_target_names(step: &ScriptStep) -> Vec<String> {
    let mut refs = Vec::new();
    scan_refs(&step.params, &mut refs);
    if let Some(expect) = &step.expect {
        scan_refs(expect, &mut refs);
    }
    if let Some(verify) = &step.verify {
        scan_refs(verify, &mut refs);
    }
    let mut names: Vec<String> = Vec::new();
    for reference in refs {
        if let Some(rest) = reference.strip_prefix("target.") {
            if let Some(name) = rest.split('.').next() {
                if !name.is_empty() && !names.iter().any(|existing| existing == name) {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

/// 断言求值（`expect`）：`ok` / `path`（对 `data` 的点路径）/ `equals` /
/// `matches` / `not_matches`（子串语义，无正则依赖）。全部条件须通过。
pub fn eval_expect(expect: &Value, result: &Value) -> Result<(), String> {
    let object = expect
        .as_object()
        .ok_or_else(|| "expect must be an object".to_string())?;
    if let Some(expected) = object.get("ok").and_then(Value::as_bool) {
        if result["ok"].as_bool() != Some(expected) {
            return Err(format!(
                "expect.ok={expected} but result.ok={}",
                result["ok"]
            ));
        }
    }
    let path = object.get("path").and_then(Value::as_str);
    let node = match path {
        Some(path) => {
            let segments: Vec<&str> = path.split('.').collect();
            navigate(&result["data"], &segments)
                .ok_or_else(|| format!("expect.path '{path}' does not exist in result.data"))?
        }
        None => &result["data"],
    };
    if let Some(expected) = object.get("equals") {
        if node != expected {
            return Err(format!(
                "expect.equals mismatch: expected {expected}, got {node}"
            ));
        }
    }
    if let Some(needle) = object.get("matches").and_then(Value::as_str) {
        let haystack = node
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| node.to_string());
        if !haystack.contains(needle) {
            return Err(format!("expect.matches '{needle}' not found"));
        }
    }
    if let Some(needle) = object.get("not_matches").and_then(Value::as_str) {
        let haystack = node
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| node.to_string());
        if haystack.contains(needle) {
            return Err(format!("expect.not_matches '{needle}' was found"));
        }
    }
    Ok(())
}

/// 运行参数（`desktop_script_run` 入参解析结果）。
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub dry_run: bool,
    pub params: Map<String, Value>,
    pub on_error: String,
    pub from_step: Option<String>,
    pub to_step: Option<String>,
    pub risk_max: Option<String>,
    pub allow_external_commit: bool,
    pub allow_resume_after_unknown: bool,
    pub no_fragile: bool,
    /// D4：运行前保存、运行后校验的检查点名（`desktop_script_run.checkpoint`）。
    pub checkpoint: Option<String>,
    /// D4：失败时的补偿脚本 `{script?|path?, apply?}`。
    pub compensation: Option<Value>,
}

pub fn parse_options(args: &Value) -> Result<RunOptions, ToolError> {
    let invalid = |message: &str| ToolError::new(ErrorCode::InvalidParams, message.to_string());
    let on_error = match args.get("on_error").and_then(Value::as_str) {
        None => "stop".to_string(),
        Some(value @ ("stop" | "continue")) => value.to_string(),
        Some(other) => {
            return Err(invalid(&format!(
                "on_error must be stop/continue, got '{other}'"
            )))
        }
    };
    let params = match args.get("params") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => return Err(invalid("params must be an object")),
    };
    let checkpoint = match args.get("checkpoint") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => {
            crate::checkpoint::validate_name(name)?;
            Some(name.clone())
        }
        Some(_) => return Err(invalid("checkpoint must be a string name")),
    };
    let compensation = match args.get("compensation") {
        None | Some(Value::Null) => None,
        Some(Value::Object(spec)) => {
            let has_script = spec.get("script").is_some_and(Value::is_object);
            let has_path = spec.get("path").and_then(Value::as_str).is_some();
            if has_script == has_path {
                return Err(invalid(
                    "compensation needs exactly one of script (inline) or path (file)",
                ));
            }
            Some(Value::Object(spec.clone()))
        }
        Some(_) => return Err(invalid("compensation must be an object")),
    };
    Ok(RunOptions {
        dry_run: args
            .get("dry_run")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        params,
        on_error,
        from_step: args
            .get("from_step")
            .and_then(Value::as_str)
            .map(str::to_string),
        to_step: args
            .get("to_step")
            .and_then(Value::as_str)
            .map(str::to_string),
        risk_max: args
            .get("risk_max")
            .and_then(Value::as_str)
            .map(str::to_string),
        allow_external_commit: args
            .get("allow_external_commit")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        allow_resume_after_unknown: args
            .get("allow_resume_after_unknown")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        no_fragile: args
            .get("no_fragile")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        checkpoint,
        compensation,
    })
}

fn tool_by_name(name: &str) -> Option<ToolName> {
    ToolName::ALL
        .iter()
        .copied()
        .find(|tool| tool.as_str() == name)
}

fn match_spec_matches(window: &Value, match_spec: &Value) -> bool {
    let Some(object) = match_spec.as_object() else {
        return false;
    };
    if object.is_empty() {
        return true;
    }
    let mut checked_any = false;
    if let Some(process_name) = object.get("process_name").and_then(Value::as_str) {
        checked_any = true;
        let actual = window["process_name"].as_str().unwrap_or_default();
        let expected = process_name.trim_end_matches(".exe");
        if !actual
            .trim_end_matches(".exe")
            .eq_ignore_ascii_case(expected)
        {
            return false;
        }
    }
    if let Some(class_name) = object.get("class_name").and_then(Value::as_str) {
        checked_any = true;
        if window["class_name"].as_str() != Some(class_name) {
            return false;
        }
    }
    if let Some(needle) = object.get("title_contains").and_then(Value::as_str) {
        checked_any = true;
        let haystack = window["title"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !haystack.contains(&needle.to_ascii_lowercase()) {
            return false;
        }
    }
    if let Some(app_ref) = object.get("app_ref").and_then(Value::as_str) {
        checked_any = true;
        let mut parts = app_ref.splitn(3, ':');
        let _scheme = parts.next();
        let _pid = parts.next();
        let process = parts.next().unwrap_or_default();
        let actual = window["process_name"].as_str().unwrap_or_default();
        if !actual
            .trim_end_matches(".exe")
            .eq_ignore_ascii_case(process.trim_end_matches(".exe"))
        {
            return false;
        }
    }
    checked_any
}

/// 指纹强度：`loose` 比进程名/标题；`balanced`/`strict` 比 pid+class+title。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strictness {
    Loose,
    Balanced,
    Strict,
}

impl Strictness {
    fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("strict") => Self::Strict,
            Some("loose") => Self::Loose,
            _ => Self::Balanced,
        }
    }
}

fn strictness_of(spec: &Value) -> Strictness {
    Strictness::parse(spec.get("strictness").and_then(Value::as_str))
}

fn fingerprint_score(window: &Value, fingerprint: &Value) -> i32 {
    let mut score = 0;
    if let Some(pid) = fingerprint.get("pid").and_then(Value::as_i64) {
        if window["process_id"].as_i64() == Some(pid) {
            score += 4;
        }
    }
    if let Some(class_name) = fingerprint.get("class_name").and_then(Value::as_str) {
        if !class_name.is_empty() && window["class_name"].as_str() == Some(class_name) {
            score += 2;
        }
    }
    if let Some(title) = fingerprint.get("title").and_then(Value::as_str) {
        if !title.is_empty() && window["title"].as_str() == Some(title) {
            score += 2;
        }
    }
    score
}

fn fingerprint_matches(window: &Value, fingerprint: &Value, strictness: Strictness) -> bool {
    match strictness {
        Strictness::Loose => {
            let process_ok = fingerprint
                .get("process_name")
                .and_then(Value::as_str)
                .map(|expected| {
                    window["process_name"]
                        .as_str()
                        .unwrap_or_default()
                        .trim_end_matches(".exe")
                        .eq_ignore_ascii_case(expected.trim_end_matches(".exe"))
                })
                .unwrap_or(true);
            let title_ok = fingerprint
                .get("title")
                .and_then(Value::as_str)
                .map(|expected| {
                    window["title"]
                        .as_str()
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                        .contains(&expected.to_ascii_lowercase())
                })
                .unwrap_or(true);
            process_ok && title_ok
        }
        Strictness::Balanced | Strictness::Strict => {
            let pid_ok = fingerprint
                .get("pid")
                .and_then(Value::as_i64)
                .map(|expected| window["process_id"].as_i64() == Some(expected))
                .unwrap_or(true);
            let class_ok = fingerprint
                .get("class_name")
                .and_then(Value::as_str)
                .map(|expected| {
                    expected.is_empty() || window["class_name"].as_str() == Some(expected)
                })
                .unwrap_or(true);
            let title_ok = fingerprint
                .get("title")
                .and_then(Value::as_str)
                .map(|expected| expected.is_empty() || window["title"].as_str() == Some(expected))
                .unwrap_or(true);
            pid_ok && class_ok && title_ok
        }
    }
}

fn candidate_summary(window: &Value) -> Value {
    json!({
        "hwnd": window["hwnd"],
        "title": window["title"],
        "process_name": window["process_name"],
        "process_id": window["process_id"]
    })
}

/// 目标解析（§4.4）：描述匹配 → 指纹排序/校验 → 必要时受限启动并重绑。
fn resolve_target(
    session: &Session,
    name: &str,
    spec: &Value,
    params_override: Option<i64>,
) -> Result<TargetRef, ToolError> {
    let match_spec = spec.get("match").cloned().unwrap_or_else(|| json!({}));
    let fingerprint = spec.get("fingerprint").cloned().unwrap_or(Value::Null);
    let launch_ref = spec.get("launch_ref").and_then(Value::as_str);
    let allow_relaunch = spec
        .get("allow_relaunch")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let strictness = strictness_of(spec);

    if let Some(hwnd) = params_override {
        let info = platform::window_info(hwnd).map_err(|error| {
            ToolError::new(
                error.code,
                format!(
                    "target '{name}' override hwnd {hwnd} is not usable: {}",
                    error.message
                ),
            )
        })?;
        if !fingerprint.is_null()
            && !fingerprint_matches(&info, &fingerprint, strictness)
            && strictness != Strictness::Loose
        {
            return Err(ToolError::new(
                ErrorCode::IdentityMismatch,
                format!(
                    "target '{name}' override hwnd {hwnd} does not match the recorded fingerprint"
                ),
            )
            .details(json!({ "target": name, "expected": fingerprint, "actual": candidate_summary(&info) }))
            .retryable(false));
        }
        return bind_target(session, name, spec, &info, false);
    }

    let windows = platform::windows_list()?;
    let matching: Vec<Value> = windows
        .into_iter()
        .filter(|window| match_spec_matches(window, &match_spec))
        .collect();
    if matching.is_empty() {
        if allow_relaunch {
            if let Some(window) = relaunch_window(launch_ref, name)? {
                return bind_target(session, name, spec, &window, true);
            }
        }
        return Err(ToolError::new(
            ErrorCode::TargetUnavailable,
            format!("target '{name}' did not match any running window"),
        )
        .details(json!({ "target": name, "match": match_spec }))
        .retryable(true));
    }

    let mut scored: Vec<(i32, Value)> = matching
        .into_iter()
        .map(|window| (fingerprint_score(&window, &fingerprint), window))
        .collect();
    scored.sort_by(|left, right| right.0.cmp(&left.0));
    let (best_score, best) = scored.first().cloned().expect("non-empty");
    let runner_up = scored.get(1).map(|(score, _)| *score);
    if let Some(second) = runner_up {
        if best_score - second < 3 && strictness != Strictness::Loose {
            let candidates: Vec<Value> = scored
                .iter()
                .take(5)
                .map(|(_, window)| candidate_summary(window))
                .collect();
            return Err(ToolError::new(
                ErrorCode::AmbiguousTarget,
                format!("target '{name}' matches multiple windows; disambiguate with a stricter match or params_patch"),
            )
            .details(json!({ "target": name, "candidates": candidates }))
            .retryable(false));
        }
    }
    if !fingerprint.is_null()
        && !fingerprint_matches(&best, &fingerprint, strictness)
        && strictness != Strictness::Loose
    {
        if allow_relaunch {
            if let Some(window) = relaunch_window(launch_ref, name)? {
                return bind_target(session, name, spec, &window, true);
            }
        }
        return Err(ToolError::new(
            ErrorCode::IdentityMismatch,
            format!(
                "target '{name}' exists but its fingerprint no longer matches the recording; refusing to act blind"
            ),
        )
        .details(json!({
            "target": name,
            "expected": fingerprint,
            "actual": candidate_summary(&best),
            "candidates": scored.iter().take(5).map(|(_, window)| candidate_summary(window)).collect::<Vec<_>>()
        }))
        .retryable(false));
    }
    bind_target(session, name, spec, &best, false)
}

fn relaunch_window(launch_ref: Option<&str>, name: &str) -> Result<Option<Value>, ToolError> {
    let Some(launch_ref) = launch_ref else {
        return Ok(None);
    };
    let entry = crate::apps::find(launch_ref).ok_or_else(|| {
        ToolError::new(
            ErrorCode::PolicyDenied,
            format!("target '{name}' launch_ref '{launch_ref}' is not in the local app catalog"),
        )
    })?;
    let pid = crate::apps::launch(&entry)?;
    let hwnd = crate::apps::wait_for_window(pid, &entry.name, Duration::from_millis(10_000))?
        .ok_or_else(|| {
            ToolError::new(
                ErrorCode::TargetUnavailable,
                format!(
                    "launched '{}' (pid {pid}) but no window appeared within 10s",
                    entry.name
                ),
            )
            .details(json!({ "target": name, "pid": pid }))
            .retryable(false)
        })?;
    Ok(Some(platform::window_info(hwnd)?))
}

/// 绑定（或重绑）目标：写入会话 target 表并返回引用。
fn bind_target(
    session: &Session,
    name: &str,
    spec: &Value,
    window: &Value,
    relaunched: bool,
) -> Result<TargetRef, ToolError> {
    let hwnd = window["hwnd"]
        .as_i64()
        .ok_or_else(|| ToolError::new(ErrorCode::Internal, "window_info did not return hwnd"))?;
    let title = window["title"].as_str().unwrap_or_default().to_string();
    let pid = window["process_id"].as_i64().unwrap_or_default();
    let class_name = window["class_name"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let app_ref = spec
        .get("match")
        .and_then(|value| value.get("app_ref"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            let pid = window["process_id"].as_u64().unwrap_or_default();
            let process = window["process_name"].as_str().unwrap_or("unknown");
            format!("app:{pid}:{}", sanitize_ref(process))
        });
    let token = format!("target-{}", Uuid::new_v4().simple());
    session.store_target(TargetBinding {
        token: token.clone(),
        app_ref: app_ref.clone(),
        window_ref: Some(format!("win:{hwnd}")),
        hwnd,
        title: title.clone(),
        created_at: Instant::now(),
    })?;
    Ok(TargetRef {
        name: name.to_string(),
        hwnd,
        token,
        app_ref,
        title,
        pid,
        class_name,
        spec: spec.clone(),
        relaunched,
    })
}

fn sanitize_ref(raw: &str) -> String {
    raw.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// 步骤执行前的目标新鲜度检查：句柄失效或指纹不符 → 需要重绑。
fn target_needs_rebind(target: &TargetRef) -> Option<String> {
    match platform::window_info(target.hwnd) {
        Err(_) => Some("stale_target".to_string()),
        Ok(window) => {
            let fingerprint = target
                .spec
                .get("fingerprint")
                .cloned()
                .unwrap_or(Value::Null);
            if fingerprint.is_null() {
                return None;
            }
            if fingerprint_matches(&window, &fingerprint, strictness_of(&target.spec)) {
                None
            } else {
                Some("identity_mismatch".to_string())
            }
        }
    }
}

/// 静态校验（`deskctl script validate` 与 preflight 共用；不触达运行时状态）。
pub fn validate_static(script: &Script, allowed_risk: RiskClass) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut produced_vars: HashSet<String> = HashSet::new();
    let mut seen_steps: HashSet<String> = HashSet::new();
    let mut declared_params: HashSet<String> = script.params.keys().cloned().collect();
    for step in &script.steps {
        let Some(tool) = tool_by_name(&step.tool) else {
            issues.push(Issue::at(
                "unknown_tool",
                &step.id,
                format!("tool '{}' does not exist", step.tool),
            ));
            continue;
        };
        let meta = tool.meta();
        let risk = meta.risk;
        // 控制类工具不能作为脚本步骤嵌套（会污染录制器/run 状态或无限递归）。
        if matches!(
            tool,
            ToolName::ScriptRun | ToolName::RecorderStart | ToolName::RecorderStop
        ) {
            issues.push(Issue::at(
                "nested_control_tool",
                &step.id,
                format!("tool '{}' cannot be nested inside a script", step.tool),
            ));
        }
        if risk == RiskClass::DestructiveCritical {
            issues.push(Issue::at(
                "risk_exceeded",
                &step.id,
                format!(
                    "tool '{}' is destructive and is never allowed in scripts",
                    step.tool
                ),
            ));
        } else if risk_rank(risk) > risk_rank(allowed_risk) {
            issues.push(Issue::at(
                "risk_exceeded",
                &step.id,
                format!(
                    "tool '{}' is {} risk, above the allowed max ({}); pass --risk-max/--allow-external-commit to widen",
                    step.tool,
                    crate::audit::risk_str(risk),
                    crate::audit::risk_str(allowed_risk)
                ),
            ));
        }
        if step.require_readback == Some(true) {
            let tier = step
                .verify
                .as_ref()
                .and_then(|verify| verify.get("tier"))
                .and_then(Value::as_str)
                .unwrap_or("screen");
            if tier != "readback" {
                issues.push(Issue::at(
                    "missing_readback",
                    &step.id,
                    "require_readback=true but verify.tier is not readback",
                ));
            }
        }
        if let Some(verify) = &step.verify {
            let tier = verify
                .get("tier")
                .and_then(Value::as_str)
                .unwrap_or("screen");
            if tier == "readback" {
                if let Err(message) = validate_readback_shape(verify) {
                    issues.push(Issue::at("invalid_verify", &step.id, message));
                }
            }
        }
        // 引用可解析性（按脚本内声明推断）。
        let mut refs = Vec::new();
        scan_refs(&step.params, &mut refs);
        if let Some(expect) = &step.expect {
            scan_refs(expect, &mut refs);
        }
        if let Some(verify) = &step.verify {
            scan_refs(verify, &mut refs);
        }
        if let Some(set) = &step.set {
            scan_refs(set, &mut refs);
        }
        for reference in &refs {
            if let Some(rest) = reference.strip_prefix("target.") {
                let name = rest.split('.').next().unwrap_or_default();
                if !script.targets.contains_key(name) {
                    issues.push(Issue::at(
                        "target_not_declared",
                        &step.id,
                        format!(
                            "reference ${reference} has no target named '{name}' in the script"
                        ),
                    ));
                }
                continue;
            }
            if let Some(rest) = reference.strip_prefix("param.") {
                let key = rest.split('.').next().unwrap_or_default();
                if !declared_params.contains(key) {
                    issues.push(Issue::at(
                        "unresolved_param",
                        &step.id,
                        format!("$param.{key} is not declared; provide it via params/--param"),
                    ));
                }
                continue;
            }
            if let Some(rest) = reference.strip_prefix("var.") {
                let key = rest.split('.').next().unwrap_or_default();
                if !produced_vars.contains(key) {
                    issues.push(Issue::at(
                        "unresolved_var",
                        &step.id,
                        format!("$var.{key} is not produced by an earlier step's set"),
                    ));
                }
                continue;
            }
            if let Some(rest) = reference.strip_prefix("steps.") {
                let id = rest.split('.').next().unwrap_or_default();
                if !seen_steps.contains(id) {
                    issues.push(Issue::at(
                        "unresolved_step_ref",
                        &step.id,
                        format!("$steps.{id} refers to a step that does not run before this one"),
                    ));
                }
                continue;
            }
            if let Some(rest) = reference.strip_prefix("env.") {
                let key = rest.split('.').next().unwrap_or_default();
                if !env_allowed(key) {
                    issues.push(Issue::at(
                        "env_not_allowed",
                        &step.id,
                        format!("$env.{key} is not allowed (only DESKCTL_*)"),
                    ));
                }
                continue;
            }
            if reference == "session.id" {
                continue; // 运行时由会话提供
            }
            if let Some(rest) = reference.strip_prefix("session.") {
                issues.push(Issue::at(
                    "unknown_reference",
                    &step.id,
                    format!("$session.{rest} is not supported (only $session.id)"),
                ));
                continue;
            }
            if reference.starts_with("result.") || reference == "result" {
                continue; // 仅 set 内合法，求值时校验
            }
            if !produced_vars.contains(reference) && !seen_steps.contains(reference) {
                issues.push(Issue::at(
                    "unknown_reference",
                    &step.id,
                    format!("reference '${reference}' is not recognized"),
                ));
            }
        }
        if let Some(set) = step.set.as_ref().and_then(Value::as_object) {
            for (key, _) in set {
                produced_vars.insert(key.clone());
            }
        }
        // 脚本参数可由 params 段声明，也允许调用时覆盖；此处只收集。
        for key in script.params.keys() {
            declared_params.insert(key.clone());
        }
        seen_steps.insert(step.id.clone());
    }
    issues
}

/// readback 核验形态校验（kind 与必需字段）。
fn validate_readback_shape(verify: &Value) -> Result<(), String> {
    let kind = verify
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "readback verify requires a kind".to_string())?;
    let has_target = verify.get("target").and_then(Value::as_str).is_some();
    match kind {
        "file_exists" => {
            if !has_target {
                return Err("file_exists requires target".to_string());
            }
        }
        "file_sha256" => {
            if !has_target || verify.get("sha256").and_then(Value::as_str).is_none() {
                return Err("file_sha256 requires target and sha256".to_string());
            }
        }
        "docx_contains" | "pptx_contains" => {
            if !has_target || verify.get("contains").and_then(Value::as_str).is_none() {
                return Err(format!("{kind} requires target and contains"));
            }
        }
        "xlsx_cell" => {
            if !has_target || verify.get("cell").and_then(Value::as_str).is_none() {
                return Err("xlsx_cell requires target and cell".to_string());
            }
            if verify.get("equals").is_none() && verify.get("matches").is_none() {
                return Err("xlsx_cell requires equals or matches".to_string());
            }
        }
        "json_path" => {
            if !has_target || verify.get("path").and_then(Value::as_str).is_none() {
                return Err("json_path requires target and path".to_string());
            }
        }
        other => return Err(format!("unsupported readback kind '{other}'")),
    }
    Ok(())
}

/// 读取脚本文档：`script`（内联对象）或 `path`（`.dscript.json`，受允许根策略约束）。
fn load_document(engine: &Engine, args: &Value) -> Result<Value, ToolError> {
    if let Some(script) = args.get("script") {
        if !script.is_object() {
            return Err(ToolError::new(
                ErrorCode::InvalidParams,
                "script must be an object",
            ));
        }
        return Ok(script.clone());
    }
    let Some(raw) = args.get("path").and_then(Value::as_str) else {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            "script_run requires either script (inline) or path",
        ));
    };
    let path = crate::tools::normalize_allowed_path(engine, raw)?;
    let bytes = std::fs::read(&path).map_err(|error| {
        ToolError::new(
            ErrorCode::InvalidParams,
            format!("cannot read script file {}: {error}", path.display()),
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        ToolError::new(
            ErrorCode::InvalidParams,
            format!("script file {} is not valid JSON: {error}", path.display()),
        )
    })
}

/// preflight 产出的执行计划。
struct Plan {
    script: Script,
    options: RunOptions,
    from: usize,
    to: usize,
    targets: HashMap<String, TargetRef>,
    skipped_fragile: HashSet<String>,
    warnings: Vec<String>,
    allowed_risk: RiskClass,
    params: Map<String, Value>,
}

fn preflight(session: &Session, script: &Script, options: &RunOptions) -> Result<Plan, Vec<Issue>> {
    let allowed_risk = parse_risk_max(options.risk_max.as_deref(), options.allow_external_commit)
        .map_err(|error| vec![Issue::new("invalid_risk_max", error.message)])?;
    // 参数合并：脚本 params 为默认值，调用参数覆盖。
    let mut params = script.params.clone();
    for (key, value) in &options.params {
        params.insert(key.clone(), value.clone());
    }
    // 静态校验看到的是"脚本声明 + 调用时 params"的合并结果，否则仅由
    // `--param` 提供的参数会被误报为 unresolved_param。
    let mut effective = script.clone();
    effective.params = params.clone();
    let mut issues = validate_static(&effective, allowed_risk);
    let mut warnings = Vec::new();

    // 范围：--from/--to。
    let index_of = |id: &str| script.steps.iter().position(|step| step.id == id);
    let mut from = 0usize;
    if let Some(id) = &options.from_step {
        match index_of(id) {
            Some(index) => from = index,
            None => issues.push(Issue::new(
                "unknown_step",
                format!("from_step '{id}' does not exist"),
            )),
        }
    }
    // 续跑门禁：上一次 run 为 RECONCILIATION_REQUIRED 时默认拒绝 `--from`
    // （§4.6.2），除非调用方显式允许。
    if options.from_step.is_some()
        && !options.allow_resume_after_unknown
        && session.last_run_outcome().as_deref() == Some("RECONCILIATION_REQUIRED")
    {
        issues.push(
            Issue::new(
                "resume_after_unknown",
                "the previous run needs reconciliation; verify the effect first, then pass allow_resume_after_unknown=true to resume",
            )
            .with_details(json!({ "previous_outcome": "RECONCILIATION_REQUIRED" })),
        );
    }
    let mut to = script.steps.len();
    if let Some(id) = &options.to_step {
        match index_of(id) {
            Some(index) => to = index + 1,
            None => issues.push(Issue::new(
                "unknown_step",
                format!("to_step '{id}' does not exist"),
            )),
        }
    }
    if from >= to {
        issues.push(Issue::new(
            "empty_range",
            "--from/--to select an empty step range",
        ));
    }

    // 续跑：被跳过的步骤若产出后续引用的变量/结果 → 需调用方用 params 显式提供。
    if from > 0 {
        let mut skipped_vars: Vec<(String, String)> = Vec::new();
        let mut skipped_steps: Vec<String> = Vec::new();
        for step in &script.steps[..from.min(script.steps.len())] {
            if let Some(set) = step.set.as_ref().and_then(Value::as_object) {
                for key in set.keys() {
                    skipped_vars.push((key.clone(), step.id.clone()));
                }
            }
            skipped_steps.push(step.id.clone());
        }
        let mut needed: Vec<String> = Vec::new();
        for step in &script.steps[from.min(script.steps.len())..to.min(script.steps.len())] {
            let mut refs = Vec::new();
            scan_refs(&step.params, &mut refs);
            if let Some(expect) = &step.expect {
                scan_refs(expect, &mut refs);
            }
            if let Some(verify) = &step.verify {
                scan_refs(verify, &mut refs);
            }
            for reference in refs {
                if let Some(rest) = reference.strip_prefix("var.") {
                    let key = rest.split('.').next().unwrap_or_default().to_string();
                    if skipped_vars.iter().any(|(name, _)| name == &key)
                        && !params.contains_key(&key)
                        && !needed.iter().any(|existing| existing == &key)
                    {
                        needed.push(key);
                    }
                }
                if let Some(rest) = reference.strip_prefix("steps.") {
                    let id = rest.split('.').next().unwrap_or_default().to_string();
                    if skipped_steps.contains(&id)
                        && !needed
                            .iter()
                            .any(|existing| existing == &format!("$steps.{id}"))
                    {
                        needed.push(format!("$steps.{id}"));
                    }
                }
            }
        }
        if !needed.is_empty() {
            issues.push(
                Issue::new(
                    "resume_reference_missing",
                    format!(
                        "resume from '{}' needs values produced by skipped steps: {}; pass them via params",
                        script.steps[from.min(script.steps.len())].id,
                        needed.join(", ")
                    ),
                )
                .with_details(json!({ "unresolved": needed })),
            );
        }
    }

    // 会话画像：工具可用性。
    for step in &script.steps[from.min(script.steps.len())..to.min(script.steps.len())] {
        if let Some(tool) = tool_by_name(&step.tool) {
            if !session.allows(tool) {
                issues.push(Issue::at(
                    "tool_unavailable",
                    &step.id,
                    format!(
                        "tool '{}' is not available in profile {}",
                        step.tool, session.profile
                    ),
                ));
            }
        }
    }

    // capture 引用：字面值必须在本会话存在。
    for step in &script.steps[from.min(script.steps.len())..to.min(script.steps.len())] {
        if let Some(capture_id) = step.params.get("capture_id").and_then(Value::as_str) {
            if !capture_id.starts_with('$') && session.capture(capture_id).is_err() {
                issues.push(Issue::at(
                    "stale_capture",
                    &step.id,
                    format!(
                        "capture '{capture_id}' is not available in this session; take a fresh screenshot or re-record"
                    ),
                ));
            }
        }
    }

    // 目标解析与绑定（--from 之前未用到的目标不强制解析）。
    let mut targets: HashMap<String, TargetRef> = HashMap::new();
    let referenced: HashSet<String> = script.steps
        [from.min(script.steps.len())..to.min(script.steps.len())]
        .iter()
        .flat_map(step_target_names)
        .collect();
    for (name, spec) in &script.targets {
        let in_scope = referenced.contains(name);
        let override_hwnd = params
            .get(name)
            .and_then(Value::as_u64)
            .map(|value| value as i64);
        if !in_scope && override_hwnd.is_none() {
            warnings.push(format!(
                "target '{name}' is declared but not used in this range"
            ));
            continue;
        }
        match resolve_target(session, name, spec, override_hwnd) {
            Ok(target) => {
                targets.insert(name.clone(), target);
            }
            Err(error) => {
                if !in_scope {
                    warnings.push(format!(
                        "unused target '{name}' could not be resolved: {}",
                        error.message
                    ));
                    continue;
                }
                issues.push(
                    Issue::at("target_resolution", name, error.message.clone()).with_details(
                        json!({
                            "error_code": error.code.as_str(),
                            "details": error.details.clone()
                        }),
                    ),
                );
            }
        }
    }

    if !issues.is_empty() {
        return Err(issues);
    }

    // no_fragile：跳过坐标类步骤（执行时进入 skipped 列表）。
    let mut skipped_fragile = HashSet::new();
    if options.no_fragile {
        for step in &script.steps[from..to] {
            if step.fragile == Some(true) {
                skipped_fragile.insert(step.id.clone());
            }
        }
        if !skipped_fragile.is_empty() {
            warnings.push(format!(
                "--no-fragile skips {} fragile step(s)",
                skipped_fragile.len()
            ));
        }
    }

    Ok(Plan {
        script: script.clone(),
        options: options.clone(),
        from,
        to,
        targets,
        skipped_fragile,
        warnings,
        allowed_risk,
        params,
    })
}

/// 预检拒绝的统一错误（附 run report 形态的 details，便于调用方消费）。
fn rejected(issues: Vec<Issue>, script_name: &str, warnings: &[String]) -> ToolError {
    let format_only = issues.iter().all(Issue::is_format);
    let code = if format_only {
        ErrorCode::InvalidParams
    } else {
        ErrorCode::PolicyDenied
    };
    let report = json!({
        "v": 1,
        "format": RUN_REPORT_FORMAT,
        "run_id": null,
        "script": script_name,
        "dry_run": false,
        "result": "failed",
        "outcome": "REJECTED_POLICY",
        "handoff": {
            "run_id": null,
            "failed_step": null,
            "reason": "preflight_rejected",
            "effect_may_have_occurred": false,
            "safe_to_retry": true,
            "resume_hint": null,
            "suggested_actions": [
                { "action": "fix_script", "detail": "fix the issues list (script/params/risk), then run again from the start" }
            ]
        },
        "steps": [],
        "rebinds": [],
        "warnings": warnings,
        "stats": { "total": 0, "ok": 0, "failed": 0, "skipped": 0, "readback_verified": 0 },
        "issues": issues.iter().map(Issue::to_value).collect::<Vec<_>>()
    });
    ToolError::new(
        code,
        format!(
            "script preflight rejected {} issue(s); see details.issues",
            issues.len()
        ),
    )
    .details(report)
    .retryable(false)
}

/// dry-run 报告：只给每步判定，不执行任何工具（AC-RPL-004）。
fn dry_run_report(plan: &Plan, warnings: &[String]) -> Value {
    let mut decisions = Vec::new();
    for step in &plan.script.steps[plan.from..plan.to] {
        let decision = if plan.skipped_fragile.contains(&step.id) {
            "skip_fragile"
        } else if let Some(tool) = tool_by_name(&step.tool) {
            if risk_rank(tool.meta().risk) > risk_rank(plan.allowed_risk) {
                "reject_risk"
            } else {
                "execute"
            }
        } else {
            "reject_unknown_tool"
        };
        decisions.push(json!({
            "id": step.id,
            "tool": step.tool,
            "decision": decision,
            "risk": tool_by_name(&step.tool).map(|tool| crate::audit::risk_str(tool.meta().risk)),
            "verify_tier": step
                .verify
                .as_ref()
                .and_then(|verify| verify.get("tier"))
                .and_then(Value::as_str),
            "fragile": step.fragile == Some(true),
            "volatile": step.volatile == Some(true)
        }));
    }
    json!({
        "v": 1,
        "format": RUN_REPORT_FORMAT,
        "run_id": null,
        "script": plan.script.name,
        "dry_run": true,
        "result": "ok",
        "outcome": Value::Null,
        "handoff": {
            "failed_step": null,
            "reason": null,
            "effect_may_have_occurred": false,
            "safe_to_retry": true,
            "resume_hint": null,
            "suggested_actions": []
        },
        "steps": decisions,
        "rebinds": [],
        "warnings": warnings,
        "targets": plan.targets.keys().cloned().collect::<Vec<_>>(),
        "stats": {
            "total": plan.to - plan.from,
            "ok": 0,
            "failed": 0,
            "skipped": plan.skipped_fragile.len(),
            "readback_verified": 0
        }
    })
}

/// 单步失败信息（用于 report / handoff）。
#[derive(Debug, Clone)]
struct StepFailure {
    code: String,
    message: String,
    dispatch_state: DispatchState,
    effect_may_have_occurred: bool,
    details: Option<Value>,
    retryable: bool,
}

impl StepFailure {
    fn from_result(result: &ToolResult) -> Self {
        let dispatch_state = result.meta.dispatch_state;
        let (code, message, details, retryable) = match &result.error {
            Some(error) => (
                error.code.as_str().to_string(),
                error.message.clone(),
                (!error.details.is_null()).then(|| error.details.clone()),
                error.retryable,
            ),
            None => (
                "step_failed".to_string(),
                "tool call failed".to_string(),
                None,
                false,
            ),
        };
        Self {
            code,
            message,
            dispatch_state,
            effect_may_have_occurred: dispatch_state != DispatchState::NotSent,
            details,
            retryable,
        }
    }

    fn local(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            dispatch_state: DispatchState::NotSent,
            effect_may_have_occurred: false,
            details: None,
            retryable: false,
        }
    }

    fn to_value(&self) -> Value {
        json!({
            "code": self.code,
            "message": self.message,
            "dispatch_state": crate::audit::dispatch_state_str(self.dispatch_state),
            "details": self.details
        })
    }
}

/// 单步核验结果。
#[derive(Debug, Clone)]
struct VerifyOutcome {
    tier: &'static str,
    ok: bool,
    detail: Value,
    failure: Option<String>,
}

fn tool_result_value(result: &ToolResult) -> Value {
    serde_json::to_value(result).unwrap_or(Value::Null)
}

/// screen 弱核验：统一走 `desktop_verify_state`（复用 expectation 语义）。
async fn verify_screen(
    engine: &Arc<Engine>,
    session: &Arc<Session>,
    run_id: &str,
    step_id: &str,
    verify: &Value,
    cancel: &CancelFlag,
) -> VerifyOutcome {
    let expectation = match expectation_for(verify) {
        Ok(expectation) => expectation,
        Err(message) => {
            return VerifyOutcome {
                tier: "screen",
                ok: false,
                detail: json!({ "error": message }),
                failure: Some(message),
            }
        }
    };
    let result = engine
        .execute_leaf_with_session(
            session.clone(),
            ToolName::VerifyState,
            json!({ "expectation": expectation }),
            Some(format!("{run_id}:{step_id}:verify")),
            cancel.clone(),
            AuditSource::Daemon,
        )
        .await;
    let data = result.data.clone().unwrap_or(Value::Null);
    let satisfied = data.get("satisfied").and_then(Value::as_bool);
    let ok = result.ok && satisfied != Some(false);
    VerifyOutcome {
        tier: "screen",
        ok,
        detail: data,
        failure: (!ok).then(|| {
            result
                .error
                .as_ref()
                .map(|error| format!("{}: {}", error.code.as_str(), error.message))
                .unwrap_or_else(|| "screen verification did not hold".to_string())
        }),
    }
}

fn expectation_for(verify: &Value) -> Result<Value, String> {
    if let Some(expectation) = verify.get("expectation") {
        if expectation.is_object() {
            let mut expectation = expectation.clone();
            if let Some(samples) = verify.get("stable_samples") {
                expectation["stable_samples"] = samples.clone();
            }
            return Ok(expectation);
        }
    }
    let kind = verify
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "screen verify requires expectation or kind".to_string())?;
    let locator = verify.get("locator").cloned().unwrap_or_else(|| json!({}));
    let value = verify.get("value").cloned();
    match kind {
        "text_present" => {
            let mut locator = locator.as_object().cloned().unwrap_or_default();
            if let Some(value) = value.as_ref().and_then(Value::as_str) {
                locator.insert("accessible_name".to_string(), json!(value));
            }
            Ok(with_samples(
                json!({ "locator": Value::Object(locator), "condition": "exists" }),
                verify,
            ))
        }
        "element_exists" => Ok(with_samples(
            json!({ "locator": locator, "condition": "exists" }),
            verify,
        )),
        "value_equals" => Ok(with_samples(
            json!({
                "locator": locator,
                "condition": "value_equals",
                "value": value.unwrap_or(Value::Null)
            }),
            verify,
        )),
        "checked" => Ok(with_samples(
            json!({
                "locator": locator,
                "condition": "checked",
                "expected": verify.get("expected").and_then(Value::as_bool).unwrap_or(true)
            }),
            verify,
        )),
        other => Err(format!("unsupported screen verify kind '{other}'")),
    }
}

fn with_samples(mut expectation: Value, verify: &Value) -> Value {
    if let Some(samples) = verify.get("stable_samples") {
        expectation["stable_samples"] = samples.clone();
    }
    expectation
}

/// readback 强核验：结果是"文件/产物里的真实值"（§2.6）。
async fn verify_readback(
    engine: &Arc<Engine>,
    session: &Arc<Session>,
    run_id: &str,
    step_id: &str,
    verify: &Value,
    ctx: &Ctx,
    cancel: &CancelFlag,
) -> VerifyOutcome {
    let failed = |detail: Value, message: String| VerifyOutcome {
        tier: "readback",
        ok: false,
        detail,
        failure: Some(message),
    };
    let kind = verify.get("kind").and_then(Value::as_str).unwrap_or("");
    let Some(target_ref) = verify.get("target").and_then(Value::as_str) else {
        return failed(
            json!({ "error": "missing target" }),
            "readback verify requires target".into(),
        );
    };
    let target_value = match substitute(&json!(target_ref), ctx, None) {
        Ok(value) => value,
        Err(message) => {
            return failed(
                json!({ "error": message }),
                format!("target did not resolve: {message}"),
            )
        }
    };
    let Some(path) = target_value.as_str() else {
        return failed(
            json!({ "resolved": target_value }),
            "target must resolve to a file path string".into(),
        );
    };
    let call = |tool: ToolName, args: Value| {
        let engine = engine.clone();
        let session = session.clone();
        let cancel = cancel.clone();
        let request_id = Some(format!("{run_id}:{step_id}:verify"));
        async move {
            engine
                .execute_leaf_with_session(
                    session,
                    tool,
                    args,
                    request_id,
                    cancel,
                    AuditSource::Daemon,
                )
                .await
        }
    };
    match kind {
        "file_exists" => {
            let result = call(ToolName::FileHash, json!({ "path": path })).await;
            if !result.ok {
                return failed(
                    result.data.clone().unwrap_or(Value::Null),
                    "desktop_file_hash failed".into(),
                );
            }
            let data = result.data.clone().unwrap_or(Value::Null);
            let exists = data["exists"].as_bool() == Some(true);
            VerifyOutcome {
                tier: "readback",
                ok: exists,
                detail: data,
                failure: (!exists).then(|| format!("file {path} does not exist")),
            }
        }
        "file_sha256" => {
            let expected = verify
                .get("sha256")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim_start_matches("sha256:")
                .to_ascii_lowercase();
            let result = call(ToolName::FileHash, json!({ "path": path })).await;
            if !result.ok {
                return failed(
                    result.data.clone().unwrap_or(Value::Null),
                    "desktop_file_hash failed".into(),
                );
            }
            let data = result.data.clone().unwrap_or(Value::Null);
            let actual = data["sha256"]
                .as_str()
                .unwrap_or_default()
                .trim_start_matches("sha256:");
            let ok = !expected.is_empty() && actual.eq_ignore_ascii_case(&expected);
            let detail = json!({ "expected": expected, "actual": actual, "path": path });
            VerifyOutcome {
                tier: "readback",
                ok,
                detail,
                failure: (!ok).then(|| format!("sha256 mismatch for {path}")),
            }
        }
        "docx_contains" | "pptx_contains" => {
            let needle = verify
                .get("contains")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let tool = if kind == "docx_contains" {
                ToolName::DocxRead
            } else {
                ToolName::PptxRead
            };
            let result = call(tool, json!({ "path": path })).await;
            if !result.ok {
                return failed(
                    result.data.clone().unwrap_or(Value::Null),
                    format!("{kind} read failed"),
                );
            }
            let data = result.data.clone().unwrap_or(Value::Null);
            let haystack = data.to_string();
            let ok = !needle.is_empty() && haystack.contains(needle);
            VerifyOutcome {
                tier: "readback",
                ok,
                detail: json!({ "needle": needle, "path": path, "read_ok": true }),
                failure: (!ok).then(|| format!("'{needle}' not found in {path}")),
            }
        }
        "xlsx_cell" => {
            let cell = verify.get("cell").and_then(Value::as_str).unwrap_or("A1");
            let mut args = json!({ "path": path, "range": format!("{cell}:{cell}") });
            if let Some(sheet) = verify.get("sheet").and_then(Value::as_str) {
                args["sheet"] = json!(sheet);
            }
            let result = call(ToolName::XlsxRead, args).await;
            if !result.ok {
                return failed(
                    result.data.clone().unwrap_or(Value::Null),
                    "xlsx read failed".into(),
                );
            }
            let data = result.data.clone().unwrap_or(Value::Null);
            let value = data
                .get("sheets")
                .and_then(Value::as_array)
                .and_then(|sheets| sheets.first())
                .and_then(|sheet| sheet.get("rows"))
                .and_then(Value::as_array)
                .and_then(|rows| rows.first())
                .and_then(Value::as_array)
                .and_then(|cells| cells.first())
                .cloned()
                .unwrap_or(Value::Null);
            let mut ok = true;
            if let Some(expected) = verify.get("equals") {
                ok = &value == expected;
            }
            if let Some(needle) = verify.get("matches").and_then(Value::as_str) {
                ok = ok && value.to_string().contains(needle);
            }
            VerifyOutcome {
                tier: "readback",
                ok,
                detail: json!({ "cell": cell, "value": value, "path": path }),
                failure: (!ok).then(|| format!("cell {cell} does not match the expected value")),
            }
        }
        "json_path" => {
            let base = if let Some(pointer) = target_ref.strip_prefix('$') {
                match resolve_ref(pointer, ctx, None) {
                    Ok(value) => value,
                    Err(message) => return failed(json!({ "error": message }), message),
                }
            } else {
                json!(target_ref)
            };
            let path_spec = verify.get("path").and_then(Value::as_str).unwrap_or("");
            let segments: Vec<&str> = path_spec
                .split('.')
                .filter(|part| !part.is_empty())
                .collect();
            let node = navigate(&base, &segments).cloned();
            let mut ok = node.is_some();
            if let Some(expected) = verify.get("equals") {
                ok = node.as_ref() == Some(expected);
            }
            if let Some(needle) = verify.get("contains").and_then(Value::as_str) {
                ok = ok
                    && node
                        .as_ref()
                        .map(|value| value.to_string().contains(needle))
                        .unwrap_or(false);
            }
            VerifyOutcome {
                tier: "readback",
                ok,
                detail: json!({ "path": path_spec, "value": node }),
                failure: (!ok).then(|| format!("json_path '{path_spec}' assertion failed")),
            }
        }
        other => failed(
            json!({ "error": format!("unsupported readback kind '{other}'") }),
            format!("unsupported readback kind '{other}'"),
        ),
    }
}

/// 执行一次脚本（`desktop_script_run`）。
pub async fn run(
    engine: &Arc<Engine>,
    session: &Arc<Session>,
    args: &Value,
    cancel: &CancelFlag,
) -> Result<ToolExecution, ToolError> {
    run_with(engine, session, args, cancel, false).await
}

/// `nested=true` 时不占用会话的 run 槽（供补偿子运行使用，避免与主 run 冲突）。
async fn run_with(
    engine: &Arc<Engine>,
    session: &Arc<Session>,
    args: &Value,
    cancel: &CancelFlag,
    nested: bool,
) -> Result<ToolExecution, ToolError> {
    let mut options = parse_options(args)?;
    let document = load_document(engine, args)?;
    let script = match parse_document(&document) {
        Ok(script) => script,
        Err(issues) => return Err(rejected(issues, "script", &[])),
    };
    // D4/AC-TXN-003：`purpose=compensation` 的脚本默认 dry-run；调用方显式
    // 传 `dry_run` 时以其为准（补偿子运行的 dry_run 由 apply 决定）。
    if script.purpose.as_deref() == Some("compensation") && args.get("dry_run").is_none() {
        options.dry_run = true;
    }
    let plan = match preflight(session, &script, &options) {
        Ok(plan) => plan,
        Err(issues) => return Err(rejected(issues, &script.name, &[])),
    };
    if options.dry_run {
        return Ok(ToolExecution::read(dry_run_report(&plan, &plan.warnings)));
    }
    execute_plan(engine, session, plan, cancel, nested).await
}

/// `deskctl script validate` 的静态校验入口（不触达会话/平台状态）。
pub fn validate_document(
    document: &Value,
    risk_max: Option<&str>,
    allow_external_commit: bool,
) -> Result<Vec<Issue>, ToolError> {
    let parsed = parse_document(document);
    let (script, mut issues) = match parsed {
        Ok(script) => (script, Vec::new()),
        Err(issues) => {
            let format_only = issues.iter().all(Issue::is_format);
            if format_only {
                return Ok(issues);
            }
            (
                parse_document(document).ok().unwrap_or(Script {
                    name: "script".to_string(),
                    created: None,
                    targets: Map::new(),
                    params: Map::new(),
                    steps: Vec::new(),
                    notes: Vec::new(),
                    purpose: None,
                }),
                issues,
            )
        }
    };
    let allowed = parse_risk_max(risk_max, allow_external_commit)?;
    if issues.is_empty() {
        issues = validate_static(&script, allowed);
    }
    Ok(issues)
}

struct RunGuard<'a> {
    session: &'a Session,
}

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        self.session.run_end();
    }
}

/// 执行计划并产出 run report。
async fn execute_plan(
    engine: &Arc<Engine>,
    session: &Arc<Session>,
    plan: Plan,
    cancel: &CancelFlag,
    nested: bool,
) -> Result<ToolExecution, ToolError> {
    let run_id = format!("run-{}", &Uuid::new_v4().simple().to_string()[..8]);
    let _guard = if nested {
        None
    } else {
        session.run_begin(&run_id)?;
        Some(RunGuard { session })
    };
    let started_at = chrono::Local::now();
    let started = Instant::now();

    let mut ctx = Ctx {
        params: plan.params.clone(),
        targets: plan.targets.clone(),
        vars: Map::new(),
        steps: Map::new(),
        session_id: session.id.clone(),
    };
    // `--from` 续跑：被跳过步骤的 `set` 变量由调用方 params 回填（§4.6.2）。
    if plan.from > 0 {
        for step in &plan.script.steps[..plan.from.min(plan.script.steps.len())] {
            if let Some(set) = step.set.as_ref().and_then(Value::as_object) {
                for key in set.keys() {
                    if let Some(value) = ctx.params.get(key).cloned() {
                        ctx.vars.insert(key.clone(), value);
                    }
                }
            }
        }
    }
    let mut warnings = plan.warnings.clone();
    let mut rebinds: Vec<Value> = Vec::new();
    let mut entries: Vec<Value> = Vec::new();
    let mut stats = Stats::default();
    let mut rebind_budget = REBIND_BUDGET;
    let mut first_failure: Option<FailureRecord> = None;
    let mut any_dispatched = false;
    let mut canceled = false;

    // D4：运行前保存检查点（dry-run 已在上面提前返回，不触达状态）。
    if let Some(name) = plan.options.checkpoint.as_deref() {
        let targets: Vec<Value> = plan
            .targets
            .values()
            .map(|target| json!({ "name": target.name, "hwnd": target.hwnd }))
            .collect();
        crate::checkpoint::save(
            engine,
            session,
            &json!({ "name": name, "targets": targets }),
        )?;
    }

    let steps: Vec<ScriptStep> = plan.script.steps[plan.from..plan.to].to_vec();
    for (offset, step) in steps.iter().enumerate() {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            canceled = true;
            break;
        }
        stats.total += 1;
        if plan.skipped_fragile.contains(&step.id) {
            stats.skipped += 1;
            entries.push(json!({
                "id": step.id,
                "tool": step.tool,
                "ok": true,
                "skipped": "fragile",
                "decision": "skip_fragile",
                "dispatch_state": crate::audit::dispatch_state_str(DispatchState::NotSent),
                "verification": crate::audit::verification_str(Verification::NotApplicable),
                "verify_tier": Value::Null,
                "rebind": Value::Null,
                "retries": 0,
                "duration_ms": 0,
                "error": Value::Null
            }));
            continue;
        }
        let Some(tool) = tool_by_name(&step.tool) else {
            // parse_document/preflight 已拦截；此处兜底。
            break;
        };
        let targets_used = step_target_names(step);
        let mut rebind_reason: Option<String> = None;
        let mut entry = execute_step(
            engine,
            session,
            &run_id,
            step,
            tool,
            &mut ctx,
            &plan,
            cancel,
            &mut rebinds,
            &mut rebind_budget,
            &targets_used,
            &mut rebind_reason,
            &mut any_dispatched,
            &mut stats,
            &mut warnings,
        )
        .await;
        let failed = entry
            .get("ok")
            .and_then(Value::as_bool)
            .map(|ok| !ok)
            .unwrap_or(true);

        // `set` 求值（成功与失败都登记，失败步骤的结果同样可被后续引用与核对）。
        collect_set(step, &mut ctx, &mut warnings);

        if failed {
            stats.failed += 1;
            let failure = FailureRecord {
                step: step.id.clone(),
                failure: entry_failure(&entry),
                targets: targets_used.clone(),
            };
            if first_failure.is_none() {
                first_failure = Some(failure);
            }
            let optional = step.optional == Some(true);
            entry["optional"] = json!(optional);
            entries.push(entry);
            if !optional && plan.options.on_error == "stop" {
                break;
            }
            continue;
        }
        stats.ok += 1;
        entries.push(entry);
        // 供 $steps.<id> 引用。
        let _ = offset;
    }

    // D4：失败时的补偿。`apply=false`（默认）只生成 dry-run 补偿计划，不下发输入。
    let mut compensation = Value::Null;
    if first_failure.is_some() {
        if let Some(spec) = plan.options.compensation.clone() {
            compensation = Box::pin(run_compensation(engine, session, &spec, cancel)).await;
        }
    }

    // D4：收尾检查点校验（成功与失败都做），放在补偿之后以反映最终状态。
    let mut checkpoint_report = Value::Null;
    if let Some(name) = plan.options.checkpoint.as_deref() {
        checkpoint_report =
            match crate::checkpoint::verify(engine, session, &json!({ "name": name })) {
                Ok(execution) => execution.data,
                Err(error) => json!({
                    "name": name,
                    "matched": Value::Null,
                    "diffs": [],
                    "error": { "code": error.code.as_str(), "message": error.message }
                }),
            };
    }

    let finished_at = chrono::Local::now();
    let (result, outcome) = classify(first_failure.as_ref(), canceled, &stats);
    let handoff = build_handoff(&run_id, outcome, first_failure.as_ref(), &plan);
    session.set_last_run_outcome(outcome);
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let relaunched = plan.targets.values().any(|target| target.relaunched)
        || rebinds
            .iter()
            .any(|event| event["relaunched"].as_bool() == Some(true));
    let targets: Vec<Value> = plan
        .targets
        .values()
        .map(|target| {
            json!({
                "name": target.name,
                "hwnd": target.hwnd,
                "token": target.token,
                "app_ref": target.app_ref,
                "title": target.title,
                "relaunched": target.relaunched
            })
        })
        .collect();
    let report = json!({
        "v": 1,
        "format": RUN_REPORT_FORMAT,
        "run_id": run_id,
        "script": plan.script.name,
        "started": started_at.to_rfc3339(),
        "finished": finished_at.to_rfc3339(),
        "dry_run": false,
        "result": result,
        "outcome": outcome,
        "handoff": handoff,
        "steps": entries,
        "rebinds": rebinds,
        "relaunched": relaunched,
        "targets": targets,
        "checkpoint": checkpoint_report,
        "compensation": compensation,
        "warnings": warnings,
        "stats": {
            "total": stats.total,
            "ok": stats.ok,
            "failed": stats.failed,
            "skipped": stats.skipped,
            "readback_verified": stats.readback_verified
        },
        "elapsed_ms": elapsed_ms
    });
    let dispatch_state = if any_dispatched {
        DispatchState::Sent
    } else {
        DispatchState::NotSent
    };
    let verification = if outcome == "VERIFIED" {
        Verification::Satisfied
    } else {
        Verification::Unknown
    };
    Ok(ToolExecution {
        data: report,
        dispatch_state,
        effect: Some(if any_dispatched {
            Effect::Unverifiable
        } else {
            Effect::Read
        }),
        verification,
        requires_observation: true,
    })
}

/// D4：主运行失败后的补偿子运行（AC-TXN-003）。
///
/// - `apply=false`（默认）：只把补偿脚本编译成 dry-run 计划，不下发真实输入；
/// - `apply=true`：真实执行，但风险上限固定 `bounded_write`、`on_error=stop`，
///   且不再触发二级补偿（避免递归放大风险）；
/// - 补偿自身失败不改写主运行的结果，只记录在 `compensation.error`。
async fn run_compensation(
    engine: &Arc<Engine>,
    session: &Arc<Session>,
    spec: &Value,
    cancel: &CancelFlag,
) -> Value {
    let apply = spec.get("apply").and_then(Value::as_bool).unwrap_or(false);
    let document = if let Some(inline) = spec.get("script").filter(|value| value.is_object()) {
        inline.clone()
    } else if let Some(raw) = spec.get("path").and_then(Value::as_str) {
        match crate::tools::normalize_allowed_path(engine, raw) {
            Ok(path) => match std::fs::read(&path) {
                Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                    Ok(value) => value,
                    Err(error) => {
                        return compensation_error(
                            apply,
                            "invalid_params",
                            &format!(
                                "compensation file {} is not valid JSON: {error}",
                                path.display()
                            ),
                        )
                    }
                },
                Err(error) => {
                    return compensation_error(
                        apply,
                        "invalid_params",
                        &format!("cannot read compensation file {}: {error}", path.display()),
                    )
                }
            },
            Err(error) => return compensation_error(apply, error.code.as_str(), &error.message),
        }
    } else {
        return compensation_error(
            apply,
            "invalid_params",
            "compensation needs exactly one of script (inline) or path (file)",
        );
    };
    let inner = json!({
        "script": document,
        "dry_run": !apply,
        "risk_max": "bounded_write",
        "on_error": "stop"
    });
    // 递归点必须 Box::pin：run_with -> execute_plan -> run_compensation -> run_with。
    match Box::pin(run_with(engine, session, &inner, cancel, true)).await {
        Ok(execution) => json!({ "applied": apply, "report": execution.data }),
        Err(error) => compensation_error(apply, error.code.as_str(), &error.message),
    }
}

fn compensation_error(applied: bool, code: &str, message: &str) -> Value {
    json!({ "applied": applied, "error": { "code": code, "message": message } })
}

/// 一步的完整执行（含重绑、重试、核验、断言）。
#[allow(clippy::too_many_arguments)]
async fn execute_step(
    engine: &Arc<Engine>,
    session: &Arc<Session>,
    run_id: &str,
    step: &ScriptStep,
    tool: ToolName,
    ctx: &mut Ctx,
    plan: &Plan,
    cancel: &CancelFlag,
    rebinds: &mut Vec<Value>,
    rebind_budget: &mut u32,
    targets_used: &[String],
    rebind_reason: &mut Option<String>,
    any_dispatched: &mut bool,
    stats: &mut Stats,
    warnings: &mut Vec<String>,
) -> Value {
    // 执行前的目标新鲜度检查（句柄/指纹失效 → 立即重绑，§4.4）。
    for name in targets_used {
        let stale = ctx
            .targets
            .get(name)
            .and_then(|target| target_needs_rebind(target).map(|reason| (reason, target.clone())));
        if let Some((reason, old)) = stale {
            if *rebind_budget == 0 {
                warnings.push(format!(
                    "step {}: rebind budget exhausted while refreshing target '{name}'",
                    step.id
                ));
                break;
            }
            let spec = plan
                .script
                .targets
                .get(name)
                .cloned()
                .unwrap_or(Value::Null);
            match resolve_target(session, name, &spec, None) {
                Ok(new) => {
                    *rebind_budget -= 1;
                    rebinds.push(json!({
                        "step": step.id,
                        "reason": reason,
                        "from_hwnd": old.hwnd,
                        "to_hwnd": new.hwnd,
                        "relaunched": new.relaunched
                    }));
                    *rebind_reason = Some(reason);
                    ctx.targets.insert(name.clone(), new);
                }
                Err(error) => {
                    warnings.push(format!(
                        "step {}: target '{name}' is stale ({reason}) and could not be re-resolved: {}",
                        step.id, error.message
                    ));
                }
            }
        }
    }

    let max_attempts = step.retry.unwrap_or(0) + 1;
    let mut attempt = 0u32;
    let mut retries = 0u32;
    let mut rebind_retried = false;
    let mut entry = json!({
        "id": step.id,
        "tool": step.tool,
        "ok": false,
        "dispatch_state": crate::audit::dispatch_state_str(DispatchState::NotSent),
        "effect": Value::Null,
        "verification": crate::audit::verification_str(Verification::Unknown),
        "verify_tier": step
            .verify
            .as_ref()
            .and_then(|verify| verify.get("tier"))
            .and_then(Value::as_str),
        "rebind": rebind_reason.clone(),
        "retries": 0,
        "duration_ms": 0,
        "error": Value::Null
    });

    loop {
        attempt += 1;
        let step_started = Instant::now();
        let params = match substitute(&step.params, ctx, None) {
            Ok(params) => params,
            Err(message) => {
                entry["duration_ms"] = json!(step_started.elapsed().as_millis() as u64);
                entry["error"] = StepFailure::local("unresolved_reference", message).to_value();
                entry["dispatch_state"] =
                    json!(crate::audit::dispatch_state_str(DispatchState::NotSent));
                entry["retries"] = json!(retries);
                return entry;
            }
        };
        let result = engine
            .execute_leaf_with_session(
                session.clone(),
                tool,
                params.clone(),
                Some(format!("{run_id}:{}", step.id)),
                cancel.clone(),
                AuditSource::Daemon,
            )
            .await;
        let result_value = tool_result_value(&result);
        let duration_ms = step_started.elapsed().as_millis() as u64;

        entry["duration_ms"] = json!(duration_ms);
        entry["dispatch_state"] =
            json!(crate::audit::dispatch_state_str(result.meta.dispatch_state));
        entry["effect"] = json!(crate::audit::effect_str(result.meta.effect));
        entry["retries"] = json!(retries);
        if result.meta.dispatch_state != DispatchState::NotSent {
            *any_dispatched = true;
        }

        let mut failure: Option<StepFailure> = None;
        if !result.ok {
            failure = Some(StepFailure::from_result(&result));
        }

        // expect 断言。
        if failure.is_none() {
            if let Some(expect) = &step.expect {
                if let Err(message) = eval_expect(expect, &result_value) {
                    failure = Some(StepFailure {
                        code: "assert_failed".to_string(),
                        message,
                        dispatch_state: result.meta.dispatch_state,
                        effect_may_have_occurred: result.meta.dispatch_state
                            != DispatchState::NotSent,
                        details: None,
                        retryable: false,
                    });
                }
            }
        }

        // 核验（verify）。
        if failure.is_none() {
            if let Some(verify) = &step.verify {
                let tier = verify
                    .get("tier")
                    .and_then(Value::as_str)
                    .unwrap_or("screen");
                let outcome = if tier == "readback" {
                    verify_readback(engine, session, run_id, &step.id, verify, ctx, cancel).await
                } else {
                    verify_screen(engine, session, run_id, &step.id, verify, cancel).await
                };
                entry["verify_tier"] = json!(outcome.tier);
                entry["verification"] = json!(if outcome.ok {
                    crate::audit::verification_str(Verification::Satisfied)
                } else {
                    crate::audit::verification_str(Verification::NoChange)
                });
                entry["verify"] = outcome.detail.clone();
                if outcome.ok {
                    if outcome.tier == "readback" {
                        stats.readback_verified += 1;
                    }
                } else {
                    let code = if outcome.tier == "readback" {
                        "readback_failed"
                    } else {
                        "verify_failed"
                    };
                    failure = Some(StepFailure {
                        code: code.to_string(),
                        message: outcome
                            .failure
                            .unwrap_or_else(|| "verification failed".to_string()),
                        dispatch_state: result.meta.dispatch_state,
                        effect_may_have_occurred: outcome.tier == "readback"
                            || result.meta.dispatch_state != DispatchState::NotSent,
                        details: Some(outcome.detail.clone()),
                        retryable: false,
                    });
                }
            }
        }

        // 步骤结果登记（供 $steps.<id> 引用，含失败）。
        ctx.steps.insert(
            step.id.clone(),
            json!({
                "ok": result.ok,
                "result": result_value,
                "verification": entry["verification"],
                "dispatch_state": entry["dispatch_state"]
            }),
        );

        match failure {
            None => {
                entry["ok"] = json!(true);
                entry["rebind"] = json!(rebind_reason.clone());
                return entry;
            }
            Some(failure) => {
                let retryable_not_sent = failure.retryable
                    && failure.dispatch_state == DispatchState::NotSent
                    && attempt < max_attempts;
                if retryable_not_sent {
                    retries += 1;
                    continue;
                }
                // 目标相关失败 + not_sent：自动重绑一次并重试（§4.4）。
                if failure.dispatch_state == DispatchState::NotSent
                    && !rebind_retried
                    && !targets_used.is_empty()
                {
                    rebind_retried = true;
                    let mut rebound = false;
                    for name in targets_used {
                        if *rebind_budget == 0 {
                            break;
                        }
                        let spec = plan
                            .script
                            .targets
                            .get(name)
                            .cloned()
                            .unwrap_or(Value::Null);
                        let old = ctx.targets.get(name).cloned();
                        if let Ok(new) = resolve_target(session, name, &spec, None) {
                            *rebind_budget -= 1;
                            rebound = true;
                            if let Some(old) = &old {
                                rebinds.push(json!({
                                    "step": step.id,
                                    "reason": "stale_target",
                                    "from_hwnd": old.hwnd,
                                    "to_hwnd": new.hwnd,
                                    "relaunched": new.relaunched
                                }));
                            }
                            *rebind_reason = Some("stale_target".to_string());
                            ctx.targets.insert(name.clone(), new);
                        }
                    }
                    if rebound {
                        entry["rebind"] = json!(rebind_reason.clone());
                        continue;
                    }
                }
                entry["error"] = failure.to_value();
                entry["rebind"] = json!(rebind_reason.clone());
                return entry;
            }
        }
    }
}

fn collect_set(step: &ScriptStep, ctx: &mut Ctx, warnings: &mut Vec<String>) {
    let Some(set) = step.set.as_ref().and_then(Value::as_object) else {
        return;
    };
    let current = ctx
        .steps
        .get(&step.id)
        .and_then(|value| value.get("result"))
        .cloned();
    for (key, expression) in set {
        match expression {
            Value::String(raw) if raw.starts_with('$') => {
                let symbol = &raw[1..];
                let value = if symbol == "result" || symbol.starts_with("result.") {
                    // $result → 当前步骤的工具结果 JSON。
                    let Some(current) = current.as_ref() else {
                        continue;
                    };
                    let segments: Vec<&str> = symbol.split('.').collect();
                    navigate(current, &segments[1..]).cloned()
                } else {
                    resolve_ref(symbol, ctx, None).ok()
                };
                match value {
                    Some(value) => {
                        ctx.vars.insert(key.clone(), value);
                    }
                    None => warnings.push(format!(
                        "step {}: set variable '{key}' could not be resolved from {}",
                        step.id, raw
                    )),
                }
            }
            other => {
                ctx.vars.insert(key.clone(), other.clone());
            }
        }
    }
}

struct FailureRecord {
    step: String,
    failure: StepFailure,
    targets: Vec<String>,
}

fn entry_failure(entry: &Value) -> StepFailure {
    let error = entry.get("error").cloned().unwrap_or(Value::Null);
    let dispatch_state = match entry.get("dispatch_state").and_then(Value::as_str) {
        Some("sent") => DispatchState::Sent,
        Some("partial") => DispatchState::Partial,
        Some("unknown") => DispatchState::Unknown,
        _ => DispatchState::NotSent,
    };
    StepFailure {
        code: error
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("step_failed")
            .to_string(),
        message: error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("step failed")
            .to_string(),
        dispatch_state,
        effect_may_have_occurred: dispatch_state != DispatchState::NotSent,
        details: error
            .get("details")
            .cloned()
            .filter(|value| !value.is_null()),
        retryable: false,
    }
}

#[derive(Default)]
struct Stats {
    total: u64,
    ok: u64,
    failed: u64,
    skipped: u64,
    readback_verified: u64,
}

/// outcome 分类（§4.5 映射表；"全部执行完但未声明强核验" → COMPLETED_UNVERIFIED）。
fn classify(
    failure: Option<&FailureRecord>,
    canceled: bool,
    stats: &Stats,
) -> (&'static str, &'static str) {
    if canceled {
        return ("failed", "CANCELED");
    }
    if let Some(record) = failure {
        let code = record.failure.code.as_str();
        let partial = stats.ok > 0;
        let result = if partial { "partial" } else { "failed" };
        if record.failure.dispatch_state == DispatchState::NotSent
            && !matches!(code, "readback_failed" | "verify_failed")
        {
            return (result, "HALTED_BEFORE_EFFECT");
        }
        if matches!(code, "readback_failed" | "verify_failed")
            || matches!(
                record.failure.dispatch_state,
                DispatchState::Partial | DispatchState::Unknown
            )
        {
            return (result, "RECONCILIATION_REQUIRED");
        }
        if record.failure.effect_may_have_occurred {
            return (result, "RECONCILIATION_REQUIRED");
        }
        return (result, "HALTED_BEFORE_EFFECT");
    }
    let declared_readback = stats.readback_verified > 0;
    if declared_readback && stats.failed == 0 {
        return ("ok", "VERIFIED");
    }
    ("ok", "COMPLETED_UNVERIFIED")
}

fn build_handoff(
    run_id: &str,
    outcome: &str,
    failure: Option<&FailureRecord>,
    _plan: &Plan,
) -> Value {
    let target_patch = |record: &FailureRecord| -> Value {
        let mut patch = Map::new();
        if let Some(details) = &record.failure.details {
            if let Some(candidate) = details
                .get("candidates")
                .and_then(Value::as_array)
                .and_then(|list| list.first())
            {
                if let (Some(name), Some(hwnd)) = (
                    record.targets.first(),
                    candidate.get("hwnd").and_then(Value::as_i64),
                ) {
                    patch.insert(name.clone(), json!(hwnd));
                }
            }
        }
        Value::Object(patch)
    };
    match outcome {
        "REJECTED_POLICY" => json!({
            "run_id": run_id,
            "failed_step": null,
            "reason": "preflight_rejected",
            "effect_may_have_occurred": false,
            "safe_to_retry": true,
            "resume_hint": null,
            "suggested_actions": [
                { "action": "fix_script", "detail": "fix the issues list, then run again from the start" }
            ]
        }),
        "CANCELED" => json!({
            "run_id": run_id,
            "failed_step": failure.map(|record| record.step.clone()),
            "reason": "canceled",
            "effect_may_have_occurred": failure
                .map(|record| record.failure.effect_may_have_occurred)
                .unwrap_or(false),
            "safe_to_retry": true,
            "resume_hint": null,
            "suggested_actions": [
                { "action": "manual", "detail": "confirm what already ran, then rerun or resume explicitly" }
            ]
        }),
        "HALTED_BEFORE_EFFECT" => {
            let Some(record) = failure else {
                return json!({
                    "run_id": run_id,
                    "failed_step": null,
                    "reason": null,
                    "effect_may_have_occurred": false,
                    "safe_to_retry": true,
                    "resume_hint": null,
                    "suggested_actions": []
                });
            };
            json!({
                "run_id": run_id,
                "failed_step": record.step,
                "reason": record.failure.code,
                "effect_may_have_occurred": false,
                "safe_to_retry": true,
                "resume_hint": {
                    "from_step": record.step,
                    "params_patch": target_patch(record)
                },
                "suggested_actions": [
                    { "action": "override_param", "detail": "fix the failing input (hwnd/param) and resume with --from <failed step>" },
                    { "action": "reobserve", "detail": "run desktop_semantic_observe to refresh locators, then --from <failed step>" },
                    { "action": "manual", "detail": "complete the step manually, then resume with --from <next step>" }
                ]
            })
        }
        "RECONCILIATION_REQUIRED" => {
            let record = failure;
            json!({
                "run_id": run_id,
                "failed_step": record.map(|record| record.step.clone()),
                "reason": record.map(|record| record.failure.code.clone()),
                "effect_may_have_occurred": true,
                "safe_to_retry": false,
                "resume_hint": null,
                "suggested_actions": [
                    { "action": "manual", "detail": "confirm whether the effect actually happened (read back the target), then decide whether to resume" },
                    { "action": "reobserve", "detail": "refresh the window/capture state before any retry" }
                ]
            })
        }
        _ => json!({
            "run_id": run_id,
            "failed_step": null,
            "reason": null,
            "effect_may_have_occurred": false,
            "safe_to_retry": true,
            "resume_hint": null,
            "suggested_actions": []
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script_document() -> Value {
        json!({
            "v": 1,
            "format": SCRIPT_FORMAT,
            "name": "unit-test",
            "targets": {
                "notepad": {
                    "match": { "process_name": "notepad.exe" },
                    "fingerprint": { "pid": 42, "class_name": "Notepad", "title": "Untitled" }
                }
            },
            "params": { "text": "hello" },
            "steps": [
                {
                    "id": "s1",
                    "tool": "desktop_window_activate",
                    "params": { "hwnd": "$target.notepad.hwnd" },
                    "set": { "hwnd": "$target.notepad.hwnd" }
                },
                {
                    "id": "s2",
                    "tool": "desktop_input",
                    "params": { "text": "$param.text", "hwnd": "$var.hwnd" },
                    "expect": { "ok": true },
                    "retry": 1
                }
            ]
        })
    }

    #[test]
    fn parses_minimal_script_and_rejects_bad_format() {
        let script = parse_document(&script_document()).expect("valid script");
        assert_eq!(script.name, "unit-test");
        assert_eq!(script.steps.len(), 2);
        assert_eq!(script.steps[1].retry, Some(1));
        assert_eq!(script.targets.len(), 1);

        for bad in [
            json!({}),
            json!({ "v": 2, "format": SCRIPT_FORMAT, "steps": [] }),
            json!({ "v": 1, "format": "other", "steps": [] }),
            json!({ "v": 1, "format": SCRIPT_FORMAT, "steps": [] }),
        ] {
            let issues = parse_document(&bad).expect_err("must reject");
            assert!(!issues.is_empty());
        }
    }

    #[test]
    fn rejects_step_ids_with_colon_and_duplicates() {
        let mut document = script_document();
        document["steps"][0]["id"] = json!("bad:id");
        let issues = parse_document(&document).expect_err("colon id must be rejected");
        assert!(issues.iter().any(|issue| issue.code == "invalid_step"));

        let mut duplicate = script_document();
        duplicate["steps"][1]["id"] = json!("s1");
        let issues = parse_document(&duplicate).expect_err("duplicate id must be rejected");
        assert!(issues.iter().any(|issue| issue.code == "invalid_step"));
    }

    #[test]
    fn substitutes_params_vars_targets_and_steps() {
        let mut ctx = Ctx {
            params: Map::new(),
            targets: HashMap::new(),
            vars: Map::new(),
            steps: Map::new(),
            session_id: "sess-test".into(),
        };
        ctx.params.insert("text".into(), json!("hello"));
        ctx.vars.insert("hwnd".into(), json!(1234));
        ctx.targets.insert(
            "notepad".into(),
            TargetRef {
                name: "notepad".into(),
                hwnd: 99,
                token: "target-token".into(),
                app_ref: "app:7:notepad".into(),
                title: "Untitled".into(),
                pid: 7,
                class_name: "Notepad".into(),
                spec: json!({}),
                relaunched: false,
            },
        );
        ctx.steps.insert(
            "s1".into(),
            json!({ "result": { "data": { "path": "out.txt" } } }),
        );

        let value = json!({
            "a": "$param.text",
            "b": "$target.notepad.hwnd",
            "c": "$var.hwnd",
            "d": "$steps.s1.result.data.path",
            "e": "prefix-$param.text/suffix",
            "f": 7,
            "g": "$target.notepad.pid",
            "h": "$target.notepad.class_name",
            "i": "$session.id"
        });
        let resolved = substitute(&value, &ctx, None).expect("resolve");
        assert_eq!(resolved["a"], "hello");
        assert_eq!(resolved["b"], 99);
        assert_eq!(resolved["c"], 1234);
        assert_eq!(resolved["d"], "out.txt");
        assert_eq!(resolved["e"], "prefix-hello/suffix");
        assert_eq!(resolved["f"], 7);
        assert_eq!(resolved["g"], 7);
        assert_eq!(resolved["h"], "Notepad");
        assert_eq!(resolved["i"], "sess-test");
    }

    #[test]
    fn rejects_unresolved_and_env_references() {
        let ctx = Ctx {
            params: Map::new(),
            targets: HashMap::new(),
            vars: Map::new(),
            steps: Map::new(),
            session_id: "sess-test".into(),
        };
        assert!(substitute(&json!("$param.missing"), &ctx, None).is_err());
        assert!(substitute(&json!("$target.none.hwnd"), &ctx, None).is_err());
        assert!(substitute(&json!("$env.PATH"), &ctx, None).is_err());
        assert!(substitute(&json!("$nonsense"), &ctx, None).is_err());
    }

    #[test]
    fn evaluates_expect_conditions() {
        let result = json!({ "ok": true, "data": { "path": "a/b.txt", "nested": { "n": 3 } } });
        assert!(eval_expect(&json!({ "ok": true }), &result).is_ok());
        assert!(eval_expect(&json!({ "path": "nested.n", "equals": 3 }), &result).is_ok());
        assert!(eval_expect(&json!({ "path": "path", "matches": "b.txt" }), &result).is_ok());
        assert!(eval_expect(&json!({ "ok": false }), &result).is_err());
        assert!(eval_expect(&json!({ "path": "missing", "equals": 1 }), &result).is_err());
        assert!(eval_expect(&json!({ "path": "path", "not_matches": "a/" }), &result).is_err());
    }

    #[test]
    fn risk_max_defaults_to_bounded_write_and_requires_opt_in() {
        assert_eq!(
            parse_risk_max(None, false).unwrap(),
            RiskClass::BoundedWrite
        );
        assert_eq!(
            parse_risk_max(Some("external_commit"), false).unwrap(),
            RiskClass::ExternalCommit
        );
        assert_eq!(
            parse_risk_max(Some("external_commit"), true).unwrap(),
            RiskClass::ExternalCommit
        );
        assert!(parse_risk_max(Some("destructive_critical"), true).is_err());
        assert!(parse_risk_max(Some("nonsense"), false).is_err());
    }

    #[test]
    fn validate_static_flags_unknown_tools_risk_and_references() {
        let allowed = RiskClass::BoundedWrite;
        let script = parse_document(&script_document()).expect("valid");
        assert!(validate_static(&script, allowed).is_empty());

        let mut document = script_document();
        document["steps"][1]["tool"] = json!("desktop_not_a_tool");
        let script = parse_document(&document).expect("valid shape");
        let issues = validate_static(&script, allowed);
        assert!(issues.iter().any(|issue| issue.code == "unknown_tool"));

        let mut document = script_document();
        document["steps"][1]["tool"] = json!("desktop_script_run");
        let script = parse_document(&document).expect("valid shape");
        let issues = validate_static(&script, allowed);
        assert!(issues.iter().any(|issue| issue.code == "risk_exceeded"));

        let mut document = script_document();
        document["steps"][0]["params"] = json!({ "hwnd": "$var.unknown" });
        let script = parse_document(&document).expect("valid shape");
        let issues = validate_static(&script, allowed);
        assert!(issues.iter().any(|issue| issue.code == "unresolved_var"));
    }

    #[test]
    fn validate_static_rejects_external_commit_by_default() {
        let script = Script {
            name: "destructive".into(),
            created: None,
            purpose: None,
            targets: Map::new(),
            params: Map::new(),
            steps: vec![ScriptStep {
                id: "s1".into(),
                tool: "desktop_script_run".into(),
                params: json!({}),
                expect: None,
                set: None,
                verify: None,
                require_readback: None,
                optional: None,
                fragile: None,
                volatile: None,
                retry: None,
                note: None,
            }],
            notes: Vec::new(),
        };
        let issues = validate_static(&script, RiskClass::BoundedWrite);
        assert!(issues.iter().any(|issue| issue.code == "risk_exceeded"));
    }

    fn failure(code: &str, state: DispatchState) -> FailureRecord {
        FailureRecord {
            step: "s2".into(),
            failure: StepFailure {
                code: code.into(),
                message: "boom".into(),
                dispatch_state: state,
                effect_may_have_occurred: state != DispatchState::NotSent,
                details: None,
                retryable: false,
            },
            targets: vec!["notepad".into()],
        }
    }

    #[test]
    fn classifies_outcomes() {
        let stats = Stats {
            total: 2,
            ok: 1,
            failed: 1,
            ..Stats::default()
        };
        assert_eq!(
            classify(
                Some(&failure("stale_target", DispatchState::NotSent)),
                false,
                &stats
            )
            .1,
            "HALTED_BEFORE_EFFECT"
        );
        assert_eq!(
            classify(
                Some(&failure("unknown", DispatchState::Unknown)),
                false,
                &stats
            )
            .1,
            "RECONCILIATION_REQUIRED"
        );
        assert_eq!(
            classify(
                Some(&failure("readback_failed", DispatchState::Sent)),
                false,
                &stats
            )
            .1,
            "RECONCILIATION_REQUIRED"
        );
        assert_eq!(classify(None, true, &stats).1, "CANCELED");

        let verified = Stats {
            total: 1,
            ok: 1,
            readback_verified: 1,
            ..Stats::default()
        };
        assert_eq!(classify(None, false, &verified).1, "VERIFIED");

        let unverified = Stats {
            total: 1,
            ok: 1,
            ..Stats::default()
        };
        assert_eq!(classify(None, false, &unverified).1, "COMPLETED_UNVERIFIED");
    }

    #[test]
    fn handoff_carries_run_id_and_retry_guidance() {
        let plan = Plan {
            script: parse_document(&script_document()).expect("valid"),
            options: RunOptions {
                dry_run: false,
                params: Map::new(),
                on_error: "stop".into(),
                from_step: None,
                to_step: None,
                risk_max: None,
                allow_external_commit: false,
                allow_resume_after_unknown: false,
                no_fragile: false,
                checkpoint: None,
                compensation: None,
            },
            from: 0,
            to: 2,
            targets: HashMap::new(),
            skipped_fragile: HashSet::new(),
            warnings: Vec::new(),
            allowed_risk: RiskClass::BoundedWrite,
            params: Map::new(),
        };
        let halted = build_handoff(
            "run-abc",
            "HALTED_BEFORE_EFFECT",
            Some(&failure("stale_target", DispatchState::NotSent)),
            &plan,
        );
        assert_eq!(halted["run_id"], "run-abc");
        assert_eq!(halted["failed_step"], "s2");
        assert_eq!(halted["safe_to_retry"], true);
        assert_eq!(halted["resume_hint"]["from_step"], "s2");

        let reconcile = build_handoff(
            "run-abc",
            "RECONCILIATION_REQUIRED",
            Some(&failure("partial", DispatchState::Partial)),
            &plan,
        );
        assert_eq!(reconcile["safe_to_retry"], false);
        assert!(reconcile["resume_hint"].is_null());
        assert_eq!(reconcile["effect_may_have_occurred"], true);
    }
}
