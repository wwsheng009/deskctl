//! Contract tests for the D1 audit trail and `deskctl history`.
//!
//! AC-AUD-001/002/003/005/007/008: one-shot calls are recorded (including
//! failures), `args_digest` is recomputable, `history` filters/aggregates and
//! `DESKCTL_AUDIT=off` creates no files.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::Value;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("deskctl-audit-it-{tag}-{}", std::process::id()));
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

fn stdout_json_lines(output: &Output) -> Vec<Value> {
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("json line"))
        .collect()
}

fn digest(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("sha256:{}", hex::encode(sha2::Sha256::digest(bytes)))
}

#[test]
fn oneshot_calls_are_audited_and_queryable() {
    let root = temp_dir("oneshot");
    let audit_dir = root.join("audit");
    let audit_dir_arg = audit_dir.display().to_string();
    let envs: &[(&str, &str)] = &[
        ("DESKCTL_AUDIT", "full"),
        ("DESKCTL_AUDIT_DIR", &audit_dir_arg),
        ("DESKCTL_AUDIT_TEXT", "digest"),
    ];

    let run = deskctl(&["tool", "desktop_screen_size"], envs);
    assert!(run.status.success(), "screen_size should succeed");

    let history = deskctl(&["history", "--json"], envs);
    assert!(history.status.success());
    let records = stdout_json_lines(&history);
    assert!(
        !records.is_empty(),
        "history should return the recorded call"
    );
    let record = records
        .iter()
        .find(|record| record["tool"] == "desktop_screen_size")
        .expect("screen_size record");
    assert_eq!(record["v"], 1);
    assert_eq!(record["kind"], "tool_call");
    assert_eq!(record["source"], "oneshot");
    assert_eq!(record["owner"], "oneshot");
    assert_eq!(record["ok"], true);
    // screen_size is a pure read: nothing is dispatched to the desktop.
    assert_eq!(record["dispatch_state"], "not_sent");
    assert_eq!(record["effect"], "read");
    assert_eq!(record["verification"], "not_applicable");
    assert_eq!(record["retryable"], false);
    assert_eq!(
        record["args_digest"].as_str(),
        Some(digest(b"{}").as_str()),
        "args_digest must be recomputable from raw args"
    );
    let ts = record["ts"].as_str().expect("ts");
    chrono::DateTime::parse_from_rfc3339(ts).expect("rfc3339 timestamp");

    let limited = deskctl(&["history", "--limit", "1", "--json"], envs);
    assert_eq!(
        stdout_json_lines(&limited).len(),
        1,
        "--limit caps the list"
    );

    let empty = deskctl(
        &["history", "--since", "30m", "--until", "now", "--json"],
        envs,
    );
    assert!(
        !stdout_json_lines(&empty).is_empty(),
        "recent window matches"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn failed_calls_land_in_history() {
    let root = temp_dir("failed");
    let audit_dir = root.join("audit");
    let audit_dir_arg = audit_dir.display().to_string();
    let envs: &[(&str, &str)] = &[
        ("DESKCTL_AUDIT", "full"),
        ("DESKCTL_AUDIT_DIR", &audit_dir_arg),
    ];

    let run = deskctl(
        &[
            "tool",
            "desktop_mouse",
            "--json",
            r#"{"action":"teleport"}"#,
        ],
        envs,
    );
    assert_eq!(run.status.code(), Some(2), "invalid params exit code");

    let history = deskctl(&["history", "--failed-only", "--json"], envs);
    assert!(history.status.success());
    let records = stdout_json_lines(&history);
    let record = records
        .iter()
        .find(|record| record["tool"] == "desktop_mouse")
        .expect("failed mouse record");
    assert_eq!(record["ok"], false);
    assert_eq!(record["dispatch_state"], "not_sent");
    assert_eq!(record["retryable"], false);
    assert_eq!(record["error"]["code"], "invalid_params");
    assert!(
        record["args_digest"].as_str().is_some(),
        "failure records still carry args_digest"
    );

    let ok_only = deskctl(&["history", "--json"], envs);
    assert!(
        stdout_json_lines(&ok_only)
            .iter()
            .any(|record| record["ok"] == true || record["ok"] == false),
        "unfiltered history still returns the row"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn summary_aggregates_by_tool() {
    let root = temp_dir("summary");
    let audit_dir = root.join("audit");
    let audit_dir_arg = audit_dir.display().to_string();
    let envs: &[(&str, &str)] = &[
        ("DESKCTL_AUDIT", "basic"),
        ("DESKCTL_AUDIT_DIR", &audit_dir_arg),
    ];

    for _ in 0..2 {
        let run = deskctl(&["tool", "desktop_screen_size"], envs);
        assert!(run.status.success());
    }

    let summary = deskctl(&["history", "--summary"], envs);
    assert!(summary.status.success());
    let text = String::from_utf8_lossy(&summary.stdout);
    let value: Value = serde_json::from_str(&text).expect("summary json");
    assert!(value["window"]["calls"].as_u64().unwrap_or(0) >= 2);
    let tool = &value["by_tool"][0];
    assert_eq!(tool["tool"], "desktop_screen_size");
    assert!(tool["calls"].as_u64().unwrap_or(0) >= 2);
    assert!(tool["p50_ms"].as_u64().is_some());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn audit_off_writes_nothing_and_history_hints() {
    let root = temp_dir("off");
    let audit_dir = root.join("audit");
    let audit_dir_arg = audit_dir.display().to_string();
    let envs: &[(&str, &str)] = &[
        ("DESKCTL_AUDIT", "off"),
        ("DESKCTL_AUDIT_DIR", &audit_dir_arg),
    ];

    let run = deskctl(&["tool", "desktop_screen_size"], envs);
    assert!(run.status.success());
    assert!(
        !audit_dir.exists(),
        "off mode must not create the audit dir"
    );

    let history = deskctl(&["history"], envs);
    assert!(history.status.success());
    let text = String::from_utf8_lossy(&history.stdout);
    assert!(
        text.contains("audit is disabled"),
        "history should explain that auditing is off: {text}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn history_supports_dir_override_and_rejects_bad_kind() {
    let root = temp_dir("override");
    let audit_dir = root.join("audit");
    let audit_dir_arg = audit_dir.display().to_string();
    let envs: &[(&str, &str)] = &[
        ("DESKCTL_AUDIT", "basic"),
        ("DESKCTL_AUDIT_DIR", &audit_dir_arg),
    ];
    let run = deskctl(&["tool", "desktop_screen_size"], envs);
    assert!(run.status.success());

    // `--dir` wins over whatever the environment points at.
    let history = Command::new(env!("CARGO_BIN_EXE_deskctl"))
        .args(["history", "--json", "--dir", &audit_dir_arg])
        .env(
            "DESKCTL_AUDIT_DIR",
            root.join("elsewhere").display().to_string(),
        )
        .output()
        .expect("run history --dir");
    assert!(history.status.success());
    assert!(!stdout_json_lines(&history).is_empty());

    let bad = deskctl(&["history", "--kind", "bogus"], envs);
    assert_eq!(bad.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(
        stderr.contains("invalid_params"),
        "bad kind must be invalid_params: {stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// AC-AUD-001: a daemon writes exactly one record per tool call (success and
/// failure alike), attributed to the calling session, and flushes on shutdown.
#[test]
fn daemon_calls_are_audited_exactly_once() {
    let root = temp_dir("daemon");
    let audit_dir = root.join("audit");
    let audit_dir_arg = audit_dir.display().to_string();
    let socket = format!(r"\\.\pipe\deskctl-audit-it-{}", std::process::id());
    let token_file = root.join("daemon.token");
    let envs: &[(&str, &str)] = &[
        ("DESKCTL_AUDIT", "basic"),
        ("DESKCTL_AUDIT_DIR", &audit_dir_arg),
    ];
    let common: Vec<String> = vec![
        "--socket".to_string(),
        socket.clone(),
        "--token-file".to_string(),
        token_file.display().to_string(),
    ];

    let mut daemon = Command::new(env!("CARGO_BIN_EXE_deskctl"))
        .args(["serve", "--max-sessions", "4"])
        .args(&common)
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut ready = false;
    while Instant::now() < deadline {
        if deskctl_with_socket(&["session", "list"], &common, envs)
            .status
            .success()
        {
            ready = true;
            break;
        }
        sleep(Duration::from_millis(250));
    }
    if !ready {
        let _ = daemon.kill();
        panic!("daemon did not become ready within 20s");
    }

    let created = deskctl_with_socket(&["session", "create"], &common, envs);
    assert!(created.status.success(), "session create must succeed");
    let info: Value = serde_json::from_slice(&created.stdout).expect("session create json");
    let session_id = info["session_id"]
        .as_str()
        .expect("session_id field")
        .to_string();
    let session_token = info["session_token"]
        .as_str()
        .expect("session_token field")
        .to_string();

    let ok_call = deskctl_with_socket(
        &[
            "call",
            &session_id,
            "desktop_screen_size",
            "--session-token",
            &session_token,
        ],
        &common,
        envs,
    );
    assert!(
        ok_call.status.success(),
        "screen_size call: {}",
        String::from_utf8_lossy(&ok_call.stderr)
    );

    let failed = deskctl_with_socket(
        &[
            "call",
            &session_id,
            "desktop_mouse",
            "--json",
            r#"{"action":"teleport"}"#,
            "--session-token",
            &session_token,
        ],
        &common,
        envs,
    );
    assert_eq!(failed.status.code(), Some(2), "invalid params exit code");

    let shutdown = deskctl_with_socket(&["shutdown"], &common, envs);
    assert!(shutdown.status.success(), "shutdown must succeed");
    let mut exited = false;
    for _ in 0..40 {
        if daemon.try_wait().expect("poll daemon").is_some() {
            exited = true;
            break;
        }
        sleep(Duration::from_millis(250));
    }
    if !exited {
        let _ = daemon.kill();
        panic!("daemon did not exit after shutdown");
    }

    let history = deskctl(&["history", "--json", "--dir", &audit_dir_arg], envs);
    assert!(history.status.success());
    let records = stdout_json_lines(&history);
    let mine: Vec<&Value> = records
        .iter()
        .filter(|record| record["session_id"] == session_id.as_str())
        .collect();
    assert_eq!(
        mine.len(),
        2,
        "exactly one record per call, including the failure: {records:#?}"
    );

    let ok_record = mine
        .iter()
        .find(|record| record["tool"] == "desktop_screen_size")
        .expect("success record");
    assert_eq!(ok_record["source"], "daemon");
    assert_eq!(ok_record["kind"], "tool_call");
    assert_eq!(ok_record["ok"], true);

    let fail_record = mine
        .iter()
        .find(|record| record["tool"] == "desktop_mouse")
        .expect("failure record");
    assert_eq!(fail_record["source"], "daemon");
    assert_eq!(fail_record["ok"], false);
    assert_eq!(fail_record["error"]["code"], "invalid_params");

    let _ = std::fs::remove_dir_all(root);
}

/// AC-AUD-006: when the audit directory cannot be created, the tool still
/// succeeds and the audit pipeline degrades with a single warning.
#[test]
fn audit_write_failure_degrades_without_failing_the_tool() {
    let root = temp_dir("degraded");
    let blocker = root.join("blocked");
    std::fs::write(&blocker, b"not a directory").expect("write blocker file");
    let audit_dir = blocker.join("audit");
    let audit_dir_arg = audit_dir.display().to_string();
    let envs: &[(&str, &str)] = &[
        ("DESKCTL_AUDIT", "basic"),
        ("DESKCTL_AUDIT_DIR", &audit_dir_arg),
    ];

    let run = deskctl(&["tool", "desktop_screen_size"], envs);
    assert!(
        run.status.success(),
        "tool must succeed when the audit dir is unusable: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("audit disabled"),
        "degradation must be observable once: {stderr}"
    );
    assert_eq!(
        stderr.matches("audit disabled").count(),
        1,
        "the warning is emitted exactly once: {stderr}"
    );

    let history = deskctl(&["history"], envs);
    assert!(history.status.success());

    let _ = std::fs::remove_dir_all(root);
}
