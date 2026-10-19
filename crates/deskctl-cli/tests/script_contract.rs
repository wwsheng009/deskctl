//! Contract tests for D3 record/replay (AC-RPL-001..014).
//!
//! These exercise the CLI + daemon end to end with tools that do not need a
//! real GUI: `desktop_screen_size` (read), `desktop_file_hash` (readback) and
//! `desktop_recorder_*` / `desktop_script_run` themselves.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("deskctl-script-it-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
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

fn deskctl(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deskctl"));
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run deskctl")
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON stdout ({error}): {} / stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn start_daemon(root: &Path, tag: &str, envs: &[(&str, &str)]) -> (Child, Vec<String>) {
    let socket = format!(r"\\.\pipe\deskctl-script-{tag}-{}", std::process::id());
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

fn create_session(common: &[String], envs: &[(&str, &str)]) -> Value {
    let output = deskctl_with_socket(&["session", "create"], common, envs);
    assert!(
        output.status.success(),
        "session create: {}",
        stderr_text(&output)
    );
    stdout_json(&output)
}

fn call_tool(
    common: &[String],
    envs: &[(&str, &str)],
    session: &Value,
    tool: &str,
    args: Value,
) -> Output {
    let id = session["session_id"].as_str().expect("session_id");
    let token = session["session_token"].as_str().expect("session_token");
    let raw = serde_json::to_string(&args).expect("args JSON");
    deskctl_with_socket(
        &["call", id, tool, "--json", &raw, "--session-token", token],
        common,
        envs,
    )
}

/// AC-RPL-001/002/004: record real calls, export a valid script, validate it,
/// dry-run it, and confirm dry-run leaves no audit trail.
#[test]
fn record_export_validate_and_dry_run() {
    let root = temp_dir("record");
    let audit = root.join("audit");
    let runs = root.join("runs");
    let envs: &[(&str, &str)] = &[
        ("DESKCTL_AUDIT", "full"),
        ("DESKCTL_AUDIT_DIR", audit.to_str().unwrap()),
        ("DESKCTL_RUNS_DIR", runs.to_str().unwrap()),
    ];
    let (mut daemon, common) = start_daemon(&root, "record", envs);
    let session = create_session(&common, envs);
    let id = session["session_id"].as_str().expect("session_id");

    let token = session["session_token"].as_str().expect("token");
    let started = deskctl_with_socket(
        &[
            "record",
            "--session",
            id,
            "--session-token",
            token,
            "start",
            "--include-reads",
            "all",
            "--max-steps",
            "10",
        ],
        &common,
        envs,
    );
    assert!(started.status.success(), "{}", stderr_text(&started));
    assert_eq!(stdout_json(&started)["data"]["recording"], true);

    // A second recorder in the same session must be rejected.
    let busy = call_tool(&common, envs, &session, "desktop_recorder_start", json!({}));
    let busy = stdout_json(&busy);
    assert_eq!(busy["error"]["code"], "resource_busy");

    let first = call_tool(&common, envs, &session, "desktop_screen_size", json!({}));
    assert!(first.status.success(), "{}", stderr_text(&first));
    let second = call_tool(&common, envs, &session, "desktop_screen_size", json!({}));
    assert!(second.status.success(), "{}", stderr_text(&second));

    let path = root.join("recorded.dscript.json");
    let stopped = deskctl_with_socket(
        &[
            "record",
            "--session",
            id,
            "--session-token",
            token,
            "stop",
            "--out",
            path.to_str().unwrap(),
        ],
        &common,
        envs,
    );
    assert!(stopped.status.success(), "{}", stderr_text(&stopped));
    let stopped = stdout_json(&stopped);
    let script = &stopped["data"]["script"];
    assert_eq!(script["v"], 1);
    assert_eq!(script["format"], "deskctl-script/1");
    let steps = script["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 2, "two screen_size calls recorded");
    assert_eq!(steps[0]["id"], "s1");
    assert_eq!(steps[0]["tool"], "desktop_screen_size");
    assert_eq!(stopped["output"], path.to_str().unwrap());
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("written script"))
            .expect("written script JSON");
    assert_eq!(written["format"], "deskctl-script/1");
    assert_eq!(written["steps"].as_array().map(Vec::len), Some(2));

    // `script validate` is local and must accept the recorded document.
    let validated = deskctl(
        &["script", "validate", "--file", path.to_str().unwrap()],
        envs,
    );
    assert_eq!(
        validated.status.code(),
        Some(0),
        "validate: {} / {}",
        String::from_utf8_lossy(&validated.stdout),
        stderr_text(&validated)
    );
    assert_eq!(stdout_json(&validated)["ok"], true);

    // Dry-run must not execute anything nor create audit records.
    let before = audit_records(&audit);
    let dry = deskctl_with_socket(
        &[
            "script",
            "run",
            "--file",
            path.to_str().unwrap(),
            "--session",
            id,
            "--session-token",
            token,
            "--dry-run",
        ],
        &common,
        envs,
    );
    assert!(dry.status.success(), "{}", stderr_text(&dry));
    let dry = stdout_json(&dry);
    assert_eq!(dry["data"]["dry_run"], true);
    assert!(dry["data"]["outcome"].is_null());
    assert_eq!(dry["data"]["steps"].as_array().map(Vec::len), Some(2));
    for step in dry["data"]["steps"].as_array().expect("steps") {
        assert_eq!(step["decision"], "execute");
    }
    sleep(Duration::from_millis(300));
    assert_eq!(
        run_scoped_records(&audit),
        0,
        "dry-run must not audit any replayed step"
    );
    assert!(
        audit_records(&audit) >= before,
        "the script_run call itself is audited"
    );

    // A real run produces a run report with run_id/outcome/handoff and audits
    // each step under `<run_id>:<step>`.
    let run = call_tool(
        &common,
        envs,
        &session,
        "desktop_script_run",
        json!({ "script": script }),
    );
    assert!(run.status.success(), "{}", stderr_text(&run));
    let run = stdout_json(&run);
    let report = &run["data"];
    let run_id = report["run_id"].as_str().expect("run_id").to_string();
    assert!(run_id.starts_with("run-"));
    assert_eq!(report["format"], "deskctl-run-report/1");
    assert_eq!(report["result"], "ok");
    assert_eq!(report["outcome"], "COMPLETED_UNVERIFIED");
    assert_eq!(report["handoff"]["run_id"], run_id.as_str());
    assert_eq!(report["stats"]["ok"], 2);
    assert_eq!(report["steps"][0]["retries"], 0);

    sleep(Duration::from_millis(400));
    let history = deskctl(&["history", "--run", &run_id, "--json"], envs);
    assert!(
        history.status.success(),
        "history: {}",
        stderr_text(&history)
    );
    let records = json_lines(&history);
    assert_eq!(records.len(), 2, "one audit record per step");
    for record in &records {
        assert_eq!(record["run_id"], run_id.as_str());
        let request = record["request_id"].as_str().expect("request_id");
        assert!(
            request.starts_with(&format!("{run_id}:")),
            "request_id {request} must be scoped to the run"
        );
    }

    // Same session cannot run two scripts at once: start a slow one first.
    let slow = json!({
        "v": 1,
        "format": "deskctl-script/1",
        "name": "slow",
        "steps": [{
            "id": "s1",
            "tool": "desktop_wait_for",
            "params": { "condition": "stable", "stable_ms": 600, "timeout_ms": 5000 }
        }]
    });
    let raw = serde_json::to_string(&json!({ "script": slow })).expect("args");
    let token = session["session_token"].as_str().expect("token");
    let mut command = Command::new(env!("CARGO_BIN_EXE_deskctl"));
    command
        .args([
            "call",
            id,
            "desktop_script_run",
            "--json",
            &raw,
            "--session-token",
            token,
        ])
        .args(&common)
        .envs(envs.iter().copied())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut first_run = command.spawn().expect("spawn first run");
    sleep(Duration::from_millis(120));
    let second_run = call_tool(
        &common,
        envs,
        &session,
        "desktop_script_run",
        json!({ "script": script }),
    );
    let second_run = stdout_json(&second_run);
    assert_eq!(
        second_run["error"]["code"], "resource_busy",
        "concurrent runs must be rejected: {second_run}"
    );
    assert!(
        second_run["error"]["details"]["run_id"]
            .as_str()
            .is_some_and(|value| value.starts_with("run-")),
        "busy error must name the active run: {second_run}"
    );
    let _ = first_run.wait();

    stop_daemon(&mut daemon, &common, envs);
}

/// AC-RPL-003/013: preflight rejects unsafe scripts with a report-shaped
/// `policy_denied` payload, and a `not_sent` failure yields a resumable handoff.
#[test]
fn preflight_rejections_and_handoff() {
    let root = temp_dir("reject");
    let audit_dir = root.join("audit");
    let envs: &[(&str, &str)] = &[("DESKCTL_AUDIT_DIR", audit_dir.to_str().unwrap())];
    let (mut daemon, common) = start_daemon(&root, "reject", envs);
    let session = create_session(&common, envs);

    // Unknown tool → policy_denied with a REJECTED_POLICY report.
    let unknown = call_tool(
        &common,
        envs,
        &session,
        "desktop_script_run",
        json!({
            "script": {
                "v": 1,
                "format": "deskctl-script/1",
                "steps": [{ "id": "s1", "tool": "desktop_nope", "params": {} }]
            }
        }),
    );
    let unknown = stdout_json(&unknown);
    assert_eq!(unknown["error"]["code"], "policy_denied");
    assert_eq!(unknown["error"]["details"]["outcome"], "REJECTED_POLICY");
    assert_eq!(
        unknown["error"]["details"]["handoff"]["safe_to_retry"],
        true
    );

    // External-commit step above the default risk ceiling → rejected.
    let risky = call_tool(
        &common,
        envs,
        &session,
        "desktop_script_run",
        json!({
            "script": {
                "v": 1,
                "format": "deskctl-script/1",
                "steps": [{
                    "id": "s1",
                    "tool": "desktop_script_run",
                    "params": { "script": { "v": 1, "format": "deskctl-script/1", "steps": [] } }
                }]
            }
        }),
    );
    let risky = stdout_json(&risky);
    assert_eq!(risky["error"]["code"], "policy_denied", "{risky}");
    let codes: Vec<&str> = risky["error"]["details"]["issues"]
        .as_array()
        .expect("issues")
        .iter()
        .filter_map(|issue| issue["code"].as_str())
        .collect();
    assert!(codes.contains(&"risk_exceeded"), "{codes:?}");

    // Unresolved variable → rejected (AC-RPL-003).
    let unresolved = call_tool(
        &common,
        envs,
        &session,
        "desktop_script_run",
        json!({
            "script": {
                "v": 1,
                "format": "deskctl-script/1",
                "steps": [{
                    "id": "s1",
                    "tool": "desktop_screen_size",
                    "params": { "unused": "$var.missing" }
                }]
            }
        }),
    );
    let unresolved = stdout_json(&unresolved);
    assert_eq!(unresolved["error"]["code"], "policy_denied");
    let codes: Vec<&str> = unresolved["error"]["details"]["issues"]
        .as_array()
        .expect("issues")
        .iter()
        .filter_map(|issue| issue["code"].as_str())
        .collect();
    assert!(codes.contains(&"unresolved_var"), "{codes:?}");

    // A `not_sent` failure (bad hwnd) → HALTED_BEFORE_EFFECT + resumable handoff.
    let failing = call_tool(
        &common,
        envs,
        &session,
        "desktop_script_run",
        json!({
            "script": {
                "v": 1,
                "format": "deskctl-script/1",
                "steps": [{
                    "id": "s1",
                    "tool": "desktop_window_info",
                    "params": { "hwnd": 999999999 }
                }]
            }
        }),
    );
    let failing = stdout_json(&failing);
    let report = &failing["data"];
    assert_eq!(report["result"], "failed", "{failing}");
    assert_eq!(report["outcome"], "HALTED_BEFORE_EFFECT");
    assert_eq!(report["handoff"]["failed_step"], "s1");
    assert_eq!(report["handoff"]["safe_to_retry"], true);
    assert_eq!(report["handoff"]["resume_hint"]["from_step"], "s1");
    assert_eq!(report["steps"][0]["dispatch_state"], "not_sent");

    // Resuming from the failed step works in the same session.
    let resumed = call_tool(
        &common,
        envs,
        &session,
        "desktop_script_run",
        json!({
            "script": {
                "v": 1,
                "format": "deskctl-script/1",
                "steps": [
                    { "id": "s1", "tool": "desktop_screen_size", "params": {} },
                    { "id": "s2", "tool": "desktop_screen_size", "params": {} }
                ]
            },
            "from_step": "s2"
        }),
    );
    let resumed = stdout_json(&resumed);
    assert_eq!(resumed["data"]["stats"]["total"], 1, "{resumed}");
    assert_eq!(resumed["data"]["steps"][0]["id"], "s2");

    stop_daemon(&mut daemon, &common, envs);
}

/// AC-RPL-008: readback verification reads the file back via `desktop_file_hash`
/// and downgrades to a failing outcome when the digest does not match.
#[test]
fn readback_verification_uses_file_hash() {
    let root = temp_dir("readback");
    let allowed = root.join("allowed");
    std::fs::create_dir_all(&allowed).expect("allowed root");
    let file = allowed.join("artifact.txt");
    std::fs::write(&file, "payload").expect("write artifact");
    let audit_dir = root.join("audit");
    let envs: &[(&str, &str)] = &[
        ("DESKCTL_ALLOWED_INPUT_ROOT", allowed.to_str().unwrap()),
        ("DESKCTL_AUDIT_DIR", audit_dir.to_str().unwrap()),
    ];
    let (mut daemon, common) = start_daemon(&root, "readback", envs);
    let session = create_session(&common, envs);

    let hash = call_tool(
        &common,
        envs,
        &session,
        "desktop_file_hash",
        json!({ "path": file.to_str().unwrap() }),
    );
    let hash = stdout_json(&hash);
    assert_eq!(hash["data"]["exists"], true);
    let sha = hash["data"]["sha256"].as_str().expect("sha256").to_string();

    let verify = |sha: &str| {
        json!({
            "v": 1,
            "format": "deskctl-script/1",
            "params": { "path": file.to_str().unwrap() },
            "steps": [{
                "id": "s1",
                "tool": "desktop_file_hash",
                "params": { "path": "$param.path" },
                "verify": {
                    "tier": "readback",
                    "kind": "file_sha256",
                    "target": "$param.path",
                    "sha256": sha
                }
            }]
        })
    };

    let passing = call_tool(
        &common,
        envs,
        &session,
        "desktop_script_run",
        json!({ "script": verify(&sha) }),
    );
    let passing = stdout_json(&passing);
    assert_eq!(passing["data"]["outcome"], "VERIFIED", "{passing}");
    assert_eq!(passing["data"]["stats"]["readback_verified"], 1);
    assert_eq!(passing["data"]["steps"][0]["verify_tier"], "readback");

    let failing = call_tool(
        &common,
        envs,
        &session,
        "desktop_script_run",
        json!({ "script": verify(&"0".repeat(64)) }),
    );
    let failing = stdout_json(&failing);
    assert_eq!(
        failing["data"]["outcome"], "RECONCILIATION_REQUIRED",
        "{failing}"
    );
    assert_eq!(failing["data"]["handoff"]["safe_to_retry"], false);
    assert!(failing["data"]["handoff"]["resume_hint"].is_null());

    // The next `--from` resume must be blocked until the caller acknowledges.
    let blocked = call_tool(
        &common,
        envs,
        &session,
        "desktop_script_run",
        json!({ "script": verify(&sha), "from_step": "s1" }),
    );
    let blocked = stdout_json(&blocked);
    assert_eq!(blocked["error"]["code"], "policy_denied", "{blocked}");
    let codes: Vec<&str> = blocked["error"]["details"]["issues"]
        .as_array()
        .expect("issues")
        .iter()
        .filter_map(|issue| issue["code"].as_str())
        .collect();
    assert!(codes.contains(&"resume_after_unknown"), "{codes:?}");

    let allowed_resume = call_tool(
        &common,
        envs,
        &session,
        "desktop_script_run",
        json!({
            "script": verify(&sha),
            "from_step": "s1",
            "allow_resume_after_unknown": true
        }),
    );
    let allowed_resume = stdout_json(&allowed_resume);
    assert_eq!(
        allowed_resume["data"]["outcome"], "VERIFIED",
        "{allowed_resume}"
    );

    stop_daemon(&mut daemon, &common, envs);
}

fn audit_records(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .map(|entry| {
            std::fs::read_to_string(entry.path())
                .map(|text| text.lines().filter(|line| !line.trim().is_empty()).count())
                .unwrap_or(0)
        })
        .sum()
}

/// Audit lines attributed to a replay step (`run_id` set from
/// `<run_id>:<step>` request ids).
fn run_scoped_records(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .map(|entry| {
            std::fs::read_to_string(entry.path())
                .map(|text| {
                    text.lines()
                        .filter(|line| !line.trim().is_empty())
                        .filter(|line| {
                            serde_json::from_str::<Value>(line)
                                .ok()
                                .and_then(|record| record["run_id"].as_str().map(str::to_string))
                                .is_some()
                        })
                        .count()
                })
                .unwrap_or(0)
        })
        .sum()
}

fn json_lines(output: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("json line"))
        .collect()
}
