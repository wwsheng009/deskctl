use std::process::Command;

#[test]
fn help_lists_required_commands() {
    let output = Command::new(env!("CARGO_BIN_EXE_deskctl"))
        .arg("--help")
        .output()
        .expect("run deskctl --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for command in ["mcp", "serve", "call", "tool", "session", "doctor"] {
        assert!(stdout.contains(command), "missing command {command}");
    }
    assert!(!stdout.contains("approve"));
    assert!(!stdout.contains("reject"));
}

#[test]
fn doctor_returns_structured_json() {
    let output = Command::new(env!("CARGO_BIN_EXE_deskctl"))
        .args(["doctor"])
        .output()
        .expect("run deskctl doctor");
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("doctor JSON");
    assert_eq!(report["platform"]["supported_first_phase"], true);
    assert!(report["models"].is_object());
    assert!(report["onnx_runtime"].is_object());
    assert!(report["onnx_runtime"]["search_dirs"].is_array());
    assert!(report["onnx_runtime"].get("resolved").is_some());
}

#[test]
fn doctor_writes_compact_utf8_output_file() {
    let path = std::env::temp_dir().join(format!(
        "deskctl-doctor-output-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_deskctl"))
        .args([
            "doctor",
            "--compact",
            "--output",
            path.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("run deskctl doctor --output");
    let bytes = std::fs::read(&path).expect("read doctor output file");
    let text = String::from_utf8(bytes).expect("doctor output must be utf-8");
    assert_eq!(text.lines().count(), 1, "compact output must be one line");
    let report: serde_json::Value = serde_json::from_str(text.trim()).expect("doctor output JSON");
    assert!(report["platform"].is_object());
    let _ = std::fs::remove_file(path);
    assert!(
        matches!(output.status.code(), Some(0) | Some(6)),
        "doctor exit status: {:?}",
        output.status
    );
}

#[test]
fn exported_contracts_cover_full_profile() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = root.join("contracts/tools");
    let index: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(directory.join("index.json")).expect("contracts index"),
    )
    .expect("contracts index JSON");
    assert_eq!(index["count"], 42);
    assert_eq!(index["schema_version"], "0.1.0");
    for entry in index["tools"].as_array().expect("tools") {
        let file = entry["file"].as_str().expect("file");
        let descriptor: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(directory.join(file)).expect("contract"))
                .expect("contract JSON");
        assert_eq!(descriptor["name"], entry["name"]);
        assert_eq!(descriptor["inputSchema"]["type"], "object");
        assert!(!file.contains("approve"));
        assert!(!file.contains("reject"));
    }
}
