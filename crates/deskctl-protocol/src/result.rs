use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ToolError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchState {
    #[default]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    #[default]
    Read,
    Unverifiable,
    ExternalCommit,
    Confirmed,
    Refused,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    NotApplicable,
    #[default]
    Unknown,
    Satisfied,
    Progress,
    NoChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    #[default]
    ReadOnly,
    Reversible,
    BoundedWrite,
    ExternalCommit,
    DestructiveCritical,
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
