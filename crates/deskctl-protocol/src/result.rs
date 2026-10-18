use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ToolError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchState {
    NotSent,
    Sent,
    Partial,
    Unknown,
}

impl DispatchState {
    pub fn is_retry_safe(self) -> bool {
        self == Self::NotSent
    }
}

impl Default for DispatchState {
    fn default() -> Self {
        Self::NotSent
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Read,
    Unverifiable,
    ExternalCommit,
    Confirmed,
    Refused,
    Partial,
}

impl Default for Effect {
    fn default() -> Self {
        Self::Unverifiable
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    NotApplicable,
    Unknown,
    Satisfied,
    Progress,
    NoChange,
}

impl Default for Verification {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    ReadOnly,
    Reversible,
    BoundedWrite,
    ExternalCommit,
    DestructiveCritical,
}

impl Default for RiskClass {
    fn default() -> Self {
        Self::ReadOnly
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub dispatch_state: DispatchState,
    #[serde(default)]
    pub effect: Effect,
    #[serde(default)]
    pub verification: Verification,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default)]
    pub requires_observation: bool,
    #[serde(default)]
    pub risk: RiskClass,
}

impl Default for ResultMeta {
    fn default() -> Self {
        Self {
            tool: None,
            request_id: None,
            session_id: None,
            dispatch_state: DispatchState::NotSent,
            effect: Effect::Unverifiable,
            verification: Verification::Unknown,
            retryable: false,
            requires_observation: false,
            risk: RiskClass::ReadOnly,
        }
    }
}

/// Stable envelope returned by every tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ToolError>,
    pub meta: ResultMeta,
}

impl ToolResult {
    pub fn success(data: Value, meta: ResultMeta) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
            meta,
        }
    }

    pub fn failure(error: ToolError, meta: ResultMeta) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(error),
            meta,
        }
    }
}

/// Compatibility name used by the CLI/daemon/MCP adapter.
pub type ToolEnvelope = ToolResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    #[serde(default)]
    pub requires_session: bool,
    #[serde(default)]
    pub requires_observation: bool,
    #[serde(default)]
    pub optional: bool,
}
