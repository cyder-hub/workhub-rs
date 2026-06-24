use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
    path::Path,
    time::{Duration, SystemTime},
};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::observability::{
    rotation::{ERROR_LOG_FILE, RUN_LOG_FILE},
    support_bundle::{LogFileSummary, read_log_text, recent_log_files},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsageSourceFilter {
    All,
    Mcp,
    Cli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsageSort {
    Calls,
    Failures,
    SuccessRate,
    AvgDuration,
    Name,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UsageOptions {
    pub since: Duration,
    pub source: UsageSourceFilter,
    pub limit: usize,
    pub sort: UsageSort,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct UsageReport {
    pub window_start_utc: String,
    pub window_end_utc: String,
    pub files_scanned: Vec<LogFileSummary>,
    pub events_read: u64,
    pub events_used: u64,
    pub events_skipped: u64,
    pub items: Vec<UsageItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct UsageItem {
    pub source: String,
    pub name: String,
    pub provider: String,
    pub calls: u64,
    pub started: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub incomplete: u64,
    pub success_rate: f64,
    pub failure_rate: f64,
    pub avg_duration_ms: Option<u64>,
    pub p95_duration_ms: Option<u64>,
    pub max_duration_ms: Option<u64>,
    pub last_seen_utc: Option<String>,
    pub last_failure_utc: Option<String>,
    pub last_error_kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawUsageEvent {
    timestamp_utc: Option<String>,
    event: Option<String>,
    mode: Option<String>,
    tool_call_id: Option<String>,
    command_id: Option<String>,
    outcome: Option<String>,
    duration_ms: Option<u64>,
    #[serde(rename = "error.kind")]
    error_kind: Option<String>,
    tool_name: Option<String>,
    command_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum UsageSource {
    Mcp,
    Cli,
}

impl UsageSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Cli => "cli",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalOutcome {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone)]
struct RecognizedUsageEvent {
    timestamp: DateTime<Utc>,
    source: UsageSource,
    name: String,
    call_id: String,
    event_key: String,
    terminal: Option<TerminalOutcome>,
    duration_ms: Option<u64>,
    error_kind: Option<String>,
}

#[derive(Debug, Clone)]
struct CallState {
    source: UsageSource,
    name: String,
    provider: String,
    started: bool,
    terminal: Option<TerminalOutcome>,
    duration_ms: Option<u64>,
    last_seen: DateTime<Utc>,
    last_failure: Option<DateTime<Utc>>,
    last_error_kind: Option<String>,
}

#[derive(Debug, Default)]
struct ItemAccumulator {
    source: UsageSource,
    name: String,
    provider: String,
    calls: u64,
    started: u64,
    succeeded: u64,
    failed: u64,
    incomplete: u64,
    durations: Vec<u64>,
    last_seen: Option<DateTime<Utc>>,
    last_failure: Option<DateTime<Utc>>,
    last_error_kind: Option<String>,
}

impl Default for UsageSource {
    fn default() -> Self {
        Self::Mcp
    }
}

pub(crate) fn analyze_usage(
    log_dir: &Path,
    options: &UsageOptions,
    now: DateTime<Utc>,
) -> io::Result<UsageReport> {
    let window_start = now
        - chrono::Duration::from_std(options.since)
            .unwrap_or_else(|_| chrono::Duration::seconds(i64::MAX));
    let cutoff = SystemTime::from(window_start);
    let mut files = recent_log_files(log_dir, Some(cutoff))?
        .into_iter()
        .filter(|file| is_usage_log_file(&file.name))
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.name.cmp(&right.name));

    let mut report = UsageReport {
        window_start_utc: format_timestamp(window_start),
        window_end_utc: format_timestamp(now),
        files_scanned: files
            .iter()
            .map(|file| LogFileSummary {
                name: file.name.clone(),
                bytes: file.bytes,
            })
            .collect(),
        events_read: 0,
        events_used: 0,
        events_skipped: 0,
        items: Vec::new(),
    };
    let mut seen_events = HashSet::new();
    let mut calls = HashMap::<String, CallState>::new();

    for file in files {
        let content = read_log_text(&file.path)?;
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            report.events_read += 1;
            let event = match serde_json::from_str::<RawUsageEvent>(line) {
                Ok(event) => event,
                Err(_) => {
                    report.events_skipped += 1;
                    continue;
                }
            };
            let Some(event) = recognize_usage_event(event, window_start, now, options.source)
            else {
                report.events_skipped += 1;
                continue;
            };
            if !seen_events.insert(event.event_key.clone()) {
                report.events_skipped += 1;
                continue;
            }

            report.events_used += 1;
            merge_call_event(&mut calls, event);
        }
    }

    report.items = build_items(calls, options.sort, options.limit);
    Ok(report)
}

fn is_usage_log_file(name: &str) -> bool {
    name == RUN_LOG_FILE
        || name == ERROR_LOG_FILE
        || name.starts_with("workhub.log.")
        || name.starts_with("workhub-error.log.")
}

fn recognize_usage_event(
    event: RawUsageEvent,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
    source_filter: UsageSourceFilter,
) -> Option<RecognizedUsageEvent> {
    if event.mode.as_deref() == Some("logs") {
        return None;
    }
    let event_name = event.event.as_deref()?;
    let timestamp = DateTime::parse_from_rfc3339(event.timestamp_utc.as_deref()?)
        .ok()?
        .with_timezone(&Utc);
    if timestamp < window_start || timestamp > window_end {
        return None;
    }

    match event_name {
        "mcp.tool_call.started" | "mcp.tool_call.completed" | "mcp.tool_call.failed" => {
            if source_filter == UsageSourceFilter::Cli {
                return None;
            }
            let event_key = stable_event_key(
                UsageSource::Mcp,
                event_name,
                &event.tool_call_id,
                &event.tool_name,
                timestamp,
            );
            let name = event.tool_name?;
            let call_id = event.tool_call_id.unwrap_or_else(|| {
                format!(
                    "missing:{}:{}:{}",
                    UsageSource::Mcp.as_str(),
                    event_name,
                    timestamp.timestamp_millis()
                )
            });
            let terminal = match event_name {
                "mcp.tool_call.completed" => Some(TerminalOutcome::Succeeded),
                "mcp.tool_call.failed" => Some(TerminalOutcome::Failed),
                _ => None,
            };
            Some(RecognizedUsageEvent {
                timestamp,
                source: UsageSource::Mcp,
                name,
                call_id,
                event_key,
                terminal,
                duration_ms: event.duration_ms,
                error_kind: event.error_kind,
            })
        }
        "cli.command.started" | "cli.command.completed" => {
            if source_filter == UsageSourceFilter::Mcp {
                return None;
            }
            let event_key = stable_event_key(
                UsageSource::Cli,
                event_name,
                &event.command_id,
                &event.command_path,
                timestamp,
            );
            let name = event.command_path?;
            if name == "logs" || name.starts_with("logs ") {
                return None;
            }
            let call_id = event.command_id.unwrap_or_else(|| {
                format!(
                    "missing:{}:{}:{}",
                    UsageSource::Cli.as_str(),
                    event_name,
                    timestamp.timestamp_millis()
                )
            });
            let terminal = match (event_name, event.outcome.as_deref()) {
                ("cli.command.completed", Some("failed")) => Some(TerminalOutcome::Failed),
                ("cli.command.completed", _) => Some(TerminalOutcome::Succeeded),
                _ => None,
            };
            Some(RecognizedUsageEvent {
                timestamp,
                source: UsageSource::Cli,
                name,
                call_id,
                event_key,
                terminal,
                duration_ms: event.duration_ms,
                error_kind: event.error_kind,
            })
        }
        _ if is_cli_failure_diagnostic_event(event_name, &event) => {
            if source_filter == UsageSourceFilter::Mcp {
                return None;
            }
            let event_key = stable_event_key(
                UsageSource::Cli,
                event_name,
                &event.command_id,
                &event.command_path,
                timestamp,
            );
            let name = event.command_path?;
            if name == "logs" || name.starts_with("logs ") {
                return None;
            }
            Some(RecognizedUsageEvent {
                timestamp,
                source: UsageSource::Cli,
                name,
                call_id: event.command_id?,
                event_key,
                terminal: Some(TerminalOutcome::Failed),
                duration_ms: event.duration_ms,
                error_kind: event.error_kind,
            })
        }
        _ => None,
    }
}

fn is_cli_failure_diagnostic_event(event_name: &str, event: &RawUsageEvent) -> bool {
    event_name.ends_with(".failed")
        && event.command_id.is_some()
        && event.command_path.is_some()
        && event.error_kind.is_some()
}

fn stable_event_key(
    source: UsageSource,
    event_name: &str,
    id: &Option<String>,
    name: &Option<String>,
    timestamp: DateTime<Utc>,
) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        source.as_str(),
        event_name,
        id.as_deref().unwrap_or(""),
        name.as_deref().unwrap_or(""),
        timestamp.to_rfc3339_opts(SecondsFormat::Millis, true)
    )
}

fn merge_call_event(calls: &mut HashMap<String, CallState>, event: RecognizedUsageEvent) {
    let key = format!("{}|{}", event.source.as_str(), event.call_id);
    let provider = provider_for(event.source, &event.name);
    let state = calls.entry(key).or_insert_with(|| CallState {
        source: event.source,
        name: event.name.clone(),
        provider,
        started: false,
        terminal: None,
        duration_ms: None,
        last_seen: event.timestamp,
        last_failure: None,
        last_error_kind: None,
    });

    if event.terminal.is_none() {
        state.started = true;
    } else {
        state.terminal = event.terminal;
        if event.duration_ms.is_some() {
            state.duration_ms = event.duration_ms;
        }
        if event.terminal == Some(TerminalOutcome::Failed) {
            state.last_failure = Some(event.timestamp);
            if event.error_kind.is_some() {
                state.last_error_kind = event.error_kind;
            }
        }
    }
    if event.timestamp > state.last_seen {
        state.last_seen = event.timestamp;
    }
}

fn build_items(calls: HashMap<String, CallState>, sort: UsageSort, limit: usize) -> Vec<UsageItem> {
    let mut items = BTreeMap::<(UsageSource, String), ItemAccumulator>::new();
    for call in calls.into_values() {
        let key = (call.source, call.name.clone());
        let item = items.entry(key).or_insert_with(|| ItemAccumulator {
            source: call.source,
            name: call.name.clone(),
            provider: call.provider.clone(),
            ..ItemAccumulator::default()
        });
        item.calls += 1;
        if call.started {
            item.started += 1;
        }
        match call.terminal {
            Some(TerminalOutcome::Succeeded) => item.succeeded += 1,
            Some(TerminalOutcome::Failed) => item.failed += 1,
            None if call.started => item.incomplete += 1,
            None => {}
        }
        if let Some(duration) = call.duration_ms {
            item.durations.push(duration);
        }
        if item
            .last_seen
            .is_none_or(|last_seen| call.last_seen > last_seen)
        {
            item.last_seen = Some(call.last_seen);
        }
        if let Some(last_failure) = call.last_failure
            && item
                .last_failure
                .is_none_or(|current| last_failure > current)
        {
            item.last_failure = Some(last_failure);
            item.last_error_kind = call.last_error_kind;
        }
    }

    let mut output = items
        .into_values()
        .map(ItemAccumulator::into_item)
        .collect::<Vec<_>>();
    sort_items(&mut output, sort);
    if limit > 0 {
        output.truncate(limit);
    }
    output
}

impl ItemAccumulator {
    fn into_item(mut self) -> UsageItem {
        self.durations.sort_unstable();
        let avg_duration_ms = average_duration(&self.durations);
        let p95_duration_ms = percentile_duration(&self.durations, 95);
        let max_duration_ms = self.durations.last().copied();
        let calls = self.calls.max(1);

        UsageItem {
            source: self.source.as_str().to_string(),
            name: self.name,
            provider: self.provider,
            calls: self.calls,
            started: self.started,
            succeeded: self.succeeded,
            failed: self.failed,
            incomplete: self.incomplete,
            success_rate: self.succeeded as f64 / calls as f64,
            failure_rate: self.failed as f64 / calls as f64,
            avg_duration_ms,
            p95_duration_ms,
            max_duration_ms,
            last_seen_utc: self.last_seen.map(format_timestamp),
            last_failure_utc: self.last_failure.map(format_timestamp),
            last_error_kind: self.last_error_kind,
        }
    }
}

fn average_duration(durations: &[u64]) -> Option<u64> {
    if durations.is_empty() {
        return None;
    }
    Some((durations.iter().sum::<u64>() as f64 / durations.len() as f64).round() as u64)
}

fn percentile_duration(durations: &[u64], percentile: u64) -> Option<u64> {
    if durations.is_empty() {
        return None;
    }
    let rank = ((percentile as f64 / 100.0) * durations.len() as f64).ceil() as usize;
    durations.get(rank.saturating_sub(1)).copied()
}

fn sort_items(items: &mut [UsageItem], sort: UsageSort) {
    items.sort_by(|left, right| match sort {
        UsageSort::Calls => right
            .calls
            .cmp(&left.calls)
            .then_with(|| left.name.cmp(&right.name)),
        UsageSort::Failures => right
            .failed
            .cmp(&left.failed)
            .then_with(|| right.calls.cmp(&left.calls))
            .then_with(|| left.name.cmp(&right.name)),
        UsageSort::SuccessRate => left
            .success_rate
            .partial_cmp(&right.success_rate)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.calls.cmp(&left.calls))
            .then_with(|| left.name.cmp(&right.name)),
        UsageSort::AvgDuration => right
            .avg_duration_ms
            .unwrap_or(0)
            .cmp(&left.avg_duration_ms.unwrap_or(0))
            .then_with(|| right.calls.cmp(&left.calls))
            .then_with(|| left.name.cmp(&right.name)),
        UsageSort::Name => left
            .source
            .cmp(&right.source)
            .then_with(|| left.name.cmp(&right.name)),
    });
}

fn provider_for(source: UsageSource, name: &str) -> String {
    match source {
        UsageSource::Mcp => name
            .split_once('_')
            .map(|(provider, _)| provider)
            .unwrap_or("unknown")
            .to_string(),
        UsageSource::Cli => name
            .split_whitespace()
            .next()
            .unwrap_or("unknown")
            .to_string(),
    }
}

fn format_timestamp(timestamp: DateTime<Utc>) -> String {
    timestamp.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write};

    use chrono::TimeZone;
    use flate2::{Compression, write::GzEncoder};
    use serde_json::json;

    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 23, 8, 0, 0).unwrap()
    }

    fn options() -> UsageOptions {
        UsageOptions {
            since: Duration::from_secs(24 * 3600),
            source: UsageSourceFilter::All,
            limit: 50,
            sort: UsageSort::Calls,
        }
    }

    fn write_lines(path: &Path, lines: &[serde_json::Value]) {
        let text = lines
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, format!("{text}\n")).unwrap();
    }

    #[test]
    fn usage_aggregates_mcp_and_cli_calls() {
        let temp = tempfile::tempdir().unwrap();
        write_lines(
            &temp.path().join(RUN_LOG_FILE),
            &[
                json!({
                    "timestamp_utc": "2026-06-23T07:00:00.000Z",
                    "event": "mcp.tool_call.started",
                    "mode": "stdio",
                    "tool_call_id": "tool_1",
                    "tool_name": "jira_get_issue"
                }),
                json!({
                    "timestamp_utc": "2026-06-23T07:00:01.000Z",
                    "event": "mcp.tool_call.completed",
                    "mode": "stdio",
                    "tool_call_id": "tool_1",
                    "tool_name": "jira_get_issue",
                    "duration_ms": 120
                }),
                json!({
                    "timestamp_utc": "2026-06-23T07:01:00.000Z",
                    "event": "cli.command.started",
                    "mode": "cli",
                    "command_id": "cmd_1",
                    "command_path": "jira issue get"
                }),
                json!({
                    "timestamp_utc": "2026-06-23T07:01:01.000Z",
                    "event": "cli.command.completed",
                    "mode": "cli",
                    "command_id": "cmd_1",
                    "command_path": "jira issue get",
                    "outcome": "failed",
                    "duration_ms": 80,
                    "error.kind": "cli_command_failed"
                }),
                json!({
                    "timestamp_utc": "2026-06-23T07:02:00.000Z",
                    "event": "mcp.tool_call.started",
                    "mode": "streamhttp",
                    "tool_call_id": "tool_2",
                    "tool_name": "confluence_search"
                }),
            ],
        );

        let report = analyze_usage(temp.path(), &options(), now()).unwrap();

        assert_eq!(report.events_used, 5);
        assert_eq!(report.items.len(), 3);
        let jira_tool = report
            .items
            .iter()
            .find(|item| item.name == "jira_get_issue")
            .unwrap();
        assert_eq!(jira_tool.source, "mcp");
        assert_eq!(jira_tool.provider, "jira");
        assert_eq!(jira_tool.calls, 1);
        assert_eq!(jira_tool.succeeded, 1);
        assert_eq!(jira_tool.avg_duration_ms, Some(120));
        let cli = report
            .items
            .iter()
            .find(|item| item.name == "jira issue get")
            .unwrap();
        assert_eq!(cli.source, "cli");
        assert_eq!(cli.failed, 1);
        assert_eq!(cli.last_error_kind.as_deref(), Some("cli_command_failed"));
        let incomplete = report
            .items
            .iter()
            .find(|item| item.name == "confluence_search")
            .unwrap();
        assert_eq!(incomplete.incomplete, 1);
    }

    #[test]
    fn usage_deduplicates_error_events_across_run_and_error_logs() {
        let temp = tempfile::tempdir().unwrap();
        let failed = json!({
            "timestamp_utc": "2026-06-23T07:00:01.000Z",
            "event": "mcp.tool_call.failed",
            "mode": "stdio",
            "tool_call_id": "tool_1",
            "tool_name": "jira_get_issue",
            "duration_ms": 120,
            "error.kind": "mcp_error_-32603"
        });
        write_lines(
            &temp.path().join(RUN_LOG_FILE),
            std::slice::from_ref(&failed),
        );
        write_lines(
            &temp.path().join(ERROR_LOG_FILE),
            std::slice::from_ref(&failed),
        );

        let report = analyze_usage(temp.path(), &options(), now()).unwrap();

        assert_eq!(report.events_used, 1);
        assert_eq!(report.events_skipped, 1);
        assert_eq!(report.items[0].calls, 1);
        assert_eq!(report.items[0].failed, 1);
    }

    #[test]
    fn usage_merges_cli_failure_diagnostic_with_completed_event() {
        let temp = tempfile::tempdir().unwrap();
        write_lines(
            &temp.path().join(RUN_LOG_FILE),
            &[
                json!({
                    "timestamp_utc": "2026-06-23T07:00:00.000Z",
                    "event": "cli.command.started",
                    "mode": "cli",
                    "command_id": "cmd_1",
                    "command_path": "jira project list"
                }),
                json!({
                    "timestamp_utc": "2026-06-23T07:00:01.000Z",
                    "event": "cli.command.completed",
                    "mode": "cli",
                    "command_id": "cmd_1",
                    "command_path": "jira project list",
                    "outcome": "failed",
                    "duration_ms": 80
                }),
                json!({
                    "timestamp_utc": "2026-06-23T07:00:01.000Z",
                    "event": "cli.failed",
                    "mode": "cli",
                    "command_id": "cmd_1",
                    "command_path": "jira project list",
                    "duration_ms": 80,
                    "error.kind": "cli_command_failed"
                }),
            ],
        );

        let report = analyze_usage(temp.path(), &options(), now()).unwrap();

        assert_eq!(report.events_used, 3);
        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items[0].calls, 1);
        assert_eq!(report.items[0].failed, 1);
        assert_eq!(report.items[0].avg_duration_ms, Some(80));
        assert_eq!(
            report.items[0].last_error_kind.as_deref(),
            Some("cli_command_failed")
        );
    }

    #[test]
    fn usage_filters_by_time_source_and_limit() {
        let temp = tempfile::tempdir().unwrap();
        write_lines(
            &temp.path().join(RUN_LOG_FILE),
            &[
                json!({
                    "timestamp_utc": "2026-06-20T07:00:00.000Z",
                    "event": "mcp.tool_call.completed",
                    "mode": "stdio",
                    "tool_call_id": "old",
                    "tool_name": "jira_search",
                    "duration_ms": 50
                }),
                json!({
                    "timestamp_utc": "2026-06-23T07:00:00.000Z",
                    "event": "mcp.tool_call.completed",
                    "mode": "stdio",
                    "tool_call_id": "tool_1",
                    "tool_name": "jira_get_issue",
                    "duration_ms": 120
                }),
                json!({
                    "timestamp_utc": "2026-06-23T07:01:00.000Z",
                    "event": "cli.command.completed",
                    "mode": "cli",
                    "command_id": "cmd_1",
                    "command_path": "jira issue get",
                    "outcome": "succeeded",
                    "duration_ms": 80
                }),
            ],
        );
        let mut options = options();
        options.source = UsageSourceFilter::Mcp;
        options.limit = 1;

        let report = analyze_usage(temp.path(), &options, now()).unwrap();

        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items[0].name, "jira_get_issue");
        assert_eq!(report.items[0].calls, 1);
    }

    #[test]
    fn usage_reads_gzip_logs_and_skips_bad_lines() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workhub.log.2026-06-23.0.gz");
        let output = fs::File::create(path).unwrap();
        let mut encoder = GzEncoder::new(output, Compression::default());
        writeln!(
            encoder,
            "{}",
            json!({
                "timestamp_utc": "2026-06-23T07:00:00.000Z",
                "event": "mcp.tool_call.completed",
                "mode": "stdio",
                "tool_call_id": "tool_1",
                "tool_name": "jira_get_issue",
                "duration_ms": 120
            })
        )
        .unwrap();
        writeln!(encoder, "not json").unwrap();
        encoder.finish().unwrap();

        let report = analyze_usage(temp.path(), &options(), now()).unwrap();

        assert_eq!(report.files_scanned.len(), 1);
        assert_eq!(report.events_read, 2);
        assert_eq!(report.events_used, 1);
        assert_eq!(report.events_skipped, 1);
        assert_eq!(report.items[0].name, "jira_get_issue");
    }

    #[test]
    fn usage_excludes_logs_mode_self_events() {
        let temp = tempfile::tempdir().unwrap();
        write_lines(
            &temp.path().join(RUN_LOG_FILE),
            &[
                json!({
                    "timestamp_utc": "2026-06-23T07:00:00.000Z",
                    "event": "cli.command.completed",
                    "mode": "logs",
                    "command_id": "cmd_logs",
                    "command_path": "logs usage",
                    "outcome": "succeeded"
                }),
                json!({
                    "timestamp_utc": "2026-06-23T07:01:00.000Z",
                    "event": "cli.command.completed",
                    "mode": "cli",
                    "command_id": "cmd_1",
                    "command_path": "jira project list",
                    "outcome": "succeeded"
                }),
            ],
        );

        let report = analyze_usage(temp.path(), &options(), now()).unwrap();

        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items[0].name, "jira project list");
    }
}
