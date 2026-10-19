use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidParams,
    UnknownTool,
    PlatformNotSupported,
    PermissionDenied,
    PolicyDenied,
    StaleCapture,
    StaleObservation,
    TargetUnavailable,
    /// A previously bound target (hwnd/token) is no longer valid; re-resolve.
    StaleTarget,
    /// Target resolution matched multiple windows and none is uniquely best.
    AmbiguousTarget,
    /// Target fingerprint mismatch (suspected hwnd reuse); refuses by default.
    IdentityMismatch,
    ResourceBusy,
    Cancelled,
    Timeout,
    ModelUnavailable,
    DispatchUnknown,
    Internal,
    ServerRestarted,
    AuthRequired,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidParams => "invalid_params",
            Self::UnknownTool => "unknown_tool",
            Self::PlatformNotSupported => "platform_not_supported",
            Self::PermissionDenied => "permission_denied",
            Self::PolicyDenied => "policy_denied",
            Self::StaleCapture => "stale_capture",
            Self::StaleObservation => "stale_observation",
            Self::TargetUnavailable => "target_unavailable",
            Self::StaleTarget => "stale_target",
            Self::AmbiguousTarget => "ambiguous_target",
            Self::IdentityMismatch => "identity_mismatch",
            Self::ResourceBusy => "resource_busy",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::ModelUnavailable => "model_unavailable",
            Self::DispatchUnknown => "dispatch_unknown",
            Self::Internal => "internal",
            Self::ServerRestarted => "server_restarted",
            Self::AuthRequired => "auth_required",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub details: serde_json::Value,
}

impl ToolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
            details: serde_json::Value::Null,
        }
    }

    pub fn retryable(mut self, value: bool) -> Self {
        self.retryable = value;
        self
    }

    pub fn details(mut self, details: serde_json::Value) -> Self {
        self.details = details;
        self
    }

    pub fn with_details(self, details: serde_json::Value) -> Self {
        self.details(details)
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ToolError {}

impl From<std::io::Error> for ToolError {
    fn from(error: std::io::Error) -> Self {
        Self::new(ErrorCode::Internal, error.to_string()).retryable(true)
    }
}
