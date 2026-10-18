use std::collections::HashSet;
use std::path::Path;

use serde_json::Value;

#[test]
fn skill_manifests_parse_and_reference_known_tools() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("skills");
    let known: HashSet<&str> = deskctl_protocol::ToolName::ALL
        .iter()
        .map(|tool| tool.as_str())
        .collect();
    let mut found = 0;
    for entry in std::fs::read_dir(&root).expect("skills directory") {
        let entry = entry.expect("skill entry");
        if !entry.file_type().expect("file type").is_dir() {
            continue;
        }
        found += 1;
        let manifest_path = entry.path().join("skill.json");
        let raw = std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", manifest_path.display()));
        let manifest: Value = serde_json::from_str(&raw)
            .unwrap_or_else(|error| panic!("parse {}: {error}", manifest_path.display()));
        assert!(manifest["id"].is_string());
        assert!(manifest["version"].is_string());
        for tool in manifest["tools"].as_array().expect("tools array") {
            let name = tool.as_str().expect("tool name");
            assert!(known.contains(name), "unknown skill tool {name}");
        }
        assert!(entry.path().join("SKILL.md").is_file());
    }
    assert!(found >= 5, "expected at least five skills, found {found}");
}
