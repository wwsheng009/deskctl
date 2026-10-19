//! Contract tests for D4 checkpoint & compensation (AC-TXN-001..003).
//!
//! Everything runs CLI + daemon end to end without a real GUI: checkpoints made
//! of value/rect samples that need no windows, plus `desktop_screen_size` as the
//! workhorse read step for script runs and compensation scripts.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("deskctl-cp-it-{tag}-{}", std::process::id()));
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
    let socket = format!(r"\\.\pipe\deskctl-cp-{tag}-{}", std::process::id());
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

fn write_script(dir: &Path, file_name: &str, document: Value) -> PathBuf {
    let path = dir.join(file_name);
    std::fs::write(&path, format!("{document}\n")).expect("write script");
    path
}

fn run_script(common: &[String], envs: &[(&str, &str)], session: &Value, extra: &[&str]) -> Output {
    let id = session["session_id"].as_str().expect("session_id");
    let token = session["session_token"].as_str().expect("session_token");
    let mut args = vec!["script", "run", "--session", id, "--session-token", token];
    args.extend_from_slice(extra);
    deskctl_with_socket(&args, common, envs)
}

/// AC-TXN-001: save/verify roundtrip, file export/import, and input validation.
#[test]
fn checkpoint_roundtrip_exports_and_validation() {
    let root = temp_dir("roundtrip");
    let audit = root.join("audit");
    let envs: &[(&str, &str)] = &[("DESKCTL_AUDIT_DIR", audit.to_str().unwrap())];
    let (mut daemon, common) = start_daemon(&root, "roundtrip", envs);
    let session = create_session(&common, envs);

    // A checkpoint without targets/values is legal: nothing to compare, matched.
    let saved = stdout_json(&call_tool(
        &common,
        envs,
        &session,
        "desktop_checkpoint_save",
        json!({ "name": "base" }),
    ));
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(saved["data"]["saved"], true);
    assert_eq!(
        saved["data"]["checkpoint"]["format"],
        "deskctl-checkpoint/1"
    );
    assert_eq!(saved["data"]["checkpoint"]["name"], "base");
    assert_eq!(saved["data"]["checkpoint"]["targets"], json!([]));

    let verified = stdout_json(&call_tool(
        &common,
        envs,
        &session,
        "desktop_checkpoint_verify",
        json!({ "name": "base" }),
    ));
    assert_eq!(verified["ok"], true, "{verified}");
    assert_eq!(verified["data"]["matched"], true);
    assert_eq!(verified["data"]["checked"], 0);

    // Export to a file inside an allowed root, then verify by path.
    let export = root.join("base.checkpoint.json");
    let saved = stdout_json(&call_tool(
        &common,
        envs,
        &session,
        "desktop_checkpoint_save",
        json!({ "name": "base", "path": export.display().to_string() }),
    ));
    assert_eq!(
        saved["data"]["path"],
        export.display().to_string(),
        "{saved}"
    );
    assert!(export.exists(), "export file must be written");
    let by_path = stdout_json(&call_tool(
        &common,
        envs,
        &session,
        "desktop_checkpoint_verify",
        json!({ "path": export.display().to_string() }),
    ));
    assert_eq!(by_path["data"]["matched"], true, "{by_path}");

    // Unknown name / malformed name / ambiguous source are all invalid_params.
    let unknown = call_tool(
        &common,
        envs,
        &session,
        "desktop_checkpoint_verify",
        json!({ "name": "nope" }),
    );
    assert_eq!(stdout_json(&unknown)["error"]["code"], "invalid_params");
    assert_eq!(unknown.status.code(), Some(2));
    let bad_name = call_tool(
        &common,
        envs,
        &session,
        "desktop_checkpoint_save",
        json!({ "name": "Bad Name" }),
    );
    assert_eq!(stdout_json(&bad_name)["error"]["code"], "invalid_params");
    let ambiguous = call_tool(
        &common,
        envs,
        &session,
        "desktop_checkpoint_verify",
        json!({ "name": "base", "path": export.display().to_string() }),
    );
    assert_eq!(stdout_json(&ambiguous)["error"]["code"], "invalid_params");

    stop_daemon(&mut daemon, &common, envs);
}

/// AC-TXN-001: the 8-per-session cap, overwrite at the cap, and rect diffs.
#[test]
fn checkpoint_quota_overwrite_and_rect_diff() {
    let root = temp_dir("quota");
    let audit = root.join("audit");
    let envs: &[(&str, &str)] = &[("DESKCTL_AUDIT_DIR", audit.to_str().unwrap())];
    let (mut daemon, common) = start_daemon(&root, "quota", envs);
    let session = create_session(&common, envs);

    for index in 1..=8 {
        let name = format!("c{index}");
        let saved = stdout_json(&call_tool(
            &common,
            envs,
            &session,
            "desktop_checkpoint_save",
            json!({ "name": name }),
        ));
        assert_eq!(saved["ok"], true, "{name}: {saved}");
    }
    let ninth = call_tool(
        &common,
        envs,
        &session,
        "desktop_checkpoint_save",
        json!({ "name": "c9" }),
    );
    assert_eq!(stdout_json(&ninth)["error"]["code"], "resource_busy");
    assert_eq!(ninth.status.code(), Some(5), "resource_busy maps to exit 5");
    // Overwriting an existing name is still allowed at the cap.
    let overwrite = stdout_json(&call_tool(
        &common,
        envs,
        &session,
        "desktop_checkpoint_save",
        json!({ "name": "c2" }),
    ));
    assert_eq!(overwrite["ok"], true, "{overwrite}");

    // An inline document with a dead hwnd verifies as a `rect` diff (missing).
    let inline = json!({
        "v": 1,
        "format": "deskctl-checkpoint/1",
        "name": "inline",
        "targets": [{
            "name": "ghost",
            "hwnd": 999999999,
            "rect": { "x": 1, "y": 2, "width": 3, "height": 4 }
        }],
        "values": []
    });
    let diff = stdout_json(&call_tool(
        &common,
        envs,
        &session,
        "desktop_checkpoint_verify",
        json!({ "checkpoint": inline }),
    ));
    assert_eq!(diff["data"]["matched"], false, "{diff}");
    let first = &diff["data"]["diffs"][0];
    assert_eq!(first["kind"], "rect");
    assert_eq!(first["name"], "ghost");
    assert_eq!(first["missing"], true);
    assert_eq!(
        first["expected"],
        json!({ "x": 1, "y": 2, "width": 3, "height": 4 })
    );
    assert_eq!(first["actual"], Value::Null);

    // values_only skips window comparison entirely.
    let values_only = stdout_json(&call_tool(
        &common,
        envs,
        &session,
        "desktop_checkpoint_verify",
        json!({ "checkpoint": inline, "values_only": true }),
    ));
    assert_eq!(values_only["data"]["matched"], true, "{values_only}");
    assert_eq!(values_only["data"]["checked"], 0);

    // A checkpoint sampled from a live window is round-trip stable.
    let windows = stdout_json(&call_tool(
        &common,
        envs,
        &session,
        "desktop_windows_list",
        json!({ "limit": 1 }),
    ));
    if let Some(row) = windows["data"]["windows"]
        .as_array()
        .and_then(|rows| rows.first())
    {
        let hwnd = row["hwnd"].as_i64().expect("hwnd");
        // Overwrite an existing name: new names are rejected at the cap.
        let saved = stdout_json(&call_tool(
            &common,
            envs,
            &session,
            "desktop_checkpoint_save",
            json!({ "name": "c2", "targets": [{ "name": "win", "hwnd": hwnd }] }),
        ));
        assert_eq!(saved["ok"], true, "{saved}");
        assert_eq!(saved["data"]["checkpoint"]["targets"][0]["hwnd"], hwnd);
        let verified = stdout_json(&call_tool(
            &common,
            envs,
            &session,
            "desktop_checkpoint_verify",
            json!({ "name": "c2" }),
        ));
        assert_eq!(verified["data"]["matched"], true, "{verified}");
        assert!(verified["data"]["checked"].as_u64().unwrap_or_default() >= 1);
    }

    stop_daemon(&mut daemon, &common, envs);
}

/// AC-TXN-002: `script run --checkpoint` saves before the run and reports the
/// post-run verification; a failure triggers the compensation script.
#[test]
fn script_run_checkpoint_and_compensation() {
    let root = temp_dir("script");
    let audit = root.join("audit");
    let envs: &[(&str, &str)] = &[("DESKCTL_AUDIT_DIR", audit.to_str().unwrap())];
    let (mut daemon, common) = start_daemon(&root, "script", envs);
    let session = create_session(&common, envs);

    let main = write_script(
        &root,
        "main.dscript.json",
        json!({
            "v": 1,
            "format": "deskctl-script/1",
            "name": "main",
            "steps": [{ "id": "s1", "tool": "desktop_screen_size", "params": {} }]
        }),
    );
    let run = run_script(
        &common,
        envs,
        &session,
        &["--file", main.to_str().unwrap(), "--checkpoint", "pre"],
    );
    assert!(run.status.success(), "{}", stderr_text(&run));
    let report = stdout_json(&run);
    assert_eq!(report["data"]["result"], "ok", "{report}");
    assert_eq!(report["data"]["checkpoint"]["name"], "pre");
    assert_eq!(report["data"]["checkpoint"]["matched"], true);
    assert_eq!(report["data"]["compensation"], Value::Null);

    // The checkpoint the run saved is a normal, verifiable session checkpoint.
    let verified = stdout_json(&call_tool(
        &common,
        envs,
        &session,
        "desktop_checkpoint_verify",
        json!({ "name": "pre" }),
    ));
    assert_eq!(verified["data"]["matched"], true, "{verified}");

    let failing = write_script(
        &root,
        "failing.dscript.json",
        json!({
            "v": 1,
            "format": "deskctl-script/1",
            "name": "failing",
            "steps": [{
                "id": "s1",
                "tool": "desktop_window_info",
                "params": { "hwnd": 999999999 }
            }]
        }),
    );
    let compensation = write_script(
        &root,
        "compensation.dscript.json",
        json!({
            "v": 1,
            "format": "deskctl-script/1",
            "name": "recover",
            "steps": [{ "id": "s1", "tool": "desktop_screen_size", "params": {} }]
        }),
    );

    // Default: compensation produces a dry-run plan and dispatches nothing.
    let run = run_script(
        &common,
        envs,
        &session,
        &[
            "--file",
            failing.to_str().unwrap(),
            "--compensation",
            compensation.to_str().unwrap(),
        ],
    );
    assert_eq!(run.status.code(), Some(1), "failed run maps to exit 1");
    let report = stdout_json(&run);
    assert_eq!(report["data"]["result"], "failed", "{report}");
    assert_eq!(report["data"]["outcome"], "HALTED_BEFORE_EFFECT");
    let comp = &report["data"]["compensation"];
    assert_eq!(comp["applied"], false, "{report}");
    assert_eq!(comp["report"]["dry_run"], true);
    assert_eq!(comp["report"]["steps"][0]["decision"], "execute");

    // `--apply-compensation` really runs the compensation script.
    let run = run_script(
        &common,
        envs,
        &session,
        &[
            "--file",
            failing.to_str().unwrap(),
            "--compensation",
            compensation.to_str().unwrap(),
            "--apply-compensation",
        ],
    );
    let report = stdout_json(&run);
    let comp = &report["data"]["compensation"];
    assert_eq!(comp["applied"], true, "{report}");
    assert_eq!(comp["report"]["dry_run"], false);
    assert_eq!(comp["report"]["result"], "ok");
    assert_eq!(comp["report"]["stats"]["total"], 1);

    stop_daemon(&mut daemon, &common, envs);
}

/// AC-TXN-003: `purpose=compensation` scripts default to dry-run.
#[test]
fn compensation_purpose_defaults_to_dry_run() {
    let root = temp_dir("purpose");
    let audit = root.join("audit");
    let envs: &[(&str, &str)] = &[("DESKCTL_AUDIT_DIR", audit.to_str().unwrap())];
    let (mut daemon, common) = start_daemon(&root, "purpose", envs);
    let session = create_session(&common, envs);

    let undo = json!({
        "v": 1,
        "format": "deskctl-script/1",
        "name": "undo",
        "purpose": "compensation",
        "steps": [{ "id": "s1", "tool": "desktop_screen_size", "params": {} }]
    });
    // No explicit dry_run: the compensation purpose forces a dry-run plan.
    let call = stdout_json(&call_tool(
        &common,
        envs,
        &session,
        "desktop_script_run",
        json!({ "script": undo }),
    ));
    assert_eq!(call["data"]["dry_run"], true, "{call}");
    assert_eq!(call["data"]["steps"][0]["decision"], "execute");

    // An explicit dry_run=false still runs it for real.
    let call = stdout_json(&call_tool(
        &common,
        envs,
        &session,
        "desktop_script_run",
        json!({ "script": undo, "dry_run": false }),
    ));
    assert_eq!(call["data"]["dry_run"], false, "{call}");
    assert_eq!(call["data"]["result"], "ok");

    // A normal script (no purpose) keeps executing by default.
    let normal = json!({
        "v": 1,
        "format": "deskctl-script/1",
        "name": "normal",
        "steps": [{ "id": "s1", "tool": "desktop_screen_size", "params": {} }]
    });
    let call = stdout_json(&call_tool(
        &common,
        envs,
        &session,
        "desktop_script_run",
        json!({ "script": normal }),
    ));
    assert_eq!(call["data"]["dry_run"], false, "{call}");
    assert_eq!(call["data"]["result"], "ok");

    // Unknown purpose values are format errors.
    let bad = call_tool(
        &common,
        envs,
        &session,
        "desktop_script_run",
        json!({
            "script": {
                "v": 1,
                "format": "deskctl-script/1",
                "purpose": "bogus",
                "steps": [{ "id": "s1", "tool": "desktop_screen_size", "params": {} }]
            }
        }),
    );
    let bad = stdout_json(&bad);
    assert_eq!(bad["error"]["code"], "invalid_params", "{bad}");
    let codes: Vec<&str> = bad["error"]["details"]["issues"]
        .as_array()
        .expect("issues")
        .iter()
        .filter_map(|issue| issue["code"].as_str())
        .collect();
    assert!(codes.contains(&"invalid_format"), "{codes:?}");

    stop_daemon(&mut daemon, &common, envs);
}
