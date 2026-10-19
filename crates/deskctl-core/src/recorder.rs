//! D3 录制器（record-replay.md §4.2/§4.7）：把会话内已执行的工具调用转换为
//! `deskctl-script/1` 步骤。
//!
//! 转换规则（设计 §4.2）：
//! - 写操作（risk != read_only）默认记录；读操作按 `include_reads` 过滤；
//! - `desktop_semantic_execute`（candidate_id）→ `desktop_semantic_action`（locator）；
//! - 会话内已绑定的 `hwnd` → `$target.<名>.hwnd`；`app_id` → `$target.<名>.token`；
//! - 失败且 `dispatch_state=not_sent` 的步骤标记 `optional:true`；
//! - 坐标类步骤（mouse/裁剪）标记 `fragile:true`；`capture_id` 引用标记 `volatile:true`。

use std::collections::HashMap;
use std::time::Instant;

use deskctl_protocol::{DispatchState, ErrorCode, RiskClass, ToolError, ToolName, ToolResult};
use serde_json::{json, Map, Value};

use crate::script::ScriptStep;
use crate::session::{ObservationState, Session, TargetBinding};

pub const DEFAULT_MAX_STEPS: usize = 200;
pub const MAX_MAX_STEPS: usize = 1000;
/// 录制内容只存会话内存（设计 §3.3：≤256 KiB）。
pub const MAX_BYTES: usize = 256 * 1024;

/// 会话内录制器状态。同一会话同时只允许一个录制器（§4.7）。
#[derive(Debug)]
pub struct RecorderState {
    pub include_reads: String,
    pub max_steps: usize,
    pub allow_relaunch: bool,
    pub steps: Vec<ScriptStep>,
    /// 名称 → target 规格（脚本 `targets` 段）。
    pub targets: Map<String, Value>,
    pub target_names: HashMap<i64, String>,
    pub token_names: HashMap<String, String>,
    pub token_bindings: HashMap<String, TargetBinding>,
    name_counts: HashMap<String, usize>,
    pub warnings: Vec<String>,
    pub truncated: bool,
    /// 已录制步骤的近似 JSON 字节数（`MAX_BYTES` 上限）。
    pub bytes: usize,
    pub total_calls: u64,
    pub started: Instant,
}

impl RecorderState {
    fn new(include_reads: String, max_steps: usize, allow_relaunch: bool) -> Self {
        Self {
            include_reads,
            max_steps,
            allow_relaunch,
            steps: Vec::new(),
            targets: Map::new(),
            target_names: HashMap::new(),
            token_names: HashMap::new(),
            token_bindings: HashMap::new(),
            name_counts: HashMap::new(),
            warnings: Vec::new(),
            truncated: false,
            bytes: 0,
            total_calls: 0,
            started: Instant::now(),
        }
    }

    pub fn status(&self) -> Value {
        json!({
            "active": true,
            "include_reads": self.include_reads,
            "max_steps": self.max_steps,
            "recorded_steps": self.steps.len(),
            "truncated": self.truncated,
            "targets": self.targets.keys().collect::<Vec<_>>(),
            "warnings": self.warnings,
            "elapsed_ms": self.started.elapsed().as_millis() as u64
        })
    }

    /// 产出 `deskctl-script/1` 脚本（步骤 + 捕获到的 targets）。
    pub fn script(&self) -> Value {
        let mut doc = Map::new();
        doc.insert("v".to_string(), json!(1));
        doc.insert("format".to_string(), json!("deskctl-script/1"));
        doc.insert(
            "name".to_string(),
            json!(format!(
                "recorded-{}",
                chrono::Local::now().format("%Y%m%d-%H%M%S")
            )),
        );
        doc.insert("created".to_string(), json!(crate::audit::now_ts()));
        if !self.targets.is_empty() {
            doc.insert("targets".to_string(), Value::Object(self.targets.clone()));
        }
        doc.insert("params".to_string(), json!({}));
        if !self.warnings.is_empty() {
            doc.insert("notes".to_string(), json!(self.warnings));
        }
        doc.insert(
            "steps".to_string(),
            Value::Array(
                self.steps
                    .iter()
                    .map(ScriptStep::to_value)
                    .collect::<Vec<_>>(),
            ),
        );
        Value::Object(doc)
    }
}

/// `desktop_recorder_start` 实现。
pub fn start(session: &Session, args: &Value) -> Result<Value, ToolError> {
    let include_reads = args
        .get("include_reads")
        .and_then(Value::as_str)
        .unwrap_or("essential")
        .to_string();
    if !matches!(include_reads.as_str(), "essential" | "all" | "none") {
        return Err(invalid("include_reads must be one of essential/all/none"));
    }
    let max_steps = match args.get("max_steps") {
        None | Some(Value::Null) => DEFAULT_MAX_STEPS,
        Some(value) => {
            let value = value
                .as_u64()
                .ok_or_else(|| invalid("max_steps must be an integer between 1 and 1000"))?;
            if value == 0 || value > MAX_MAX_STEPS as u64 {
                return Err(invalid("max_steps must be an integer between 1 and 1000"));
            }
            value as usize
        }
    };
    let allow_relaunch = args
        .get("allow_relaunch")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut recorder = session.recorder_mut();
    if let Some(current) = recorder.as_ref() {
        return Err(ToolError::new(
            ErrorCode::ResourceBusy,
            "a recorder is already active in this session; stop it first",
        )
        .details(current.status())
        .retryable(true));
    }
    *recorder = Some(RecorderState::new(
        include_reads.clone(),
        max_steps,
        allow_relaunch,
    ));
    Ok(json!({
        "recording": true,
        "include_reads": include_reads,
        "max_steps": max_steps,
        "allow_relaunch": allow_relaunch
    }))
}

/// `desktop_recorder_stop` 实现；可选把脚本写入 `path`（仍走允许根策略）。
pub fn stop(
    engine: &crate::engine::Engine,
    session: &Session,
    args: &Value,
) -> Result<Value, ToolError> {
    if let Some(format) = args.get("format").and_then(Value::as_str) {
        if format != "script" {
            return Err(invalid("format must be \"script\""));
        }
    }
    let state = session
        .recorder_mut()
        .take()
        .ok_or_else(|| invalid("no recorder is active in this session"))?;
    let mut script = state.script();
    // `recorder_stop {allow_relaunch:false}` 可覆盖录制期默认值（设计 §4.4.6）。
    if let Some(flag) = args.get("allow_relaunch").and_then(Value::as_bool) {
        if let Some(targets) = script.get_mut("targets").and_then(Value::as_object_mut) {
            for spec in targets.values_mut() {
                if spec.get("launch_ref").is_some() {
                    spec["allow_relaunch"] = json!(flag);
                }
            }
        }
    }
    let mut path_value = Value::Null;
    let mut bytes = Value::Null;
    if let Some(raw) = args.get("path").and_then(Value::as_str) {
        let path = crate::tools::normalize_allowed_path(engine, raw)?;
        let serialized = serde_json::to_vec_pretty(&script)
            .map_err(|error| ToolError::new(ErrorCode::Internal, error.to_string()))?;
        std::fs::write(&path, &serialized).map_err(|error| {
            ToolError::new(
                ErrorCode::PermissionDenied,
                format!("cannot write script to {}: {error}", path.display()),
            )
        })?;
        path_value = json!(path.display().to_string());
        bytes = json!(serialized.len());
    }
    Ok(json!({
        "recording": false,
        "steps": state.steps.len(),
        "truncated": state.truncated,
        "warnings": state.warnings,
        "stats": {
            "steps": state.steps.len(),
            "truncated": state.truncated,
            "bytes": state.bytes,
            "total_calls": state.total_calls,
            "elapsed_ms": state.started.elapsed().as_millis() as u64
        },
        "path": path_value,
        "bytes": bytes,
        "script": script
    }))
}

/// 引擎回调：一次已完成的工具调用。
pub fn note_call(
    session: &Session,
    tool: ToolName,
    args: &Value,
    result: &ToolResult,
    request_id: Option<&str>,
) {
    // 回放步骤自身（request_id = <run_id>:<step>）不再二次录制。
    if request_id.is_some_and(|id| crate::audit::run_id_from_request_id(id).is_some()) {
        return;
    }
    let mut guard = session.recorder_mut();
    let Some(recorder) = guard.as_mut() else {
        return;
    };
    recorder.total_calls += 1;
    if !should_record(tool, &recorder.include_reads) {
        return;
    }
    if recorder.steps.len() >= recorder.max_steps {
        if !recorder.truncated {
            recorder.truncated = true;
            recorder.warnings.push(format!(
                "step limit {} reached; later calls were not recorded",
                recorder.max_steps
            ));
        }
        return;
    }
    if recorder.bytes >= MAX_BYTES {
        if !recorder.truncated {
            recorder.truncated = true;
            recorder.warnings.push(format!(
                "recorded script reached the {MAX_BYTES} byte memory limit; later calls were not recorded"
            ));
        }
        return;
    }
    match convert(session, recorder, tool, args, result) {
        Ok(step) => {
            let size = serde_json::to_vec(&step.to_value())
                .map(|bytes| bytes.len())
                .unwrap_or(0);
            if recorder.bytes + size > MAX_BYTES {
                recorder.truncated = true;
                recorder.warnings.push(format!(
                    "recorded script reached the {MAX_BYTES} byte memory limit; later calls were not recorded"
                ));
                return;
            }
            recorder.bytes += size;
            recorder.steps.push(step);
        }
        Err(warning) => recorder.warnings.push(warning),
    }
}

fn should_record(tool: ToolName, include_reads: &str) -> bool {
    match tool {
        ToolName::RecorderStart | ToolName::RecorderStop | ToolName::ScriptRun => false,
        _ => {
            if tool.meta().risk == RiskClass::ReadOnly {
                match include_reads {
                    "none" => false,
                    "all" => true,
                    _ => matches!(
                        tool,
                        ToolName::WaitFor | ToolName::VerifyState | ToolName::WindowScreenshot
                    ),
                }
            } else {
                true
            }
        }
    }
}

fn convert(
    session: &Session,
    recorder: &mut RecorderState,
    tool: ToolName,
    args: &Value,
    result: &ToolResult,
) -> Result<ScriptStep, String> {
    let id = format!("s{}", recorder.steps.len() + 1);
    let mut params = args.as_object().cloned().unwrap_or_default();
    let mut fragile = false;
    let mut volatile = false;
    let mut notes: Vec<String> = Vec::new();

    let mut tool_name = tool.as_str().to_string();

    // target_bind 的结果是后续语义步骤改写 app_id 的依据：登记 token → 绑定。
    if tool == ToolName::TargetBind && result.ok {
        if let Some(data) = result.data.as_ref() {
            if let (Some(token), Some(hwnd)) = (
                data.get("target_token")
                    .or_else(|| data.get("token"))
                    .and_then(Value::as_str),
                data.get("hwnd").and_then(Value::as_i64),
            ) {
                if !recorder.token_names.contains_key(token) {
                    if let Some(binding) = session
                        .targets_snapshot()
                        .into_iter()
                        .find(|target| target.token == token)
                    {
                        let _ = register_binding(recorder, &binding, hwnd);
                    }
                }
            }
        }
    }

    // 语义执行：candidate_id → locator（回放用 desktop_semantic_action）。
    if tool == ToolName::SemanticExecute {
        let observation_token = args.get("observation_token").and_then(Value::as_str);
        let candidate_id = args.get("candidate_id").and_then(Value::as_str);
        match (observation_token, candidate_id) {
            (Some(token), Some(candidate_id)) => match session.observation_snapshot() {
                Some(observation) if observation.token == token => match observation
                    .candidates
                    .get(candidate_id)
                {
                    Some(candidate) => {
                        tool_name = ToolName::SemanticAction.as_str().to_string();
                        params = semantic_params(recorder, &observation, candidate, &mut volatile);
                    }
                    None => {
                        volatile = true;
                        notes.push("candidate_id was no longer in the observation cache".into());
                    }
                },
                _ => {
                    volatile = true;
                    notes.push("semantic step recorded without a matching observation".into());
                }
            },
            _ => {
                volatile = true;
                notes.push("semantic step has no re-playable locator".into());
            }
        }
    }

    // 已绑定 hwnd → $target.<名>.hwnd；合成 app_ref → $target.<名>.app_ref。
    if let Some(hwnd) = params.get("hwnd").and_then(Value::as_i64) {
        match target_ref(recorder, session, hwnd) {
            Some(name) => {
                params.insert("hwnd".to_string(), json!(format!("$target.{name}.hwnd")));
            }
            None => {
                fragile = true;
                notes.push(format!("hwnd {hwnd} was not bound to a target"));
            }
        }
    }
    if let Some(app_ref) = params.get("app_ref").and_then(Value::as_str) {
        if let Some(name) = name_for_app_ref(recorder, app_ref) {
            params.insert(
                "app_ref".to_string(),
                json!(format!("$target.{name}.app_ref")),
            );
        }
    }
    // 直接调用的 semantic_action / semantic_observe：locator 里的会话级引用同样必须改写，
    // 否则回放会带着录制时的 app_id token / hwnd 而失败（§4.2 可回放性）。
    if let Some(locator) = params.get_mut("locator").and_then(Value::as_object_mut) {
        locator.remove("window_id");
        let app_id = locator
            .get("app_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(app_id) = app_id {
            if let Some(name) = name_for_token(recorder, &app_id) {
                locator.insert("app_id".to_string(), json!(format!("$target.{name}.token")));
            }
        }
    }
    if let Some(token) = params
        .get("target_token")
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        if let Some(name) = name_for_token(recorder, &token) {
            params.insert(
                "target_token".to_string(),
                json!(format!("$target.{name}.token")),
            );
        }
    }

    // 坐标类 / capture 引用：脆弱与易变标记。
    if matches!(tool, ToolName::Mouse | ToolName::MouseDrag)
        && (params.contains_key("x") || params.contains_key("y") || params.contains_key("start_x"))
    {
        fragile = true;
    }
    if matches!(
        tool,
        ToolName::ImageCrop | ToolName::FindImage | ToolName::FindColor | ToolName::FindMultiColor
    ) && params.get("region").is_some_and(|value| value.is_object())
    {
        fragile = true;
    }
    if params.contains_key("capture_id") {
        volatile = true;
    }

    // expect：结果断言交给调用方消费；录制器只保留调用参数与内联期望。
    let verify = args
        .get("expectation")
        .map(|expectation| json!({ "tier": "screen", "expectation": expectation }));

    let failed_not_sent = !result.ok && result.meta.dispatch_state == DispatchState::NotSent;
    if failed_not_sent {
        notes.push("call failed before dispatch; replay keeps it optional".into());
    }

    Ok(ScriptStep {
        id,
        tool: tool_name,
        params: Value::Object(params),
        expect: None,
        set: None,
        verify,
        require_readback: None,
        optional: failed_not_sent.then_some(true),
        fragile: fragile.then_some(true),
        volatile: volatile.then_some(true),
        retry: None,
        note: (!notes.is_empty()).then(|| notes.join("; ")),
    })
}

/// 语义步骤参数：locator + action + 值类参数 + 目标引用。
fn semantic_params(
    recorder: &mut RecorderState,
    observation: &ObservationState,
    candidate: &crate::session::Candidate,
    volatile: &mut bool,
) -> Map<String, Value> {
    let mut locator = candidate.locator.as_object().cloned().unwrap_or_default();
    // window_id 是录制时的 hwnd，回放时必须重新解析：清空后用 app_id 引用。
    locator.remove("window_id");
    let target_name = observation
        .target_token
        .as_deref()
        .and_then(|token| name_for_token(recorder, token));
    if let Some(name) = &target_name {
        locator.insert("app_id".to_string(), json!(format!("$target.{name}.token")));
    } else if observation.hwnd.is_some() {
        locator.insert("app_id".to_string(), json!("foreground"));
    }
    let mut params = Map::new();
    params.insert("locator".to_string(), Value::Object(locator));
    params.insert("action".to_string(), json!(candidate.action.as_str()));
    if target_name.is_none() {
        *volatile = true;
    }
    params
}

/// hwnd → `$target.<名>`；首次出现时登记 target 规格。
fn target_ref(recorder: &mut RecorderState, session: &Session, hwnd: i64) -> Option<String> {
    if let Some(name) = recorder.target_names.get(&hwnd) {
        return Some(name.clone());
    }
    let binding = session
        .targets_snapshot()
        .into_iter()
        .filter(|target| target.hwnd == hwnd)
        // 同一 hwnd 可能被绑定过多次（探测/重录）：取最近一次，避免沿用旧 token。
        .max_by_key(|target| target.created_at)?;
    let name = register_binding(recorder, &binding, hwnd)?;
    Some(name)
}

fn name_for_token(recorder: &mut RecorderState, token: &str) -> Option<String> {
    if let Some(name) = recorder.token_names.get(token) {
        return Some(name.clone());
    }
    let binding = recorder.token_bindings.get(token).cloned()?;
    let name = register_binding(recorder, &binding, binding.hwnd)?;
    Some(name)
}

fn name_for_app_ref(recorder: &mut RecorderState, app_ref: &str) -> Option<String> {
    recorder
        .targets
        .iter()
        .find(|(_, spec)| spec.get("app_ref").and_then(Value::as_str) == Some(app_ref))
        .map(|(name, _)| name.clone())
}

/// 登记一个 target：生成名称、match/fingerprint 规格与引用。
fn register_binding(
    recorder: &mut RecorderState,
    binding: &TargetBinding,
    hwnd: i64,
) -> Option<String> {
    let info = crate::platform::window_info(hwnd).ok();
    // app_ref 形如 app:<pid>:<process>；窗口不可达时退化使用绑定信息。
    let mut ref_parts = binding.app_ref.splitn(3, ':');
    let _scheme = ref_parts.next();
    let ref_pid = ref_parts.next().and_then(|value| value.parse::<i64>().ok());
    let ref_process = ref_parts.next().map(str::to_string);
    let process_name = info
        .as_ref()
        .and_then(|value| value["process_name"].as_str().map(str::to_string))
        .or_else(|| ref_process.clone())
        .unwrap_or_else(|| "window".to_string());
    let stem = process_name
        .trim_end_matches(".exe")
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    let stem = if stem.is_empty() {
        "window".to_string()
    } else {
        stem
    };
    let count = recorder.name_counts.entry(stem.clone()).or_insert(0);
    *count += 1;
    let name = if *count == 1 {
        stem.clone()
    } else {
        format!("{stem}-{}", *count)
    };
    let title = info
        .as_ref()
        .and_then(|value| value["title"].as_str().map(str::to_string))
        .unwrap_or_else(|| binding.title.clone());
    let class_name = info
        .as_ref()
        .and_then(|value| value["class_name"].as_str().map(str::to_string))
        .unwrap_or_default();
    let pid = info
        .as_ref()
        .and_then(|value| value["process_id"].as_i64())
        .or(ref_pid)
        .unwrap_or_default();
    let mut match_spec = Map::new();
    match_spec.insert("process_name".to_string(), json!(process_name));
    match_spec.insert("class_name".to_string(), json!(class_name));
    match_spec.insert("title_contains".to_string(), json!(title));
    if !binding.app_ref.is_empty() && crate::apps::find(&binding.app_ref).is_some() {
        match_spec.insert("app_ref".to_string(), json!(binding.app_ref));
    }
    let mut spec = Map::new();
    spec.insert("match".to_string(), Value::Object(match_spec));
    spec.insert(
        "fingerprint".to_string(),
        json!({
            "pid": pid,
            "class_name": class_name,
            "title": title
        }),
    );
    if crate::apps::find(&binding.app_ref).is_some() {
        spec.insert("launch_ref".to_string(), json!(binding.app_ref));
        spec.insert("allow_relaunch".to_string(), json!(recorder.allow_relaunch));
    }
    recorder.targets.insert(name.clone(), Value::Object(spec));
    recorder.target_names.insert(hwnd, name.clone());
    recorder
        .token_names
        .insert(binding.token.clone(), name.clone());
    recorder
        .token_bindings
        .insert(binding.token.clone(), binding.clone());
    Some(name)
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::new(ErrorCode::InvalidParams, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deskctl_protocol::{ResultMeta, RiskClass, ToolProfile};

    fn session() -> Session {
        Session::new(
            ToolProfile::Full,
            "test",
            32,
            std::time::Duration::from_secs(120),
        )
    }

    fn result(ok: bool, dispatch_state: DispatchState) -> ToolResult {
        let meta = ResultMeta {
            tool: Some("test".into()),
            dispatch_state,
            risk: RiskClass::BoundedWrite,
            ..ResultMeta::default()
        };
        if ok {
            ToolResult::success(json!({}), meta)
        } else {
            ToolResult::failure(
                ToolError::new(ErrorCode::TargetUnavailable, "window closed"),
                meta,
            )
        }
    }

    fn result_with(data: Value) -> ToolResult {
        let meta = ResultMeta {
            tool: Some("test".into()),
            dispatch_state: DispatchState::Sent,
            risk: RiskClass::BoundedWrite,
            ..ResultMeta::default()
        };
        ToolResult::success(data, meta)
    }

    fn recording(include_reads: &str, max_steps: usize) -> (Session, RecorderState) {
        let session = session();
        let recorder = RecorderState::new(include_reads.to_string(), max_steps, true);
        (session, recorder)
    }

    #[test]
    fn include_reads_filters_read_only_tools() {
        assert!(should_record(ToolName::Mouse, "none"));
        assert!(should_record(ToolName::Mouse, "essential"));
        assert!(!should_record(ToolName::ScreenSize, "none"));
        assert!(!should_record(ToolName::ScreenSize, "essential"));
        assert!(should_record(ToolName::ScreenSize, "all"));
        assert!(should_record(ToolName::WindowScreenshot, "essential"));
        assert!(should_record(ToolName::WaitFor, "essential"));
        assert!(!should_record(ToolName::RecorderStart, "all"));
        assert!(!should_record(ToolName::RecorderStop, "all"));
        assert!(!should_record(ToolName::ScriptRun, "all"));
    }

    #[test]
    fn start_rejects_a_second_recorder() {
        let session = session();
        start(&session, &json!({})).expect("first start");
        let error = start(&session, &json!({})).expect_err("second start");
        assert_eq!(error.code, ErrorCode::ResourceBusy);
        assert!(error.details.get("active").and_then(Value::as_bool) == Some(true));
    }

    #[test]
    fn start_validates_include_reads() {
        let session = session();
        let error = start(&session, &json!({ "include_reads": "everything" }))
            .expect_err("invalid include_reads");
        assert_eq!(error.code, ErrorCode::InvalidParams);
    }

    #[test]
    fn stop_without_recorder_is_invalid() {
        let session = session();
        let engine = crate::engine::Engine::new(crate::config::Config::default());
        let error = stop(&engine, &session, &json!({})).expect_err("no recorder");
        assert_eq!(error.code, ErrorCode::InvalidParams);
    }

    #[test]
    fn records_coordinates_as_fragile_and_capture_as_volatile() {
        let (session, mut recorder) = recording("essential", 10);
        let step = convert(
            &session,
            &mut recorder,
            ToolName::Mouse,
            &json!({ "action": "click", "x": 10, "y": 20 }),
            &result(true, DispatchState::Sent),
        )
        .expect("convert");
        assert_eq!(step.fragile, Some(true));
        assert_eq!(step.volatile, None);

        let step = convert(
            &session,
            &mut recorder,
            ToolName::ImageCrop,
            &json!({ "capture_id": "cap-1", "region": { "x": 0, "y": 0, "width": 5, "height": 5 } }),
            &result(true, DispatchState::Sent),
        )
        .expect("convert");
        assert_eq!(step.volatile, Some(true));
        assert_eq!(step.fragile, Some(true));
    }

    #[test]
    fn failed_not_sent_calls_become_optional() {
        let (session, mut recorder) = recording("essential", 10);
        let step = convert(
            &session,
            &mut recorder,
            ToolName::WindowActivate,
            &json!({ "hwnd": 4242 }),
            &result(false, DispatchState::NotSent),
        )
        .expect("convert");
        assert_eq!(step.optional, Some(true));
        assert_eq!(step.fragile, Some(true), "unbound hwnd is fragile");
        assert!(step
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("optional"));
    }

    #[test]
    fn semantic_action_locator_references_are_replayable() {
        let session = session();
        // 同一 hwnd 的旧绑定（探测遗留）：录制器必须优先使用本次 bind 返回的 token。
        session
            .store_target(TargetBinding {
                token: "target-old".into(),
                app_ref: "app:999000001:saplogon.exe".into(),
                window_ref: Some("win:999000001".into()),
                hwnd: 999_000_001,
                title: "SAP".into(),
                created_at: Instant::now() - std::time::Duration::from_secs(30),
            })
            .expect("store old target");
        session
            .store_target(TargetBinding {
                token: "target-abc".into(),
                app_ref: "app:999000001:saplogon.exe".into(),
                window_ref: Some("win:999000001".into()),
                hwnd: 999_000_001,
                title: "SAP".into(),
                created_at: Instant::now(),
            })
            .expect("store target");
        start(&session, &json!({})).expect("start");
        note_call(
            &session,
            ToolName::TargetBind,
            &json!({ "hwnd": 999_000_001 }),
            &result_with(json!({ "target_token": "target-abc", "hwnd": 999_000_001 })),
            None,
        );
        note_call(
            &session,
            ToolName::SemanticAction,
            &json!({
                "locator": {
                    "app_id": "target-abc",
                    "window_id": "hwnd:999000001",
                    "role": "text_field",
                    "ordinal_hint": 0
                },
                "action": "focus"
            }),
            &result(true, DispatchState::Sent),
            None,
        );
        let guard = session.recorder_mut();
        let recorder = guard.as_ref().expect("recorder");
        assert_eq!(recorder.steps.len(), 2);
        assert_eq!(recorder.steps[1].tool, "desktop_semantic_action");
        let locator = &recorder.steps[1].params["locator"];
        assert_eq!(locator["app_id"], json!("$target.saplogon.token"));
        assert!(
            locator.get("window_id").is_none(),
            "recorded hwnd must not leak into the replay locator"
        );
    }

    #[test]
    fn max_steps_truncates_with_a_warning() {
        let session = session();
        start(&session, &json!({ "max_steps": 1 })).expect("start");
        note_call(
            &session,
            ToolName::Mouse,
            &json!({ "action": "click", "x": 1, "y": 2 }),
            &result(true, DispatchState::Sent),
            None,
        );
        note_call(
            &session,
            ToolName::Mouse,
            &json!({ "action": "click", "x": 3, "y": 4 }),
            &result(true, DispatchState::Sent),
            None,
        );
        let state = session.recorder_mut();
        let state = state.as_ref().expect("recorder active");
        assert_eq!(state.steps.len(), 1);
        assert!(state.truncated);
        assert!(state
            .warnings
            .iter()
            .any(|warning| warning.contains("step limit")));
    }

    #[test]
    fn replay_steps_are_not_recorded_again() {
        let session = session();
        start(&session, &json!({})).expect("start");
        note_call(
            &session,
            ToolName::Mouse,
            &json!({ "action": "click", "x": 1, "y": 2 }),
            &result(true, DispatchState::Sent),
            Some("run-abc:s1"),
        );
        let state = session.recorder_mut();
        let state = state.as_ref().expect("recorder active");
        assert!(state.steps.is_empty());
        assert_eq!(state.total_calls, 0, "run steps are ignored entirely");
    }

    #[test]
    fn produced_script_has_expected_shape() {
        let session = session();
        start(&session, &json!({})).expect("start");
        note_call(
            &session,
            ToolName::WindowActivate,
            &json!({ "hwnd": 4242 }),
            &result(true, DispatchState::Sent),
            None,
        );
        let state = session.recorder_mut().take().expect("recorder");
        let script = state.script();
        assert_eq!(script["v"], 1);
        assert_eq!(script["format"], crate::script::SCRIPT_FORMAT);
        assert_eq!(script["steps"].as_array().map(Vec::len), Some(1));
        assert_eq!(script["steps"][0]["id"], "s1");
        assert_eq!(script["steps"][0]["tool"], "desktop_window_activate");
        assert_eq!(script["steps"][0]["fragile"], true);
    }
}
