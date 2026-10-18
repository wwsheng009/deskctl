use std::collections::HashMap;
use std::time::{Duration, Instant};

use deskctl_protocol::{DispatchState, ErrorCode, RiskClass, ToolError, ToolName, Verification};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::engine::Engine;
use crate::platform::{self, ActionOptions, UiaAction};
use crate::session::{Candidate, ObservationState, SemanticActionKind, Session};

use super::ToolExecution;

const PAGE_SIZE: usize = 20;
const TREE_PAGE_SIZE: usize = 200;
const TREE_NODE_LIMIT: usize = 2000;

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
    let tree_cursor = args
        .get("tree_cursor")
        .and_then(Value::as_str)
        .map(parse_tree_cursor)
        .transpose()?;
    let existing_token = args
        .get("observation_token")
        .and_then(Value::as_str)
        .filter(|_| page_cursor > 0 || tree_cursor.is_some());
    if let Some(token) = existing_token {
        let observation = session.observation(token)?;
        return Ok(ToolExecution::read(observation_page(
            &observation,
            page_cursor,
            tree_cursor.unwrap_or(0),
        )?));
    }

    let preferred_delivery = args
        .get("delivery_mode")
        .and_then(Value::as_str)
        .map(parse_delivery_mode)
        .transpose()?
        .map(str::to_string);

    let (hwnd, title, target_token) = resolve_target(session, args)?;
    let native_nodes = platform::observe_window(hwnd, TREE_NODE_LIMIT)?;
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
        preferred_delivery,
    };
    let page = observation_page(&observation, 0, 0)?;
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
    let mode = resolve_delivery_mode(args, observation.preferred_delivery.as_deref())?;
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
    if matches!(
        action,
        SemanticActionKind::SetValue | SemanticActionKind::SetRangeValue
    ) && value.is_none()
    {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            "value is required for value-writing candidates",
        ));
    }
    let options = action_options(args);
    perform_with_delivery(hwnd, node, uia_action(action), &options, mode)?;
    session.clear_observation()?;
    completion_response(
        session,
        args,
        json!({
            "receipt": {
                "candidate_id": candidate.id,
                "action": action.as_str(),
                "hwnd": hwnd,
                "delivery_mode": mode,
                "dispatch_state": "sent"
            },
            "requires_observation": true
        }),
    )
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
    let mode = resolve_delivery_mode(args, None)?;
    let hwnd = match args.get("launch_ref").and_then(Value::as_str) {
        Some(launch_ref) => match resolve_locator_window_strict(session, locator)? {
            Some(hwnd) => hwnd,
            None => relaunch_and_retry_locator(session, locator, launch_ref)?,
        },
        None => resolve_locator_window(session, locator)?,
    };
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
    if matches!(
        action,
        SemanticActionKind::SetValue | SemanticActionKind::SetRangeValue
    ) && value.is_none()
    {
        return Err(super::invalid(
            "value is required for value-writing actions",
        ));
    }
    let options = action_options(args);
    perform_with_delivery(hwnd, node, uia_action(action), &options, mode)?;
    session.clear_observation()?;
    completion_response(
        session,
        args,
        json!({
            "receipt": {
                "action": action.as_str(),
                "hwnd": hwnd,
                "delivery_mode": mode,
                "dispatch_state": "sent"
            }
        }),
    )
}

fn relaunch_and_retry_locator(
    session: &Session,
    locator: &Value,
    launch_ref: &str,
) -> Result<i64, ToolError> {
    let entry = crate::apps::find(launch_ref).ok_or_else(|| {
        super::invalid("launch_ref must be an app_ref returned by desktop_targets_list")
    })?;
    let pid = crate::apps::launch(&entry)?;
    let hwnd = crate::apps::wait_for_window(pid, &entry.name, Duration::from_millis(10_000))?
        .ok_or_else(|| {
            ToolError::new(
                ErrorCode::TargetUnavailable,
                format!(
                    "launched '{}' (pid {pid}) but no window appeared within 10s",
                    entry.name
                ),
            )
            .details(json!({ "pid": pid }))
            .retryable(false)
        })?;
    platform::window_info(hwnd)?;
    match resolve_locator_window_strict(session, locator)? {
        Some(resolved) => Ok(resolved),
        None => Ok(hwnd),
    }
}

fn perform_with_delivery(
    hwnd: i64,
    node: &platform::UiNode,
    action: UiaAction,
    options: &ActionOptions<'_>,
    mode: &str,
) -> Result<(), ToolError> {
    let mut background = options.clone();
    background.require_foreground = false;
    let mut foreground = options.clone();
    foreground.require_foreground = true;
    match mode {
        "background" => platform::perform_action(hwnd, node, action, &background),
        "auto" => match platform::perform_action(hwnd, node, action, &background) {
            Ok(()) => Ok(()),
            Err(error) => {
                // WebView2 提供程序可能把 S_OK 当错误上报（"操作成功完成 0x00000000"）；
                // 此时动作已经派发，绝不能回退重发。
                if format!("{error}").contains("0x00000000") {
                    return Err(error);
                }
                ensure_foreground(hwnd)?;
                platform::perform_action(hwnd, node, action, &foreground)
            }
        },
        _ => {
            ensure_foreground(hwnd)?;
            platform::perform_action(hwnd, node, action, &foreground)
        }
    }
}

fn action_options(args: &Value) -> ActionOptions<'_> {
    ActionOptions {
        value: args.get("value").and_then(Value::as_str),
        checked: args.get("checked").and_then(Value::as_bool),
        direction: args.get("direction").and_then(Value::as_str),
        amount: args.get("amount").and_then(Value::as_str),
        require_foreground: true,
    }
}

fn resolve_delivery_mode(args: &Value, preferred: Option<&str>) -> Result<&'static str, ToolError> {
    if let Some(value) = args.get("delivery_mode").and_then(Value::as_str) {
        return parse_delivery_mode(value);
    }
    if let Some(value) = preferred {
        return parse_delivery_mode(value);
    }
    Ok("foreground")
}

fn parse_delivery_mode(value: &str) -> Result<&'static str, ToolError> {
    match value {
        "foreground" => Ok("foreground"),
        "auto" => Ok("auto"),
        "background" => Ok("background"),
        _ => Err(super::invalid(
            "delivery_mode must be foreground, auto or background",
        )),
    }
}

fn parse_tree_cursor(value: &str) -> Result<usize, ToolError> {
    value
        .strip_prefix("t:")
        .unwrap_or(value)
        .parse::<usize>()
        .map_err(|_| {
            super::invalid(
                "tree_cursor is malformed; use next_tree_cursor from the same observation",
            )
        })
}

struct CompletionOutcome {
    policy: &'static str,
    verification: &'static str,
    satisfied: Option<bool>,
    matched: Option<usize>,
    samples: Option<u32>,
}

fn complete_dispatch(session: &Session, args: &Value) -> Result<CompletionOutcome, ToolError> {
    let policy = match args.get("completion_policy").and_then(Value::as_str) {
        None | Some("auto") => "auto",
        Some("verified") => "verified",
        Some("dispatched") => "dispatched",
        Some(_) => {
            return Err(super::invalid(
                "completion_policy must be auto, verified or dispatched",
            ))
        }
    };
    let expectation = args.get("expectation").filter(|value| value.is_object());
    if policy == "verified" && expectation.is_none() {
        return Err(super::invalid(
            "completion_policy=verified requires an expectation",
        ));
    }
    if policy == "dispatched" || expectation.is_none() {
        return Ok(CompletionOutcome {
            policy,
            verification: "unknown",
            satisfied: None,
            matched: None,
            samples: None,
        });
    }
    let outcome = verify_expectation(session, expectation.unwrap())?;
    Ok(CompletionOutcome {
        policy,
        verification: outcome.status,
        satisfied: Some(outcome.status == "satisfied"),
        matched: Some(outcome.matched),
        samples: Some(outcome.samples),
    })
}

fn completion_response(
    session: &Session,
    args: &Value,
    mut data: Value,
) -> Result<ToolExecution, ToolError> {
    let outcome = complete_dispatch(session, args)?;
    if let Some(object) = data.as_object_mut() {
        object.insert("verification".into(), json!(outcome.verification));
        object.insert("completion_policy".into(), json!(outcome.policy));
        if let Some(satisfied) = outcome.satisfied {
            object.insert("satisfied".into(), json!(satisfied));
        }
        if let Some(matched) = outcome.matched {
            object.insert("matched".into(), json!(matched));
        }
        if let Some(samples) = outcome.samples {
            object.insert("samples".into(), json!(samples));
        }
    }
    Ok(ToolExecution {
        data,
        dispatch_state: DispatchState::Sent,
        effect: Some(deskctl_protocol::Effect::ExternalCommit),
        verification: if outcome.satisfied == Some(true) {
            Verification::Satisfied
        } else {
            Verification::Unknown
        },
        requires_observation: true,
    })
}

fn verify_state(session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
    let expectation = args
        .get("expectation")
        .filter(|value| value.is_object())
        .ok_or_else(|| super::invalid("expectation is required"))?;
    let condition = expectation
        .get("condition")
        .and_then(Value::as_str)
        .ok_or_else(|| super::invalid("expectation.condition is required"))?;
    let outcome = verify_expectation(session, expectation)?;
    Ok(ToolExecution::read(json!({
        "status": outcome.status,
        "satisfied": outcome.status == "satisfied",
        "condition": condition,
        "matched": outcome.matched,
        "samples": outcome.samples
    })))
}

struct VerifyOutcome {
    status: &'static str,
    matched: usize,
    samples: u32,
}

/// 有界等待 + 稳定采样的后置条件验证：轮询至超时；satisfied 需要连续
/// `stable_samples` 次采样一致，抖动时返回 unknown，不冒充 satisfied。
fn verify_expectation(session: &Session, expectation: &Value) -> Result<VerifyOutcome, ToolError> {
    let locator = expectation
        .get("locator")
        .filter(|value| value.is_object())
        .ok_or_else(|| super::invalid("expectation.locator is required"))?;
    let condition = expectation
        .get("condition")
        .and_then(Value::as_str)
        .ok_or_else(|| super::invalid("expectation.condition is required"))?;
    let expected_value = expectation.get("value").and_then(Value::as_str);
    let expected = expectation
        .get("expected")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let timeout_ms = expectation
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(5_000)
        .min(300_000);
    let stable_target = expectation
        .get("stable_samples")
        .and_then(Value::as_u64)
        .unwrap_or(2)
        .clamp(1, 5) as u32;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);

    let mut status: &'static str;
    let mut matched: usize;
    let mut samples = 0u32;
    let mut streak = 0u32;
    loop {
        samples += 1;
        let sampled = sample_condition(session, locator, condition, expected_value, expected)?;
        status = sampled.0;
        matched = sampled.1;
        if status == "satisfied" {
            streak += 1;
            if streak >= stable_target {
                return Ok(VerifyOutcome {
                    status,
                    matched,
                    samples,
                });
            }
        } else {
            streak = 0;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    if status == "satisfied" {
        // 超时时满足但未达到稳定采样数：未确立稳定态，不冒充 satisfied。
        status = "unknown";
    }
    Ok(VerifyOutcome {
        status,
        matched,
        samples,
    })
}

fn sample_condition(
    session: &Session,
    locator: &Value,
    condition: &str,
    expected_value: Option<&str>,
    expected: bool,
) -> Result<(&'static str, usize), ToolError> {
    let hwnd = resolve_locator_window_strict(session, locator)?;
    let matching: Vec<Value> = match hwnd {
        Some(hwnd) => platform::observe_window(hwnd, 1000)?
            .into_iter()
            .map(|node| node.value)
            .filter(|node| locator_matches(node, locator))
            .collect(),
        None => Vec::new(),
    };
    let probe: Option<bool> = match condition {
        "exists" | "window_exists" => Some(!matching.is_empty()),
        "absent" | "window_absent" => Some(matching.is_empty()),
        "focused" => matching.first().and_then(|node| node["focused"].as_bool()),
        "checked" => matching.first().and_then(|node| node["toggled"].as_bool()),
        "selected" => matching.first().and_then(|node| node["selected"].as_bool()),
        "expanded" => matching.first().and_then(|node| node["expanded"].as_bool()),
        "value_equals" => match (
            matching
                .first()
                .and_then(|node| node["value_fingerprint"].as_str()),
            expected_value.map(fingerprint).as_deref(),
        ) {
            (Some(actual), Some(want)) => Some(actual == want),
            _ => None,
        },
        _ => {
            return Err(super::invalid(format!(
                "unknown verification condition: {condition}"
            )))
        }
    };
    // expected=false 提供反向语义：探针结果与期望一致才 satisfied。
    let status = match probe {
        Some(value) if value == expected => "satisfied",
        Some(_) => "unsatisfied",
        None => "unknown",
    };
    Ok((status, matching.len()))
}

/// 验证专用解析：不落到前台兜底，解析不到返回 None，
/// 使 absent 类条件在窗口已关闭时仍可正确判定。
fn resolve_locator_window_strict(
    session: &Session,
    locator: &Value,
) -> Result<Option<i64>, ToolError> {
    if let Some(window_id) = locator.get("window_id").and_then(Value::as_str) {
        if let Some(raw) = window_id.strip_prefix("hwnd:") {
            let hwnd = raw
                .parse::<i64>()
                .map_err(|_| super::invalid("locator window_id is malformed"))?;
            return Ok(platform::window_info(hwnd).is_ok().then_some(hwnd));
        }
    }
    if let Some(token) = locator.get("app_id").and_then(Value::as_str) {
        if let Ok(target) = session.target(token) {
            return Ok(platform::window_info(target.hwnd)
                .is_ok()
                .then_some(target.hwnd));
        }
    }
    if let Some(title) = locator.get("window_title").and_then(Value::as_str) {
        if let Some(hwnd) = platform::find_window_by_title(title)? {
            return Ok(Some(hwnd));
        }
    }
    Ok(None)
}

fn observation_page(
    observation: &ObservationState,
    cursor: usize,
    tree_start: usize,
) -> Result<Value, ToolError> {
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
    let tree_total = observation.nodes.len();
    let tree_start = tree_start.min(tree_total);
    let tree_end = (tree_start + TREE_PAGE_SIZE).min(tree_total);
    let nodes: Vec<Value> = observation.nodes[tree_start..tree_end].to_vec();
    Ok(json!({
        "observation_token": observation.token,
        "target_token": observation.target_token,
        "window": { "hwnd": observation.hwnd, "title": observation.title },
        "nodes": nodes,
        "tree_count": tree_total,
        "next_tree_cursor": (tree_end < tree_total).then(|| format!("t:{tree_end}")),
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
        "set_checked" => Ok(SemanticActionKind::SetChecked),
        "select" => Ok(SemanticActionKind::Select),
        "expand" => Ok(SemanticActionKind::Expand),
        "collapse" => Ok(SemanticActionKind::Collapse),
        "focus" => Ok(SemanticActionKind::Focus),
        "set_value" => Ok(SemanticActionKind::SetValue),
        "scroll" => Ok(SemanticActionKind::Scroll),
        "scroll_into_view" => Ok(SemanticActionKind::ScrollIntoView),
        "set_range_value" => Ok(SemanticActionKind::SetRangeValue),
        _ => Err(super::invalid(format!(
            "unsupported semantic action: {value}"
        ))),
    }
}

fn uia_action(action: SemanticActionKind) -> UiaAction {
    match action {
        SemanticActionKind::Invoke => UiaAction::Invoke,
        SemanticActionKind::Toggle => UiaAction::Toggle,
        SemanticActionKind::SetChecked => UiaAction::SetChecked,
        SemanticActionKind::Select => UiaAction::Select,
        SemanticActionKind::Expand => UiaAction::Expand,
        SemanticActionKind::Collapse => UiaAction::Collapse,
        SemanticActionKind::Focus => UiaAction::Focus,
        SemanticActionKind::SetValue => UiaAction::SetValue,
        SemanticActionKind::Scroll => UiaAction::Scroll,
        SemanticActionKind::ScrollIntoView => UiaAction::ScrollIntoView,
        SemanticActionKind::SetRangeValue => UiaAction::SetRangeValue,
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

fn fingerprint(value: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("value:{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_extended_semantic_actions() {
        assert_eq!(parse_action("set_checked").unwrap().as_str(), "set_checked");
        assert_eq!(parse_action("scroll").unwrap().as_str(), "scroll");
        assert_eq!(
            parse_action("scroll_into_view").unwrap().as_str(),
            "scroll_into_view"
        );
        assert_eq!(
            parse_action("set_range_value").unwrap().as_str(),
            "set_range_value"
        );
        assert_eq!(parse_action("toggle").unwrap().as_str(), "toggle");
        assert!(parse_action("teleport").is_err());
    }

    #[test]
    fn parses_tree_cursor_and_delivery_mode() {
        assert_eq!(parse_tree_cursor("t:200").unwrap(), 200);
        assert_eq!(parse_tree_cursor("40").unwrap(), 40);
        assert!(parse_tree_cursor("nope").is_err());
        assert_eq!(parse_delivery_mode("auto").unwrap(), "auto");
        assert_eq!(parse_delivery_mode("background").unwrap(), "background");
        assert!(parse_delivery_mode("turbo").is_err());
    }
}
