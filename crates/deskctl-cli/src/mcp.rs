use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use deskctl_core::{Config, Engine};
use deskctl_protocol::{schema, ToolName, ToolProfile, ToolResult};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PROMPTS: &[(&str, &str)] = &[
    ("desktop-basic", "截图、窗口、鼠标和键盘的基础桌面操作"),
    ("desktop-window", "窗口枚举、激活、移动、缩放和信息读取"),
    ("desktop-text", "文本输入、快捷键、剪贴板"),
    ("desktop-semantic", "Windows UIA 目标绑定、观察、执行和验证"),
    ("desktop-recovery", "过期 token、窗口变化和 dispatch unknown 的恢复"),
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
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();

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
                    &mut stdout,
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
            if method == "notifications/initialized" {
                tracing::debug!("MCP client initialized");
            }
            continue;
        }

        let result = match method {
            "initialize" => Ok(initialize_result(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({
                "tools": schema::tool_descriptors(session.profile)
            })),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "missing tool name".to_string());
                match name {
                    Ok(name) => match name.parse::<ToolName>() {
                        Ok(tool) => {
                            let arguments = params
                                .get("arguments")
                                .cloned()
                                .unwrap_or_else(|| json!({}));
                            let request_id = id.as_ref().map(value_to_id);
                            let result: ToolResult = engine
                                .execute_with_session(
                                    session.clone(),
                                    tool,
                                    arguments,
                                    request_id,
                                    Arc::new(AtomicBool::new(false)),
                                )
                                .await;
                            let text = serde_json::to_string(&result)
                                .map_err(|error| format!("serialize tool result: {error}"))?;
                            Ok(json!({
                                "content": [{ "type": "text", "text": text }],
                                "isError": !result.ok,
                                "structuredContent": result,
                            }))
                        }
                        Err(error) => Err(error),
                    },
                    Err(error) => Err(error),
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
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "missing prompt name".to_string());
                match name {
                    Ok(name) => {
                        let text = read_skill(&skill_root, name)
                            .unwrap_or_else(|| fallback_skill_prompt(name));
                        Ok(json!({
                            "description": format!("Deskctl skill {name}"),
                            "messages": [{
                                "role": "user",
                                "content": { "type": "text", "text": text }
                            }]
                        }))
                    }
                    Err(error) => Err(error),
                }
            }
            "shutdown" | "exit" => {
                if let Some(id) = id {
                    write_response(&mut stdout, Some(id), Ok(json!({}))).await?;
                }
                break;
            }
            other => Err(format!("method not found: {other}")),
        };

        match result {
            Ok(result) => write_response(&mut stdout, id, Ok(result)).await?,
            Err(error) => {
                let code = if error.starts_with("method not found") {
                    -32601
                } else {
                    -32602
                };
                write_response(&mut stdout, id, Err((code, error))).await?;
            }
        }
    }
    Ok(())
}

fn initialize_result(params: &Value) -> Value {
    let protocol = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("2024-11-05");
    json!({
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
    })
}

async fn write_response(
    stdout: &mut tokio::io::Stdout,
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
    stdout
        .write_all(&encoded)
        .await
        .map_err(|error| format!("write MCP output: {error}"))?;
    stdout
        .flush()
        .await
        .map_err(|error| format!("flush MCP output: {error}"))
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
    std::fs::read_to_string(root.join(name).join("SKILL.md")).ok()
}

fn fallback_skill_prompt(name: &str) -> String {
    format!(
        "# {name}\n\nUse the corresponding `desktop_*` tools. Re-observe after every action; \
         never retry an action whose dispatch_state is sent, partial or unknown. \
         Deskctl has no approval flow."
    )
}
