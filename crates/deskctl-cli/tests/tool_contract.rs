use std::collections::HashSet;

use deskctl_protocol::{
    schema::{input_schema, tool_descriptors},
    ToolName, ToolProfile,
};

#[test]
fn every_tool_schema_is_object_with_required_fields() {
    for tool in ToolName::ALL {
        let schema = input_schema(tool);
        assert_eq!(schema["type"], "object", "{}", tool.as_str());
        assert!(schema["properties"].is_object(), "{}", tool.as_str());
        for required in schema["required"].as_array().unwrap() {
            let key = required.as_str().unwrap();
            assert!(
                schema["properties"].get(key).is_some(),
                "{} requires missing property {}",
                tool.as_str(),
                key
            );
        }
    }
}

#[test]
fn profiles_expose_expected_tool_counts() {
    assert_eq!(tool_descriptors(ToolProfile::Standard).len(), 33);
    assert_eq!(tool_descriptors(ToolProfile::Full).len(), 35);
    assert!(tool_descriptors(ToolProfile::Minimal).len() < 33);
    assert!(tool_descriptors(ToolProfile::Readonly).len() < 33);
}

#[test]
fn tool_names_are_unique_and_desktop_prefixed() {
    let mut seen = HashSet::new();
    for tool in ToolName::ALL {
        assert!(tool.as_str().starts_with("desktop_"));
        assert!(
            seen.insert(tool.as_str()),
            "duplicate tool {}",
            tool.as_str()
        );
    }
}

#[test]
fn no_approval_surface_is_exposed() {
    for descriptor in tool_descriptors(ToolProfile::Full) {
        let name = descriptor["name"].as_str().unwrap();
        assert!(!name.contains("approve"));
        assert!(!name.contains("reject"));
        assert!(!name.contains("approval"));
    }
}

#[test]
fn optional_tools_are_excluded_from_standard_profile() {
    let standard: HashSet<String> = tool_descriptors(ToolProfile::Standard)
        .into_iter()
        .map(|descriptor| descriptor["name"].as_str().unwrap().to_string())
        .collect();
    assert!(!standard.contains("desktop_vision"));
    assert!(!standard.contains("desktop_agent_step"));
}

#[test]
fn readonly_profile_excludes_input_tools() {
    let readonly: HashSet<String> = tool_descriptors(ToolProfile::Readonly)
        .into_iter()
        .map(|descriptor| descriptor["name"].as_str().unwrap().to_string())
        .collect();
    assert!(!readonly.contains("desktop_mouse"));
    assert!(!readonly.contains("desktop_input"));
    assert!(!readonly.contains("desktop_semantic_execute"));
    assert!(readonly.contains("desktop_screenshot"));
    assert!(readonly.contains("desktop_windows_list"));
}
