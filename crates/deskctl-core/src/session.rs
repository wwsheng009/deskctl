use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use deskctl_protocol::{ErrorCode, RiskClass, ToolError, ToolName, ToolProfile};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
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
}

pub struct Session {
    pub id: String,
    pub token: String,
    pub profile: ToolProfile,
    pub owner: String,
    pub created_at: Instant,
    captures: Mutex<HashMap<String, CaptureState>>,
    targets: Mutex<HashMap<String, TargetBinding>>,
    observation: Mutex<Option<ObservationState>>,
    capture_cache_limit: usize,
    observation_ttl: Duration,
    last_used: Mutex<Instant>,
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
            owner: owner.into(),
            created_at: Instant::now(),
            captures: Mutex::new(HashMap::new()),
            targets: Mutex::new(HashMap::new()),
            observation: Mutex::new(None),
            capture_cache_limit: capture_cache_limit.max(1),
            observation_ttl,
            last_used: Mutex::new(Instant::now()),
            closed: AtomicBool::new(false),
        }
    }

    pub fn allows(&self, tool: ToolName) -> bool {
        tool.profile_allows(self.profile)
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
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

    pub fn summary(&self) -> Value {
        let captures = self.captures.lock().map(|value| value.len()).unwrap_or(0);
        let targets = self.targets.lock().map(|value| value.len()).unwrap_or(0);
        let observation = self
            .observation
            .lock()
            .ok()
            .and_then(|value| value.as_ref().map(|observation| observation.token.clone()));
        json!({
            "session_id": self.id,
            "profile": self.profile,
            "owner": self.owner,
            "closed": self.is_closed(),
            "captures": captures,
            "targets": targets,
            "observation_token": observation,
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
}

fn internal(message: impl Into<String>) -> ToolError {
    ToolError::new(ErrorCode::Internal, message)
}
