use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn mcp_initialize_list_and_call() {
    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"desktop_screen_size","arguments":{}}}"#,
    ]
    .join("\n")
        + "\n";
    let mut child = Command::new(env!("CARGO_BIN_EXE_deskctl"))
        .args(["mcp", "--skill-root", "skills"])
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
    let output = child.wait_with_output().expect("wait mcp");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let lines: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("protocol JSON"))
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["result"]["serverInfo"]["name"], "deskctl");
    assert_eq!(
        lines[1]["result"]["tools"].as_array().expect("tools").len(),
        28
    );
    assert_eq!(lines[2]["result"]["isError"], false);
    assert!(
        lines[2]["result"]["structuredContent"]["data"]["width"]
            .as_u64()
            .unwrap_or_default()
            > 0
    );
    let tools = lines[1]["result"]["tools"].as_array().expect("tools");
    assert!(tools.iter().all(|tool| {
        tool["name"]
            .as_str()
            .is_some_and(|name| !name.contains("approve") && !name.contains("reject"))
    }));
}

#[test]
fn mcp_rejects_tools_before_initialize_and_bad_versions() {
    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2099-01-01","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#,
    ]
    .join("\n")
        + "\n";
    let mut child = Command::new(env!("CARGO_BIN_EXE_deskctl"))
        .args(["mcp", "--skill-root", "skills"])
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
    let output = child.wait_with_output().expect("wait mcp");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let lines: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("protocol JSON"))
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["error"]["code"], -32002);
    assert_eq!(lines[1]["error"]["code"], -32602);
}

#[test]
fn mcp_cancel_notification_cancels_wait() {
    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"desktop_wait_for","arguments":{"condition":"stable","timeout_ms":30000,"stable_ms":30000,"interval_ms":100,"region":{"x":0,"y":0,"width":1,"height":1}}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}}"#,
    ]
    .join("\n")
        + "\n";
    let mut child = Command::new(env!("CARGO_BIN_EXE_deskctl"))
        .args(["mcp", "--skill-root", "skills"])
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
    let output = child.wait_with_output().expect("wait mcp");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let call = stdout
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("protocol JSON"))
        .find(|message| message["id"] == 2)
        .expect("tools/call response");
    assert_eq!(call["result"]["isError"], true);
    assert_eq!(
        call["result"]["structuredContent"]["error"]["code"],
        "cancelled"
    );
}
