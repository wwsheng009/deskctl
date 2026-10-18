use std::collections::HashMap;
use std::time::Instant;

use deskctl_protocol::{DispatchState, ErrorCode, RiskClass, ToolError, ToolName, Verification};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::engine::Engine;
use crate::platform::{self, UiaAction};
use crate::session::{Candidate, ObservationState, SemanticActionKind, Session};

use super::ToolExecution;

const PAGE_SIZE: usize = 20;

pub fn execute(
    _engine: &Engine,
    session: &Session,
    tool: ToolName,
    args: &Value,
) -> Result<ToolExecution, ToolError> {
    match tool {
        ToolName::SemanticObserve => observe(session, args),
        ToolName::SemanticCandidate => candidate(session, args),
        ToolName::SemanticExecute => execute_candidate(session, args),
        ToolName::SemanticAction => execute_action(session, args),
        ToolName::VerifyState => verify_state(session, args),
        ToolName::AgentStep => Err(ToolError::new(
            ErrorCode::ModelUnavailable,
            "desktop_agent_step requires the optional enhanced-decision provider",
        )
        .retryable(false)),
        _ => Err(ToolError::new(
            ErrorCode::UnknownTool,
            format!("{} is not a semantic tool", tool.as_str()),
        )),
    }
}

fn observe(session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
    let page_cursor = args.get("cursor").and_then(Value::as_u64).unwrap_or(0) as usize;
    let existing_token = args
        .get("observation_token")
        .and_then(Value::as_str)
        .filter(|_| page_cursor > 0);
    if let Some(token) = existing_token {
        let observation = session.observation(token)?;
        return Ok(ToolExecution::read(observation_page(
            &observation,
            page_cursor,
        )?));
    }

    let (hwnd, title, target_token) = resolve_target(session, args)?;
    let limit = 200usize;
    let native_nodes = platform::observe_window(hwnd, limit)?;
    let mut nodes = Vec::with_capacity(native_nodes.len());
    let mut candidates = HashMap::new();
    for node in native_nodes {
        let value = node.value.clone();
        nodes.push(value.clone());
        if let Some(candidate) = candidate_for_node(&value, target_token.as_deref(), &title) {
            candidates.insert(candidate.id.clone(), candidate);
        }
    }
    let observation = ObservationState {
        token: format!("obs-{}", Uuid::new_v4().simple()),
        target_token,
        hwnd: Some(hwnd),
        title,
        created_at: Some(Instant::now()),
        candidates,
        nodes,
    };
    let page = observation_page(&observation, 0)?;
    session.store_observation(observation)?;
    Ok(ToolExecution::read(page))
}

fn candidate(session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
    let token = super::required_str(args, "observation_token")?;
    let candidate_id = super::required_str(args, "candidate_id")?;
    let observation = session.observation(token)?;
    let candidate = observation.candidates.get(candidate_id).ok_or_else(|| {
        ToolError::new(
            ErrorCode::StaleObservation,
            "candidate_id does not belong to the latest observation",
        )
        .retryable(true)
    })?;
    Ok(ToolExecution::read(json!({
        "observation_token": token,
        "candidate_id": candidate.id,
        "role": candidate.role,
        "name": candidate.name,
        "automation_id": candidate.automation_id,
        "action": candidate.action.as_str(),
        "risk": candidate.risk,
        "description": candidate.description,
        "workflow_step": {
            "tool": "desktop_semantic_action",
            "params": {
                "locator": candidate.locator,
                "action": candidate.action.as_str()
            }
        }
    })))
}

fn execute_candidate(session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
    let token = super::required_str(args, "observation_token")?;
    let candidate_id = super::required_str(args, "candidate_id")?;
    let observation = session.observation(token)?;
    let candidate = observation
        .candidates
        .get(candidate_id)
        .cloned()
        .ok_or_else(|| {
            ToolError::new(
                ErrorCode::StaleObservation,
                "candidate_id does not belong to the latest observation",
            )
            .retryable(true)
        })?;
    let hwnd = observation.hwnd.ok_or_else(|| {
        ToolError::new(
            ErrorCode::StaleObservation,
            "observation has no bound window",
        )
        .retryable(true)
    })?;
    ensure_foreground(hwnd)?;
    let fresh = platform::observe_window(hwnd, 500)?;
    let node = fresh
        .iter()
        .find(|node| node.value["opaque_id"].as_str() == Some(candidate.opaque_id.as_str()))
        .ok_or_else(|| {
            ToolError::new(
                ErrorCode::StaleObservation,
                "candidate target no longer exists in the fresh UIA tree",
            )
            .retryable(true)
        })?;
    let action = candidate.action;
    let value = args.get("value").and_then(Value::as_str);
    if action == SemanticActionKind::SetValue && value.is_none() {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            "value is required for a set_value candidate",
        ));
    }
    platform::perform_action(hwnd, node, uia_action(action), value)?;
    session.clear_observation()?;
    Ok(ToolExecution {
        data: json!({
            "receipt": {
                "candidate_id": candidate.id,
                "action": action.as_str(),
                "hwnd": hwnd,
                "dispatch_state": "sent"
            },
            "verification": "unknown",
            "requires_observation": true
        }),
        dispatch_state: DispatchState::Sent,
        effect: Some(deskctl_protocol::Effect::ExternalCommit),
        verification: Verification::Unknown,
        requires_observation: true,
    })
}

fn execute_action(session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
    let locator = args
        .get("locator")
        .filter(|value| value.is_object())
        .ok_or_else(|| super::invalid("locator must be a workflow_step locator object"))?;
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| super::invalid("action is required"))?;
    let action = parse_action(action)?;
    let hwnd = resolve_locator_window(session, locator)?;
    ensure_foreground(hwnd)?;
    let nodes = platform::observe_window(hwnd, 500)?;
    let matches: Vec<_> = nodes
        .iter()
        .filter(|node| locator_matches(&node.value, locator))
        .collect();
    let node = match matches.as_slice() {
        [one] => *one,
        [] => {
            return Err(ToolError::new(
                ErrorCode::TargetUnavailable,
                "semantic locator did not match exactly one current element",
            )
            .retryable(true))
        }
        _ => {
            return Err(ToolError::new(
                ErrorCode::TargetUnavailable,
                "semantic locator is ambiguous; use a more specific locator",
            )
            .retryable(true))
        }
    };
    let value = args.get("value").and_then(Value::as_str);
    if action == SemanticActionKind::SetValue && value.is_none() {
        return Err(super::invalid("value is required for action=set_value"));
    }
    platform::perform_action(hwnd, node, uia_action(action), value)?;
    session.clear_observation()?;
    Ok(ToolExecution {
        data: json!({
            "receipt": { "action": action.as_str(), "hwnd": hwnd, "dispatch_state": "sent" },
            "verification": "unknown"
        }),
        dispatch_state: DispatchState::Sent,
        effect: Some(deskctl_protocol::Effect::ExternalCommit),
        verification: Verification::Unknown,
        requires_observation: true,
    })
}

fn verify_state(session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
    let expectation = args
        .get("expectation")
        .filter(|value| value.is_object())
        .ok_or_else(|| super::invalid("expectation is required"))?;
    let locator = expectation
        .get("locator")
        .ok_or_else(|| super::invalid("expectation.locator is required"))?;
    let condition = expectation
        .get("condition")
        .and_then(Value::as_str)
        .ok_or_else(|| super::invalid("expectation.condition is required"))?;
    let hwnd = resolve_locator_window(session, locator)?;
    let nodes = platform::observe_window(hwnd, 500)?;
    let matching: Vec<&Value> = nodes
        .iter()
        .map(|node| &node.value)
        .filter(|node| locator_matches(node, locator))
        .collect();
    let status = match condition {
        "exists" | "window_exists" => {
            if matching.is_empty() {
                "unsatisfied"
            } else {
                "satisfied"
            }
        }
        "absent" | "window_absent" => {
            if matching.is_empty() {
                "satisfied"
            } else {
                "unsatisfied"
            }
        }
        "focused" => matching
            .first()
            .and_then(|node| node["focused"].as_bool())
            .map_or(
                "unknown",
                |value| if value { "satisfied" } else { "unsatisfied" },
            ),
        "checked" => matching
            .first()
            .and_then(|node| node["toggled"].as_bool())
            .map_or(
                "unknown",
                |value| if value { "satisfied" } else { "unsatisfied" },
            ),
        "selected" => matching
            .first()
            .and_then(|node| node["selected"].as_bool())
            .map_or(
                "unknown",
                |value| if value { "satisfied" } else { "unsatisfied" },
            ),
        "expanded" => matching
            .first()
            .and_then(|node| node["expanded"].as_bool())
            .map_or(
                "unknown",
                |value| if value { "satisfied" } else { "unsatisfied" },
            ),
        "value_equals" => {
            let expected = expectation.get("value").and_then(Value::as_str);
            match (
                matching
                    .first()
                    .and_then(|node| node["short_value"].as_str()),
                expected,
            ) {
                (Some(actual), Some(expected)) if actual == expected => "satisfied",
                (Some(_), Some(_)) => "unsatisfied",
                _ => "unknown",
            }
        }
        _ => {
            return Err(super::invalid(format!(
                "unknown verification condition: {condition}"
            )))
        }
    };
    Ok(ToolExecution::read(json!({
        "status": status,
        "satisfied": status == "satisfied",
        "condition": condition,
        "matched": matching.len()
    })))
}

fn observation_page(observation: &ObservationState, cursor: usize) -> Result<Value, ToolError> {
    let mut candidates: Vec<&Candidate> = observation.candidates.values().collect();
    candidates.sort_by_key(|candidate| candidate.id.clone());
    let total = candidates.len();
    let start = cursor.min(total);
    let end = (start + PAGE_SIZE).min(total);
    let page: Vec<Value> = candidates[start..end]
        .iter()
        .map(|candidate| {
            json!({
                "candidate_id": candidate.id,
                "action": candidate.action.as_str(),
                "risk": candidate.risk,
                "target": candidate.opaque_id,
                "role": candidate.role,
                "name": candidate.name,
                "description": candidate.description
            })
        })
        .collect();
    Ok(json!({
        "observation_token": observation.token,
        "target_token": observation.target_token,
        "window": { "hwnd": observation.hwnd, "title": observation.title },
        "nodes": observation.nodes,
        "candidates": page,
        "candidate_count": total,
        "next_cursor": (end < total).then_some(end),
        "truncated": total > end
    }))
}

fn candidate_for_node(node: &Value, target_token: Option<&str>, title: &str) -> Option<Candidate> {
    let opaque_id = node.get("opaque_id")?.as_str()?.to_string();
    let role = node.get("role")?.as_str()?.to_string();
    let name = node.get("name").and_then(Value::as_str).map(str::to_string);
    let automation_id = node
        .get("automation_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let supported = node.get("supported_actions")?.as_array()?;
    let action = supported
        .iter()
        .filter_map(Value::as_str)
        .find_map(|action| parse_action(action).ok())?;
    let risk = match action {
        SemanticActionKind::Invoke | SemanticActionKind::Select => RiskClass::ExternalCommit,
        _ => RiskClass::BoundedWrite,
    };
    Some(Candidate {
        id: format!("cand-{}", Uuid::new_v4().simple()),
        opaque_id,
        role: role.clone(),
        name: name.clone(),
        automation_id: automation_id.clone(),
        action,
        risk,
        description: format!(
            "{} {}",
            action.as_str(),
            name.as_deref()
                .or(automation_id.as_deref())
                .unwrap_or("element")
        ),
        locator: json!({
            "app_id": target_token.unwrap_or("foreground"),
            "window_id": node.get("window_id").cloned().unwrap_or(Value::Null),
            "window_title": title,
            "role": role,
            "automation_id": automation_id,
            "accessible_name": name,
            "supported_action": action.as_str()
        }),
    })
}

fn resolve_target(
    session: &Session,
    args: &Value,
) -> Result<(i64, String, Option<String>), ToolError> {
    if let Some(token) = args.get("target_token").and_then(Value::as_str) {
        let target = session.target(token)?;
        return Ok((target.hwnd, target.title, Some(token.to_string())));
    }
    let hwnd = platform::foreground_hwnd()?.ok_or_else(|| {
        ToolError::new(
            ErrorCode::TargetUnavailable,
            "no foreground window is available; bind a target first",
        )
        .retryable(true)
    })?;
    let info = platform::window_info(hwnd)?;
    Ok((
        hwnd,
        info["title"].as_str().unwrap_or_default().to_string(),
        None,
    ))
}

fn resolve_locator_window(session: &Session, locator: &Value) -> Result<i64, ToolError> {
    if let Some(window_id) = locator.get("window_id").and_then(Value::as_str) {
        if let Some(raw) = window_id.strip_prefix("hwnd:") {
            let hwnd = raw
                .parse::<i64>()
                .map_err(|_| super::invalid("locator window_id is malformed"))?;
            platform::window_info(hwnd)?;
            return Ok(hwnd);
        }
    }
    if let Some(token) = locator.get("app_id").and_then(Value::as_str) {
        if let Ok(target) = session.target(token) {
            return Ok(target.hwnd);
        }
    }
    if let Some(title) = locator.get("window_title").and_then(Value::as_str) {
        if let Some(hwnd) = platform::find_window_by_title(title)? {
            return Ok(hwnd);
        }
    }
    platform::foreground_hwnd()?.ok_or_else(|| {
        ToolError::new(
            ErrorCode::TargetUnavailable,
            "locator did not resolve to a window and no foreground window is available",
        )
        .retryable(true)
    })
}

fn locator_matches(node: &Value, locator: &Value) -> bool {
    if let Some(role) = locator.get("role").and_then(Value::as_str) {
        if node["role"].as_str() != Some(role) {
            return false;
        }
    }
    if let Some(automation_id) = locator.get("automation_id").and_then(Value::as_str) {
        if node["automation_id"].as_str() != Some(automation_id) {
            return false;
        }
    }
    if let Some(name) = locator.get("accessible_name").and_then(Value::as_str) {
        if node["name"].as_str() != Some(name) {
            return false;
        }
    }
    true
}

fn parse_action(value: &str) -> Result<SemanticActionKind, ToolError> {
    match value {
        "invoke" => Ok(SemanticActionKind::Invoke),
        "toggle" => Ok(SemanticActionKind::Toggle),
        "select" => Ok(SemanticActionKind::Select),
        "expand" => Ok(SemanticActionKind::Expand),
        "collapse" => Ok(SemanticActionKind::Collapse),
        "focus" => Ok(SemanticActionKind::Focus),
        "set_value" => Ok(SemanticActionKind::SetValue),
        _ => Err(super::invalid(format!(
            "unsupported semantic action: {value}"
        ))),
    }
}

fn uia_action(action: SemanticActionKind) -> UiaAction {
    match action {
        SemanticActionKind::Invoke => UiaAction::Invoke,
        SemanticActionKind::Toggle => UiaAction::Toggle,
        SemanticActionKind::Select => UiaAction::Select,
        SemanticActionKind::Expand => UiaAction::Expand,
        SemanticActionKind::Collapse => UiaAction::Collapse,
        SemanticActionKind::Focus => UiaAction::Focus,
        SemanticActionKind::SetValue => UiaAction::SetValue,
    }
}

fn ensure_foreground(hwnd: i64) -> Result<(), ToolError> {
    platform::window_info(hwnd)?;
    if platform::foreground_hwnd()? == Some(hwnd) || platform::activate_window(hwnd)? {
        Ok(())
    } else {
        Err(ToolError::new(
            ErrorCode::TargetUnavailable,
            "bound target could not be activated; semantic action was not sent",
        )
        .retryable(true))
    }
}
