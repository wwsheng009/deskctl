use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use deskctl_protocol::{
    DispatchState, Effect, ErrorCode, ResultMeta, RiskClass, ToolError, ToolName, ToolProfile,
    ToolResult, Verification,
};
use serde::Serialize;

use crate::config::Config;
use crate::models::ModelManager;
use crate::resource_gate::{ResourceGate, ResourceKind};
use crate::session::Session;
use crate::tools::{self, ToolExecution};
use crate::vision::VisionEngine;

pub type CancelFlag = Arc<AtomicBool>;

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub session_token: String,
    pub profile: ToolProfile,
    pub owner: String,
}

pub struct Engine {
    config: Config,
    sessions: RwLock<HashMap<String, Arc<Session>>>,
    resource_gate: ResourceGate,
    models: ModelManager,
    vision: VisionEngine,
    shutting_down: AtomicBool,
    started_at: Mutex<Instant>,
}

impl Engine {
    pub fn new(config: Config) -> Arc<Self> {
        config.ensure_directories();
        let models = ModelManager::new(config.models_dir.clone());
        let vision = VisionEngine::new(models.clone());
        Arc::new(Self {
            resource_gate: ResourceGate::new(config.resource_wait),
            config,
            sessions: RwLock::new(HashMap::new()),
            models,
            vision,
            shutting_down: AtomicBool::new(false),
            started_at: Mutex::new(Instant::now()),
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn models(&self) -> &ModelManager {
        &self.models
    }

    pub fn vision(&self) -> &VisionEngine {
        &self.vision
    }

    pub fn shutdown_requested(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    pub fn request_shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        if let Ok(mut sessions) = self.sessions.write() {
            for session in sessions.values() {
                session.close();
            }
            sessions.clear();
        }
    }

    pub fn uptime_ms(&self) -> u128 {
        self.started_at
            .lock()
            .map(|started| started.elapsed().as_millis())
            .unwrap_or_default()
    }

    pub fn create_session(
        &self,
        profile: Option<ToolProfile>,
        owner: impl Into<String>,
    ) -> Result<SessionInfo, ToolError> {
        self.gc_sessions();
        let mut sessions = self
            .sessions
            .write()
            .map_err(|_| ToolError::new(ErrorCode::Internal, "session table poisoned"))?;
        if sessions.len() >= self.config.max_sessions {
            return Err(ToolError::new(
                ErrorCode::ResourceBusy,
                format!(
                    "session limit reached ({}); close an idle session first",
                    self.config.max_sessions
                ),
            )
            .retryable(true));
        }
        let session = Arc::new(Session::new(
            profile.unwrap_or(self.config.profile),
            owner.into(),
            self.config.capture_cache_limit,
            self.config.observation_ttl,
        ));
        let info = SessionInfo {
            session_id: session.id.clone(),
            session_token: session.token.clone(),
            profile: session.profile,
            owner: session.owner.clone(),
        };
        sessions.insert(session.id.clone(), session);
        Ok(info)
    }

    pub fn get_session(
        &self,
        session_id: &str,
        token: Option<&str>,
    ) -> Result<Arc<Session>, ToolError> {
        let sessions = self
            .sessions
            .read()
            .map_err(|_| ToolError::new(ErrorCode::Internal, "session table poisoned"))?;
        let session = sessions.get(session_id).cloned().ok_or_else(|| {
            ToolError::new(
                ErrorCode::ServerRestarted,
                "session is unknown or belongs to a previous daemon",
            )
            .retryable(false)
        })?;
        if let Some(token) = token {
            if token != session.token {
                return Err(
                    ToolError::new(ErrorCode::AuthRequired, "session token is not valid")
                        .retryable(false),
                );
            }
        }
        if session.is_closed() {
            return Err(ToolError::new(
                ErrorCode::ServerRestarted,
                "session is closed",
            ));
        }
        Ok(session)
    }

    pub fn require_session(
        &self,
        session_id: &str,
        token: Option<&str>,
    ) -> Result<Arc<Session>, ToolError> {
        let token = token.filter(|token| !token.is_empty()).ok_or_else(|| {
            ToolError::new(
                ErrorCode::AuthRequired,
                "session_token is required for daemon calls",
            )
            .retryable(false)
        })?;
        self.get_session(session_id, Some(token))
    }

    pub fn list_sessions(&self) -> Vec<serde_json::Value> {
        self.sessions
            .read()
            .map(|sessions| sessions.values().map(|session| session.summary()).collect())
            .unwrap_or_default()
    }

    pub fn inspect_session(
        &self,
        session_id: &str,
        token: Option<&str>,
    ) -> Result<serde_json::Value, ToolError> {
        Ok(self.get_session(session_id, token)?.summary())
    }

    pub fn close_session(
        &self,
        session_id: &str,
        token: Option<&str>,
    ) -> Result<serde_json::Value, ToolError> {
        let session = self.require_session(session_id, token)?;
        session.close();
        if let Ok(mut sessions) = self.sessions.write() {
            sessions.remove(session_id);
        }
        Ok(serde_json::json!({
            "session_id": session_id,
            "closed": true
        }))
    }

    pub fn gc_sessions(&self) {
        let mut remove = Vec::new();
        if let Ok(sessions) = self.sessions.read() {
            for (id, session) in sessions.iter() {
                if session.is_closed() || session.last_used().elapsed() > self.config.session_idle {
                    remove.push(id.clone());
                }
            }
        }
        if remove.is_empty() {
            return;
        }
        if let Ok(mut sessions) = self.sessions.write() {
            for id in remove {
                if let Some(session) = sessions.remove(&id) {
                    session.close();
                }
            }
        }
    }

    pub fn sweep_idle_sessions(&self) {
        self.gc_sessions();
    }

    pub fn execute_oneshot(
        self: &Arc<Self>,
        tool: ToolName,
        args: serde_json::Value,
    ) -> impl std::future::Future<Output = ToolResult> + Send {
        let engine = self.clone();
        async move {
            if tool.meta().requires_session {
                return failure_meta(
                    tool,
                    None,
                    ToolError::new(
                        ErrorCode::InvalidParams,
                        format!("{} requires a daemon session", tool.as_str()),
                    ),
                );
            }
            let session = Arc::new(Session::new(
                ToolProfile::Full,
                "oneshot".to_string(),
                engine.config.capture_cache_limit,
                engine.config.observation_ttl,
            ));
            engine
                .execute_with_session(
                    session,
                    tool,
                    args,
                    Some(format!("oneshot-{}", uuid::Uuid::new_v4().simple())),
                    Arc::new(AtomicBool::new(false)),
                )
                .await
        }
    }

    pub async fn execute_with_session(
        self: &Arc<Self>,
        session: Arc<Session>,
        tool: ToolName,
        args: serde_json::Value,
        request_id: Option<String>,
        cancel: CancelFlag,
    ) -> ToolResult {
        let engine = self.clone();
        let joined = tokio::spawn(async move {
            engine
                .execute_inner(session, tool, args, request_id, cancel)
                .await
        })
        .await;
        match joined {
            Ok(result) => result,
            Err(error) => failure_meta(
                tool,
                None,
                ToolError::new(
                    ErrorCode::Internal,
                    format!("tool task failed or panicked: {error}"),
                ),
            ),
        }
    }

    async fn execute_inner(
        self: Arc<Self>,
        session: Arc<Session>,
        tool: ToolName,
        args: serde_json::Value,
        request_id: Option<String>,
        cancel: CancelFlag,
    ) -> ToolResult {
        session.touch();
        if cancel.load(Ordering::Acquire) {
            return result_for_session(
                &session,
                tool,
                request_id,
                Err(ToolError::new(
                    ErrorCode::Cancelled,
                    "request was cancelled",
                )),
            );
        }
        if !session.allows(tool) {
            return result_for_session(
                &session,
                tool,
                request_id,
                Err(ToolError::new(
                    ErrorCode::PolicyDenied,
                    format!(
                        "{} is not available in profile {}",
                        tool.as_str(),
                        session.profile
                    ),
                )),
            );
        }
        let resources = resource_needs(tool, &args);
        let permit = if resources.is_empty() {
            None
        } else {
            match self.resource_gate.acquire(&resources).await {
                Ok(permit) => Some(permit),
                Err(error) => {
                    return result_for_session(&session, tool, request_id, Err(error));
                }
            }
        };
        let result = tools::execute(&self, &session, tool, &args, &cancel).await;
        drop(permit);
        result_for_session(&session, tool, request_id, result)
    }
}

fn resource_needs(tool: ToolName, args: &serde_json::Value) -> Vec<ResourceKind> {
    match tool {
        ToolName::Mouse
            if args.get("action").and_then(|value| value.as_str()) == Some("position") =>
        {
            Vec::new()
        }
        ToolName::Mouse
        | ToolName::MouseDrag
        | ToolName::Input
        | ToolName::ClipboardWrite
        | ToolName::ClipboardClean => vec![ResourceKind::Input],
        ToolName::SemanticExecute | ToolName::SemanticAction | ToolName::AgentStep => {
            vec![ResourceKind::Semantic, ResourceKind::Input]
        }
        ToolName::Perceive | ToolName::FindText | ToolName::Vision => {
            vec![ResourceKind::Vision, ResourceKind::Capture]
        }
        ToolName::Screenshot
        | ToolName::WindowScreenshot
        | ToolName::FindImage
        | ToolName::FindColor
        | ToolName::FindMultiColor
        | ToolName::WaitFor => vec![ResourceKind::Capture],
        _ => Vec::new(),
    }
}

fn result_for_session(
    session: &Session,
    tool: ToolName,
    request_id: Option<String>,
    result: Result<ToolExecution, ToolError>,
) -> ToolResult {
    let meta = tool.meta();
    let base = ResultMeta {
        tool: Some(tool.as_str().to_string()),
        request_id,
        session_id: Some(session.id.clone()),
        dispatch_state: DispatchState::NotSent,
        effect: match meta.risk {
            RiskClass::ReadOnly => Effect::Read,
            RiskClass::Reversible | RiskClass::BoundedWrite => Effect::Unverifiable,
            RiskClass::ExternalCommit | RiskClass::DestructiveCritical => Effect::ExternalCommit,
        },
        verification: if meta.read_only {
            Verification::NotApplicable
        } else {
            Verification::Unknown
        },
        retryable: false,
        requires_observation: meta.requires_observation,
        risk: meta.risk,
    };
    match result {
        Ok(execution) => ToolResult::success(
            execution.data,
            ResultMeta {
                dispatch_state: execution.dispatch_state,
                effect: execution.effect.unwrap_or(base.effect),
                verification: execution.verification,
                retryable: false,
                requires_observation: execution.requires_observation,
                ..base
            },
        ),
        Err(error) => {
            let dispatch_state = error
                .details
                .get("dispatch_state")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or(match error.code {
                    ErrorCode::DispatchUnknown => DispatchState::Unknown,
                    _ => DispatchState::NotSent,
                });
            let retryable = error.retryable && dispatch_state.is_retry_safe();
            ToolResult::failure(
                ToolError { retryable, ..error },
                ResultMeta {
                    dispatch_state,
                    retryable,
                    requires_observation: meta.requires_observation
                        || !dispatch_state.is_retry_safe(),
                    ..base
                },
            )
        }
    }
}

fn failure_meta(tool: ToolName, request_id: Option<String>, error: ToolError) -> ToolResult {
    let meta = tool.meta();
    let dispatch_state = if error.code == ErrorCode::DispatchUnknown {
        DispatchState::Unknown
    } else {
        DispatchState::NotSent
    };
    let retryable = error.retryable && dispatch_state.is_retry_safe();
    ToolResult::failure(
        ToolError { retryable, ..error },
        ResultMeta {
            tool: Some(tool.as_str().to_string()),
            request_id,
            session_id: None,
            dispatch_state,
            effect: if meta.read_only {
                Effect::Read
            } else {
                Effect::Unverifiable
            },
            verification: if meta.read_only {
                Verification::NotApplicable
            } else {
                Verification::Unknown
            },
            retryable,
            requires_observation: meta.requires_observation,
            risk: meta.risk,
        },
    )
}
