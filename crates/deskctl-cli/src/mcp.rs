use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use deskctl_core::audit::AuditSource;
use deskctl_core::{Config, Engine};
use deskctl_protocol::{schema, ToolName, ToolProfile, ToolResult};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PROMPTS: &[(&str, &str)] = &[
    ("desktop-basic", "截图、窗口、鼠标和键盘的基础桌面操作"),
    ("desktop-window", "窗口枚举、激活、移动、缩放和信息读取"),
    ("desktop-text", "文本输入、快捷键、剪贴板"),
    ("desktop-semantic", "Windows UIA 目标绑定、观察、执行和验证"),
    (
        "desktop-recovery",
        "过期 token、窗口变化和 dispatch unknown 的恢复",
    ),
];

pub async fn run(profile: ToolProfile, skill_root: PathBuf) -> Result<(), String> {
    let engine = Engine::new(Config {
        profile,
        ..Config::default()
    });
    let info = engine
        .create_session(Some(profile), "mcp")
        .map_err(|error| error.to_string())?;
    let session = engine
        .get_session(&info.session_id, Some(&info.session_token))
        .map_err(|error| error.to_string())?;

    let stdin = tokio::io::stdin();
    let writer = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    let active: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut tasks = Vec::new();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();
    let mut initialized = false;

    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .await
            .map_err(|error| format!("read MCP input: {error}"))?;
        if read == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(error) => {
                write_response(
                    &writer,
                    None,
                    Err((-32700, format!("parse error: {error}"))),
                )
                .await?;
                continue;
            }
        };

        let id = request.get("id").cloned();
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

        if method.starts_with("notifications/") {
            if method == "notifications/cancelled" {
                if let Some(request_id) = params
                    .get("requestId")
                    .or_else(|| params.get("request_id"))
                    .map(value_to_id)
                {
                    if let Ok(active) = active.lock() {
                        if let Some(cancel) = active.get(&request_id) {
                            cancel.store(true, Ordering::Release);
                        }
                    }
                }
            } else if method == "notifications/initialized" {
                tracing::debug!("MCP client initialized");
            }
            continue;
        }

        if id.is_none() {
            continue;
        }

        if !initialized && method != "initialize" && method != "ping" {
            write_response(
                &writer,
                id,
                Err((-32002, "server is not initialized".to_string())),
            )
            .await?;
            continue;
        }

        let result = match method {
            "initialize" => {
                initialized = true;
                initialize_result(&params)
            }
            "ping" => Ok(json!({})),
            "tools/list" => {
                if initialized {
                    Ok(json!({
                        "tools": schema::tool_descriptors(session.profile)
                    }))
                } else {
                    Err((-32002, "server is not initialized".to_string()))
                }
            }
            "tools/call" => {
                let Some(name) = params.get("name").and_then(Value::as_str) else {
                    write_response(&writer, id, Err((-32602, "missing tool name".to_string())))
                        .await?;
                    continue;
                };
                match name.parse::<ToolName>() {
                    Ok(tool) => {
                        let arguments = params
                            .get("arguments")
                            .cloned()
                            .unwrap_or_else(|| json!({}));
                        let request_id = id.as_ref().map(value_to_id);
                        let progress_token = params
                            .get("_meta")
                            .and_then(|meta| meta.get("progressToken"))
                            .cloned();
                        let cancel: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
                        if let Some(request_id) = &request_id {
                            if let Ok(mut active) = active.lock() {
                                active.insert(request_id.clone(), cancel.clone());
                            }
                        }
                        let engine = engine.clone();
                        let session = session.clone();
                        let writer = writer.clone();
                        let active = active.clone();
                        let response_id = id.clone();
                        tasks.push(tokio::spawn(async move {
                            if let Some(token) = progress_token.clone() {
                                send_progress(&writer, token, 0, Some(1), "started").await;
                            }
                            let result: ToolResult = engine
                                .execute_with_session(
                                    session,
                                    tool,
                                    arguments,
                                    request_id.clone(),
                                    cancel,
                                    AuditSource::Mcp,
                                )
                                .await;
                            if let Some(token) = progress_token {
                                send_progress(&writer, token, 1, Some(1), "completed").await;
                            }
                            let is_error = !result.ok;
                            let text = serde_json::to_string(&result).unwrap_or_else(|_| {
                                "{\"ok\":false,\"error\":{\"code\":\"internal\"}}".to_string()
                            });
                            let response = json!({
                                "content": [{ "type": "text", "text": text }],
                                "isError": is_error,
                                "structuredContent": result,
                            });
                            let _ = write_response(&writer, response_id, Ok(response)).await;
                            if let Some(request_id) = request_id {
                                if let Ok(mut active) = active.lock() {
                                    active.remove(&request_id);
                                }
                            }
                        }));
                        continue;
                    }
                    Err(error) => {
                        write_response(&writer, id, Err((-32602, error))).await?;
                        continue;
                    }
                }
            }
            "prompts/list" => {
                let prompts: Vec<Value> = PROMPTS
                    .iter()
                    .map(|(name, description)| {
                        json!({
                            "name": name,
                            "description": description,
                            "arguments": []
                        })
                    })
                    .collect();
                Ok(json!({ "prompts": prompts }))
            }
            "prompts/get" => {
                let Some(name) = params.get("name").and_then(Value::as_str) else {
                    write_response(
                        &writer,
                        id,
                        Err((-32602, "missing prompt name".to_string())),
                    )
                    .await?;
                    continue;
                };
                let text =
                    read_skill(&skill_root, name).unwrap_or_else(|| fallback_skill_prompt(name));
                Ok(json!({
                    "description": format!("Deskctl skill {name}"),
                    "messages": [{
                        "role": "user",
                        "content": { "type": "text", "text": text }
                    }]
                }))
            }
            "shutdown" | "exit" => {
                if let Some(id) = id {
                    write_response(&writer, Some(id), Ok(json!({}))).await?;
                }
                break;
            }
            other => Err((-32601, format!("method not found: {other}"))),
        };

        match result {
            Ok(result) => write_response(&writer, id, Ok(result)).await?,
            Err((code, error)) => write_response(&writer, id, Err((code, error))).await?,
        }
    }
    for task in tasks {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), task).await;
    }
    if let Ok(active) = active.lock() {
        for cancel in active.values() {
            cancel.store(true, Ordering::Release);
        }
    }
    session.close();
    engine.flush_audit();
    Ok(())
}

fn initialize_result(params: &Value) -> Result<Value, (i32, String)> {
    let protocol = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("2024-11-05");
    let major = protocol.split('-').next().unwrap_or_default();
    if !matches!(major, "2024" | "2025") {
        return Err((
            -32602,
            format!("unsupported MCP protocol version: {protocol}"),
        ));
    }
    Ok(json!({
        "protocolVersion": protocol,
        "capabilities": {
            "tools": { "listChanged": false },
            "prompts": { "listChanged": false }
        },
        "serverInfo": {
            "name": "deskctl",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": "Deskctl exposes desktop_* tools. Use desktop_semantic_* for Windows UIA, and desktop_perceive for image coordinates. No approval flow is provided."
    }))
}

async fn write_response(
    stdout: &Arc<tokio::sync::Mutex<tokio::io::Stdout>>,
    id: Option<Value>,
    result: Result<Value, (i32, String)>,
) -> Result<(), String> {
    let payload = match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        }),
    };
    let mut encoded = serde_json::to_vec(&payload).map_err(|error| error.to_string())?;
    encoded.push(b'\n');
    let mut stdout = stdout.lock().await;
    stdout
        .write_all(&encoded)
        .await
        .map_err(|error| format!("write MCP output: {error}"))?;
    stdout
        .flush()
        .await
        .map_err(|error| format!("flush MCP output: {error}"))
}

async fn send_progress(
    stdout: &Arc<tokio::sync::Mutex<tokio::io::Stdout>>,
    token: Value,
    progress: u64,
    total: Option<u64>,
    message: &str,
) {
    let mut params = json!({
        "progressToken": token,
        "progress": progress,
        "message": message
    });
    if let Some(total) = total {
        params["total"] = json!(total);
    }
    let payload = json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": params
    });
    if let Ok(mut encoded) = serde_json::to_vec(&payload) {
        encoded.push(b'\n');
        let mut stdout = stdout.lock().await;
        let _ = stdout.write_all(&encoded).await;
        let _ = stdout.flush().await;
    }
}

fn value_to_id(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}

fn read_skill(root: &Path, name: &str) -> Option<String> {
    if !PROMPTS.iter().any(|(known, _)| *known == name) {
        return None;
    }
    let raw = std::fs::read_to_string(root.join(name).join("SKILL.md")).ok()?;
    Some(strip_frontmatter(&raw).to_string())
}

/// Drop the YAML frontmatter so MCP prompts stay body-only. The frontmatter is
/// skill-discovery metadata (Claude/Codex format); prompt text only needs the
/// markdown body.
fn strip_frontmatter(content: &str) -> &str {
    let text = content.strip_prefix('\u{feff}').unwrap_or(content);
    let Some(rest) = text.strip_prefix("---") else {
        return text;
    };
    let Some(rest) = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
    else {
        return text;
    };
    match rest.find("\n---") {
        Some(end) => rest[end + 4..].trim_start_matches(['\r', '\n']),
        None => text,
    }
}

fn fallback_skill_prompt(name: &str) -> String {
    format!(
        "# {name}\n\nUse the corresponding `desktop_*` tools. Re-observe after every action; \
         never retry an action whose dispatch_state is sent, partial or unknown. \
         Deskctl has no approval flow."
    )
}

#[cfg(test)]
mod tests {
    use super::strip_frontmatter;

    #[test]
    fn strip_frontmatter_removes_yaml_header() {
        let raw = "---\nname: demo\ndescription: \"demo skill\"\n---\n\n# Demo\n\nBody.\n";
        assert_eq!(strip_frontmatter(raw), "# Demo\n\nBody.\n");
    }

    #[test]
    fn strip_frontmatter_handles_crlf_and_plain_body() {
        let raw = "---\r\nname: demo\r\n---\r\nBody";
        assert_eq!(strip_frontmatter(raw), "Body");
        let plain = "# Demo\n\nNo frontmatter here.";
        assert_eq!(strip_frontmatter(plain), plain);
    }
}
