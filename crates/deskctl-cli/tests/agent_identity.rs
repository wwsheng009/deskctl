//! Contract tests for the D2 agent identity work (AC-AGT-001..004).
//!
//! - AC-AGT-001: `session create --owner` round-trips through `session list`;
//!   invalid characters and reserved internal names are rejected as
//!   `invalid_params`.
//! - AC-AGT-002: MCP `initialize.clientInfo.name` maps to `mcp:<name>`
//!   (sanitized), missing names fall back to `mcp:unknown`.
//! - AC-AGT-003: audits carry the session owner, `history --owner` filters by
//!   identity (prefix wildcards included) and `--summary` aggregates by owner.
//! - AC-AGT-004: `DESKCTL_OWNER_MAX_SESSIONS` caps sessions per owner without
//!   affecting other owners.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::Value;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("deskctl-agent-it-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn deskctl(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deskctl"));
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run deskctl")
}

fn deskctl_with_socket(args: &[&str], common: &[String], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deskctl"));
    command.args(args);
    command.args(common);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run deskctl")
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("json stdout")
}

fn stdout_json_lines(output: &Output) -> Vec<Value> {
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("json line"))
        .collect()
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn start_daemon(root: &Path, tag: &str, envs: &[(&str, &str)]) -> (Child, Vec<String>) {
    let socket = format!(r"\\.\pipe\deskctl-agent-{tag}-{}", std::process::id());
    let token_file = root.join("daemon.token");
    let common: Vec<String> = vec![
        "--socket".to_string(),
        socket,
        "--token-file".to_string(),
        token_file.display().to_string(),
    ];
    let mut command = Command::new(env!("CARGO_BIN_EXE_deskctl"));
    command
        .args(["serve", "--max-sessions", "8"])
        .args(&common)
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().expect("spawn daemon");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if deskctl_with_socket(&["session", "list"], &common, envs)
            .status
            .success()
        {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("daemon '{tag}' did not become ready within 20s");
        }
        sleep(Duration::from_millis(250));
    }
    (child, common)
}

fn stop_daemon(child: &mut Child, common: &[String], envs: &[(&str, &str)]) {
    let _ = deskctl_with_socket(&["shutdown"], common, envs);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if child.try_wait().expect("poll daemon").is_some() {
            return;
        }
        sleep(Duration::from_millis(200));
    }
    let _ = child.kill();
}

fn create_session(common: &[String], envs: &[(&str, &str)], owner: Option<&str>) -> Output {
    let mut args: Vec<String> = vec!["session".into(), "create".into()];
    if let Some(owner) = owner {
        args.push("--owner".into());
        args.push(owner.into());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    deskctl_with_socket(&refs, common, envs)
}

fn call_tool(common: &[String], envs: &[(&str, &str)], session: &Value, tool: &str) -> Output {
    let id = session["session_id"].as_str().expect("session_id");
    let token = session["session_token"].as_str().expect("session_token");
    deskctl_with_socket(&["call", id, tool, "--session-token", token], common, envs)
}

/// AC-AGT-001: owner round-trip, normalization, list filtering and rejection
/// of invalid / reserved labels.
#[test]
fn owner_flag_round_trips_and_validates() {
    let root = temp_dir("owner-create");
    let (mut daemon, common) = start_daemon(&root, "create", &[]);
    let envs: &[(&str, &str)] = &[];

    // Lowercase normalization + round-trip through create/list.
    let created = create_session(&common, envs, Some("Agent:Alpha-1"));
    assert!(
        created.status.success(),
        "create: {}",
        stderr_text(&created)
    );
    let info = stdout_json(&created);
    assert_eq!(info["owner"], "agent:alpha-1");
    let session_id = info["session_id"].as_str().expect("session_id").to_string();

    let listed = deskctl_with_socket(&["session", "list"], &common, envs);
    assert!(listed.status.success());
    let sessions = stdout_json(&listed);
    let mine = sessions
        .as_array()
        .expect("session list array")
        .iter()
        .find(|session| session["session_id"] == session_id.as_str())
        .expect("created session must be listed");
    assert_eq!(mine["owner"], "agent:alpha-1");

    // `--owner` filters the list down to exactly that identity.
    let filtered = deskctl_with_socket(
        &["session", "list", "--owner", "agent:alpha-1"],
        &common,
        envs,
    );
    assert_eq!(stdout_json(&filtered).as_array().expect("array").len(), 1);
    let empty = deskctl_with_socket(
        &["session", "list", "--owner", "agent:other"],
        &common,
        envs,
    );
    assert!(stdout_json(&empty).as_array().expect("array").is_empty());

    // Invalid characters and reserved internal names are rejected up front.
    let long = "x".repeat(65);
    let rejected: Vec<(&str, &str)> = vec![
        ("daemon-client", "reserved"),
        ("MCP", "reserved"),
        ("mcp:evil", "reserved"),
        ("oneshot", "reserved"),
        ("bench", "reserved"),
        ("system-root", "reserved"),
        ("agent:bad name", "invalid character"),
        ("agent:不良", "invalid character"),
        (long.as_str(), "1..=64"),
    ];
    for (label, needle) in rejected {
        let output = create_session(&common, envs, Some(label));
        assert_eq!(
            output.status.code(),
            Some(2),
            "label {label:?} must fail with invalid_params: {}",
            stderr_text(&output)
        );
        let stderr = stderr_text(&output);
        assert!(stderr.contains("invalid_params"), "stderr: {stderr}");
        assert!(stderr.contains(needle), "stderr for {label:?}: {stderr}");
    }

    // Callers that do not pass --owner keep the historical default label.
    let default_created = create_session(&common, envs, None);
    assert_eq!(stdout_json(&default_created)["owner"], "daemon-client");

    stop_daemon(&mut daemon, &common, envs);
    let _ = std::fs::remove_dir_all(root);
}

/// Session tokens can be persisted with `--token-out` and consumed by
/// `call --session-token-file`, avoiding token values in shell history.
#[test]
fn session_token_file_round_trips_and_calls_tool() {
    let root = temp_dir("token-file");
    let (mut daemon, common) = start_daemon(&root, "token", &[]);
    let envs: &[(&str, &str)] = &[];
    let token_path = root.join("session.token");
    let token_path = token_path.to_str().expect("utf-8 token path");

    let created = deskctl_with_socket(
        &["session", "create", "--token-out", token_path],
        &common,
        envs,
    );
    assert!(
        created.status.success(),
        "create: {}",
        stderr_text(&created)
    );
    let info = stdout_json(&created);
    let session_id = info["session_id"].as_str().expect("session_id");
    let token = info["session_token"].as_str().expect("session_token");
    let file_token = std::fs::read_to_string(token_path)
        .expect("read token file")
        .trim()
        .to_string();
    assert_eq!(file_token, token);

    let called = deskctl_with_socket(
        &[
            "call",
            session_id,
            "desktop_screen_size",
            "--session-token-file",
            token_path,
        ],
        &common,
        envs,
    );
    assert!(
        called.status.success(),
        "call with token file: {}",
        stderr_text(&called)
    );
    let result = stdout_json(&called);
    assert_eq!(result["ok"], true);

    stop_daemon(&mut daemon, &common, envs);
    let _ = std::fs::remove_dir_all(root);
}

/// AC-AGT-004: the per-owner session cap only bites the owner that reaches it.
#[test]
fn owner_session_limit_is_per_owner() {
    let root = temp_dir("owner-limit");
    let envs: &[(&str, &str)] = &[("DESKCTL_OWNER_MAX_SESSIONS", "2")];
    let (mut daemon, common) = start_daemon(&root, "limit", envs);

    assert!(create_session(&common, envs, Some("agent:lim"))
        .status
        .success());
    assert!(create_session(&common, envs, Some("agent:lim"))
        .status
        .success());

    let third = create_session(&common, envs, Some("agent:lim"));
    assert_eq!(
        third.status.code(),
        Some(5),
        "third session for the same owner must be resource_busy: {}",
        stderr_text(&third)
    );
    let stderr = stderr_text(&third);
    assert!(stderr.contains("resource_busy"), "stderr: {stderr}");
    assert!(
        stderr.contains("agent:lim"),
        "stderr names the owner: {stderr}"
    );
    assert!(
        stderr.contains("limit 2"),
        "stderr states the limit: {stderr}"
    );

    // Other owners and the default owner remain unaffected.
    let other = create_session(&common, envs, Some("agent:other"));
    assert!(
        other.status.success(),
        "other owner: {}",
        stderr_text(&other)
    );
    let default_owner = create_session(&common, envs, None);
    assert!(
        default_owner.status.success(),
        "default owner: {}",
        stderr_text(&default_owner)
    );

    stop_daemon(&mut daemon, &common, envs);
    let _ = std::fs::remove_dir_all(root);
}

/// AC-AGT-003: daemon audits attribute every call to the session owner, and
/// `history --owner` / `--summary` group by that identity.
#[test]
fn daemon_attribution_filters_history() {
    let root = temp_dir("owner-audit");
    let audit_dir = root.join("audit");
    let audit_dir_arg = audit_dir.display().to_string();
    let envs: &[(&str, &str)] = &[
        ("DESKCTL_AUDIT", "basic"),
        ("DESKCTL_AUDIT_DIR", &audit_dir_arg),
    ];
    let (mut daemon, common) = start_daemon(&root, "audit", envs);

    let alice = stdout_json(&create_session(&common, envs, Some("agent:alice")));
    let bob = stdout_json(&create_session(&common, envs, Some("agent:bob")));

    assert!(call_tool(&common, envs, &alice, "desktop_screen_size")
        .status
        .success());
    assert!(call_tool(&common, envs, &alice, "desktop_screen_size")
        .status
        .success());
    assert!(call_tool(&common, envs, &bob, "desktop_screen_size")
        .status
        .success());

    stop_daemon(&mut daemon, &common, envs);

    let history = deskctl(&["history", "--json", "--dir", &audit_dir_arg], envs);
    assert!(history.status.success());
    let records = stdout_json_lines(&history);
    let alice_id = alice["session_id"].as_str().expect("alice session_id");
    let alice_records: Vec<&Value> = records
        .iter()
        .filter(|record| record["session_id"] == alice_id)
        .collect();
    assert_eq!(alice_records.len(), 2);
    assert!(
        alice_records
            .iter()
            .all(|record| record["owner"] == "agent:alice"),
        "one session keeps one owner across calls: {alice_records:#?}"
    );
    let bob_id = bob["session_id"].as_str().expect("bob session_id");
    let bob_records: Vec<&Value> = records
        .iter()
        .filter(|record| record["session_id"] == bob_id)
        .collect();
    assert_eq!(bob_records.len(), 1);
    assert!(bob_records
        .iter()
        .all(|record| record["owner"] == "agent:bob"));

    // Exact filter.
    let filtered = deskctl(
        &[
            "history",
            "--json",
            "--dir",
            &audit_dir_arg,
            "--owner",
            "agent:alice",
        ],
        envs,
    );
    let filtered = stdout_json_lines(&filtered);
    assert_eq!(filtered.len(), 2);
    assert!(filtered
        .iter()
        .all(|record| record["owner"] == "agent:alice"));

    // Prefix wildcard (`agent:*`), as documented in the identity design.
    let wildcard = deskctl(
        &[
            "history",
            "--json",
            "--dir",
            &audit_dir_arg,
            "--owner",
            "agent:*",
        ],
        envs,
    );
    let wildcard = stdout_json_lines(&wildcard);
    assert!(wildcard.iter().all(|record| record["owner"]
        .as_str()
        .is_some_and(|o| o.starts_with("agent:"))));
    assert!(wildcard
        .iter()
        .any(|record| record["owner"] == "agent:alice"));
    assert!(wildcard.iter().any(|record| record["owner"] == "agent:bob"));

    let summary = deskctl(&["history", "--summary", "--dir", &audit_dir_arg], envs);
    let summary = stdout_json(&summary);
    let owners = summary["by_owner"].as_array().expect("by_owner");
    let alice_entry = owners
        .iter()
        .find(|entry| entry["owner"] == "agent:alice")
        .expect("alice in by_owner");
    assert_eq!(alice_entry["calls"], 2);
    assert_eq!(alice_entry["ok"], 2);
    assert_eq!(alice_entry["failed"], 0);
    assert!(alice_entry["p50_ms"].as_u64().is_some());
    assert!(owners.iter().any(|entry| entry["owner"] == "agent:bob"));

    let _ = std::fs::remove_dir_all(root);
}

fn run_mcp(envs: &[(&str, &str)], initialize_params: &str) -> Output {
    let input = [
        initialize_params.to_string(),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"desktop_screen_size","arguments":{}}}"#.to_string(),
        r#"{"jsonrpc":"2.0","id":3,"method":"shutdown","params":{}}"#.to_string(),
    ]
    .join("\n")
        + "\n";
    let mut command = Command::new(env!("CARGO_BIN_EXE_deskctl"));
    command.args(["mcp", "--skill-root", "skills"]);
    for (key, value) in envs {
        command.env(key, value);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mcp");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait mcp")
}

/// AC-AGT-002: `initialize.clientInfo.name` becomes `mcp:<name>`; a missing
/// client info falls back to `mcp:unknown`.
#[test]
fn mcp_client_info_maps_to_owner() {
    let root = temp_dir("mcp-owner");
    let audit_dir = root.join("audit");
    let audit_dir_arg = audit_dir.display().to_string();
    let envs: &[(&str, &str)] = &[
        ("DESKCTL_AUDIT", "basic"),
        ("DESKCTL_AUDIT_DIR", &audit_dir_arg),
    ];

    let output = run_mcp(
        envs,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"Claude Desktop","version":"9.9"}}}"#,
    );
    assert!(output.status.success(), "mcp: {}", stderr_text(&output));

    let output = run_mcp(
        envs,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#,
    );
    assert!(output.status.success(), "mcp: {}", stderr_text(&output));

    let history = deskctl(&["history", "--json", "--dir", &audit_dir_arg], envs);
    assert!(history.status.success());
    let records = stdout_json_lines(&history);
    let mcp_records: Vec<&Value> = records
        .iter()
        .filter(|record| record["source"] == "mcp" && record["tool"] == "desktop_screen_size")
        .collect();
    assert_eq!(mcp_records.len(), 2, "one call per MCP run: {records:#?}");
    assert!(
        mcp_records
            .iter()
            .all(|record| record["owner"] == "mcp:claude-desktop"
                || record["owner"] == "mcp:unknown"),
        "owners must be mapped client names: {mcp_records:#?}"
    );
    assert!(mcp_records
        .iter()
        .any(|record| record["owner"] == "mcp:claude-desktop"));
    assert!(mcp_records
        .iter()
        .any(|record| record["owner"] == "mcp:unknown"));

    let _ = std::fs::remove_dir_all(root);
}
