use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use deskctl_protocol::{ErrorCode, RiskClass, ToolError, ToolName, ToolProfile, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::recorder::RecorderState;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// 防粘键跟踪的两种实体：鼠标按钮与键盘按键（按规范化 spec 存储）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressedKind {
    Button,
    Key,
}

#[derive(Debug, Default)]
struct PressedState {
    buttons: BTreeSet<String>,
    keys: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub struct ElementLocation {
    pub id: u32,
    pub kind: String,
    pub text: String,
    pub rect: ScreenRect,
    pub confidence: f32,
    pub screen_center: Option<(i32, i32)>,
}

#[derive(Debug, Clone)]
pub struct CaptureState {
    pub id: String,
    pub path: String,
    pub screen_rect: ScreenRect,
    pub image_width: u32,
    pub image_height: u32,
    pub created_at: Instant,
    pub elements: BTreeMap<u32, ElementLocation>,
    pub foreground_hwnd: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TargetBinding {
    pub token: String,
    pub app_ref: String,
    pub window_ref: Option<String>,
    pub hwnd: i64,
    pub title: String,
    pub created_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticActionKind {
    Invoke,
    Toggle,
    SetChecked,
    Select,
    Expand,
    Collapse,
    Focus,
    SetValue,
    Scroll,
    ScrollIntoView,
    SetRangeValue,
}

impl SemanticActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invoke => "invoke",
            Self::Toggle => "toggle",
            Self::SetChecked => "set_checked",
            Self::Select => "select",
            Self::Expand => "expand",
            Self::Collapse => "collapse",
            Self::Focus => "focus",
            Self::SetValue => "set_value",
            Self::Scroll => "scroll",
            Self::ScrollIntoView => "scroll_into_view",
            Self::SetRangeValue => "set_range_value",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: String,
    pub opaque_id: String,
    pub role: String,
    pub name: Option<String>,
    pub automation_id: Option<String>,
    pub action: SemanticActionKind,
    pub risk: RiskClass,
    pub description: String,
    pub locator: Value,
}

#[derive(Debug, Clone)]
pub struct ObservationState {
    pub token: String,
    pub target_token: Option<String>,
    pub hwnd: Option<i64>,
    pub title: String,
    pub created_at: Option<Instant>,
    pub candidates: HashMap<String, Candidate>,
    pub nodes: Vec<Value>,
    /// delivery preference recorded at observe time; executing tools default to
    /// this mode when the call does not pass its own `delivery_mode`.
    pub preferred_delivery: Option<String>,
    /// 目标窗口被模态弹窗禁用时自动跟随的来源窗口（原 hwnd）。
    pub redirected_from: Option<i64>,
}

pub struct Session {
    pub id: String,
    pub token: String,
    pub profile: ToolProfile,
    owner: Mutex<String>,
    pub created_at: Instant,
    captures: Mutex<HashMap<String, CaptureState>>,
    targets: Mutex<HashMap<String, TargetBinding>>,
    observation: Mutex<Option<ObservationState>>,
    capture_cache_limit: usize,
    observation_ttl: Duration,
    last_used: Mutex<Instant>,
    pressed: Mutex<PressedState>,
    recorder: Mutex<Option<RecorderState>>,
    /// D4 检查点（名字 → `deskctl-checkpoint/1` 文档，上限 8）。
    checkpoints: Mutex<HashMap<String, serde_json::Value>>,
    active_run: Mutex<Option<String>>,
    last_run_outcome: Mutex<Option<String>>,
    closed: AtomicBool,
}

impl Session {
    pub fn new(
        profile: ToolProfile,
        owner: impl Into<String>,
        capture_cache_limit: usize,
        observation_ttl: Duration,
    ) -> Self {
        Self {
            id: format!("sess-{}", uuid::Uuid::new_v4().simple()),
            token: format!("st-{}", uuid::Uuid::new_v4().simple()),
            profile,
            owner: Mutex::new(owner.into()),
            created_at: Instant::now(),
            captures: Mutex::new(HashMap::new()),
            targets: Mutex::new(HashMap::new()),
            observation: Mutex::new(None),
            capture_cache_limit: capture_cache_limit.max(1),
            observation_ttl,
            last_used: Mutex::new(Instant::now()),
            pressed: Mutex::new(PressedState::default()),
            recorder: Mutex::new(None),
            checkpoints: Mutex::new(HashMap::new()),
            active_run: Mutex::new(None),
            last_run_outcome: Mutex::new(None),
            closed: AtomicBool::new(false),
        }
    }

    pub fn allows(&self, tool: ToolName) -> bool {
        tool.profile_allows(self.profile)
    }

    /// Owner label used for audit attribution. This is a label, not an auth
    /// boundary; token checks stay untouched by it.
    pub fn owner(&self) -> String {
        self.owner
            .lock()
            .map(|value| value.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    /// Update the attribution label. Only the MCP server uses this: it learns
    /// the client identity from `initialize` after the session was created,
    /// and no tool call can run before `initialize` has been handled.
    pub fn set_owner(&self, owner: impl Into<String>) {
        if let Ok(mut value) = self.owner.lock() {
            *value = owner.into();
        }
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        // 防粘键兜底：会话关闭时释放仍未配对的鼠标按钮/键盘按键（尽力而为）。
        let (buttons, keys) = self.take_pressed();
        for button in buttons {
            let _ = crate::platform::mouse_button(&button, false);
        }
        for key in keys {
            if let Ok(spec) = crate::platform::parse_key_spec(&key) {
                let _ = crate::platform::send_key(&spec, true, false);
            }
        }
        if let Ok(mut captures) = self.captures.lock() {
            let temporary = std::env::temp_dir();
            for capture in captures.values() {
                let path = std::path::Path::new(&capture.path);
                if path.starts_with(&temporary) {
                    let _ = std::fs::remove_file(path);
                }
            }
            captures.clear();
        }
        if let Ok(mut targets) = self.targets.lock() {
            targets.clear();
        }
        if let Ok(mut observation) = self.observation.lock() {
            *observation = None;
        }
        if let Ok(mut recorder) = self.recorder.lock() {
            *recorder = None;
        }
        if let Ok(mut run) = self.active_run.lock() {
            *run = None;
        }
        if let Ok(mut outcome) = self.last_run_outcome.lock() {
            *outcome = None;
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub fn touch(&self) {
        if let Ok(mut last_used) = self.last_used.lock() {
            *last_used = Instant::now();
        }
    }

    pub fn last_used(&self) -> Instant {
        self.last_used
            .lock()
            .map(|instant| *instant)
            .unwrap_or_else(|_| Instant::now())
    }

    /// 记录一个"已按下"的按钮/按键；返回 `true` 表示此前已按下（冲突调用，
    /// 调用方应先释放再重新按下，避免状态错乱）。
    pub fn mark_pressed(&self, kind: PressedKind, id: &str) -> bool {
        let Ok(mut pressed) = self.pressed.lock() else {
            return false;
        };
        let set = match kind {
            PressedKind::Button => &mut pressed.buttons,
            PressedKind::Key => &mut pressed.keys,
        };
        !set.insert(id.to_string())
    }

    /// 清除"已按下"记录；返回 `true` 表示此前确实处于按下状态。
    pub fn clear_pressed(&self, kind: PressedKind, id: &str) -> bool {
        let Ok(mut pressed) = self.pressed.lock() else {
            return false;
        };
        let set = match kind {
            PressedKind::Button => &mut pressed.buttons,
            PressedKind::Key => &mut pressed.keys,
        };
        set.remove(id)
    }

    pub fn pressed_ids(&self, kind: PressedKind) -> Vec<String> {
        let Ok(pressed) = self.pressed.lock() else {
            return Vec::new();
        };
        let set = match kind {
            PressedKind::Button => &pressed.buttons,
            PressedKind::Key => &pressed.keys,
        };
        set.iter().cloned().collect()
    }

    fn take_pressed(&self) -> (Vec<String>, Vec<String>) {
        let Ok(mut pressed) = self.pressed.lock() else {
            return (Vec::new(), Vec::new());
        };
        let buttons = std::mem::take(&mut pressed.buttons).into_iter().collect();
        let keys = std::mem::take(&mut pressed.keys).into_iter().collect();
        (buttons, keys)
    }

    pub fn summary(&self) -> Value {
        let captures = self.captures.lock().map(|value| value.len()).unwrap_or(0);
        let targets = self.targets.lock().map(|value| value.len()).unwrap_or(0);
        let observation = self
            .observation
            .lock()
            .ok()
            .and_then(|value| value.as_ref().map(|observation| observation.token.clone()));
        let pressed_buttons = self.pressed_ids(PressedKind::Button).len();
        let pressed_keys = self.pressed_ids(PressedKind::Key).len();
        let recorder_active = self
            .recorder
            .lock()
            .map(|value| value.is_some())
            .unwrap_or(false);
        let active_run = self.active_run.lock().ok().and_then(|value| value.clone());
        json!({
            "session_id": self.id,
            "profile": self.profile,
            "owner": self.owner(),
            "closed": self.is_closed(),
            "captures": captures,
            "targets": targets,
            "observation_token": observation,
            "pressed_buttons": pressed_buttons,
            "pressed_keys": pressed_keys,
            "recorder_active": recorder_active,
            "checkpoints": self.checkpoints_count(),
            "active_run": active_run,
            "last_used_ms_ago": self.last_used().elapsed().as_millis() as u64
        })
    }

    pub fn store_capture(&self, capture: CaptureState) -> Result<(), ToolError> {
        let mut captures = self
            .captures
            .lock()
            .map_err(|_| internal("capture cache poisoned"))?;
        if captures.len() >= self.capture_cache_limit {
            if let Some(oldest) = captures
                .iter()
                .min_by_key(|(_, capture)| capture.created_at)
                .map(|(id, _)| id.clone())
            {
                if let Some(removed) = captures.remove(&oldest) {
                    let path = std::path::Path::new(&removed.path);
                    if path.starts_with(std::env::temp_dir()) {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }
        }
        captures.insert(capture.id.clone(), capture);
        Ok(())
    }

    pub fn capture(&self, id: &str) -> Result<CaptureState, ToolError> {
        if self.is_closed() {
            return Err(ToolError::new(
                ErrorCode::ServerRestarted,
                "session is closed",
            ));
        }
        self.captures
            .lock()
            .map_err(|_| internal("capture cache poisoned"))?
            .get(id)
            .cloned()
            .ok_or_else(|| {
                ToolError::new(
                    ErrorCode::StaleCapture,
                    format!("capture {id} is not available; take a fresh screenshot"),
                )
                .retryable(true)
            })
    }

    pub fn find_capture_by_path(&self, path: &str) -> Option<CaptureState> {
        self.captures
            .lock()
            .ok()?
            .values()
            .filter(|capture| capture.path == path)
            .max_by_key(|capture| capture.created_at)
            .cloned()
    }

    pub fn update_capture(
        &self,
        id: &str,
        update: impl FnOnce(&mut CaptureState),
    ) -> Result<(), ToolError> {
        let mut captures = self
            .captures
            .lock()
            .map_err(|_| internal("capture cache poisoned"))?;
        let capture = captures.get_mut(id).ok_or_else(|| {
            ToolError::new(ErrorCode::StaleCapture, "capture is no longer available")
                .retryable(true)
        })?;
        update(capture);
        Ok(())
    }

    pub fn store_target(&self, target: TargetBinding) -> Result<(), ToolError> {
        let mut targets = self
            .targets
            .lock()
            .map_err(|_| internal("target table poisoned"))?;
        if targets.len() >= 256 {
            targets.clear();
        }
        targets.insert(target.token.clone(), target);
        Ok(())
    }

    pub fn target(&self, token: &str) -> Result<TargetBinding, ToolError> {
        self.targets
            .lock()
            .map_err(|_| internal("target table poisoned"))?
            .get(token)
            .cloned()
            .ok_or_else(|| {
                ToolError::new(
                    ErrorCode::TargetUnavailable,
                    "target token is unknown or belongs to another session; bind again",
                )
                .retryable(true)
            })
    }

    pub fn store_observation(&self, observation: ObservationState) -> Result<(), ToolError> {
        *self
            .observation
            .lock()
            .map_err(|_| internal("observation table poisoned"))? = Some(observation);
        Ok(())
    }

    pub fn observation(&self, token: &str) -> Result<ObservationState, ToolError> {
        let mut guard = self
            .observation
            .lock()
            .map_err(|_| internal("observation table poisoned"))?;
        let observation = guard.as_ref().ok_or_else(|| {
            ToolError::new(
                ErrorCode::StaleObservation,
                "no semantic observation is available",
            )
            .retryable(true)
        })?;
        if observation.token != token {
            return Err(ToolError::new(
                ErrorCode::StaleObservation,
                "observation_token does not belong to the latest observation",
            )
            .retryable(true));
        }
        if observation
            .created_at
            .is_some_and(|created| created.elapsed() > self.observation_ttl)
        {
            *guard = None;
            return Err(ToolError::new(
                ErrorCode::StaleObservation,
                "semantic observation expired; observe again",
            )
            .retryable(true));
        }
        Ok(observation.clone())
    }

    pub fn clear_observation(&self) -> Result<(), ToolError> {
        *self
            .observation
            .lock()
            .map_err(|_| internal("observation table poisoned"))? = None;
        Ok(())
    }

    /// 录制器状态（D3）；跨模块借用，poisoned 时取回内部值继续。
    pub(crate) fn recorder_mut(&self) -> std::sync::MutexGuard<'_, Option<RecorderState>> {
        self.recorder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn targets_snapshot(&self) -> Vec<TargetBinding> {
        self.targets
            .lock()
            .map(|map| map.values().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) fn observation_snapshot(&self) -> Option<ObservationState> {
        self.observation.lock().ok().and_then(|guard| guard.clone())
    }

    /// 引擎在每个工具调用结束后回调；录制器激活时才做步骤转换。
    pub fn record_call(
        &self,
        tool: ToolName,
        args: &Value,
        result: &ToolResult,
        request_id: Option<&str>,
    ) {
        if self.recorder_mut().is_none() {
            return;
        }
        crate::recorder::note_call(self, tool, args, result, request_id);
    }

    /// 当前会话内正在执行的脚本 run id（D3 并发：每会话至多 1 个）。
    pub fn active_run(&self) -> Option<String> {
        self.active_run.lock().ok().and_then(|value| value.clone())
    }

    pub fn run_begin(&self, run_id: &str) -> Result<(), ToolError> {
        let mut guard = self
            .active_run
            .lock()
            .map_err(|_| internal("run state poisoned"))?;
        if let Some(current) = guard.as_ref() {
            if current != run_id {
                return Err(ToolError::new(
                    ErrorCode::ResourceBusy,
                    format!("session already runs script {current}; wait for it to finish"),
                )
                .details(json!({ "run_id": current }))
                .retryable(true));
            }
        }
        *guard = Some(run_id.to_string());
        Ok(())
    }

    pub fn run_end(&self) {
        if let Ok(mut guard) = self.active_run.lock() {
            *guard = None;
        }
    }

    /// 上一次 run 的 outcome（同会话续跑门禁：`RECONCILIATION_REQUIRED`
    /// 默认拒绝 `--from`，除非显式 `allow_resume_after_unknown`）。
    pub fn set_last_run_outcome(&self, outcome: &str) {
        if let Ok(mut guard) = self.last_run_outcome.lock() {
            *guard = Some(outcome.to_string());
        }
    }

    pub fn last_run_outcome(&self) -> Option<String> {
        self.last_run_outcome
            .lock()
            .ok()
            .and_then(|value| value.clone())
    }

    /// 保存（或覆盖）一个会话内检查点；新增且超过上限时拒绝（design §2）。
    pub fn store_checkpoint(&self, name: &str, document: Value) -> Result<(), ToolError> {
        let mut checkpoints = self
            .checkpoints
            .lock()
            .map_err(|_| internal("checkpoint store poisoned"))?;
        if !checkpoints.contains_key(name)
            && checkpoints.len() >= crate::checkpoint::MAX_CHECKPOINTS
        {
            return Err(ToolError::new(
                ErrorCode::ResourceBusy,
                format!(
                    "session checkpoint limit reached ({}); overwrite an existing checkpoint or close the session",
                    crate::checkpoint::MAX_CHECKPOINTS
                ),
            )
            .retryable(false));
        }
        checkpoints.insert(name.to_string(), document);
        Ok(())
    }

    /// 读取会话内检查点；未定义时返回 `invalid_params`。
    pub fn checkpoint(&self, name: &str) -> Result<Value, ToolError> {
        if self.is_closed() {
            return Err(ToolError::new(
                ErrorCode::ServerRestarted,
                "session is closed",
            ));
        }
        self.checkpoints
            .lock()
            .map_err(|_| internal("checkpoint store poisoned"))?
            .get(name)
            .cloned()
            .ok_or_else(|| {
                ToolError::new(
                    ErrorCode::InvalidParams,
                    format!("checkpoint '{name}' is not defined in this session; save it first"),
                )
            })
    }

    pub fn checkpoints_count(&self) -> usize {
        self.checkpoints
            .lock()
            .map(|value| value.len())
            .unwrap_or(0)
    }
}

fn internal(message: impl Into<String>) -> ToolError {
    ToolError::new(ErrorCode::Internal, message)
}

/// 保留 owner 命名：这些标签只由内部调用方（daemon / MCP server / 一次性 CLI /
/// bench）设置，外部显式传入时一律拒绝（见 docs/design/agent-identity.md §3）。
pub const RESERVED_OWNER_NAMES: &[&str] = &["daemon-client", "mcp", "oneshot", "bench"];

/// 校验并规范化调用方传入的 owner 标签：
///
/// - 允许字符 `[a-zA-Z0-9._:@-]`、长度 1..=64、统一小写；
/// - 非法字符与保留命名（`daemon-client`/`mcp`/`mcp:*`/`oneshot`/`bench`/`system*`）
///   返回 `invalid_params`，不做静默清洗；
/// - owner 是归因标签，不是鉴权凭据（安全模型仍是 token + 本机 ACL）。
pub fn validate_owner(raw: &str) -> Result<String, ToolError> {
    let normalized = raw.to_ascii_lowercase();
    let length = normalized.chars().count();
    if normalized.is_empty() || length > 64 {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            format!("owner must be 1..=64 characters, got {length}"),
        ));
    }
    if let Some(bad) = normalized
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '@' | '-')))
    {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            format!("owner contains invalid character '{bad}'; allowed: [a-zA-Z0-9._:@-]"),
        ));
    }
    if is_reserved_owner(&normalized) {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            format!("owner is reserved: '{normalized}' is an internal identity"),
        ));
    }
    Ok(normalized)
}

fn is_reserved_owner(normalized: &str) -> bool {
    RESERVED_OWNER_NAMES.contains(&normalized)
        || normalized.starts_with("mcp:")
        || normalized.starts_with("system")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_owner_normalizes_and_accepts() {
        assert_eq!(
            validate_owner("Agent:SAP-Inspector").unwrap(),
            "agent:sap-inspector"
        );
        assert_eq!(
            validate_owner("user:ops.1@corp-2").unwrap(),
            "user:ops.1@corp-2"
        );
        assert_eq!(validate_owner("a").unwrap(), "a");
        assert_eq!(validate_owner(&"x".repeat(64)).unwrap().len(), 64);
    }

    #[test]
    fn validate_owner_rejects_invalid_and_reserved() {
        for bad in [
            "",
            "agent:bad name",
            "agent:不良",
            "agent/x",
            "agent\nnewline",
        ] {
            let error = validate_owner(bad).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidParams, "input {bad:?}");
        }
        assert_eq!(
            validate_owner(&"x".repeat(65)).unwrap_err().code,
            ErrorCode::InvalidParams
        );
        for reserved in [
            "daemon-client",
            "MCP",
            "mcp:evil",
            "oneshot",
            "bench",
            "system-root",
            "SYSTEM",
        ] {
            let error = validate_owner(reserved).unwrap_err();
            assert_eq!(
                error.code,
                ErrorCode::InvalidParams,
                "reserved {reserved:?}"
            );
            assert!(
                error.message.contains("reserved"),
                "message must say reserved: {}",
                error.message
            );
        }
    }

    #[test]
    fn session_owner_can_be_reassigned_for_mcp_initialize() {
        let session = Session::new(
            ToolProfile::Standard,
            "mcp",
            32,
            std::time::Duration::from_secs(1),
        );
        assert_eq!(session.owner(), "mcp");
        session.set_owner("mcp:tester");
        assert_eq!(session.owner(), "mcp:tester");
        assert_eq!(session.summary()["owner"], "mcp:tester");
    }
}
