//! Contract tests for AC-SEC-013: untrusted screen text wrapping.
//!
//! `history --json` echoes `error.message`/`error.details` text that can embed
//! screen content (candidate window titles, observed text). Those strings must
//! be returned inside `<untrusted-screen-content>` tags by default, and the
//! `DESKCTL_UNTRUSTED_WRAP=off` switch must restore the raw records.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

const OPEN_TAG: &str = "<untrusted-screen-content>";
const CLOSE_TAG: &str = "</untrusted-screen-content>";

fn temp_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("deskctl-untrusted-it-{tag}-{}", std::process::id()));
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

fn stdout_json_lines(output: &Output) -> Vec<Value> {
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("json line"))
        .collect()
}

/// Build an audit shard with one failed tool call whose error text contains
/// screen-derived strings (window title + observed text).
fn write_shard(dir: &Path, now: &str) {
    let record = json!({
        "v": 1,
        "ts": now,
        "kind": "tool_call",
        "request_id": "req:123",
        "session_id": "sess-untrusted-test",
        "owner": "agent:test",
        "source": "oneshot",
        "tool": "desktop_target_bind",
        "risk": "read_only",
        "args": {"name": "sap"},
        "args_digest": "sha256:00",
        "args_truncated": false,
        "duration_ms": 3,
        "ok": false,
        "dispatch_state": "refused",
        "effect": "none",
        "verification": "not_applicable",
        "retryable": true,
        "error": {
            "code": "ambiguous_target",
            "message": "multiple windows match target 'sap'",
            "details": {
                "candidates": [{"hwnd": 42, "title": "SAP Easy Access"}],
                "observed": "S4H sap1809demo"
            }
        }
    });
    let line = serde_json::to_string(&record).expect("serialize record");
    std::fs::write(dir.join("audit-2026-10-19.jsonl"), format!("{line}\n")).expect("write shard");
}

#[test]
fn history_json_wraps_screen_text_by_default() {
    let root = temp_dir("on");
    let audit_dir = root.join("audit");
    std::fs::create_dir_all(&audit_dir).expect("create audit dir");
    let now = chrono::Local::now().to_rfc3339();
    write_shard(&audit_dir, &now);
    let dir_arg = audit_dir.display().to_string();

    let output = deskctl(
        &[
            "history",
            "--json",
            "--dir",
            &dir_arg,
            "--kind",
            "tool_call",
        ],
        &[],
    );
    assert!(
        output.status.success(),
        "history failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = stdout_json_lines(&output);
    assert_eq!(records.len(), 1, "one record expected");
    let record = &records[0];

    let message = record["error"]["message"].as_str().expect("message");
    assert_eq!(
        message,
        format!("{OPEN_TAG}multiple windows match target 'sap'{CLOSE_TAG}")
    );
    let title = record["error"]["details"]["candidates"][0]["title"]
        .as_str()
        .expect("candidate title");
    assert_eq!(title, format!("{OPEN_TAG}SAP Easy Access{CLOSE_TAG}"));
    let observed = record["error"]["details"]["observed"]
        .as_str()
        .expect("observed text");
    assert_eq!(observed, format!("{OPEN_TAG}S4H sap1809demo{CLOSE_TAG}"));
    // Non-text fields keep their raw values.
    assert_eq!(record["error"]["details"]["candidates"][0]["hwnd"], 42);
    assert_eq!(record["session_id"], "sess-untrusted-test");
    assert_eq!(record["tool"], "desktop_target_bind");
}

#[test]
fn history_json_wrap_off_switch_restores_raw_text() {
    let root = temp_dir("off");
    let audit_dir = root.join("audit");
    std::fs::create_dir_all(&audit_dir).expect("create audit dir");
    let now = chrono::Local::now().to_rfc3339();
    write_shard(&audit_dir, &now);
    let dir_arg = audit_dir.display().to_string();

    let output = deskctl(
        &[
            "history",
            "--json",
            "--dir",
            &dir_arg,
            "--kind",
            "tool_call",
        ],
        &[("DESKCTL_UNTRUSTED_WRAP", "off")],
    );
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        !text.contains(OPEN_TAG),
        "wrap=off must not add tags: {text}"
    );
    let records = stdout_json_lines(&output);
    assert_eq!(
        records[0]["error"]["message"],
        "multiple windows match target 'sap'"
    );
    assert_eq!(
        records[0]["error"]["details"]["candidates"][0]["title"],
        "SAP Easy Access"
    );
}
