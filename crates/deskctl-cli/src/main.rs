mod client;
mod daemon;
mod mcp;

use clap::{Parser, Subcommand};
use deskctl_core::{config, Config, Engine};
use deskctl_protocol::{ToolName, ToolProfile};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "deskctl", version, about = "Standalone desktop tools for LLMs and agents")]
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
    },
    /// Manage daemon sessions.
    Session {
        #[command(subcommand)]
        command: SessionCommands,
    },
    /// Run the MCP stdio server. It uses one in-process session.
    Mcp {
        #[arg(long)]
        skill_root: Option<PathBuf>,
    },
    /// Show platform, model, token and socket diagnostics.
    Doctor,
    /// Ask the daemon to shut down.
    Shutdown {
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
    },
    Close {
        id: String,
        #[arg(long)]
        socket: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
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
            1
        }
    };
    std::process::exit(code);
}

async fn run(command: Commands, profile: ToolProfile) -> Result<i32, String> {
    match command {
        Commands::Tool { name, json, stdin } => {
            let args = read_args(json, stdin)?;
            let tool = name.parse::<ToolName>().map_err(|error| error)?;
            let engine = Engine::new(config_for(profile));
            let result = engine.execute_oneshot(tool, args).await;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
            );
            Ok(if result.ok { 0 } else { 1 })
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
            let engine = Engine::new(config);
            daemon::run(socket, token_file, engine).await?;
            Ok(0)
        }
        Commands::Call {
            session,
            tool,
            json,
            stdin,
            socket,
            token_file,
        } => {
            let args = read_args(json, stdin)?;
            let result = client::request(
                &socket.unwrap_or_else(config::default_socket_name),
                &token_file.unwrap_or_else(config::token_path),
                "tools/call",
                serde_json::json!({
                    "session_id": session,
                    "tool": tool,
                    "arguments": args,
                }),
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
            );
            let ok = result.get("ok").and_then(Value::as_bool).unwrap_or(false);
            Ok(if ok { 0 } else { 1 })
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
                } => (
                    "session/inspect",
                    serde_json::json!({ "session_id": id }),
                    socket.unwrap_or_else(config::default_socket_name),
                    token_file.unwrap_or_else(config::token_path),
                ),
                SessionCommands::Close {
                    id,
                    socket,
                    token_file,
                } => (
                    "session/close",
                    serde_json::json!({ "session_id": id }),
                    socket.unwrap_or_else(config::default_socket_name),
                    token_file.unwrap_or_else(config::token_path),
                ),
            };
            let result = client::request(&socket, &token_file, method, params).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
            );
            Ok(0)
        }
        Commands::Mcp { skill_root } => {
            mcp::run(profile, skill_root.unwrap_or_else(|| PathBuf::from("skills"))).await?;
            Ok(0)
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
                1
            })
        }
        Commands::Shutdown {
            socket,
            token_file,
        } => {
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
    }
}

fn config_for(profile: ToolProfile) -> Config {
    Config {
        profile,
        ..Config::default()
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
