mod batch;
mod bench;
mod client;
mod daemon;
mod doc_extract;
mod history;
mod mcp;
mod output;
mod script_cmd;

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
        /// Attribution label for audit records (default `oneshot`).
        #[arg(long)]
        owner: Option<String>,
        /// Write the JSON result to this UTF-8 file instead of stdout.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Emit compact JSON (default: pretty).
        #[arg(long)]
        compact: bool,
    },
    /// Start the local daemon.
    Serve {
        #[arg(long)]
        socket: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
        #[arg(long, default_value_t = 8)]
        max_sessions: usize,
        /// Preload OCR/YOLO models before accepting requests.
        #[arg(long)]
        warmup: bool,
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
        /// Read the session token from this UTF-8 file when --session-token is absent.
        #[arg(long)]
        session_token_file: Option<PathBuf>,
        #[arg(long)]
        request_id: Option<String>,
        /// Write the JSON result to this UTF-8 file instead of stdout.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Emit compact JSON (default: pretty).
        #[arg(long)]
        compact: bool,
    },
    /// Run a JSONL batch of tool calls in one daemon session.
    Batch {
        /// JSONL file; each line is {"tool":"...","arguments":{...}}.
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        session: String,
        #[arg(long)]
        session_token: Option<String>,
        #[arg(long)]
        session_token_file: Option<PathBuf>,
        #[arg(long)]
        socket: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
        /// Directory for per-item JSON results.
        #[arg(long)]
        output_dir: PathBuf,
        /// Emit compact per-item JSON files.
        #[arg(long)]
        compact: bool,
        /// Stop after the first failed item.
        #[arg(long)]
        stop_on_error: bool,
    },
    /// Extract text and embedded images from a Word document via Word COM.
    DocExtract {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        out_dir: PathBuf,
        /// Write metadata JSON to this UTF-8 file instead of stdout.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Write a JSONL batch of desktop_perceive calls for the exported images.
        #[arg(long)]
        batch_out: Option<PathBuf>,
        /// Emit compact metadata JSON.
        #[arg(long)]
        compact: bool,
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
    Doctor {
        /// Write the JSON result to this UTF-8 file instead of stdout.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Emit compact JSON (default: pretty).
        #[arg(long)]
        compact: bool,
    },
    /// Query the local audit history (reads JSONL files directly).
    History {
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        owner: Option<String>,
        #[arg(long)]
        tool: Option<String>,
        /// Select a record-only run produced by record/replay (D3).
        #[arg(long)]
        run: Option<String>,
        /// RFC3339, `today`, `now` or a relative window such as `30m`/`2h`/`7d`.
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        until: Option<String>,
        #[arg(long)]
        failed_only: bool,
        /// Filter by `kind` (tool_call or audit_health).
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Print JSONL records instead of the tabular view.
        #[arg(long)]
        json: bool,
        /// Aggregate by tool/owner instead of listing records.
        #[arg(long)]
        summary: bool,
        /// Override the audit directory (default: DESKCTL_AUDIT_DIR or data dir).
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Record a daemon session's tool calls as a `deskctl-script/1` document.
    Record {
        #[command(subcommand)]
        command: RecordCommands,
        #[arg(long, global = true)]
        session: Option<String>,
        #[arg(long, global = true)]
        socket: Option<String>,
        #[arg(long, global = true)]
        token_file: Option<PathBuf>,
        #[arg(long, global = true)]
        session_token: Option<String>,
    },
    /// Validate or replay a `deskctl-script/1` document.
    Script {
        #[command(subcommand)]
        command: ScriptCommands,
    },
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
enum RecordCommands {
    /// Start recording tool calls in the given session.
    Start {
        /// `essential` (default), `all`, or `none`.
        #[arg(long)]
        include_reads: Option<String>,
        #[arg(long)]
        max_steps: Option<u32>,
        /// Record `allow_relaunch=true` for catalog-launched targets.
        #[arg(long)]
        allow_relaunch: Option<bool>,
    },
    /// Stop recording and optionally write the script to a file.
    Stop {
        /// Output `.dscript.json` path; defaults to printing the script inline.
        #[arg(long = "out")]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
enum ScriptCommands {
    /// Statically validate a script file (no daemon required).
    Validate {
        #[arg(long = "file")]
        file: PathBuf,
        #[arg(long)]
        risk_max: Option<String>,
        #[arg(long)]
        allow_external_commit: bool,
    },
    /// Replay a script in a daemon session.
    Run {
        #[arg(long = "file")]
        file: PathBuf,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        dry_run: bool,
        /// `k=v` (repeatable); values parse as JSON when possible.
        #[arg(long = "param", value_parser = parse_param)]
        params: Vec<(String, String)>,
        #[arg(long)]
        on_error: Option<String>,
        #[arg(long = "from")]
        from_step: Option<String>,
        #[arg(long = "to")]
        to_step: Option<String>,
        #[arg(long)]
        risk_max: Option<String>,
        #[arg(long)]
        allow_external_commit: bool,
        #[arg(long)]
        allow_resume_after_unknown: bool,
        #[arg(long)]
        no_fragile: bool,
        /// Save this checkpoint before running and verify it after the run
        /// (and after any compensation).
        #[arg(long)]
        checkpoint: Option<String>,
        /// Compensation script (`deskctl-script/1` JSON) executed when the run fails.
        #[arg(long)]
        compensation: Option<PathBuf>,
        /// Apply the compensation for real (default: only produce a dry-run plan).
        #[arg(long)]
        apply_compensation: bool,
        #[arg(long)]
        socket: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
        #[arg(long)]
        session_token: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum SessionCommands {
    Create {
        #[arg(long)]
        profile: Option<String>,
        /// Attribution label for audit records (default `daemon-client`).
        #[arg(long)]
        owner: Option<String>,
        #[arg(long)]
        socket: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
        /// Write the returned session token to this UTF-8 file.
        #[arg(long)]
        token_out: Option<PathBuf>,
    },
    List {
        /// Only show sessions with this exact owner label.
        #[arg(long)]
        owner: Option<String>,
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
        #[arg(long)]
        session_token_file: Option<PathBuf>,
    },
    Close {
        id: String,
        #[arg(long)]
        socket: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
        #[arg(long)]
        session_token: Option<String>,
        #[arg(long)]
        session_token_file: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() {
    // 统一坐标系：鼠标/窗口/UIA/截图都按物理像素工作（见 platform::windows::dpi）。
    deskctl_core::platform::init_process_dpi_awareness();
    init_tracing();
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
        Commands::Tool {
            name,
            json,
            stdin,
            owner,
            output,
            compact,
        } => {
            let args = read_args(json, stdin)?;
            let owner = match owner {
                Some(raw) => Some(
                    deskctl_core::session::validate_owner(&raw)
                        .map_err(|error| error.to_string())?,
                ),
                None => None,
            };
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
                    output::print_json_with_options(&failure, compact, output.as_deref())?;
                    return Ok(2);
                }
            };
            let engine = Engine::new(config_for(profile));
            let result = engine.execute_oneshot_as(tool, args, owner).await;
            engine.flush_audit();
            output::print_json_with_options(&result, compact, output.as_deref())?;
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
            warmup,
        } => {
            let socket = socket.unwrap_or_else(config::default_socket_name);
            let token_file = token_file.unwrap_or_else(config::token_path);
            let mut config = config_for(profile);
            config.max_sessions = max_sessions.max(1);
            let idle_instances = config.max_sessions + 2;
            let engine = Engine::new(config);
            if warmup {
                let started = std::time::Instant::now();
                match engine.vision().warmup() {
                    Ok(()) => eprintln!(
                        "deskctl: vision warmup completed in {} ms",
                        started.elapsed().as_millis()
                    ),
                    Err(error) => eprintln!("deskctl: vision warmup skipped: {error}"),
                }
            }
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
            session_token_file,
            request_id,
            output,
            compact,
        } => {
            let args = read_args(json, stdin)?;
            let session_token = resolve_session_token(session_token, session_token_file)?;
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
            output::print_json_with_options(&result, compact, output.as_deref())?;
            Ok(exit_code_for(&result))
        }
        Commands::Batch {
            file,
            session,
            session_token,
            session_token_file,
            socket,
            token_file,
            output_dir,
            compact,
            stop_on_error,
        } => {
            let session_token = resolve_session_token(session_token, session_token_file)?;
            batch::run(batch::BatchOptions {
                file,
                session,
                session_token,
                socket: socket.unwrap_or_else(config::default_socket_name),
                token_file: token_file.unwrap_or_else(config::token_path),
                output_dir,
                compact,
                stop_on_error,
            })
            .await
        }
        Commands::DocExtract {
            file,
            out_dir,
            output,
            batch_out,
            compact,
        } => doc_extract::run(doc_extract::DocExtractOptions {
            file,
            out_dir,
            output,
            batch_out,
            compact,
        }),
        Commands::Session { command } => {
            let (method, params, socket, token_file, token_out) = match command {
                SessionCommands::Create {
                    profile,
                    owner,
                    socket,
                    token_file,
                    token_out,
                } => {
                    let owner = match owner {
                        Some(raw) => Some(
                            deskctl_core::session::validate_owner(&raw)
                                .map_err(|error| error.to_string())?,
                        ),
                        None => None,
                    };
                    (
                        "session/create",
                        serde_json::json!({
                            "profile": profile.unwrap_or_else(|| "standard".to_string()),
                            "owner": owner
                        }),
                        socket.unwrap_or_else(config::default_socket_name),
                        token_file.unwrap_or_else(config::token_path),
                        token_out,
                    )
                }
                SessionCommands::List {
                    owner,
                    socket,
                    token_file,
                } => (
                    "session/list",
                    serde_json::json!({ "owner": owner }),
                    socket.unwrap_or_else(config::default_socket_name),
                    token_file.unwrap_or_else(config::token_path),
                    None,
                ),
                SessionCommands::Inspect {
                    id,
                    socket,
                    token_file,
                    session_token,
                    session_token_file,
                } => (
                    "session/inspect",
                    serde_json::json!({
                        "session_id": id,
                        "session_token": resolve_session_token(session_token, session_token_file)?
                    }),
                    socket.unwrap_or_else(config::default_socket_name),
                    token_file.unwrap_or_else(config::token_path),
                    None,
                ),
                SessionCommands::Close {
                    id,
                    socket,
                    token_file,
                    session_token,
                    session_token_file,
                } => (
                    "session/close",
                    serde_json::json!({
                        "session_id": id,
                        "session_token": resolve_session_token(session_token, session_token_file)?
                    }),
                    socket.unwrap_or_else(config::default_socket_name),
                    token_file.unwrap_or_else(config::token_path),
                    None,
                ),
            };
            let result = client::request(&socket, &token_file, method, params).await?;
            if let Some(path) = token_out {
                let token = result
                    .get("session_token")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        "session/create response did not include session_token".to_string()
                    })?;
                write_session_token(&path, token)?;
            }
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
        Commands::Doctor { output, compact } => {
            let report = deskctl_core::doctor::report();
            output::print_json_with_options(&report, compact, output.as_deref())?;
            Ok(if report.get("ok").and_then(Value::as_bool) == Some(true) {
                0
            } else {
                6
            })
        }
        Commands::History {
            session,
            owner,
            tool,
            run,
            since,
            until,
            failed_only,
            kind,
            limit,
            json,
            summary,
            dir,
        } => history::run(history::HistoryOptions {
            session,
            owner,
            tool,
            run,
            since,
            until,
            failed_only,
            kind,
            limit,
            json,
            summary,
            dir,
        }),
        Commands::Record {
            command,
            session,
            socket,
            token_file,
            session_token,
        } => {
            let conn = script_cmd::Conn {
                socket: socket.unwrap_or_else(config::default_socket_name),
                token_file: token_file.unwrap_or_else(config::token_path),
                session,
                session_token,
            };
            let (result, code) = match command {
                RecordCommands::Start {
                    include_reads,
                    max_steps,
                    allow_relaunch,
                } => {
                    script_cmd::record_start(&conn, include_reads, max_steps, allow_relaunch)
                        .await?
                }
                RecordCommands::Stop { out } => script_cmd::record_stop(&conn, out).await?,
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
            );
            Ok(code)
        }
        Commands::Script { command } => match command {
            ScriptCommands::Validate {
                file,
                risk_max,
                allow_external_commit,
            } => {
                let (result, code) = script_cmd::validate(&file, risk_max, allow_external_commit)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
                );
                Ok(code)
            }
            ScriptCommands::Run {
                file,
                session,
                dry_run,
                params,
                on_error,
                from_step,
                to_step,
                risk_max,
                allow_external_commit,
                allow_resume_after_unknown,
                no_fragile,
                checkpoint,
                compensation,
                apply_compensation,
                socket,
                token_file,
                session_token,
            } => {
                let conn = script_cmd::Conn {
                    socket: socket.unwrap_or_else(config::default_socket_name),
                    token_file: token_file.unwrap_or_else(config::token_path),
                    session,
                    session_token,
                };
                let (result, code) = script_cmd::run(
                    &conn,
                    script_cmd::RunRequest {
                        file: &file,
                        dry_run,
                        params,
                        on_error,
                        from_step,
                        to_step,
                        risk_max,
                        allow_external_commit,
                        allow_resume_after_unknown,
                        no_fragile,
                        checkpoint,
                        compensation: compensation.as_deref(),
                        apply_compensation,
                    },
                )
                .await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
                );
                Ok(code)
            }
        },
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

fn resolve_session_token(
    explicit: Option<String>,
    file: Option<PathBuf>,
) -> Result<Option<String>, String> {
    if let Some(token) = explicit {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err("session token is empty".to_string());
        }
        return Ok(Some(token));
    }
    if let Some(path) = file {
        let token = std::fs::read_to_string(&path)
            .map_err(|error| format!("read session token '{}': {error}", path.display()))?;
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(format!("session token file '{}' is empty", path.display()));
        }
        return Ok(Some(token));
    }
    Ok(std::env::var("DESKCTL_SESSION_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty()))
}

fn write_session_token(path: &std::path::Path, token: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!("create token directory '{}': {error}", parent.display())
            })?;
        }
    }
    std::fs::write(path, format!("{}\n", token.trim()))
        .map_err(|error| format!("write session token '{}': {error}", path.display()))
}

/// Install the tracing subscriber (`DESKCTL_LOG`, default `info`, stderr).
///
/// Log lines never carry raw args or injected text: sensitive fields follow
/// the same redaction rules as the audit trail (see audit-history §7).
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = std::env::var("DESKCTL_LOG")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .and_then(|value| EnvFilter::try_new(value).ok())
        .unwrap_or_else(|| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
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
        "stale_capture" | "stale_observation" | "stale_target" | "ambiguous_target"
        | "identity_mismatch" | "server_restarted" => 4,
        "resource_busy" => 5,
        "platform_not_supported" => 6,
        "policy_denied" | "permission_denied" | "auth_required" => 7,
        _ => 1,
    }
}

/// `--param k=v`：拆成 (key, raw value)，JSON 解析在调用侧完成。
fn parse_param(raw: &str) -> Result<(String, String), String> {
    let (key, value) = raw
        .split_once('=')
        .ok_or_else(|| format!("expected k=v, got '{raw}'"))?;
    if key.trim().is_empty() {
        return Err(format!("parameter name is empty in '{raw}'"));
    }
    Ok((key.trim().to_string(), value.to_string()))
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
