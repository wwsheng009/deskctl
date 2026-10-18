//! Developer-facing latency measurements against a running daemon.
//!
//! `deskctl bench` reuses the ordinary CLI client path, so the numbers cover
//! process-to-daemon IPC plus the tool itself. It is hidden from `--help`.

use crate::client;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub struct BenchOptions {
    pub op: String,
    pub count: u32,
    pub warmup: u32,
    pub parallel: u32,
    pub socket: String,
    pub token_file: PathBuf,
}

pub async fn run(options: BenchOptions) -> Result<i32, String> {
    match options.op.as_str() {
        "screen_size" | "screenshot" | "window_screenshot" => sequential(&options).await,
        "parallel" => parallel_sessions(&options).await,
        other => Err(format!(
            "unknown bench op '{other}'; expected screen_size, screenshot, window_screenshot or parallel"
        )),
    }
}

async fn call_tool(
    socket: &str,
    token_file: &Path,
    session_id: &str,
    session_token: &str,
    tool: &str,
    arguments: Value,
) -> Result<Value, String> {
    client::request(
        socket,
        token_file,
        "tools/call",
        json!({
            "session_id": session_id,
            "session_token": session_token,
            "tool": tool,
            "arguments": arguments,
        }),
    )
    .await
}

async fn create_session(socket: &str, token_file: &Path) -> Result<(String, String), String> {
    let info = client::request(
        socket,
        token_file,
        "session/create",
        json!({ "profile": "standard" }),
    )
    .await?;
    let session_id = info
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "session/create returned no session_id".to_string())?
        .to_string();
    let session_token = info
        .get("session_token")
        .and_then(Value::as_str)
        .ok_or_else(|| "session/create returned no session_token".to_string())?
        .to_string();
    Ok((session_id, session_token))
}

async fn close_session(
    socket: &str,
    token_file: &Path,
    session_id: &str,
    session_token: &str,
) -> Result<Value, String> {
    client::request(
        socket,
        token_file,
        "session/close",
        json!({ "session_id": session_id, "session_token": session_token }),
    )
    .await
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn stats(mut samples: Vec<f64>) -> Value {
    if samples.is_empty() {
        return json!({ "count": 0 });
    }
    samples.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let len = samples.len();
    let pick = |quantile: f64| -> f64 {
        let index = ((len as f64 - 1.0) * quantile).round() as usize;
        samples[index.min(len - 1)]
    };
    json!({
        "count": len,
        "p50_ms": round2(pick(0.50)),
        "p95_ms": round2(pick(0.95)),
        "max_ms": round2(samples[len - 1]),
    })
}

fn hwnd_of(window: &Value) -> Option<i64> {
    window.get("hwnd").and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
    })
}

struct WindowTarget {
    hwnd: i64,
    width: u64,
    height: u64,
}

async fn select_window_target(
    socket: &str,
    token_file: &Path,
    session_id: &str,
    session_token: &str,
) -> Result<WindowTarget, String> {
    let listing = call_tool(
        socket,
        token_file,
        session_id,
        session_token,
        "desktop_windows_list",
        json!({}),
    )
    .await?;
    let windows = listing
        .pointer("/data/windows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // Prefer the foreground window, then visible, non-minimized windows: the
    // result stays comparable across runs instead of depending on z-order.
    let mut candidates: Vec<(u8, i64)> = Vec::new();
    for window in &windows {
        let title = window.get("title").and_then(Value::as_str).unwrap_or("");
        if title.is_empty() {
            continue;
        }
        let Some(hwnd) = hwnd_of(window) else {
            continue;
        };
        let minimized = window
            .get("minimized")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let foreground = window
            .get("foreground")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let rank = if foreground && !minimized {
            0
        } else if !minimized {
            1
        } else {
            2
        };
        candidates.push((rank, hwnd));
    }
    candidates.sort_by_key(|(rank, _)| *rank);
    let mut last_error = "no titled window available for window_screenshot bench".to_string();
    for (_, hwnd) in candidates {
        // Probe the candidate once: composited render targets can return
        // empty captures, and those windows must not fail the whole bench.
        match call_tool(
            socket,
            token_file,
            session_id,
            session_token,
            "desktop_window_screenshot",
            json!({ "hwnd": hwnd }),
        )
        .await
        {
            Ok(value) if value.get("ok").and_then(Value::as_bool) == Some(true) => {
                return Ok(WindowTarget {
                    hwnd,
                    width: value
                        .pointer("/data/width")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    height: value
                        .pointer("/data/height")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                });
            }
            Ok(value) => {
                last_error = format!(
                    "window {hwnd} capture failed: {}",
                    value.get("error").cloned().unwrap_or(Value::Null)
                );
            }
            Err(error) => {
                last_error = format!("window {hwnd} capture failed: {error}");
            }
        }
    }
    Err(last_error)
}

fn tool_name_for(op: &str) -> &'static str {
    match op {
        "screen_size" => "desktop_screen_size",
        "window_screenshot" => "desktop_window_screenshot",
        _ => "desktop_screenshot",
    }
}

async fn sequential(options: &BenchOptions) -> Result<i32, String> {
    let (session_id, session_token) = create_session(&options.socket, &options.token_file).await?;
    let report = sequential_inner(options, &session_id, &session_token).await;
    let closed = close_session(
        &options.socket,
        &options.token_file,
        &session_id,
        &session_token,
    )
    .await;
    let mut report = report?;
    if let Some(object) = report.as_object_mut() {
        object.insert("session_closed".to_string(), json!(closed.is_ok()));
    }
    let failures = report
        .get("failures")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    );
    Ok(if failures == 0 { 0 } else { 1 })
}

async fn sequential_inner(
    options: &BenchOptions,
    session_id: &str,
    session_token: &str,
) -> Result<Value, String> {
    let tool = tool_name_for(&options.op);
    let mut window = Value::Null;
    let arguments = if options.op == "window_screenshot" {
        let target = select_window_target(
            &options.socket,
            &options.token_file,
            session_id,
            session_token,
        )
        .await?;
        window = json!({
            "hwnd": target.hwnd,
            "width": target.width,
            "height": target.height,
        });
        json!({ "hwnd": target.hwnd })
    } else {
        json!({})
    };
    let total = options.warmup.saturating_add(options.count);
    let mut samples = Vec::new();
    let mut failures = Vec::new();
    for index in 0..total {
        let started = Instant::now();
        let result = call_tool(
            &options.socket,
            &options.token_file,
            session_id,
            session_token,
            tool,
            arguments.clone(),
        )
        .await;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        let measured = index >= options.warmup;
        match result {
            Ok(value) => {
                if value.get("ok").and_then(Value::as_bool) == Some(true) {
                    if measured {
                        samples.push(elapsed);
                    }
                } else {
                    failures.push(value.get("error").cloned().unwrap_or(Value::Null));
                }
            }
            Err(error) => failures.push(Value::String(error)),
        }
    }
    Ok(json!({
        "op": options.op,
        "tool": tool,
        "warmup": options.warmup,
        "window": window,
        "latency": stats(samples),
        "failures": failures,
    }))
}

async fn parallel_sessions(options: &BenchOptions) -> Result<i32, String> {
    let sessions_wanted = options.parallel.max(2);
    let rounds = options.count.max(1);
    let mut sessions = Vec::new();
    for _ in 0..sessions_wanted {
        sessions.push(create_session(&options.socket, &options.token_file).await?);
    }

    let mut samples = Vec::new();
    let mut failures = Vec::new();
    let mut unique_per_round = Vec::new();
    for round in 0..rounds {
        let mut handles = Vec::new();
        for (session_id, session_token) in &sessions {
            let socket = options.socket.clone();
            let token_file = options.token_file.clone();
            let session_id = session_id.clone();
            let session_token = session_token.clone();
            handles.push(tokio::spawn(async move {
                let started = Instant::now();
                let result = call_tool(
                    &socket,
                    &token_file,
                    &session_id,
                    &session_token,
                    "desktop_screenshot",
                    json!({}),
                )
                .await;
                (started.elapsed().as_secs_f64() * 1000.0, result)
            }));
        }
        let mut ids = std::collections::BTreeSet::new();
        for handle in handles {
            match handle.await {
                Ok((elapsed, Ok(value))) => {
                    if value.get("ok").and_then(Value::as_bool) == Some(true) {
                        samples.push(elapsed);
                        match value.pointer("/data/capture_id").and_then(Value::as_str) {
                            Some(id) if ids.insert(id.to_string()) => {}
                            Some(id) => failures.push(Value::String(format!(
                                "round {round}: duplicate capture_id {id}"
                            ))),
                            None => failures.push(Value::String(format!(
                                "round {round}: response without capture_id"
                            ))),
                        }
                    } else {
                        failures.push(Value::String(format!(
                            "round {round}: {}",
                            value.get("error").cloned().unwrap_or(Value::Null)
                        )));
                    }
                }
                Ok((_, Err(error))) => {
                    failures.push(Value::String(format!("round {round}: {error}")))
                }
                Err(error) => {
                    failures.push(Value::String(format!("round {round}: join error: {error}")))
                }
            }
        }
        unique_per_round.push(ids.len());
    }

    let mut closed = 0usize;
    for (session_id, session_token) in &sessions {
        if close_session(
            &options.socket,
            &options.token_file,
            session_id,
            session_token,
        )
        .await
        .is_ok()
        {
            closed += 1;
        }
    }

    let report = json!({
        "op": "parallel",
        "sessions": sessions_wanted,
        "rounds": rounds,
        "sessions_closed": closed,
        "latency": stats(samples),
        "unique_capture_ids_per_round": unique_per_round,
        "failures": failures,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    );
    Ok(
        if report
            .get("failures")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            0
        } else {
            1
        },
    )
}
