mod bench;
mod client;
mod daemon;
mod mcp;
mod output;

use clap::{Parser, Subcommand};
use deskctl_core::{config, Config, Engine};
use deskctl_protocol::{ToolName, ToolProfile};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "deskctl",
    version,
    about = "Standalone desktop tools for LLMs and agents"
)]
struct Cli {
    #[arg(long, global = true, default_value = "standard")]
    profile: String,
    #[arg(long, global = true)]
    enable_vision: bool,
    #[arg(long, global = true)]
    enable_agent_step: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run a tool once in a temporary, in-process session.
    Tool {
        name: String,
        #[arg(long)]
        json: Option<String>,
        #[arg(long)]
        stdin: bool,
    },
    /// Start the local daemon.
    Serve {
        #[arg(long)]
        socket: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
        #[arg(long, default_value_t = 8)]
        max_sessions: usize,
    },
    /// Call a tool on an existing daemon session.
    Call {
        session: String,
        tool: String,
        #[arg(long)]
        json: Option<String>,
        #[arg(long)]
        stdin: bool,
        #[arg(long)]
        socket: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
        #[arg(long)]
        session_token: Option<String>,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Manage daemon sessions.
    Session {
        #[command(subcommand)]
        command: SessionCommands,
    },
    /// Cancel a running daemon request by request_id.
    Cancel { request_id: String },
    /// Run the MCP stdio server. It uses one in-process session.
    Mcp {
        #[arg(long)]
        skill_root: Option<PathBuf>,
    },
    /// Show platform, model, token and socket diagnostics.
    Doctor,
    /// Export the current profile's tool contracts as JSON.
    Schema {
        #[arg(long, default_value = "contracts/tools")]
        out: PathBuf,
    },
    /// Ask the daemon to shut down.
    Shutdown {
        #[arg(long)]
        socket: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
    },
    /// Measure daemon latency for a fixed op (developer utility).
    #[command(hide = true)]
    Bench {
        /// One of screen_size, screenshot, window_screenshot or parallel.
        #[arg(long, default_value = "screen_size")]
        op: String,
        #[arg(long, default_value_t = 30)]
        count: u32,
        #[arg(long, default_value_t = 3)]
        warmup: u32,
        /// Sessions used by the parallel op.
        #[arg(long, default_value_t = 8)]
        parallel: u32,
        #[arg(long)]
        socket: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum SessionCommands {
    Create {
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        socket: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
    },
    List {
        #[arg(long)]
        socket: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
    },
    Inspect {
        id: String,
        #[arg(long)]
        socket: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
        #[arg(long)]
        session_token: Option<String>,
    },
    Close {
        id: String,
        #[arg(long)]
        socket: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
        #[arg(long)]
        session_token: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mut profile = match cli.profile.parse::<ToolProfile>() {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    if cli.enable_vision || cli.enable_agent_step {
        profile = ToolProfile::Full;
    }

    let code = match run(cli.command, profile).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("deskctl: {error}");
            exit_code_for_message(&error)
        }
    };
    std::process::exit(code);
}

async fn run(command: Commands, profile: ToolProfile) -> Result<i32, String> {
    match command {
        Commands::Tool { name, json, stdin } => {
            let args = read_args(json, stdin)?;
            let tool = match name.parse::<ToolName>() {
                Ok(tool) => tool,
                Err(error) => {
                    let failure = serde_json::json!({
                        "ok": false,
                        "data": null,
                        "error": {
                            "code": "unknown_tool",
                            "message": error,
                            "retryable": false
                        },
                        "meta": {
                            "dispatch_state": "not_sent",
                            "effect": "read",
                            "verification": "unknown",
                            "retryable": false,
                            "requires_observation": false,
                            "risk": "read_only"
                        }
                    });
                    output::print_json(&failure)?;
                    return Ok(2);
                }
            };
            let engine = Engine::new(config_for(profile));
            let result = engine.execute_oneshot(tool, args).await;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
            );
            Ok(if result.ok {
                0
            } else {
                result
                    .error
                    .as_ref()
                    .map_or(1, |error| exit_code_for_code(error.code.as_str()))
            })
        }
        Commands::Serve {
            socket,
            token_file,
            max_sessions,
        } => {
            let socket = socket.unwrap_or_else(config::default_socket_name);
            let token_file = token_file.unwrap_or_else(config::token_path);
            let mut config = config_for(profile);
            config.max_sessions = max_sessions.max(1);
            let idle_instances = config.max_sessions + 2;
            let engine = Engine::new(config);
            daemon::run(socket, token_file, engine, idle_instances).await?;
            Ok(0)
        }
        Commands::Call {
            session,
            tool,
            json,
            stdin,
            socket,
            token_file,
            session_token,
            request_id,
        } => {
            let args = read_args(json, stdin)?;
            let session_token =
                session_token.or_else(|| std::env::var("DESKCTL_SESSION_TOKEN").ok());
            let result = client::request(
                &socket.unwrap_or_else(config::default_socket_name),
                &token_file.unwrap_or_else(config::token_path),
                "tools/call",
                serde_json::json!({
                    "session_id": session,
                    "session_token": session_token,
                    "tool": tool,
                    "arguments": args,
                    "request_id": request_id,
                }),
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
            );
            Ok(exit_code_for(&result))
        }
        Commands::Session { command } => {
            let (method, params, socket, token_file) = match command {
                SessionCommands::Create {
                    profile,
                    socket,
                    token_file,
                } => (
                    "session/create",
                    serde_json::json!({
                        "profile": profile.unwrap_or_else(|| "standard".to_string())
                    }),
                    socket.unwrap_or_else(config::default_socket_name),
                    token_file.unwrap_or_else(config::token_path),
                ),
                SessionCommands::List { socket, token_file } => (
                    "session/list",
                    serde_json::json!({}),
                    socket.unwrap_or_else(config::default_socket_name),
                    token_file.unwrap_or_else(config::token_path),
                ),
                SessionCommands::Inspect {
                    id,
                    socket,
                    token_file,
                    session_token,
                } => (
                    "session/inspect",
                    serde_json::json!({
                        "session_id": id,
                        "session_token": session_token.or_else(|| std::env::var("DESKCTL_SESSION_TOKEN").ok())
                    }),
                    socket.unwrap_or_else(config::default_socket_name),
                    token_file.unwrap_or_else(config::token_path),
                ),
                SessionCommands::Close {
                    id,
                    socket,
                    token_file,
                    session_token,
                } => (
                    "session/close",
                    serde_json::json!({
                        "session_id": id,
                        "session_token": session_token.or_else(|| std::env::var("DESKCTL_SESSION_TOKEN").ok())
                    }),
                    socket.unwrap_or_else(config::default_socket_name),
                    token_file.unwrap_or_else(config::token_path),
                ),
            };
            let result = client::request(&socket, &token_file, method, params).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
            );
            // Daemon-level failures surface as errors from the client, so a
            // printed session result is always a success.
            Ok(0)
        }
        Commands::Mcp { skill_root } => {
            mcp::run(
                profile,
                skill_root.unwrap_or_else(|| PathBuf::from("skills")),
            )
            .await?;
            Ok(0)
        }
        Commands::Cancel { request_id } => {
            let result = client::request(
                &config::default_socket_name(),
                &config::token_path(),
                "request/cancel",
                serde_json::json!({ "request_id": request_id }),
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
            );
            Ok(
                if result.get("cancelled").and_then(Value::as_bool) == Some(true) {
                    0
                } else {
                    1
                },
            )
        }
        Commands::Doctor => {
            let report = deskctl_core::doctor::report();
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
            );
            Ok(if report.get("ok").and_then(Value::as_bool) == Some(true) {
                0
            } else {
                6
            })
        }
        Commands::Schema { out } => {
            std::fs::create_dir_all(&out)
                .map_err(|error| format!("create schema directory: {error}"))?;
            let descriptors = deskctl_protocol::schema::tool_descriptors(profile);
            let mut index = Vec::new();
            for descriptor in &descriptors {
                let name = descriptor["name"]
                    .as_str()
                    .ok_or_else(|| "schema descriptor has no name".to_string())?;
                let content =
                    serde_json::to_string_pretty(descriptor).map_err(|error| error.to_string())?;
                std::fs::write(out.join(format!("{name}.json")), format!("{content}\n"))
                    .map_err(|error| format!("write {name}.json: {error}"))?;
                index.push(serde_json::json!({
                    "name": name,
                    "file": format!("{name}.json")
                }));
            }
            let index = serde_json::json!({
                "schema_version": "0.1.0",
                "profile": profile,
                "count": descriptors.len(),
                "tools": index
            });
            std::fs::write(
                out.join("index.json"),
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&index).map_err(|error| error.to_string())?
                ),
            )
            .map_err(|error| format!("write schema index: {error}"))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&index).map_err(|error| error.to_string())?
            );
            Ok(0)
        }
        Commands::Shutdown { socket, token_file } => {
            let result = client::request(
                &socket.unwrap_or_else(config::default_socket_name),
                &token_file.unwrap_or_else(config::token_path),
                "shutdown",
                serde_json::json!({}),
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
            );
            Ok(0)
        }
        Commands::Bench {
            op,
            count,
            warmup,
            parallel,
            socket,
            token_file,
        } => {
            bench::run(bench::BenchOptions {
                op,
                count,
                warmup,
                parallel,
                socket: socket.unwrap_or_else(config::default_socket_name),
                token_file: token_file.unwrap_or_else(config::token_path),
            })
            .await
        }
    }
}

fn config_for(profile: ToolProfile) -> Config {
    Config {
        profile,
        ..Config::default()
    }
}

fn exit_code_for(value: &Value) -> i32 {
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        return 0;
    }
    exit_code_for_code(
        value
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("internal"),
    )
}

fn exit_code_for_code(code: &str) -> i32 {
    match code {
        "invalid_params" | "unknown_tool" => 2,
        "stale_capture" | "stale_observation" | "server_restarted" => 4,
        "resource_busy" => 5,
        "platform_not_supported" => 6,
        "policy_denied" | "permission_denied" | "auth_required" => 7,
        _ => 1,
    }
}

/// Daemon-level failures travel as `<code>: <message>` strings (see
/// `ToolError`'s `Display`). Map the leading code to the same exit codes used
/// for tool results so scripts can branch on them.
fn exit_code_for_message(message: &str) -> i32 {
    match message.split_once(':').map(|(prefix, _)| prefix.trim()) {
        Some(prefix) => exit_code_for_code(prefix),
        None => 1,
    }
}

fn read_args(json: Option<String>, stdin: bool) -> Result<Value, String> {
    if stdin {
        let mut input = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
            .map_err(|error| format!("read stdin: {error}"))?;
        return parse_json(&input);
    }
    let Some(raw) = json else {
        return Ok(serde_json::json!({}));
    };
    if let Some(path) = raw.strip_prefix('@') {
        let content = std::fs::read_to_string(path)
            .map_err(|error| format!("read args file '{path}': {error}"))?;
        parse_json(&content)
    } else {
        parse_json(&raw)
    }
}

fn parse_json(raw: &str) -> Result<Value, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(trimmed).map_err(|error| format!("invalid JSON arguments: {error}"))
}
