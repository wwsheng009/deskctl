use std::collections::{HashMap, HashSet};
use std::path::Path;

use deskctl_protocol::{ToolName, ToolProfile};
use serde_json::Value;

#[test]
fn skill_manifests_parse_and_reference_known_tools() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("skills");
    let known: HashSet<&str> = ToolName::ALL.iter().map(|tool| tool.as_str()).collect();
    let mut found = 0;
    for entry in std::fs::read_dir(&root).expect("skills directory") {
        let entry = entry.expect("skill entry");
        if !entry.file_type().expect("file type").is_dir() {
            continue;
        }
        found += 1;
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let manifest_path = entry.path().join("skill.json");
        let raw = std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", manifest_path.display()));
        let manifest: Value = serde_json::from_str(&raw)
            .unwrap_or_else(|error| panic!("parse {}: {error}", manifest_path.display()));
        assert!(manifest["id"].is_string());
        assert!(manifest["version"].is_string());

        let profiles: Vec<ToolProfile> = manifest["profiles"]
            .as_array()
            .expect("profiles array")
            .iter()
            .map(|profile| {
                profile
                    .as_str()
                    .expect("profile name")
                    .parse::<ToolProfile>()
                    .expect("known profile")
            })
            .collect();
        assert!(
            !profiles.is_empty(),
            "{} must list at least one profile",
            manifest_path.display()
        );

        for tool in manifest["tools"].as_array().expect("tools array") {
            let name = tool.as_str().expect("tool name");
            assert!(known.contains(name), "unknown skill tool {name}");
            let tool: ToolName = name.parse().expect("known tool");
            assert!(
                profiles.iter().any(|profile| tool.profile_allows(*profile)),
                "skill tool {name} is not available in any listed profile of {}",
                manifest_path.display()
            );
        }

        validate_skill_frontmatter(&entry.path().join("SKILL.md"), &dir_name);
    }
    assert!(found >= 5, "expected at least five skills, found {found}");
}

/// Claude/Codex skill format: `SKILL.md` starts with YAML frontmatter carrying
/// `name` (matching the folder name) and `description`.
fn validate_skill_frontmatter(skill_md: &Path, dir_name: &str) {
    let raw = std::fs::read_to_string(skill_md)
        .unwrap_or_else(|error| panic!("read {}: {error}", skill_md.display()));
    let text = raw.strip_prefix('\u{feff}').unwrap_or(raw.as_str());
    let frontmatter = text
        .strip_prefix("---")
        .and_then(|rest| {
            rest.strip_prefix("\r\n")
                .or_else(|| rest.strip_prefix('\n'))
        })
        .and_then(|rest| rest.find("\n---").map(|end| &rest[..end]))
        .unwrap_or_else(|| panic!("{} must start with YAML frontmatter", skill_md.display()));

    let mut fields = HashMap::new();
    for line in frontmatter.lines() {
        if let Some((key, value)) = line.split_once(':') {
            fields.insert(
                key.trim().to_string(),
                value.trim().trim_matches('"').to_string(),
            );
        }
    }

    let name = fields
        .get("name")
        .unwrap_or_else(|| panic!("{} frontmatter is missing `name`", skill_md.display()));
    assert_eq!(
        name,
        dir_name,
        "{} frontmatter `name` must match the folder",
        skill_md.display()
    );
    assert!(
        name.len() <= 64
            && name
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'),
        "{} frontmatter `name` must be hyphen-case (lowercase letters, digits, hyphens)",
        skill_md.display()
    );

    let description = fields.get("description").unwrap_or_else(|| {
        panic!(
            "{} frontmatter is missing `description`",
            skill_md.display()
        )
    });
    assert!(
        !description.is_empty() && description.len() <= 1024,
        "{} frontmatter `description` must be 1..=1024 characters",
        skill_md.display()
    );
    assert!(
        !description.contains('<') && !description.contains('>'),
        "{} frontmatter `description` cannot contain angle brackets",
        skill_md.display()
    );
}
