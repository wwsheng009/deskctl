//! `deskctl history`: query the local audit trail (`deskctl-audit/1`).
//!
//! The command reads the JSONL shards directly (no daemon needed) and is
//! strictly read-only. Spec: docs/design/audit-history.md §4.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use chrono::{DateTime, Duration, Local, NaiveTime};
use deskctl_core::audit::{AuditConfig, AuditMode};
use deskctl_core::untrusted;
use serde_json::{json, Value};

pub struct HistoryOptions {
    pub session: Option<String>,
    pub owner: Option<String>,
    pub tool: Option<String>,
    pub run: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub failed_only: bool,
    pub kind: Option<String>,
    pub limit: usize,
    pub json: bool,
    pub summary: bool,
    pub dir: Option<PathBuf>,
}

pub fn run(options: HistoryOptions) -> Result<i32, String> {
    let config = AuditConfig::from_env();
    let dir = options.dir.clone().unwrap_or_else(|| config.dir.clone());
    if options.dir.is_none() && config.mode == AuditMode::Off {
        println!("audit is disabled (DESKCTL_AUDIT=off); nothing was recorded");
        return Ok(0);
    }

    let now = Local::now();
    let since = parse_time(options.since.as_deref(), now - Duration::hours(24), now)?;
    let until = parse_time(options.until.as_deref(), now, now)?;

    match &options.kind {
        Some(kind) if !matches!(kind.as_str(), "tool_call" | "audit_health") => {
            return Err(format!(
                "invalid_params: --kind must be tool_call or audit_health, got '{kind}'"
            ));
        }
        _ => {}
    }

    if !dir.is_dir() {
        if options.json {
            return Ok(0);
        }
        println!(
            "no records (audit directory {} does not exist yet)",
            dir.display()
        );
        return Ok(0);
    }

    let records = collect(&dir, &options, since, until)?;
    if records.is_empty() {
        if options.json {
            return Ok(0);
        }
        println!("no records matched");
        return Ok(0);
    }

    if options.summary {
        let summary = summarize(&records, since, until);
        println!(
            "{}",
            serde_json::to_string_pretty(&summary).map_err(|error| error.to_string())?
        );
        return Ok(0);
    }

    if options.json {
        for record in &records {
            println!(
                "{}",
                serde_json::to_string(&display_record(record)).map_err(|error| error.to_string())?
            );
        }
        return Ok(0);
    }

    for record in &records {
        println!("{}", format_record(record));
    }
    Ok(0)
}

/// Enumerate audit shards in write order and keep the matching records.
fn collect(
    dir: &PathBuf,
    options: &HistoryOptions,
    since: DateTime<Local>,
    until: DateTime<Local>,
) -> Result<Vec<Value>, String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map_err(|error| format!("policy_denied: read audit dir {}: {error}", dir.display()))?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("audit-") && name.ends_with(".jsonl"))
        .collect();
    names.sort();

    let keep_all = options.summary || options.limit == 0;
    let mut kept: VecDeque<Value> = VecDeque::new();
    for name in names {
        let path = dir.join(name);
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(_) => continue,
        };
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else {
                continue;
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            if !record.is_object() {
                continue;
            }
            if !matches_filters(&record, options, since, until) {
                continue;
            }
            kept.push_back(record);
            if !keep_all && kept.len() > options.limit {
                kept.pop_front();
            }
        }
    }

    let mut records: Vec<Value> = kept.into_iter().collect();
    records.sort_by_key(|record| {
        record["ts"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_default()
    });
    Ok(records)
}

fn matches_filters(
    record: &Value,
    options: &HistoryOptions,
    since: DateTime<Local>,
    until: DateTime<Local>,
) -> bool {
    let Some(ts) = record["ts"]
        .as_str()
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
    else {
        return false;
    };
    if ts < since || ts > until {
        return false;
    }
    if let Some(kind) = &options.kind {
        if record["kind"].as_str() != Some(kind.as_str()) {
            return false;
        }
    }
    if let Some(session) = &options.session {
        if record["session_id"].as_str() != Some(session.as_str()) {
            return false;
        }
    }
    if let Some(owner) = &options.owner {
        let matched = record["owner"]
            .as_str()
            .is_some_and(|current| owner_matches(owner, current));
        if !matched {
            return false;
        }
    }
    if let Some(tool) = &options.tool {
        if record["tool"].as_str() != Some(tool.as_str()) {
            return false;
        }
    }
    if let Some(run) = &options.run {
        if record["run_id"].as_str() != Some(run.as_str()) {
            return false;
        }
    }
    if options.failed_only && record["ok"].as_bool() != Some(false) {
        return false;
    }
    true
}

fn parse_time(
    raw: Option<&str>,
    default: DateTime<Local>,
    now: DateTime<Local>,
) -> Result<DateTime<Local>, String> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("now") {
        return Ok(now);
    }
    if trimmed.eq_ignore_ascii_case("today") {
        let midnight = now
            .date_naive()
            .and_time(NaiveTime::MIN)
            .and_local_timezone(Local)
            .single()
            .unwrap_or(now);
        return Ok(midnight);
    }
    for (suffix, unit_secs) in [('m', 60u64), ('h', 3_600), ('d', 86_400)] {
        if let Some(value) = trimmed.strip_suffix(suffix) {
            if let Ok(value) = value.parse::<u64>() {
                let seconds = value.saturating_mul(unit_secs).min(10 * 365 * 86_400);
                return Ok(now - Duration::seconds(seconds as i64));
            }
        }
    }
    DateTime::parse_from_rfc3339(trimmed)
        .map(|value| value.with_timezone(&Local))
        .map_err(|_| {
            format!(
                "invalid_params: cannot parse time '{trimmed}' \
                 (use RFC3339, today, now, or a relative window such as 30m / 2h / 7d)"
            )
        })
}

/// `--owner` matching: an exact label, or a `prefix*` pattern such as
/// `agent:sap-*` (design §6). Only a trailing `*` is treated as a wildcard.
fn owner_matches(pattern: &str, owner: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => owner.starts_with(prefix),
        None => pattern == owner,
    }
}

fn summarize(records: &[Value], since: DateTime<Local>, until: DateTime<Local>) -> Value {
    let mut by_tool: BTreeMap<String, (usize, usize, usize, usize, Vec<u64>)> = BTreeMap::new();
    let mut by_owner: BTreeMap<String, (usize, usize, usize, usize, Vec<u64>)> = BTreeMap::new();
    let mut calls = 0usize;
    let mut failed = 0usize;

    for record in records {
        if record["kind"].as_str() != Some("tool_call") {
            continue;
        }
        calls += 1;
        let ok = record["ok"].as_bool() == Some(true);
        if !ok {
            failed += 1;
        }
        let tool = record["tool"].as_str().unwrap_or("-").to_string();
        let owner = record["owner"].as_str().unwrap_or("-").to_string();
        let duration = record["duration_ms"].as_u64().unwrap_or(0);
        let dispatch = record["dispatch_state"].as_str().unwrap_or("");
        let entry = by_tool.entry(tool).or_insert((0, 0, 0, 0, Vec::new()));
        entry.0 += 1;
        if !ok {
            entry.1 += 1;
        }
        if dispatch == "not_sent" {
            entry.2 += 1;
        }
        if dispatch == "unknown" {
            entry.3 += 1;
        }
        entry.4.push(duration);
        let owner_entry = by_owner.entry(owner).or_insert((0, 0, 0, 0, Vec::new()));
        owner_entry.0 += 1;
        if !ok {
            owner_entry.1 += 1;
        }
        if dispatch == "not_sent" {
            owner_entry.2 += 1;
        }
        if dispatch == "unknown" {
            owner_entry.3 += 1;
        }
        owner_entry.4.push(duration);
    }

    let mut tools: Vec<Value> = by_tool
        .into_iter()
        .map(
            |(tool, (calls, failed, not_sent, unknown, mut durations))| {
                durations.sort_unstable();
                json!({
                    "tool": tool,
                    "calls": calls,
                    "failed": failed,
                    "not_sent": not_sent,
                    "unknown": unknown,
                    "p50_ms": quantile(&durations, 0.50),
                    "p95_ms": quantile(&durations, 0.95),
                })
            },
        )
        .collect();
    tools.sort_by(|left, right| {
        right["calls"]
            .as_u64()
            .cmp(&left["calls"].as_u64())
            .then_with(|| left["tool"].as_str().cmp(&right["tool"].as_str()))
    });

    let owners: Vec<Value> = by_owner
        .into_iter()
        .map(
            |(owner, (calls, failed, not_sent, unknown, mut durations))| {
                durations.sort_unstable();
                json!({
                    "owner": owner,
                    "calls": calls,
                    "ok": calls.saturating_sub(failed),
                    "failed": failed,
                    "not_sent": not_sent,
                    "unknown": unknown,
                    "p50_ms": quantile(&durations, 0.50),
                    "p95_ms": quantile(&durations, 0.95),
                })
            },
        )
        .collect();

    let failure_rate = if calls == 0 {
        0.0
    } else {
        failed as f64 / calls as f64
    };
    json!({
        "window": {
            "since": since.to_rfc3339(),
            "until": until.to_rfc3339(),
            "records": records.len(),
            "calls": calls,
            "failed": failed,
            "failure_rate": (failure_rate * 1000.0).round() / 1000.0,
        },
        "by_tool": tools,
        "by_owner": owners,
    })
}

fn quantile(sorted: &[u64], q: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn format_record(record: &Value) -> String {
    if record["kind"].as_str() == Some("audit_health") {
        return format!(
            "{:23}  audit_health  dropped={}",
            short_time(record["ts"].as_str().unwrap_or("")),
            record["audit_dropped"].as_u64().unwrap_or(0),
        );
    }
    let outcome = if record["ok"].as_bool() == Some(true) {
        "ok".to_string()
    } else {
        match record["error"]["code"].as_str() {
            Some(code) => format!("FAIL({code})"),
            None => "FAIL".to_string(),
        }
    };
    format!(
        "{:23}  {:24}  {:8}  {:26}  {:9}  {:>6}ms  {}",
        short_time(record["ts"].as_str().unwrap_or("")),
        record["owner"].as_str().unwrap_or("-"),
        record["session_id"]
            .as_str()
            .map(short_id)
            .unwrap_or_else(|| "-".to_string()),
        record["tool"].as_str().unwrap_or("-"),
        record["dispatch_state"].as_str().unwrap_or("-"),
        record["duration_ms"].as_u64().unwrap_or(0),
        outcome,
    )
}

/// 记录里的 `error.message`/`error.details` 可能内嵌屏幕文本（候选窗口标题、
/// 观察到的文本等），AC-SEC-013 要求这类文本以 `<untrusted-screen-content>`
/// 包裹并声明"数据非指令"；`DESKCTL_UNTRUSTED_WRAP=off` 可关闭。
fn display_record(record: &Value) -> Value {
    if !untrusted::enabled() {
        return record.clone();
    }
    let mut copy = record.clone();
    if let Some(error) = copy.get_mut("error").and_then(Value::as_object_mut) {
        if let Some(message) = error.get_mut("message") {
            *message = untrusted::wrap_json_strings(message);
        }
        if let Some(details) = error.get_mut("details") {
            *details = untrusted::wrap_json_strings(details);
        }
    }
    copy
}

fn short_time(raw: &str) -> String {
    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.format("%Y-%m-%d %H:%M:%S %:z").to_string())
        .unwrap_or_else(|_| raw.to_string())
}

fn short_id(raw: &str) -> String {
    if raw.chars().count() > 8 {
        let mut short: String = raw.chars().take(8).collect();
        short.push('…');
        short
    } else {
        raw.to_string()
    }
}
