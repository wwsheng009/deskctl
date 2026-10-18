use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Serialize)]
struct Request<'a> {
    token: &'a str,
    id: String,
    method: &'a str,
    params: Value,
}

#[derive(Debug, Deserialize)]
struct Response {
    #[allow(dead_code)]
    id: Option<String>,
    ok: bool,
    result: Option<Value>,
    error: Option<String>,
}

pub async fn request(
    socket: &str,
    token_file: &Path,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let token = std::fs::read_to_string(token_file)
        .map_err(|error| {
            format!(
                "read daemon token '{}': {error}. Start `deskctl serve` first.",
                token_file.display()
            )
        })?
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(format!("daemon token '{}' is empty", token_file.display()));
    }
    let request = Request {
        token: &token,
        id: format!("req:{}", uuid::Uuid::new_v4().simple()),
        method,
        params,
    };
    let payload = serde_json::to_vec(&request).map_err(|error| error.to_string())?;

    #[cfg(windows)]
    let stream = open_pipe(socket).await?;
    #[cfg(not(windows))]
    let stream = tokio::net::UnixStream::connect(socket)
        .await
        .map_err(|error| format!("connect unix socket '{socket}': {error}"))?;

    let mut stream = stream;
    stream
        .write_all(&payload)
        .await
        .map_err(|error| format!("write daemon request: {error}"))?;
    stream
        .write_all(b"\n")
        .await
        .map_err(|error| format!("write daemon request: {error}"))?;
    stream
        .flush()
        .await
        .map_err(|error| format!("flush daemon request: {error}"))?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|error| format!("read daemon response: {error}"))?;
    if line.trim().is_empty() {
        return Err("daemon closed the connection without a response".to_string());
    }
    let response: Response = serde_json::from_str(line.trim())
        .map_err(|error| format!("invalid daemon response: {error}"))?;
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "daemon request failed".to_string()));
    }
    Ok(response.result.unwrap_or(Value::Null))
}

/// Open the daemon pipe, retrying while every instance is busy
/// (ERROR_PIPE_BUSY / os error 231) so concurrent clients queue up instead of
/// failing outright.
#[cfg(windows)]
async fn open_pipe(
    socket: &str,
) -> Result<tokio::net::windows::named_pipe::NamedPipeClient, String> {
    use tokio::net::windows::named_pipe::ClientOptions;
    const ERROR_PIPE_BUSY: i32 = 231;
    const MAX_RETRIES: u32 = 100;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(20);

    let mut attempts = 0;
    loop {
        match ClientOptions::new().open(socket) {
            Ok(client) => return Ok(client),
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempts < MAX_RETRIES =>
            {
                attempts += 1;
                tokio::time::sleep(RETRY_DELAY).await;
            }
            Err(error) => return Err(format!("connect named pipe '{socket}': {error}")),
        }
    }
}
