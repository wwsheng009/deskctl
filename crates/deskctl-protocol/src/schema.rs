//! Public JSON Schemas for the complete 28-tool surface.

use serde_json::{json, Value};

use crate::tool::{tool_names_for_profile, ToolName, ToolProfile};

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn rect() -> Value {
    json!({
        "type": "object",
        "description": "Absolute screen region.",
        "properties": {
            "x": { "type": "integer" },
            "y": { "type": "integer" },
            "width": { "type": "integer", "minimum": 1 },
            "height": { "type": "integer", "minimum": 1 }
        },
        "required": ["x", "y", "width", "height"],
        "additionalProperties": false
    })
}

fn locator() -> Value {
    json!({
        "type": "object",
        "properties": {
            "app_id": { "type": "string" },
            "window_id": { "type": "string" },
            "window_title": { "type": "string" },
            "role": {
                "type": "string",
                "enum": ["window","button","text_field","check_box","radio_button","list","list_item","menu","menu_item","tab","document","other"]
            },
            "automation_id": { "type": "string" },
            "accessible_name": { "type": "string" },
            "ancestor_chain": {
                "type": "array",
                "items": { "type": "object" }
            },
            "supported_action": {
                "type": "string",
                "enum": ["invoke","toggle","set_checked","select","expand","collapse","focus","set_value","scroll","scroll_into_view","set_range_value"]
            },
            "ordinal_hint": { "type": "integer", "minimum": 0, "maximum": 65535 }
        },
        "required": ["app_id"],
        "additionalProperties": false
    })
}

fn expectation() -> Value {
    json!({
        "type": "object",
        "properties": {
            "locator": locator(),
            "condition": {
                "type": "string",
                "enum": ["exists","absent","window_exists","window_absent","focused","checked","selected","expanded","value_equals"]
            },
            "expected": { "type": "boolean", "default": true },
            "value": { "type": "string" },
            "timeout_ms": { "type": "integer", "minimum": 0, "maximum": 300000, "default": 5000 },
            "stable_samples": { "type": "integer", "minimum": 1, "maximum": 5, "default": 2 }
        },
        "required": ["locator", "condition"],
        "additionalProperties": false
    })
}

pub fn input_schema(name: ToolName) -> Value {
    match name {
        ToolName::Mouse => object(
            json!({
                "action": { "type": "string", "enum": ["click","double_click","hover","scroll","position","move"] },
                "hwnd": { "type": "integer" },
                "capture_id": { "type": "string" },
                "element_id": { "type": "integer", "minimum": 0 },
                "x": { "type": "integer" },
                "y": { "type": "integer" },
                "button": { "type": "string", "enum": ["left","right","middle"], "default": "left" },
                "clicks": { "type": "integer", "minimum": 1, "maximum": 2, "default": 1 },
                "direction": { "type": "string", "enum": ["up","down","left","right"] },
                "amount": { "type": "integer", "minimum": 1, "maximum": 100, "default": 3 },
                "allow_occluded": { "type": "boolean", "default": false, "description": "Force a click whose target point is occluded by a different window" }
            }),
            &["action"],
        ),
        ToolName::MouseDrag => object(
            json!({
                "start_capture_id": { "type": "string" },
                "start_element_id": { "type": "integer", "minimum": 0 },
                "end_capture_id": { "type": "string" },
                "end_element_id": { "type": "integer", "minimum": 0 },
                "start_x": { "type": "integer" },
                "start_y": { "type": "integer" },
                "end_x": { "type": "integer" },
                "end_y": { "type": "integer" }
            }),
            &[],
        ),
        ToolName::Input => object(
            json!({
                "mode": { "type": "string", "enum": ["type","hotkey"] },
                "target_locator": locator(),
                "hwnd": { "type": "integer" },
                "launch_ref": { "type": "string", "description": "Optional app_ref from desktop_targets_list; relaunches the app when the target window is missing" },
                "text": { "type": "string", "maxLength": 65536 },
                "send": { "type": "string", "enum": ["enter","ctrl+enter","tab","none"], "default": "enter" },
                "keys": { "type": "array", "items": { "type": "string" }, "minItems": 1 }
            }),
            &["mode"],
        ),
        ToolName::Screenshot => object(
            json!({
                "path": { "type": "string" },
                "region": rect(),
                "format": { "type": "string", "enum": ["png","jpeg","webp"], "description": "Output format; defaults to the daemon capture policy" },
                "quality": { "type": "integer", "minimum": 1, "maximum": 100, "description": "JPEG/WebP quality" },
                "max_width": { "type": "integer", "minimum": 1, "maximum": 16384 },
                "max_height": { "type": "integer", "minimum": 1, "maximum": 16384 },
                "max_bytes": { "type": "integer", "minimum": 1024, "maximum": 67108864, "description": "Hard cap; encoder reduces quality/dimensions until it fits" }
            }),
            &[],
        ),
        ToolName::ScreenSize => object(json!({}), &[]),
        ToolName::WindowScreenshot => object(
            json!({
                "hwnd": { "type": "integer" },
                "title": { "type": "string" },
                "path": { "type": "string" },
                "format": { "type": "string", "enum": ["png","jpeg","webp"] },
                "quality": { "type": "integer", "minimum": 1, "maximum": 100 },
                "max_width": { "type": "integer", "minimum": 1, "maximum": 16384 },
                "max_height": { "type": "integer", "minimum": 1, "maximum": 16384 },
                "max_bytes": { "type": "integer", "minimum": 1024, "maximum": 67108864 }
            }),
            &[],
        ),
        ToolName::WindowsList => object(json!({}), &[]),
        ToolName::WindowActivate => object(json!({ "hwnd": { "type": "integer" } }), &["hwnd"]),
        ToolName::WindowMove => object(
            json!({
                "hwnd": { "type": "integer" },
                "x": { "type": "integer" },
                "y": { "type": "integer" }
            }),
            &["hwnd", "x", "y"],
        ),
        ToolName::WindowResize => object(
            json!({
                "hwnd": { "type": "integer" },
                "width": { "type": "integer", "minimum": 1 },
                "height": { "type": "integer", "minimum": 1 }
            }),
            &["hwnd", "width", "height"],
        ),
        ToolName::WindowInfo => object(json!({ "hwnd": { "type": "integer" } }), &["hwnd"]),
        ToolName::Vision => object(
            json!({
                "image_path": { "type": "string" },
                "prompt": { "type": "string" }
            }),
            &["image_path"],
        ),
        ToolName::Perceive => object(
            json!({ "image_path": { "type": "string" } }),
            &["image_path"],
        ),
        ToolName::FindImage => object(
            json!({
                "template_path": { "type": "string", "minLength": 1 },
                "region": rect(),
                "threshold": { "type": "number", "minimum": 0, "maximum": 1, "default": 0.9 }
            }),
            &["template_path"],
        ),
        ToolName::FindColor => object(
            json!({
                "color": { "type": "string", "minLength": 1 },
                "region": rect(),
                "direction": {
                    "type": "string",
                    "enum": ["left_top","left_bottom","right_top","right_bottom","column_left_top","column_right_top","column_left_bottom","column_right_bottom","center"],
                    "default": "left_top"
                }
            }),
            &["color"],
        ),
        ToolName::FindMultiColor => object(
            json!({
                "anchor": { "type": "string", "minLength": 1 },
                "offsets": { "type": "string", "minLength": 1 },
                "region": rect(),
                "min_match_ratio": { "type": "number", "minimum": 0, "maximum": 1, "default": 0.8 },
                "direction": {
                    "type": "string",
                    "enum": ["left_top","left_bottom","right_top","right_bottom","column_left_top","column_right_top","column_left_bottom","column_right_bottom","center"],
                    "default": "left_top"
                }
            }),
            &["anchor", "offsets"],
        ),
        ToolName::FindText => object(
            json!({
                "dict_name": { "type": "string", "minLength": 1 },
                "words": { "type": "string", "minLength": 1 },
                "region": rect(),
                "sim": { "type": "number", "minimum": 0, "maximum": 1, "default": 1.0 }
            }),
            &["dict_name", "words", "region"],
        ),
        ToolName::WaitFor => object(
            json!({
                "condition": { "type": "string", "enum": ["change","stable","color"] },
                "region": rect(),
                "color": { "type": "string" },
                "timeout_ms": { "type": "integer", "minimum": 1, "maximum": 300000, "default": 10000 },
                "interval_ms": { "type": "integer", "minimum": 50, "maximum": 5000, "default": 200 },
                "stable_ms": { "type": "integer", "minimum": 0, "maximum": 300000, "default": 500 }
            }),
            &["condition"],
        ),
        ToolName::ClipboardClean => object(json!({}), &[]),
        ToolName::ClipboardWrite => object(
            json!({ "text": { "type": "string", "maxLength": 1048576 } }),
            &["text"],
        ),
        ToolName::TargetsList => object(
            json!({
                "query": { "type": "string" },
                "cursor": { "type": "integer", "minimum": 0 }
            }),
            &[],
        ),
        ToolName::TargetBind => object(
            json!({
                "app_ref": { "type": "string" },
                "window_ref": { "type": "string" },
                "delivery_mode": { "type": "string", "enum": ["foreground","auto","background"], "default": "foreground" },
                "follow_dialog": { "type": "boolean", "default": false, "description": "When the selected window is disabled by a modal dialog (native Save/Open), bind that dialog instead" }
            }),
            &["app_ref"],
        ),
        ToolName::SemanticObserve => object(
            json!({
                "goal": { "type": "string" },
                "target_token": { "type": "string" },
                "scope": { "type": "string", "enum": ["window","menu"] },
                "subtree_id": { "type": "string" },
                "observation_token": { "type": "string" },
                "cursor": { "type": "integer", "minimum": 0 },
                "view": { "type": "string", "enum": ["candidates","regions"] },
                "tree_cursor": { "type": "string", "description": "Continue reading the native UIA tree from next_tree_cursor without re-capturing" },
                "delivery_mode": { "type": "string", "enum": ["foreground","auto","background"], "default": "auto" }
            }),
            &[],
        ),
        ToolName::SemanticCandidate => object(
            json!({
                "observation_token": { "type": "string" },
                "candidate_id": { "type": "string" }
            }),
            &["observation_token", "candidate_id"],
        ),
        ToolName::SemanticExecute => object(
            json!({
                "observation_token": { "type": "string" },
                "candidate_id": { "type": "string" },
                "value": { "type": "string", "maxLength": 16384 },
                "delivery_mode": { "type": "string", "enum": ["foreground","auto","background"], "default": "auto" },
                "checked": { "type": "boolean", "description": "Desired state for set_checked; already-satisfied targets are not clicked" },
                "direction": { "type": "string", "enum": ["up","down","left","right"] },
                "amount": { "type": "string", "enum": ["small","page"], "default": "small" },
                "completion_policy": { "type": "string", "enum": ["auto","verified","dispatched"], "default": "auto" },
                "expectation": expectation(),
                "restore_foreground": { "type": "boolean", "default": false, "description": "Restore the pre-action foreground window after the action (best effort)" }
            }),
            &["observation_token", "candidate_id"],
        ),
        ToolName::SemanticAction => object(
            json!({
                "locator": locator(),
                "action": { "type": "string", "enum": ["invoke","toggle","set_checked","select","expand","collapse","focus","set_value","scroll","scroll_into_view","set_range_value"] },
                "launch_ref": { "type": "string", "description": "Optional app_ref from desktop_targets_list; relaunches the app when the locator window is missing" },
                "value": { "type": "string", "maxLength": 16384 },
                "delivery_mode": { "type": "string", "enum": ["foreground","auto","background"], "default": "auto" },
                "checked": { "type": "boolean", "description": "Desired state for set_checked; already-satisfied targets are not clicked" },
                "direction": { "type": "string", "enum": ["up","down","left","right"] },
                "amount": { "type": "string", "enum": ["small","page"], "default": "small" },
                "completion_policy": { "type": "string", "enum": ["auto","verified","dispatched"], "default": "auto" },
                "expectation": expectation(),
                "restore_foreground": { "type": "boolean", "default": false, "description": "Restore the pre-action foreground window after the action (best effort)" }
            }),
            &["locator", "action"],
        ),
        ToolName::VerifyState => object(json!({ "expectation": expectation() }), &["expectation"]),
        ToolName::ImageCrop => object(
            json!({
                "capture_id": { "type": "string" },
                "image_path": { "type": "string" },
                "x": { "type": "integer" },
                "y": { "type": "integer" },
                "width": { "type": "integer", "minimum": 1 },
                "height": { "type": "integer", "minimum": 1 },
                "path": { "type": "string" },
                "format": { "type": "string", "enum": ["png","jpeg","webp"] },
                "quality": { "type": "integer", "minimum": 1, "maximum": 100 },
                "max_width": { "type": "integer", "minimum": 1, "maximum": 16384 },
                "max_height": { "type": "integer", "minimum": 1, "maximum": 16384 },
                "max_bytes": { "type": "integer", "minimum": 1024, "maximum": 67108864 }
            }),
            &["x", "y", "width", "height"],
        ),
        ToolName::DocumentSave => object(
            json!({
                "path": { "type": "string" },
                "title": { "type": "string" },
                "sections": {
                    "type": "array",
                    "minItems": 1,
                    "items": object(
                        json!({
                            "heading": { "type": "string" },
                            "text": { "type": "string", "maxLength": 65536 },
                            "image": object(
                                json!({
                                    "capture_id": { "type": "string" },
                                    "image_path": { "type": "string" },
                                    "caption": { "type": "string" },
                                    "width_cm": { "type": "number", "minimum": 1, "maximum": 18 }
                                }),
                                &[],
                            )
                        }),
                        &[],
                    )
                }
            }),
            &["sections"],
        ),
        ToolName::AgentStep => object(
            json!({
                "goal": { "type": "string", "minLength": 1 },
                "target_token": { "type": "string" },
                "scope": { "type": "string", "enum": ["window","menu"] },
                "subtree_id": { "type": "string" }
            }),
            &["goal"],
        ),
    }
}

/// JSON object returned by MCP `tools/list` and embedded in generated docs.
pub fn tool_descriptor(name: ToolName) -> Value {
    let meta = name.meta();
    json!({
        "name": name.as_str(),
        "description": meta.description,
        "inputSchema": input_schema(name),
        "annotations": {
            "title": name.as_str(),
            "readOnlyHint": meta.read_only,
            "destructiveHint": meta.destructive,
            "idempotentHint": meta.read_only,
            "openWorldHint": false
        }
    })
}

pub fn tool_descriptors(profile: ToolProfile) -> Vec<Value> {
    tool_names_for_profile(profile)
        .into_iter()
        .map(tool_descriptor)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_an_object_schema() {
        for name in ToolName::ALL {
            let schema = input_schema(name);
            assert_eq!(schema["type"], "object", "{}", name.as_str());
            assert!(schema["properties"].is_object(), "{}", name.as_str());
            assert!(schema["required"].is_array(), "{}", name.as_str());
        }
    }

    #[test]
    fn descriptors_have_stable_counts_and_names() {
        assert_eq!(tool_descriptors(ToolProfile::Standard).len(), 28);
        assert_eq!(tool_descriptors(ToolProfile::Full).len(), 30);
        for descriptor in tool_descriptors(ToolProfile::Full) {
            assert!(descriptor["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("desktop_")));
            assert!(!descriptor["name"]
                .as_str()
                .is_some_and(|name| name.contains("approve") || name.contains("reject")));
        }
    }

    #[test]
    fn all_required_fields_exist_in_properties() {
        for name in ToolName::ALL {
            let schema = input_schema(name);
            for required in schema["required"].as_array().unwrap() {
                let key = required.as_str().unwrap();
                assert!(
                    schema["properties"].get(key).is_some(),
                    "{} requires missing property {}",
                    name.as_str(),
                    key
                );
            }
        }
    }
}
