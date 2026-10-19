//! `deskctl record` / `deskctl script` 子命令（D3）。
//!
//! 录制通过 daemon 会话内的 `desktop_recorder_start/stop` 完成；`stop` 的脚本
//! 由 CLI 本地落盘（避免把输出路径限制强加给调用方）。`validate` 完全本地执行，
//! 不需要 daemon；`run` 走 daemon 会话。

use std::path::{Path, PathBuf};

use deskctl_core::script;
use serde_json::{json, Value};

use crate::client;

pub struct Conn {
    pub socket: String,
    pub token_file: PathBuf,
    pub session: Option<String>,
    pub session_token: Option<String>,
}

impl Conn {
    fn session(&self) -> Result<(&str, Option<String>), String> {
        let session = self
            .session
            .as_deref()
            .ok_or_else(|| "--session <id> is required".to_string())?;
        let token = self
            .session_token
            .clone()
            .or_else(|| std::env::var("DESKCTL_SESSION_TOKEN").ok());
        Ok((session, token))
    }

    async fn call(&self, tool: &str, arguments: Value) -> Result<Value, String> {
        let (session, token) = self.session()?;
        client::request(
            &self.socket,
            &self.token_file,
            "tools/call",
            json!({
                "session_id": session,
                "session_token": token,
                "tool": tool,
                "arguments": arguments,
            }),
        )
        .await
    }
}

pub async fn record_start(
    conn: &Conn,
    include_reads: Option<String>,
    max_steps: Option<u32>,
    allow_relaunch: Option<bool>,
) -> Result<(Value, i32), String> {
    let mut args = json!({});
    if let Some(include_reads) = include_reads {
        args["include_reads"] = json!(include_reads);
    }
    if let Some(max_steps) = max_steps {
        args["max_steps"] = json!(max_steps);
    }
    if let Some(allow_relaunch) = allow_relaunch {
        args["allow_relaunch"] = json!(allow_relaunch);
    }
    let result = conn.call("desktop_recorder_start", args).await?;
    Ok((result.clone(), exit_code_for(&result)))
}

pub async fn record_stop(conn: &Conn, out: Option<PathBuf>) -> Result<(Value, i32), String> {
    let result = conn.call("desktop_recorder_stop", json!({})).await?;
    let code = exit_code_for(&result);
    if code == 0 {
        if let Some(out) = out {
            let script = result
                .pointer("/data/script")
                .ok_or_else(|| "recorder_stop returned no script".to_string())?;
            write_script(&out, script)?;
            let mut enriched = result.clone();
            if let Some(object) = enriched.as_object_mut() {
                object.insert("output".to_string(), json!(out.display().to_string()));
            }
            return Ok((enriched, code));
        }
    }
    Ok((result, code))
}

fn write_script(path: &Path, script: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create {}: {error}", parent.display()))?;
        }
    }
    let content = serde_json::to_string_pretty(script).map_err(|error| error.to_string())?;
    std::fs::write(path, format!("{content}\n"))
        .map_err(|error| format!("write {}: {error}", path.display()))
}

pub fn validate(
    file: &Path,
    risk_max: Option<String>,
    allow_external_commit: bool,
) -> Result<(Value, i32), String> {
    let document = read_document(file)?;
    let issues = script::validate_document(&document, risk_max.as_deref(), allow_external_commit)
        .map_err(|error| error.to_string())?;
    let ok = issues.is_empty();
    let value = json!({
        "ok": ok,
        "format": "deskctl-script-validate/1",
        "file": file.display().to_string(),
        "issues": issues.iter().map(|issue| issue.to_value()).collect::<Vec<_>>()
    });
    Ok((value, if ok { 0 } else { 2 }))
}

pub struct RunRequest<'a> {
    pub file: &'a Path,
    pub dry_run: bool,
    pub params: Vec<(String, String)>,
    pub on_error: Option<String>,
    pub from_step: Option<String>,
    pub to_step: Option<String>,
    pub risk_max: Option<String>,
    pub allow_external_commit: bool,
    pub allow_resume_after_unknown: bool,
    pub no_fragile: bool,
}

pub async fn run(conn: &Conn, request: RunRequest<'_>) -> Result<(Value, i32), String> {
    let RunRequest {
        file,
        dry_run,
        params,
        on_error,
        from_step,
        to_step,
        risk_max,
        allow_external_commit,
        allow_resume_after_unknown,
        no_fragile,
    } = request;
    let document = read_document(file)?;
    let mut params_object = serde_json::Map::new();
    for (key, value) in params {
        // `--param k=v`：值按 JSON 解析；失败时保留字符串（覆盖文本输入等场景）。
        let parsed = serde_json::from_str::<Value>(&value).unwrap_or(Value::String(value));
        params_object.insert(key, parsed);
    }
    let mut args = json!({
        "script": document,
        "dry_run": dry_run,
        "params": params_object,
        "allow_external_commit": allow_external_commit,
        "allow_resume_after_unknown": allow_resume_after_unknown,
        "no_fragile": no_fragile
    });
    if let Some(on_error) = on_error {
        args["on_error"] = json!(on_error);
    }
    if let Some(from_step) = from_step {
        args["from_step"] = json!(from_step);
    }
    if let Some(to_step) = to_step {
        args["to_step"] = json!(to_step);
    }
    if let Some(risk_max) = risk_max {
        args["risk_max"] = json!(risk_max);
    }
    let result = conn.call("desktop_script_run", args).await?;
    let code = exit_code_for(&result);
    // 工具调用成功但 run outcome 非成功（HALTED / RECONCILIATION_REQUIRED）时，
    // 退出码仍需反映失败，便于 CI/脚本分支。
    if code == 0 {
        let outcome = result.pointer("/data/outcome").and_then(Value::as_str);
        let result_kind = result.pointer("/data/result").and_then(Value::as_str);
        if matches!(
            outcome,
            Some("HALTED_BEFORE_EFFECT" | "RECONCILIATION_REQUIRED")
        ) || matches!(result_kind, Some("failed" | "partial"))
        {
            return Ok((result, 1));
        }
    }
    Ok((result, code))
}

fn read_document(file: &Path) -> Result<Value, String> {
    let content = std::fs::read_to_string(file)
        .map_err(|error| format!("read {}: {error}", file.display()))?;
    serde_json::from_str(&content).map_err(|error| format!("parse {}: {error}", file.display()))
}

pub fn exit_code_for(value: &Value) -> i32 {
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        return 0;
    }
    let code = value
        .pointer("/error/code")
        .and_then(Value::as_str)
        .unwrap_or("internal");
    exit_code_for_code(code)
}

pub fn exit_code_for_code(code: &str) -> i32 {
    crate::exit_code_for_code(code)
}
