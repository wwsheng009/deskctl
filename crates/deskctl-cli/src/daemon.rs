use deskctl_core::audit::AuditSource;
use deskctl_core::Engine;
use deskctl_protocol::{schema, ToolName, ToolProfile, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::watch;

#[derive(Debug, Deserialize)]
struct Request {
    #[serde(default)]
    token: String,
    #[serde(default)]
    id: Option<String>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct Response {
    id: Option<String>,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

struct DaemonState {
    engine: Arc<Engine>,
    token: String,
    shutdown: watch::Sender<bool>,
    active: std::sync::Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>,
}

pub async fn run(
    socket: String,
    token_file: PathBuf,
    engine: Arc<Engine>,
    idle_instances: usize,
) -> Result<(), String> {
    let token = ensure_token(&token_file)?;
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let state = Arc::new(DaemonState {
        engine: engine.clone(),
        token,
        shutdown: shutdown_tx,
        active: std::sync::Mutex::new(std::collections::HashMap::new()),
    });

    let sweep_engine = engine.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            sweep_engine.sweep_idle_sessions();
        }
    });

    eprintln!(
        "deskctl daemon listening on {socket}; token file: {}",
        token_file.display()
    );

    #[cfg(windows)]
    let server = tokio::spawn(named_pipe_loop(socket, state.clone(), idle_instances));
    #[cfg(not(windows))]
    let server = tokio::spawn(unix_loop(socket, state.clone()));

    let loop_result: Result<(), String> = tokio::select! {
        _ = tokio::signal::ctrl_c() => Ok(()),
        _ = shutdown_rx.changed() => Ok(()),
        result = server => {
            match result {
                Ok(Ok(())) => Err("daemon listener stopped unexpectedly".to_string()),
                Ok(Err(error)) => Err(error),
                Err(error) => Err(format!("daemon listener task failed: {error}")),
            }
        }
    };
    engine.request_shutdown();
    engine.flush_audit();
    loop_result
}

fn ensure_token(path: &Path) -> Result<String, String> {
    if let Ok(value) = std::fs::read_to_string(path) {
        let value = value.trim().to_string();
        if !value.is_empty() {
            return Ok(value);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create token directory '{}': {error}", parent.display()))?;
    }
    let token = format!("dt_{}", uuid::Uuid::new_v4().simple());
    std::fs::write(path, &token)
        .map_err(|error| format!("write token file '{}': {error}", path.display()))?;
    Ok(token)
}

async fn handle_connection<S>(stream: S, state: Arc<DaemonState>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, "read daemon request failed");
                break;
            }
        }

        let response = match serde_json::from_str::<Request>(line.trim()) {
            Ok(request) => handle_request(request, state.clone()).await,
            Err(error) => Response {
                id: None,
                ok: false,
                result: None,
                error: Some(format!("invalid request: {error}")),
            },
        };
        let payload = match serde_json::to_vec(&response) {
            Ok(payload) => payload,
            Err(error) => {
                tracing::error!(%error, "serialize daemon response failed");
                break;
            }
        };
        if write_half.write_all(&payload).await.is_err()
            || write_half.write_all(b"\n").await.is_err()
            || write_half.flush().await.is_err()
        {
            break;
        }
    }
}

async fn handle_request(request: Request, state: Arc<DaemonState>) -> Response {
    let id = request.id.clone();
    if request.token != state.token {
        return Response {
            id,
            ok: false,
            result: None,
            error: Some("permission_denied: invalid daemon token".to_string()),
        };
    }

    let result = match request.method.as_str() {
        "ping" => Ok(serde_json::json!({ "pong": true })),
        "doctor" => {
            let mut report = deskctl_core::doctor::report();
            if let Some(object) = report.as_object_mut() {
                object.insert(
                    "daemon".to_string(),
                    serde_json::json!({
                        "uptime_ms": state.engine.uptime_ms(),
                        "session_count": state.engine.list_sessions(None).len(),
                        "shutting_down": state.engine.shutdown_requested(),
                        "audit": {
                            "state": state.engine.audit().state(),
                            "mode": state.engine.config().audit.mode.as_str(),
                            "dir": state.engine.config().audit.dir.display().to_string(),
                            "dropped": state.engine.audit().recorded_drops(),
                            "disabled_reason": state.engine.audit().disabled_reason(),
                        },
                        "owner_limits": {
                            "max_sessions_per_owner": state.engine.config().owner_max_sessions,
                            "fairness": state.engine.config().owner_fairness,
                            "note": "owner is an attribution label, not an auth boundary",
                        }
                    }),
                );
            }
            Ok(report)
        }
        "session/create" => {
            let profile = match request.params.get("profile").and_then(Value::as_str) {
                Some(value) => match value.parse::<ToolProfile>() {
                    Ok(profile) => Some(profile),
                    Err(error) => return failure(id, error),
                },
                None => None,
            };
            let owner = match request.params.get("owner").and_then(Value::as_str) {
                Some(raw) => match deskctl_core::session::validate_owner(raw) {
                    Ok(owner) => owner,
                    Err(error) => return failure(id, error.to_string()),
                },
                None => "daemon-client".to_string(),
            };
            match state.engine.create_session(profile, owner) {
                Ok(info) => Ok(serde_json::to_value(info).unwrap_or(Value::Null)),
                Err(error) => return failure(id, error.to_string()),
            }
        }
        "session/list" => {
            let owner = request.params.get("owner").and_then(Value::as_str);
            Ok(Value::Array(state.engine.list_sessions(owner)))
        }
        "session/inspect" => {
            let session_id = match required_string(&request.params, "session_id") {
                Ok(value) => value,
                Err(error) => return failure(id, error),
            };
            let token = request.params.get("session_token").and_then(Value::as_str);
            match state.engine.require_session(&session_id, token) {
                Ok(session) => Ok(session.summary()),
                Err(error) => return failure(id, error.to_string()),
            }
        }
        "session/close" => {
            let session_id = match required_string(&request.params, "session_id") {
                Ok(value) => value,
                Err(error) => return failure(id, error),
            };
            let token = request.params.get("session_token").and_then(Value::as_str);
            match state.engine.close_session(&session_id, token) {
                Ok(value) => Ok(value),
                Err(error) => return failure(id, error.to_string()),
            }
        }
        "tools/list" => {
            let session_id = match required_string(&request.params, "session_id") {
                Ok(value) => value,
                Err(error) => return failure(id, error),
            };
            let token = request.params.get("session_token").and_then(Value::as_str);
            match state.engine.require_session(&session_id, token) {
                Ok(session) => Ok(Value::Array(schema::tool_descriptors(session.profile))),
                Err(error) => return failure(id, error.to_string()),
            }
        }
        "tools/call" => {
            let session_id = match required_string(&request.params, "session_id") {
                Ok(value) => value,
                Err(error) => return failure(id, error),
            };
            let tool_name = match required_string(&request.params, "tool") {
                Ok(value) => value,
                Err(error) => return failure(id, error),
            };
            let tool = match tool_name.parse::<ToolName>() {
                Ok(tool) => tool,
                Err(error) => return failure(id, error),
            };
            let arguments = request
                .params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let session_token = request.params.get("session_token").and_then(Value::as_str);
            let session = match state.engine.require_session(&session_id, session_token) {
                Ok(session) => session,
                Err(error) => return failure(id, error.to_string()),
            };
            let request_id = request
                .params
                .get("request_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| request.id.clone());
            let cancel = Arc::new(AtomicBool::new(false));
            if let Some(request_id) = &request_id {
                if let Ok(mut active) = state.active.lock() {
                    active.insert(request_id.clone(), cancel.clone());
                }
            }
            let result: ToolResult = state
                .engine
                .execute_with_session(
                    session,
                    tool,
                    arguments,
                    request_id.clone(),
                    cancel,
                    AuditSource::Daemon,
                )
                .await;
            if let Some(request_id) = &request_id {
                if let Ok(mut active) = state.active.lock() {
                    active.remove(request_id);
                }
            }
            serde_json::to_value(result).map_err(|error| error.to_string())
        }
        "request/cancel" => {
            let request_id = match required_string(&request.params, "request_id") {
                Ok(value) => value,
                Err(error) => return failure(id, error),
            };
            let cancel = state
                .active
                .lock()
                .ok()
                .and_then(|active| active.get(&request_id).cloned());
            match cancel {
                Some(cancel) => {
                    cancel.store(true, std::sync::atomic::Ordering::Release);
                    Ok(serde_json::json!({
                        "request_id": request_id,
                        "cancelled": true
                    }))
                }
                None => Ok(serde_json::json!({
                    "request_id": request_id,
                    "cancelled": false
                })),
            }
        }
        "shutdown" => {
            state.engine.request_shutdown();
            let _ = state.shutdown.send(true);
            Ok(serde_json::json!({ "shutting_down": true }))
        }
        other => Err(format!("unknown method '{other}'")),
    };

    match result {
        Ok(result) => Response {
            id,
            ok: true,
            result: Some(result),
            error: None,
        },
        Err(error) => failure(id, error),
    }
}

fn failure(id: Option<String>, error: impl Into<String>) -> Response {
    Response {
        id,
        ok: false,
        result: None,
        error: Some(error.into()),
    }
}

fn required_string(params: &Value, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| format!("missing required parameter '{key}'"))
}

#[cfg(windows)]
async fn named_pipe_loop(
    socket: String,
    state: Arc<DaemonState>,
    idle_instances: usize,
) -> Result<(), String> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let target = idle_instances.max(1);
    // Create the first instance eagerly so a second daemon on the same socket
    // fails fast instead of silently joining an existing pipe.
    let first = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&socket)
        .map_err(|error| format!("start named pipe '{socket}': {error}"))?;

    // One accept worker per instance: every listening instance always has a
    // pending connect(), so concurrent clients are picked up regardless of the
    // order Windows hands out pipe instances.
    let mut initial = Some(first);
    let mut workers = tokio::task::JoinSet::new();
    for index in 0..target {
        let pending = if index == 0 { initial.take() } else { None };
        workers.spawn(pipe_accept_worker(socket.clone(), state.clone(), pending));
    }
    match workers.join_next().await {
        Some(Ok(Ok(()))) => Err("named pipe listener stopped unexpectedly".to_string()),
        Some(Ok(Err(error))) => Err(error),
        Some(Err(error)) => Err(format!("named pipe listener task failed: {error}")),
        None => Err("no named pipe listeners were started".to_string()),
    }
}

#[cfg(windows)]
async fn pipe_accept_worker(
    socket: String,
    state: Arc<DaemonState>,
    mut pending: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
) -> Result<(), String> {
    use tokio::net::windows::named_pipe::ServerOptions;

    loop {
        let server = match pending.take() {
            Some(server) => server,
            None => ServerOptions::new()
                .create(&socket)
                .map_err(|error| format!("start named pipe '{socket}': {error}"))?,
        };
        server
            .connect()
            .await
            .map_err(|error| format!("named pipe connect failed: {error}"))?;
        let state = state.clone();
        tokio::spawn(async move {
            handle_connection(server, state).await;
        });
    }
}

#[cfg(not(windows))]
async fn unix_loop(socket: String, state: Arc<DaemonState>) -> Result<(), String> {
    use tokio::net::UnixListener;

    if Path::new(&socket).exists() {
        if tokio::net::UnixStream::connect(&socket).await.is_ok() {
            return Err(format!("daemon already running on '{socket}'"));
        }
        let _ = std::fs::remove_file(&socket);
    }
    let listener = UnixListener::bind(&socket)
        .map_err(|error| format!("bind unix socket '{socket}': {error}"))?;
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|error| format!("accept unix socket connection: {error}"))?;
        let state = state.clone();
        tokio::spawn(async move {
            handle_connection(stream, state).await;
        });
    }
}
