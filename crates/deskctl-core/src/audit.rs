//! Audit trail (`deskctl-audit/1`): record model, redaction, asynchronous
//! single-writer sink, rotation/retention, and helpers behind `deskctl history`.
//!
//! Spec: docs/design/audit-history.md and docs/design/data-formats.md §1.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use chrono::{Local, SecondsFormat};
use deskctl_protocol::{DispatchState, Effect, RiskClass, Verification};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::config;

pub const AUDIT_FORMAT: &str = "deskctl-audit/1";
pub const AUDIT_VERSION: i64 = 1;

const ARGS_STRING_LIMIT: usize = 256;
const ARGS_TOTAL_LIMIT: usize = 4 * 1024;
const ARGS_SHORT_STRING_LIMIT: usize = 64;
const ARGS_ARRAY_KEEP: usize = 8;
const ARGS_DEPTH_LIMIT: usize = 8;
const ERROR_MESSAGE_LIMIT: usize = 512;

const STATE_OK: u8 = 0;
const STATE_DISABLED: u8 = 1;
const STATE_OFF: u8 = 2;

/// Audit mode (`DESKCTL_AUDIT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditMode {
    /// No directory, no files; `history` prints a hint.
    Off,
    /// Record metadata and `args_digest` only (default).
    Basic,
    /// Record redacted `args` as well.
    Full,
}

impl AuditMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "basic" => Some(Self::Basic),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Basic => "basic",
            Self::Full => "full",
        }
    }

    pub fn enabled(self) -> bool {
        self != Self::Off
    }
}

/// Text policy for keyboard/clipboard text (`DESKCTL_AUDIT_TEXT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextPolicy {
    /// Do not record length or digest.
    None,
    /// Record `text_len` + `text_sha256` (default).
    Digest,
    /// Record the raw text (isolated environments only).
    Full,
}

impl TextPolicy {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "digest" => Some(Self::Digest),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Digest => "digest",
            Self::Full => "full",
        }
    }
}

/// Where a tool call entered the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditSource {
    Daemon,
    Mcp,
    Oneshot,
}

impl AuditSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::Mcp => "mcp",
            Self::Oneshot => "oneshot",
        }
    }
}

/// Audit configuration derived from `DESKCTL_AUDIT*` / `DESKCTL_RUNS_*`.
#[derive(Debug, Clone)]
pub struct AuditConfig {
    pub mode: AuditMode,
    pub text: TextPolicy,
    pub dir: PathBuf,
    pub queue: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub retain_days: u64,
    pub runs_dir: PathBuf,
    pub runs_retain_days: u64,
    pub runs_max_bytes: u64,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

impl AuditConfig {
    pub fn from_env() -> Self {
        let mode = std::env::var("DESKCTL_AUDIT")
            .ok()
            .as_deref()
            .and_then(AuditMode::parse)
            .unwrap_or(AuditMode::Basic);
        let text = std::env::var("DESKCTL_AUDIT_TEXT")
            .ok()
            .as_deref()
            .and_then(TextPolicy::parse)
            .unwrap_or(TextPolicy::Digest);
        let dir = std::env::var_os("DESKCTL_AUDIT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| config::data_dir().join("audit"));
        Self {
            mode,
            text,
            dir,
            queue: env_usize("DESKCTL_AUDIT_QUEUE", 4096).max(1),
            max_file_bytes: env_mb("DESKCTL_AUDIT_MAX_FILE_MB", 64),
            max_total_bytes: env_mb("DESKCTL_AUDIT_MAX_MB", 256),
            retain_days: env_u64("DESKCTL_AUDIT_RETAIN_DAYS", 30),
            runs_dir: config::data_dir().join("runs"),
            runs_retain_days: env_u64("DESKCTL_RUNS_RETAIN_DAYS", 30),
            runs_max_bytes: env_mb("DESKCTL_RUNS_MAX_MB", 256),
        }
    }

    /// A configuration with auditing switched off (used by tests).
    pub fn off() -> Self {
        let mut config = Self::from_env();
        config.mode = AuditMode::Off;
        config
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_mb(name: &str, default_mb: u64) -> u64 {
    env_u64(name, default_mb).saturating_mul(1024 * 1024)
}

/// `sha256:<lowercase hex>`.
pub fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Digest of a JSON value's compact serialization.
pub fn digest_json(value: &Value) -> String {
    digest_bytes(&serde_json::to_vec(value).unwrap_or_default())
}

/// RFC3339 local timestamp with milliseconds (`2026-10-19T01:23:45.123+08:00`).
pub fn now_ts() -> String {
    Local::now().to_rfc3339_opts(SecondsFormat::Millis, false)
}

pub fn risk_str(risk: RiskClass) -> &'static str {
    match risk {
        RiskClass::ReadOnly => "read_only",
        RiskClass::Reversible => "reversible",
        RiskClass::BoundedWrite => "bounded_write",
        RiskClass::ExternalCommit => "external_commit",
        RiskClass::DestructiveCritical => "destructive_critical",
    }
}

pub fn dispatch_state_str(state: DispatchState) -> &'static str {
    match state {
        DispatchState::NotSent => "not_sent",
        DispatchState::Sent => "sent",
        DispatchState::Partial => "partial",
        DispatchState::Unknown => "unknown",
    }
}

pub fn effect_str(effect: Effect) -> &'static str {
    match effect {
        Effect::Read => "read",
        Effect::Unverifiable => "unverifiable",
        Effect::ExternalCommit => "external_commit",
        Effect::Confirmed => "confirmed",
        Effect::Refused => "refused",
        Effect::Partial => "partial",
    }
}

pub fn verification_str(verification: Verification) -> &'static str {
    match verification {
        Verification::NotApplicable => "not_applicable",
        Verification::Unknown => "unknown",
        Verification::Satisfied => "satisfied",
        Verification::Progress => "progress",
        Verification::NoChange => "no_change",
    }
}

/// Keys whose values are replaced by `<redacted>` (case-insensitive substring).
const SENSITIVE_KEYS: &[&str] = &[
    "api_key",
    "apikey",
    "token",
    "authorization",
    "password",
    "passwd",
    "secret",
];

fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SENSITIVE_KEYS.iter().any(|needle| lower.contains(needle))
}

/// Tools whose top-level `text` argument follows `DESKCTL_AUDIT_TEXT`.
fn has_text_policy(tool: &str) -> bool {
    matches!(tool, "desktop_input" | "desktop_clipboard_write")
}

#[derive(Debug, Default)]
pub struct SanitizedArgs {
    pub args: Option<Value>,
    pub redacted: bool,
    pub truncated: bool,
}

/// Redact/truncate call arguments for the audit record (audit-history §5.1).
pub fn sanitize_args(
    tool: &str,
    args: Option<&Value>,
    mode: AuditMode,
    text: TextPolicy,
) -> SanitizedArgs {
    if mode != AuditMode::Full {
        return SanitizedArgs::default();
    }
    let Some(args) = args else {
        return SanitizedArgs::default();
    };
    let mut redacted = false;
    let mut truncated = false;
    let mut value = sanitize_value(tool, args, text, 0, &mut redacted, &mut truncated);
    if serialized_len(&value) > ARGS_TOTAL_LIMIT {
        shorten_strings(&mut value, ARGS_SHORT_STRING_LIMIT);
        truncated = true;
    }
    if serialized_len(&value) > ARGS_TOTAL_LIMIT {
        trim_arrays(&mut value, ARGS_ARRAY_KEEP);
        truncated = true;
    }
    if serialized_len(&value) > ARGS_TOTAL_LIMIT {
        value = keys_only(&value);
        truncated = true;
    }
    SanitizedArgs {
        args: Some(value),
        redacted,
        truncated,
    }
}

fn serialized_len(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}

fn sanitize_value(
    tool: &str,
    value: &Value,
    text: TextPolicy,
    depth: usize,
    redacted: &mut bool,
    truncated: &mut bool,
) -> Value {
    if depth > ARGS_DEPTH_LIMIT {
        *truncated = true;
        return Value::String("<truncated>".to_string());
    }
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, entry) in map {
                if is_sensitive_key(key) {
                    *redacted = true;
                    out.insert(key.clone(), Value::String("<redacted>".to_string()));
                    continue;
                }
                if depth == 0 && key == "text" && has_text_policy(tool) {
                    if let Some(raw) = entry.as_str() {
                        for (name, item) in apply_text_policy(raw, text) {
                            out.insert(name, item);
                        }
                        continue;
                    }
                }
                out.insert(
                    key.clone(),
                    sanitize_value(tool, entry, text, depth + 1, redacted, truncated),
                );
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| sanitize_value(tool, item, text, depth + 1, redacted, truncated))
                .collect(),
        ),
        Value::String(text_value) => {
            if text_value.chars().count() > ARGS_STRING_LIMIT {
                *truncated = true;
                Value::String(text_value.chars().take(ARGS_STRING_LIMIT).collect())
            } else {
                Value::String(text_value.clone())
            }
        }
        other => other.clone(),
    }
}

fn apply_text_policy(raw: &str, policy: TextPolicy) -> Vec<(String, Value)> {
    match policy {
        TextPolicy::None => Vec::new(),
        TextPolicy::Digest => vec![
            (
                "text_len".to_string(),
                Value::from(raw.chars().count() as u64),
            ),
            (
                "text_sha256".to_string(),
                Value::String(digest_bytes(raw.as_bytes())),
            ),
        ],
        TextPolicy::Full => vec![("text".to_string(), Value::String(raw.to_string()))],
    }
}

fn shorten_strings(value: &mut Value, limit: usize) {
    match value {
        Value::Object(map) => {
            for entry in map.values_mut() {
                shorten_strings(entry, limit);
            }
        }
        Value::Array(items) => {
            for item in items {
                shorten_strings(item, limit);
            }
        }
        Value::String(text) => {
            if text.chars().count() > limit {
                *text = text.chars().take(limit).collect();
            }
        }
        _ => {}
    }
}

fn trim_arrays(value: &mut Value, keep: usize) {
    match value {
        Value::Object(map) => {
            for entry in map.values_mut() {
                trim_arrays(entry, keep);
            }
        }
        Value::Array(items) => {
            items.truncate(keep);
            for item in items {
                trim_arrays(item, keep);
            }
        }
        _ => {}
    }
}

/// Last-resort degradation: keep only top-level key names.
fn keys_only(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            out.insert("_truncated".to_string(), Value::Bool(true));
            out.insert(
                "_keys".to_string(),
                Value::Array(
                    map.keys()
                        .map(|key| Value::String(key.clone()))
                        .collect::<Vec<_>>(),
                ),
            );
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// A settled error summary for one audit row.
#[derive(Debug, Clone)]
pub struct AuditError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

/// One audit record (`kind="tool_call"` or `kind="audit_health"`).
#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub ts: String,
    pub kind: &'static str,
    pub request_id: Option<String>,
    pub session_id: Option<String>,
    pub owner: String,
    pub source: &'static str,
    pub tool: Option<String>,
    pub risk: Option<&'static str>,
    pub args: Option<Value>,
    pub args_digest: Option<String>,
    pub duration_ms: Option<u64>,
    pub ok: Option<bool>,
    pub dispatch_state: Option<&'static str>,
    pub effect: Option<&'static str>,
    pub verification: Option<&'static str>,
    pub retryable: Option<bool>,
    pub error: Option<AuditError>,
    pub result_digest: Option<String>,
    pub audit_dropped: Option<u64>,
}

/// Raw inputs for one tool-call audit row.
pub struct ToolCallInput<'a> {
    pub request_id: Option<String>,
    pub session_id: Option<String>,
    pub owner: String,
    pub source: AuditSource,
    pub tool: &'a str,
    pub risk: &'static str,
    pub args: Value,
    pub duration_ms: u64,
    pub ok: bool,
    pub dispatch_state: &'static str,
    pub effect: &'static str,
    pub verification: &'static str,
    pub retryable: bool,
    pub error: Option<AuditError>,
    pub data: Option<&'a Value>,
}

impl AuditRecord {
    pub fn tool_call(input: ToolCallInput<'_>) -> Self {
        Self {
            ts: now_ts(),
            kind: "tool_call",
            request_id: input.request_id,
            session_id: input.session_id,
            owner: input.owner,
            source: input.source.as_str(),
            tool: Some(input.tool.to_string()),
            risk: Some(input.risk),
            args_digest: Some(digest_json(&input.args)),
            args: Some(input.args),
            duration_ms: Some(input.duration_ms),
            ok: Some(input.ok),
            dispatch_state: Some(input.dispatch_state),
            effect: Some(input.effect),
            verification: Some(input.verification),
            retryable: Some(input.retryable),
            error: input.error,
            result_digest: input.data.map(digest_json),
            audit_dropped: None,
        }
    }

    /// `kind="audit_health"` row emitted when the sink had to drop records.
    pub fn audit_health(dropped: u64) -> Self {
        Self {
            ts: now_ts(),
            kind: "audit_health",
            request_id: None,
            session_id: None,
            owner: "system".to_string(),
            source: "system",
            tool: None,
            risk: None,
            args: None,
            args_digest: None,
            duration_ms: None,
            ok: None,
            dispatch_state: None,
            effect: None,
            verification: None,
            retryable: None,
            error: None,
            result_digest: None,
            audit_dropped: Some(dropped),
        }
    }

    pub fn to_json(&self, mode: AuditMode, text: TextPolicy) -> Value {
        let mut obj = Map::new();
        obj.insert("v".to_string(), Value::from(AUDIT_VERSION));
        obj.insert("ts".to_string(), Value::String(self.ts.clone()));
        obj.insert("kind".to_string(), Value::String(self.kind.to_string()));
        if let Some(value) = &self.request_id {
            obj.insert("request_id".to_string(), Value::String(value.clone()));
        }
        if let Some(value) = &self.session_id {
            obj.insert("session_id".to_string(), Value::String(value.clone()));
        }
        obj.insert("owner".to_string(), Value::String(self.owner.clone()));
        obj.insert("source".to_string(), Value::String(self.source.to_string()));
        if self.kind == "audit_health" {
            obj.insert(
                "audit_dropped".to_string(),
                Value::from(self.audit_dropped.unwrap_or(0)),
            );
            return Value::Object(obj);
        }
        if let Some(value) = &self.tool {
            obj.insert("tool".to_string(), Value::String(value.clone()));
        }
        if let Some(value) = self.risk {
            obj.insert("risk".to_string(), Value::String(value.to_string()));
        }
        let sanitized = sanitize_args(
            self.tool.as_deref().unwrap_or(""),
            self.args.as_ref(),
            mode,
            text,
        );
        obj.insert("args".to_string(), sanitized.args.unwrap_or(Value::Null));
        if let Some(value) = &self.args_digest {
            obj.insert("args_digest".to_string(), Value::String(value.clone()));
        }
        if sanitized.redacted {
            obj.insert("args_redacted".to_string(), Value::Bool(true));
        }
        obj.insert(
            "args_truncated".to_string(),
            Value::Bool(sanitized.truncated),
        );
        obj.insert(
            "duration_ms".to_string(),
            Value::from(self.duration_ms.unwrap_or(0)),
        );
        obj.insert("ok".to_string(), Value::Bool(self.ok.unwrap_or(false)));
        if let Some(value) = self.dispatch_state {
            obj.insert(
                "dispatch_state".to_string(),
                Value::String(value.to_string()),
            );
        }
        if let Some(value) = self.effect {
            obj.insert("effect".to_string(), Value::String(value.to_string()));
        }
        if let Some(value) = self.verification {
            obj.insert("verification".to_string(), Value::String(value.to_string()));
        }
        obj.insert(
            "retryable".to_string(),
            Value::Bool(self.retryable.unwrap_or(false)),
        );
        obj.insert(
            "error".to_string(),
            match &self.error {
                Some(error) => {
                    let message = if error.message.chars().count() > ERROR_MESSAGE_LIMIT {
                        error.message.chars().take(ERROR_MESSAGE_LIMIT).collect()
                    } else {
                        error.message.clone()
                    };
                    serde_json::json!({
                        "code": error.code,
                        "message": message,
                        "retryable": error.retryable,
                    })
                }
                None => Value::Null,
            },
        );
        if let Some(value) = &self.result_digest {
            obj.insert("result_digest".to_string(), Value::String(value.clone()));
        }
        Value::Object(obj)
    }
}

enum AuditMsg {
    Record(Box<AuditRecord>),
    Flush(SyncSender<()>),
}

/// Asynchronous single-writer audit sink.
///
/// `record()` never blocks: the bounded queue absorbs bursts, overflow is
/// counted and reported through `audit_health` rows. Dropping the sink closes
/// the queue and joins the writer, which is the deterministic flush for
/// one-shot CLI runs.
pub struct AuditSink {
    sender: Option<SyncSender<AuditMsg>>,
    state: Arc<AtomicU8>,
    reason: Arc<Mutex<Option<String>>>,
    dropped: Arc<AtomicU64>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl AuditSink {
    pub fn new(config: AuditConfig) -> Self {
        let state = Arc::new(AtomicU8::new(STATE_OK));
        let reason = Arc::new(Mutex::new(None));
        let dropped = Arc::new(AtomicU64::new(0));
        if !config.mode.enabled() {
            state.store(STATE_OFF, Ordering::Relaxed);
            return Self {
                sender: None,
                state,
                reason,
                dropped,
                handle: None,
            };
        }
        let (sender, receiver) = sync_channel(config.queue.max(1));
        let writer_state = Arc::clone(&state);
        let writer_reason = Arc::clone(&reason);
        let writer_dropped = Arc::clone(&dropped);
        let writer_config = config.clone();
        let spawned = std::thread::Builder::new()
            .name("deskctl-audit".to_string())
            .spawn(move || {
                writer_loop(
                    receiver,
                    writer_config,
                    writer_state,
                    writer_reason,
                    writer_dropped,
                )
            });
        match spawned {
            Ok(handle) => Self {
                sender: Some(sender),
                state,
                reason,
                dropped,
                handle: Some(handle),
            },
            Err(error) => {
                state.store(STATE_DISABLED, Ordering::Relaxed);
                *reason
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                    Some(format!("audit thread spawn failed: {error}"));
                Self {
                    sender: None,
                    state,
                    reason,
                    dropped,
                    handle: None,
                }
            }
        }
    }

    pub fn enabled(&self) -> bool {
        self.sender.is_some()
    }

    pub fn state(&self) -> &'static str {
        match self.state.load(Ordering::Relaxed) {
            STATE_OFF => "off",
            STATE_DISABLED => "disabled",
            _ => "ok",
        }
    }

    pub fn disabled_reason(&self) -> Option<String> {
        self.reason
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn recorded_drops(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn record(&self, record: AuditRecord) {
        let Some(sender) = &self.sender else {
            return;
        };
        match sender.try_send(AuditMsg::Record(Box::new(record))) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {
                self.state.store(STATE_DISABLED, Ordering::Relaxed);
            }
        }
    }

    /// Block until every queued record has been handled (bounded wait).
    pub fn flush(&self) {
        let Some(sender) = &self.sender else {
            return;
        };
        let (ack_tx, ack_rx) = sync_channel(1);
        if sender.send(AuditMsg::Flush(ack_tx)).is_ok() {
            let _ = ack_rx.recv_timeout(Duration::from_secs(2));
        }
    }
}

impl Drop for AuditSink {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn writer_loop(
    receiver: Receiver<AuditMsg>,
    config: AuditConfig,
    state: Arc<AtomicU8>,
    reason: Arc<Mutex<Option<String>>>,
    dropped: Arc<AtomicU64>,
) {
    let mut writer = ShardWriter::new(config);
    while let Ok(message) = receiver.recv() {
        match message {
            AuditMsg::Record(record) => {
                if state.load(Ordering::Relaxed) == STATE_DISABLED {
                    continue;
                }
                let losses = dropped.swap(0, Ordering::AcqRel);
                if losses > 0 {
                    let health = AuditRecord::audit_health(losses);
                    write_line(&mut writer, &health, &state, &reason);
                }
                write_line(&mut writer, &record, &state, &reason);
            }
            AuditMsg::Flush(ack) => {
                let _ = ack.send(());
            }
        }
    }
}

fn write_line(
    writer: &mut ShardWriter,
    record: &AuditRecord,
    state: &AtomicU8,
    reason: &Mutex<Option<String>>,
) {
    let value = record.to_json(writer.mode(), writer.text());
    let Ok(line) = serde_json::to_string(&value) else {
        return;
    };
    if let Err(error) = writer.append(&line) {
        let previously = state.swap(STATE_DISABLED, Ordering::AcqRel);
        if previously != STATE_DISABLED {
            eprintln!("deskctl: audit disabled: {error}");
            *reason
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error);
        }
    }
}

struct CurrentShard {
    date: String,
    path: PathBuf,
    file: File,
    size: u64,
}

struct ShardWriter {
    config: AuditConfig,
    current: Option<CurrentShard>,
    cleaned_date: Option<String>,
}

impl ShardWriter {
    fn new(config: AuditConfig) -> Self {
        Self {
            config,
            current: None,
            cleaned_date: None,
        }
    }

    fn mode(&self) -> AuditMode {
        self.config.mode
    }

    fn text(&self) -> TextPolicy {
        self.config.text
    }

    fn append(&mut self, line: &str) -> Result<(), String> {
        let today = Local::now().format("%Y-%m-%d").to_string();
        if self.cleaned_date.as_deref() != Some(today.as_str()) {
            let _ = cleanup_audit_dir(&self.config);
            let _ = cleanup_runs_dir(&self.config);
            self.cleaned_date = Some(today.clone());
        }
        self.ensure_shard(&today)?;
        let shard = self.current.as_mut().expect("shard opened");
        let bytes = line.as_bytes();
        shard
            .file
            .write_all(bytes)
            .and_then(|_| shard.file.write_all(b"\n"))
            .and_then(|_| shard.file.flush())
            .map_err(|error| format!("write {}: {error}", shard.path.display()))?;
        shard.size += bytes.len() as u64 + 1;
        Ok(())
    }

    fn ensure_shard(&mut self, date: &str) -> Result<(), String> {
        if let Some(current) = &self.current {
            if current.date == date && current.size < self.config.max_file_bytes {
                return Ok(());
            }
        }
        fs::create_dir_all(&self.config.dir)
            .map_err(|error| format!("create_dir {}: {error}", self.config.dir.display()))?;
        let path = next_shard_path(&self.config.dir, date, self.config.max_file_bytes);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        self.current = Some(CurrentShard {
            date: date.to_string(),
            path,
            file,
            size,
        });
        Ok(())
    }
}

fn next_shard_path(dir: &Path, date: &str, max_bytes: u64) -> PathBuf {
    let base = dir.join(format!("audit-{date}.jsonl"));
    for index in 0..10_000u32 {
        let path = if index == 0 {
            base.clone()
        } else {
            dir.join(format!("audit-{date}.{index}.jsonl"))
        };
        match fs::metadata(&path) {
            Ok(meta) if meta.len() >= max_bytes => continue,
            _ => return path,
        }
    }
    base
}

fn parse_audit_file_name(name: &str) -> Option<(chrono::NaiveDate, u32)> {
    let stem = name.strip_prefix("audit-")?.strip_suffix(".jsonl")?;
    if let Some((date, index)) = stem.split_once('.') {
        let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
        let index = index.parse().ok()?;
        Some((date, index))
    } else {
        let date = chrono::NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok()?;
        Some((date, 0))
    }
}

/// Delete audit shards older than `retain_days` and enforce the total cap.
pub fn cleanup_audit_dir(config: &AuditConfig) -> Result<(), String> {
    let dir = &config.dir;
    if !dir.is_dir() {
        return Ok(());
    }
    let mut files: Vec<(chrono::NaiveDate, u32, PathBuf, u64)> = Vec::new();
    let entries =
        fs::read_dir(dir).map_err(|error| format!("read_dir {}: {error}", dir.display()))?;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some((date, index)) = parse_audit_file_name(name) else {
            continue;
        };
        let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        files.push((date, index, path, size));
    }
    files.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));
    let cutoff =
        Local::now().date_naive() - chrono::Duration::days(config.retain_days.min(36_500) as i64);
    let mut total: u64 = files.iter().map(|(_, _, _, size)| *size).sum();
    files.retain(|(date, _, path, size)| {
        if *date < cutoff {
            let _ = fs::remove_file(path);
            total = total.saturating_sub(*size);
            false
        } else {
            true
        }
    });
    while total > config.max_total_bytes && files.len() > 1 {
        let (_, _, path, size) = files.remove(0);
        let _ = fs::remove_file(&path);
        total = total.saturating_sub(size);
    }
    if total > config.max_total_bytes {
        if let Some((_, _, path, _)) = files.first() {
            truncate_head(path, config.max_total_bytes.max(4096))?;
        }
    }
    Ok(())
}

/// Keep only the tail of one oversized shard, aligned to a line boundary.
fn truncate_head(path: &Path, keep_bytes: u64) -> Result<(), String> {
    let mut file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    if len <= keep_bytes {
        return Ok(());
    }
    file.seek(SeekFrom::Start(len - keep_bytes))
        .map_err(|error| format!("seek {}: {error}", path.display()))?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let start = buffer
        .iter()
        .position(|byte| *byte == b'\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let tmp = path.with_extension("jsonl.tmp");
    let backup = path.with_extension("jsonl.old");
    fs::write(&tmp, &buffer[start..])
        .map_err(|error| format!("write {}: {error}", tmp.display()))?;
    fs::rename(path, &backup).map_err(|error| format!("rename {}: {error}", path.display()))?;
    fs::rename(&tmp, path).map_err(|error| format!("rename {}: {error}", tmp.display()))?;
    let _ = fs::remove_file(&backup);
    Ok(())
}

/// Delete run directories older than `runs_retain_days` and enforce the cap.
pub fn cleanup_runs_dir(config: &AuditConfig) -> Result<(), String> {
    let dir = &config.runs_dir;
    if !dir.is_dir() {
        return Ok(());
    }
    let mut runs: Vec<(SystemTime, PathBuf, u64)> = Vec::new();
    let entries =
        fs::read_dir(dir).map_err(|error| format!("read_dir {}: {error}", dir.display()))?;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        runs.push((modified, path.clone(), dir_size(&path)));
    }
    runs.sort_by(|left, right| left.0.cmp(&right.0));
    let cutoff =
        SystemTime::now() - Duration::from_secs(config.runs_retain_days.saturating_mul(86_400));
    let mut total: u64 = runs.iter().map(|(_, _, size)| *size).sum();
    runs.retain(|(modified, path, size)| {
        if *modified < cutoff {
            let _ = fs::remove_dir_all(path);
            total = total.saturating_sub(*size);
            false
        } else {
            true
        }
    });
    while total > config.runs_max_bytes && runs.len() > 1 {
        let (_, path, size) = runs.remove(0);
        let _ = fs::remove_dir_all(&path);
        total = total.saturating_sub(size);
    }
    Ok(())
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            total = total.saturating_add(dir_size(&entry.path()));
        } else {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("deskctl-audit-{tag}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn test_config(tag: &str) -> AuditConfig {
        let mut config = AuditConfig::off();
        config.mode = AuditMode::Full;
        config.dir = temp_dir(tag);
        config.runs_dir = config.dir.join("runs");
        config.queue = 64;
        config
    }

    fn sample_record(args: Value) -> AuditRecord {
        AuditRecord::tool_call(ToolCallInput {
            request_id: Some("req-1".to_string()),
            session_id: Some("sess-1".to_string()),
            owner: "agent:tester".to_string(),
            source: AuditSource::Daemon,
            tool: "desktop_input",
            risk: "bounded_write",
            args,
            duration_ms: 7,
            ok: true,
            dispatch_state: "sent",
            effect: "unverifiable",
            verification: "satisfied",
            retryable: false,
            error: None,
            data: Some(&json!({"pressed": "enter"})),
        })
    }

    fn day(offset_days: i64) -> String {
        (Local::now().date_naive() + chrono::Duration::days(offset_days))
            .format("%Y-%m-%d")
            .to_string()
    }

    #[test]
    fn basic_mode_records_digest_only() {
        let args = json!({"mode": "type", "text": "hello world"});
        let sanitized = sanitize_args(
            "desktop_input",
            Some(&args),
            AuditMode::Basic,
            TextPolicy::Digest,
        );
        assert!(sanitized.args.is_none());
        assert!(!sanitized.redacted);
    }

    #[test]
    fn full_mode_redacts_sensitive_keys() {
        let args = json!({
            "api_key": "sk-live-123",
            "nested": {"Authorization": "Bearer xyz", "keep": 1},
            "mode": "type",
            "text": "s3cret",
        });
        let sanitized = sanitize_args(
            "desktop_input",
            Some(&args),
            AuditMode::Full,
            TextPolicy::Digest,
        );
        let value = sanitized.args.expect("args present");
        assert!(sanitized.redacted);
        assert_eq!(value["api_key"], "<redacted>");
        assert_eq!(value["nested"]["Authorization"], "<redacted>");
        assert_eq!(value["nested"]["keep"], 1);
        assert!(value.get("text").is_none());
        assert_eq!(value["text_len"], 6);
        assert_eq!(
            value["text_sha256"],
            digest_bytes("s3cret".as_bytes()).as_str()
        );
        let line = serde_json::to_string(&value).expect("serialize");
        assert!(!line.contains("s3cret"));
        assert!(!line.contains("sk-live-123"));
        assert!(!line.contains("Bearer xyz"));
    }

    #[test]
    fn text_policies_none_and_full() {
        let args = json!({"mode": "type", "text": "hello"});
        let none = sanitize_args(
            "desktop_input",
            Some(&args),
            AuditMode::Full,
            TextPolicy::None,
        );
        let value = none.args.expect("args");
        assert!(value.get("text").is_none());
        assert!(value.get("text_len").is_none());
        let full = sanitize_args(
            "desktop_input",
            Some(&args),
            AuditMode::Full,
            TextPolicy::Full,
        );
        assert_eq!(full.args.expect("args")["text"], "hello");
    }

    #[test]
    fn long_strings_truncate_and_flag() {
        let long = "x".repeat(600);
        let args = json!({"path": long});
        let sanitized = sanitize_args(
            "desktop_image_read",
            Some(&args),
            AuditMode::Full,
            TextPolicy::Digest,
        );
        let value = sanitized.args.expect("args");
        assert!(sanitized.truncated);
        assert_eq!(value["path"].as_str().expect("string").chars().count(), 256);
    }

    #[test]
    fn oversized_args_degrade_in_stages() {
        let items: Vec<Value> = (0..40).map(|i| json!(i)).collect();
        let args = json!({"items": items, "blob": "y".repeat(5000)});
        let sanitized = sanitize_args(
            "desktop_window_find",
            Some(&args),
            AuditMode::Full,
            TextPolicy::Digest,
        );
        let value = sanitized.args.expect("args");
        assert!(sanitized.truncated);
        assert!(serialized_len(&value) <= ARGS_TOTAL_LIMIT);
    }

    #[test]
    fn record_json_shape_and_digests() {
        let args = json!({"mode": "key", "key": "enter"});
        let record = sample_record(args.clone());
        assert_eq!(
            record.args_digest.as_deref(),
            Some(digest_json(&args).as_str())
        );
        assert_eq!(
            record.result_digest.as_deref(),
            Some(digest_json(&json!({"pressed": "enter"})).as_str())
        );
        let value = record.to_json(AuditMode::Basic, TextPolicy::Digest);
        assert_eq!(value["v"], 1);
        assert_eq!(value["kind"], "tool_call");
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["session_id"], "sess-1");
        assert_eq!(value["owner"], "agent:tester");
        assert_eq!(value["source"], "daemon");
        assert_eq!(value["tool"], "desktop_input");
        assert_eq!(value["risk"], "bounded_write");
        assert_eq!(value["args"], Value::Null);
        assert_eq!(value["args_truncated"], false);
        assert_eq!(value["duration_ms"], 7);
        assert_eq!(value["ok"], true);
        assert_eq!(value["dispatch_state"], "sent");
        assert_eq!(value["effect"], "unverifiable");
        assert_eq!(value["verification"], "satisfied");
        assert_eq!(value["retryable"], false);
        assert_eq!(value["error"], Value::Null);
        let ts = value["ts"].as_str().expect("ts");
        chrono::DateTime::parse_from_rfc3339(ts).expect("rfc3339 ts");

        let full = record.to_json(AuditMode::Full, TextPolicy::Digest);
        assert_eq!(full["args"]["mode"], "key");
    }

    #[test]
    fn health_row_shape() {
        let value = AuditRecord::audit_health(3).to_json(AuditMode::Basic, TextPolicy::Digest);
        assert_eq!(value["kind"], "audit_health");
        assert_eq!(value["audit_dropped"], 3);
        assert_eq!(value["owner"], "system");
    }

    #[test]
    fn sink_writes_records_and_flushes() {
        let config = test_config("sink");
        let sink = AuditSink::new(config.clone());
        assert!(sink.enabled());
        assert_eq!(sink.state(), "ok");
        sink.record(sample_record(
            json!({"mode": "type", "text": "hello world"}),
        ));
        sink.flush();
        let path = config.dir.join(format!("audit-{}.jsonl", day(0)));
        let text = fs::read_to_string(&path).expect("shard written");
        let line = text.lines().next().expect("one line");
        assert!(!line.contains("hello world"));
        let value: Value = serde_json::from_str(line).expect("json line");
        assert_eq!(value["tool"], "desktop_input");
        assert_eq!(value["args"]["text_len"], 11);
        assert_eq!(sink.recorded_drops(), 0);
    }

    #[test]
    fn sink_rotates_by_size() {
        let mut config = test_config("rotate");
        config.max_file_bytes = 400;
        let sink = AuditSink::new(config.clone());
        for index in 0..30u64 {
            sink.record(sample_record(
                json!({"mode": "key", "key": "enter", "i": index}),
            ));
        }
        sink.flush();
        let mut shard_count = 0usize;
        let mut max_index: Option<u64> = None;
        let mut records = 0usize;
        for entry in fs::read_dir(&config.dir).expect("read dir").flatten() {
            let name = entry.file_name().into_string().unwrap_or_default();
            if !name.ends_with(".jsonl") {
                continue;
            }
            shard_count += 1;
            let text = fs::read_to_string(entry.path()).expect("read shard");
            for line in text.lines() {
                let value: Value = serde_json::from_str(line).expect("json line");
                records += 1;
                if let Some(index) = value["args"]["i"].as_u64() {
                    max_index = Some(max_index.map_or(index, |current: u64| current.max(index)));
                }
            }
        }
        assert!(
            shard_count > 1,
            "expected rotation, got {shard_count} shards"
        );
        assert_eq!(records, 30, "every record must survive rotation");
        assert_eq!(max_index, Some(29), "newest record is present");
    }

    #[test]
    fn sink_disables_on_write_failure() {
        let mut config = test_config("blocked");
        let blocked = config.dir.join("audit");
        fs::write(&blocked, b"not a directory").expect("write file");
        config.dir = blocked;
        let sink = AuditSink::new(config);
        sink.record(sample_record(json!({"mode": "key"})));
        sink.flush();
        assert_eq!(sink.state(), "disabled");
        assert!(sink.disabled_reason().is_some());
    }

    #[test]
    fn off_mode_never_touches_disk() {
        let mut config = AuditConfig::off();
        config.dir = temp_dir("off").join("audit");
        let sink = AuditSink::new(config.clone());
        assert!(!sink.enabled());
        assert_eq!(sink.state(), "off");
        sink.record(sample_record(json!({"mode": "key"})));
        sink.flush();
        assert!(!config.dir.exists());
    }

    #[test]
    fn cleanup_removes_expired_shards() {
        let config = test_config("cleanup");
        let expired = config.dir.join(format!("audit-{}.jsonl", day(-40)));
        fs::write(&expired, "{\"v\":1}\n").expect("write expired");
        let recent = config.dir.join(format!("audit-{}.jsonl", day(-1)));
        fs::write(&recent, "{\"v\":1}\n").expect("write recent");
        cleanup_audit_dir(&config).expect("cleanup");
        assert!(!expired.exists(), "expired shard should be deleted");
        assert!(recent.exists(), "recent shard should be kept");
    }

    #[test]
    fn cleanup_enforces_total_cap() {
        let mut config = test_config("cap");
        config.max_total_bytes = 300;
        let oldest = config.dir.join(format!("audit-{}.jsonl", day(-2)));
        fs::write(&oldest, "z".repeat(400)).expect("write oldest");
        let newer = config.dir.join(format!("audit-{}.jsonl", day(-1)));
        fs::write(&newer, "n".repeat(200)).expect("write newer");
        cleanup_audit_dir(&config).expect("cleanup");
        assert!(!oldest.exists(), "oldest shard removed to meet the cap");
        assert!(newer.exists(), "newest shard kept");
    }

    #[test]
    fn cleanup_truncates_single_oversized_shard() {
        let mut config = test_config("truncate");
        config.max_total_bytes = 512;
        let path = config.dir.join(format!("audit-{}.jsonl", day(0)));
        let mut content = String::new();
        for index in 0..200 {
            content.push_str(&format!(
                "{{\"line\":{index},\"pad\":\"{}\"}}\n",
                "x".repeat(80)
            ));
        }
        fs::write(&path, &content).expect("write big shard");
        cleanup_audit_dir(&config).expect("cleanup");
        let text = fs::read_to_string(&path).expect("read truncated");
        let keep = config.max_total_bytes.max(4096);
        assert!((text.len() as u64) <= keep + 256, "size {}", text.len());
        assert!(text.len() < content.len());
        assert!(text.starts_with('{'), "tail aligned to a line boundary");
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn runs_cleanup_enforces_total_cap() {
        let mut config = test_config("runs");
        config.runs_dir = temp_dir("runs-root");
        config.runs_max_bytes = 400;
        let first = config.runs_dir.join("run-1");
        fs::create_dir_all(&first).expect("create run-1");
        fs::write(first.join("trace.jsonl"), "a".repeat(300)).expect("write run-1 data");
        std::thread::sleep(Duration::from_millis(50));
        let second = config.runs_dir.join("run-2");
        fs::create_dir_all(&second).expect("create run-2");
        fs::write(second.join("trace.jsonl"), "b".repeat(300)).expect("write run-2 data");
        cleanup_runs_dir(&config).expect("cleanup runs");
        assert!(!first.exists(), "oldest run removed");
        assert!(second.exists(), "newest run kept");
    }
}
