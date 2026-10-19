use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::{client, output};

#[derive(Debug, Deserialize)]
struct BatchRequest {
    tool: String,
    #[serde(default = "empty_object")]
    arguments: Value,
    #[serde(default)]
    request_id: Option<String>,
}

fn empty_object() -> Value {
    json!({})
}

pub struct BatchOptions {
    pub file: PathBuf,
    pub session: String,
    pub session_token: Option<String>,
    pub socket: String,
    pub token_file: PathBuf,
    pub output_dir: PathBuf,
    pub compact: bool,
    pub stop_on_error: bool,
}

/// Run a JSONL batch sequentially in one daemon session. Each result is written
/// to `output_dir`, and one JSON summary line is printed per request.
pub async fn run(options: BatchOptions) -> Result<i32, String> {
    let text = std::fs::read_to_string(&options.file).map_err(|error| {
        format!(
            "invalid_params: read batch file '{}': {error}",
            options.file.display()
        )
    })?;
    std::fs::create_dir_all(&options.output_dir).map_err(|error| {
        format!(
            "invalid_params: create output directory '{}': {error}",
            options.output_dir.display()
        )
    })?;

    let mut total = 0usize;
    let mut failures = 0usize;
    for (line_index, raw) in text.lines().enumerate() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        total += 1;
        let request: BatchRequest = serde_json::from_str(raw).map_err(|error| {
            format!(
                "invalid_params: {}:{}: {error}",
                options.file.display(),
                line_index + 1
            )
        })?;
        let BatchRequest {
            tool,
            arguments,
            request_id,
        } = request;
        let parameters = if arguments.is_null() {
            json!({})
        } else {
            arguments
        };
        let response = client::request(
            &options.socket,
            &options.token_file,
            "tools/call",
            json!({
                "session_id": &options.session,
                "session_token": &options.session_token,
                "tool": &tool,
                "arguments": parameters,
                "request_id": request_id,
            }),
        )
        .await;

        match response {
            Ok(value) => {
                let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(false);
                let path = output_path(&options.output_dir, total, &tool);
                output::print_json_with_options(&value, options.compact, Some(&path))?;
                let summary = json!({
                    "index": total,
                    "tool": tool,
                    "ok": ok,
                    "output": path.display().to_string(),
                    "error": if ok {
                        Value::Null
                    } else {
                        value.get("error").cloned().unwrap_or(Value::Null)
                    }
                });
                println!(
                    "{}",
                    serde_json::to_string(&summary).map_err(|error| error.to_string())?
                );
                if !ok {
                    failures += 1;
                    if options.stop_on_error {
                        break;
                    }
                }
            }
            Err(error) => {
                failures += 1;
                let summary = json!({
                    "index": total,
                    "tool": tool,
                    "ok": false,
                    "output": Value::Null,
                    "error": error
                });
                println!(
                    "{}",
                    serde_json::to_string(&summary).map_err(|error| error.to_string())?
                );
                if options.stop_on_error {
                    break;
                }
            }
        }
    }

    eprintln!("deskctl batch: {total} requests, {failures} failed");
    Ok(if failures == 0 { 0 } else { 1 })
}

fn output_path(dir: &Path, index: usize, tool: &str) -> PathBuf {
    let safe: String = tool
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() {
        "tool".to_string()
    } else {
        safe
    };
    dir.join(format!("{index:03}-{safe}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonl_requests_and_sanitizes_output_names() {
        let request: BatchRequest =
            serde_json::from_str(r#"{"tool":"desktop_perceive","arguments":{"text_only":true}}"#)
                .expect("batch request");
        assert_eq!(request.tool, "desktop_perceive");
        assert_eq!(request.arguments["text_only"], true);

        let path = output_path(Path::new("out"), 7, "desktop_perceive");
        assert_eq!(path, Path::new("out").join("007-desktop_perceive.json"));
        let path = output_path(Path::new("out"), 1, "a/b");
        assert_eq!(path, Path::new("out").join("001-a_b.json"));
    }
}
