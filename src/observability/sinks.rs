use std::{
    io::{self, Write},
    sync::{Arc, Mutex, OnceLock},
};

use chrono::{DateTime, Local, Utc};

use crate::observability::{
    config::{ConsoleFormat, LogTarget, LoggingConfig},
    context::ObservabilityContext,
    redaction::sanitize_event,
    rotation::{AUDIT_LOG_FILE, ERROR_LOG_FILE, RUN_LOG_FILE, RotatingLogFile},
    schema::{LogEvent, LogKind, LogLevel, PayloadPolicy},
};

pub(crate) struct ObservabilityGuard {
    sinks: Arc<Mutex<ObservabilitySinks>>,
}

impl ObservabilityGuard {
    pub(crate) fn new(context: ObservabilityContext, sinks: ObservabilitySinks) -> Self {
        let sinks = Arc::new(Mutex::new(sinks));
        let _ = GLOBAL_OBSERVABILITY.set(GlobalObservability {
            context,
            sinks: sinks.clone(),
        });
        Self { sinks }
    }

    pub(crate) fn write_event(&mut self, event: &LogEvent) -> SinkWriteSummary {
        self.sinks
            .lock()
            .map(|mut sinks| sinks.write_event(event))
            .unwrap_or_default()
    }

    pub(crate) fn flush(&mut self) {
        if let Ok(mut sinks) = self.sinks.lock() {
            sinks.flush();
        }
    }
}

impl Drop for ObservabilityGuard {
    fn drop(&mut self) {
        self.flush();
    }
}

struct GlobalObservability {
    context: ObservabilityContext,
    sinks: Arc<Mutex<ObservabilitySinks>>,
}

static GLOBAL_OBSERVABILITY: OnceLock<GlobalObservability> = OnceLock::new();

pub(crate) fn global_context() -> Option<ObservabilityContext> {
    GLOBAL_OBSERVABILITY
        .get()
        .map(|observability| observability.context.clone())
}

pub(crate) fn emit_global_event(event: &LogEvent) -> SinkWriteSummary {
    GLOBAL_OBSERVABILITY
        .get()
        .and_then(|observability| {
            observability
                .sinks
                .lock()
                .ok()
                .map(|mut sinks| sinks.write_event(event))
        })
        .unwrap_or_default()
}

pub(crate) struct ObservabilitySinks {
    config: LoggingConfig,
    console: Box<dyn Write + Send>,
    files: FileSinks,
    storage_diagnostic_emitted: bool,
}

impl ObservabilitySinks {
    pub(crate) fn new(config: LoggingConfig) -> Self {
        Self::new_with_console(config, Box::new(io::stderr()))
    }

    pub(crate) fn new_with_console(config: LoggingConfig, console: Box<dyn Write + Send>) -> Self {
        let now = Utc::now();
        let mut sinks = Self {
            files: FileSinks::open(&config, now),
            config,
            console,
            storage_diagnostic_emitted: false,
        };

        if let Some(message) = sinks.files.init_error.clone() {
            sinks.emit_storage_diagnostic(&message);
        }

        sinks
    }

    pub(crate) fn write_event(&mut self, event: &LogEvent) -> SinkWriteSummary {
        let mut event = sanitize_event(event);
        apply_payload_policy(&mut event, self.config.payloads);
        let mut summary = SinkWriteSummary::default();

        if !self.level_enabled(&event) {
            return summary;
        }

        if self.config.targets.contains(&LogTarget::Console) && route_to_console(&event) {
            let line = match self.config.format {
                ConsoleFormat::Compact => compact_console_line(&event),
                ConsoleFormat::Json => serde_json::to_string(&event).unwrap_or_else(|error| {
                    format!(
                        "{{\"timestamp_utc\":\"{}\",\"level\":\"error\",\"kind\":\"storage\",\"event\":\"console.serialize_failed\",\"message\":\"{}\"}}",
                        Utc::now().to_rfc3339(),
                        error
                    )
                }),
            };
            if writeln!(self.console, "{line}").is_ok() {
                summary.console = true;
            }
        }

        let ndjson = match serde_json::to_string(&event) {
            Ok(value) => value,
            Err(error) => {
                self.emit_storage_diagnostic(&format!("failed to serialize log event: {error}"));
                return summary;
            }
        };
        let now = Utc::now();

        if self.config.targets.contains(&LogTarget::File) && route_to_run_file(&event) {
            summary.run_file = self.write_file(LogFileRoute::Run, &ndjson, now);
        }
        if self.config.targets.contains(&LogTarget::ErrorFile) && route_to_error_file(&event) {
            summary.error_file = self.write_file(LogFileRoute::Error, &ndjson, now);
        }
        if self.config.targets.contains(&LogTarget::AuditFile) && route_to_audit_file(&event) {
            summary.audit_file = self.write_file(LogFileRoute::Audit, &ndjson, now);
        }

        summary
    }

    pub(crate) fn flush(&mut self) {
        let _ = self.console.flush();
    }

    fn level_enabled(&self, event: &LogEvent) -> bool {
        event.level >= effective_level(&self.config, &event.target)
    }

    fn write_file(&mut self, route: LogFileRoute, line: &str, now: DateTime<Utc>) -> bool {
        let result = match route {
            LogFileRoute::Run => self.files.run.as_mut(),
            LogFileRoute::Error => self.files.error.as_mut(),
            LogFileRoute::Audit => self.files.audit.as_mut(),
        }
        .map(|file| file.write_line(line, now));

        match result {
            Some(Ok(())) => true,
            Some(Err(error)) => {
                self.emit_storage_diagnostic(&format!("failed to write log file: {error}"));
                false
            }
            None => {
                self.emit_storage_diagnostic("log file target is unavailable");
                false
            }
        }
    }

    fn emit_storage_diagnostic(&mut self, message: &str) {
        if !self.config.targets.contains(&LogTarget::Console) {
            return;
        }
        if self.storage_diagnostic_emitted {
            return;
        }
        self.storage_diagnostic_emitted = true;
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let _ = writeln!(
            self.console,
            "{timestamp} WARN storage logging degraded message=\"{}\"",
            escape_compact_value(message)
        );
        let _ = self.console.flush();
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SinkWriteSummary {
    pub console: bool,
    pub run_file: bool,
    pub error_file: bool,
    pub audit_file: bool,
}

#[derive(Debug)]
struct FileSinks {
    run: Option<RotatingLogFile>,
    error: Option<RotatingLogFile>,
    audit: Option<RotatingLogFile>,
    init_error: Option<String>,
}

impl FileSinks {
    fn open(config: &LoggingConfig, now: DateTime<Utc>) -> Self {
        let mut init_error = None;
        let run = if config.targets.contains(&LogTarget::File) {
            open_file(config, RUN_LOG_FILE, now, &mut init_error)
        } else {
            None
        };
        let error = if config.targets.contains(&LogTarget::ErrorFile) {
            open_file(config, ERROR_LOG_FILE, now, &mut init_error)
        } else {
            None
        };
        let audit = if config.targets.contains(&LogTarget::AuditFile) {
            open_file(config, AUDIT_LOG_FILE, now, &mut init_error)
        } else {
            None
        };

        Self {
            run,
            error,
            audit,
            init_error,
        }
    }
}

fn open_file(
    config: &LoggingConfig,
    active_name: &'static str,
    now: DateTime<Utc>,
    init_error: &mut Option<String>,
) -> Option<RotatingLogFile> {
    match RotatingLogFile::open(
        config.dir.clone(),
        active_name,
        config.rotation.clone(),
        config.retention.clone(),
        config.compression,
        now,
    ) {
        Ok(file) => Some(file),
        Err(error) => {
            if init_error.is_none() {
                *init_error = Some(format!(
                    "failed to initialize log file {} in {}: {error}",
                    active_name,
                    config.dir.display()
                ));
            }
            None
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LogFileRoute {
    Run,
    Error,
    Audit,
}

fn route_to_console(event: &LogEvent) -> bool {
    event.level >= LogLevel::Warn
        || matches!(
            event.kind,
            LogKind::Lifecycle | LogKind::Config | LogKind::Cli | LogKind::Storage | LogKind::Audit
        )
}

fn route_to_run_file(event: &LogEvent) -> bool {
    !matches!(event.kind, LogKind::Audit)
}

fn route_to_error_file(event: &LogEvent) -> bool {
    event.level >= LogLevel::Error || matches!(event.kind, LogKind::Panic)
}

fn route_to_audit_file(event: &LogEvent) -> bool {
    matches!(event.kind, LogKind::Audit | LogKind::Security)
}

fn apply_payload_policy(event: &mut LogEvent, configured: PayloadPolicy) {
    let effective = least_permissive_payload_policy(configured, event.payload_policy);
    if effective == PayloadPolicy::None
        || (event.payload_policy == PayloadPolicy::SanitizedArgs
            && effective != PayloadPolicy::SanitizedArgs)
    {
        event.fields.clear();
    }
    event.payload_policy = effective;
}

fn least_permissive_payload_policy(
    configured: PayloadPolicy,
    event_policy: PayloadPolicy,
) -> PayloadPolicy {
    if payload_policy_rank(configured) <= payload_policy_rank(event_policy) {
        configured
    } else {
        event_policy
    }
}

fn payload_policy_rank(policy: PayloadPolicy) -> u8 {
    match policy {
        PayloadPolicy::None => 0,
        PayloadPolicy::Metadata => 1,
        PayloadPolicy::SanitizedArgs => 2,
    }
}

fn effective_level(config: &LoggingConfig, target: &str) -> LogLevel {
    let Some(filter) = config.filter.as_deref() else {
        return config.level;
    };

    let mut best_match = None::<(usize, LogLevel)>;
    for directive in filter
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let (directive_target, level) = directive
            .split_once('=')
            .map(|(target, level)| (target.trim(), level.trim()))
            .unwrap_or(("", directive));
        let Some(level) = parse_filter_level(level) else {
            continue;
        };
        if !filter_target_matches(directive_target, target) {
            continue;
        }

        let match_len = directive_target.len();
        if best_match.is_none_or(|(best_len, _)| match_len >= best_len) {
            best_match = Some((match_len, level));
        }
    }

    best_match.map(|(_, level)| level).unwrap_or(config.level)
}

fn filter_target_matches(filter_target: &str, event_target: &str) -> bool {
    filter_target.is_empty()
        || event_target == filter_target
        || event_target
            .strip_prefix(filter_target)
            .is_some_and(|rest| rest.starts_with("::"))
}

fn parse_filter_level(value: &str) -> Option<LogLevel> {
    match value.trim().to_ascii_lowercase().as_str() {
        "trace" => Some(LogLevel::Trace),
        "debug" => Some(LogLevel::Debug),
        "info" => Some(LogLevel::Info),
        "warn" | "warning" => Some(LogLevel::Warn),
        "error" => Some(LogLevel::Error),
        _ => None,
    }
}

fn compact_console_line(event: &LogEvent) -> String {
    let timestamp = DateTime::parse_from_rfc3339(&event.timestamp_utc)
        .map(|timestamp| {
            timestamp
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string()
        })
        .unwrap_or_else(|_| Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string());
    let mut line = format!(
        "{} {} {} {}",
        timestamp,
        level_label(event.level),
        kind_label(event.kind),
        event.message
    );

    if let Some(outcome) = event.outcome {
        line.push_str(&format!(
            " outcome={}",
            serde_json::to_value(outcome)
                .ok()
                .and_then(|value| value.as_str().map(ToString::to_string))
                .unwrap_or_else(|| "unknown".to_string())
        ));
    }
    if let Some(exit_code) = event.exit_code {
        line.push_str(&format!(" exit={exit_code}"));
    }
    if let Some(action) = &event.remediation_action {
        line.push_str(&format!(" next={}", escape_compact_value(action)));
    }
    if let Some(details) = &event.related_log_file {
        line.push_str(&format!(" details={}", escape_compact_value(details)));
    }

    line
}

fn level_label(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Trace => "TRACE",
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
    }
}

fn kind_label(kind: LogKind) -> &'static str {
    match kind {
        LogKind::Lifecycle => "lifecycle",
        LogKind::Config => "config",
        LogKind::Cli => "cli",
        LogKind::Mcp => "mcp",
        LogKind::Operation => "operation",
        LogKind::UpstreamHttp => "upstream_http",
        LogKind::Security => "security",
        LogKind::Audit => "audit",
        LogKind::Performance => "performance",
        LogKind::Diagnostic => "diagnostic",
        LogKind::Storage => "storage",
        LogKind::Panic => "panic",
    }
}

fn escape_compact_value(value: &str) -> String {
    if value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | '/' | ':' | '=')
    }) {
        return value.to_string();
    }
    format!("{value:?}")
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::{Arc, Mutex},
    };

    use crate::observability::{
        config::{
            ConsoleFormat, LogDirSource, LogProfile, RetentionConfig, RotationConfig, RotationKind,
        },
        context::CorrelationIds,
        schema::{LogEvent, LogKind, LogLevel, Outcome, PayloadPolicy, RuntimeMode},
    };
    use serde_json::json;

    use super::*;

    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn config(temp: &PathBuf) -> LoggingConfig {
        let mut config = LoggingConfig::for_profile(
            LogProfile::Production,
            temp.clone(),
            LogDirSource::ConfigFile,
        );
        config.format = ConsoleFormat::Compact;
        config.compression = false;
        config.rotation = RotationConfig {
            kinds: [RotationKind::Daily, RotationKind::Size]
                .into_iter()
                .collect(),
            max_bytes: 1024 * 1024,
        };
        config.retention = RetentionConfig {
            files: 20,
            days: 14,
        };
        config
    }

    fn event(level: LogLevel, kind: LogKind, name: &str, message: &str) -> LogEvent {
        LogEvent::new_at(
            "2026-06-22T06:03:12.481Z",
            level,
            kind,
            name,
            message,
            "workhub::test",
            RuntimeMode::Cli,
            "0.5.0",
            "run_1",
            7,
        )
    }

    #[test]
    fn sinks_route_events_to_run_error_and_audit_files() {
        let temp = tempfile::tempdir().unwrap();
        let console = SharedBuffer::default();
        let mut sinks = ObservabilitySinks::new_with_console(
            config(&temp.path().to_path_buf()),
            Box::new(console),
        );

        let lifecycle = event(
            LogLevel::Info,
            LogKind::Lifecycle,
            "process.started",
            "started token=secret-value",
        );
        let mut error = event(
            LogLevel::Error,
            LogKind::Operation,
            "operation.failed",
            "operation failed",
        )
        .with_correlation(CorrelationIds::for_command("cmd_1").with_operation_id("op_1"))
        .with_outcome(Outcome::Failed);
        error.error_message = Some("Authorization: Bearer secret-value".to_string());
        error.related_log_file = Some(RUN_LOG_FILE.to_string());
        let audit = event(
            LogLevel::Info,
            LogKind::Audit,
            "mutation.completed",
            "jira issue update completed",
        );
        let security = event(
            LogLevel::Warn,
            LogKind::Security,
            "request.rejected",
            "redirect rejected",
        );

        assert!(sinks.write_event(&lifecycle).run_file);
        let error_summary = sinks.write_event(&error);
        assert!(error_summary.run_file);
        assert!(error_summary.error_file);
        assert!(sinks.write_event(&audit).audit_file);
        let security_summary = sinks.write_event(&security);
        assert!(security_summary.run_file);
        assert!(security_summary.audit_file);

        let run_log = fs::read_to_string(temp.path().join(RUN_LOG_FILE)).unwrap();
        let error_log = fs::read_to_string(temp.path().join(ERROR_LOG_FILE)).unwrap();
        let audit_log = fs::read_to_string(temp.path().join(AUDIT_LOG_FILE)).unwrap();

        assert!(run_log.contains("process.started"));
        assert!(run_log.contains("operation.failed"));
        assert!(run_log.contains("request.rejected"));
        assert!(!run_log.contains("mutation.completed"));
        assert!(error_log.contains("operation.failed"));
        assert!(audit_log.contains("mutation.completed"));
        assert!(audit_log.contains("request.rejected"));
        assert!(!run_log.contains("secret-value"));
        assert!(!error_log.contains("secret-value"));
    }

    #[test]
    fn sinks_apply_payload_policy_before_writing_events() {
        let temp = tempfile::tempdir().unwrap();
        let console = SharedBuffer::default();
        let mut config = config(&temp.path().to_path_buf());
        config.targets = [LogTarget::File].into_iter().collect();
        config.payloads = PayloadPolicy::Metadata;
        let mut sinks = ObservabilitySinks::new_with_console(config, Box::new(console));

        let mut event = event(
            LogLevel::Info,
            LogKind::Mcp,
            "mcp.tool_call.started",
            "MCP tool call started",
        )
        .with_field(
            "arguments",
            json!({"issue_key": "ABC-123", "summary": "customer business text"}),
        );
        event.target = "workhub::mcp".to_string();
        event.tool_name = Some("jira_get_issue".to_string());
        event.payload_policy = PayloadPolicy::SanitizedArgs;

        assert!(sinks.write_event(&event).run_file);

        let run_log = fs::read_to_string(temp.path().join(RUN_LOG_FILE)).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(run_log.lines().next().unwrap()).unwrap();
        assert_eq!(value["payload_policy"], "metadata");
        assert_eq!(value["tool_name"], "jira_get_issue");
        assert!(value.get("arguments").is_none());
        assert!(!run_log.contains("ABC-123"));
        assert!(!run_log.contains("customer business text"));
    }

    #[test]
    fn sinks_clear_fields_when_payloads_are_disabled() {
        let temp = tempfile::tempdir().unwrap();
        let console = SharedBuffer::default();
        let mut config = config(&temp.path().to_path_buf());
        config.targets = [LogTarget::File].into_iter().collect();
        config.payloads = PayloadPolicy::None;
        let mut sinks = ObservabilitySinks::new_with_console(config, Box::new(console));

        let event = event(
            LogLevel::Info,
            LogKind::Lifecycle,
            "process.started",
            "workhub process started",
        )
        .with_field("log.dir", "/tmp/workhub/logs");

        assert!(sinks.write_event(&event).run_file);

        let run_log = fs::read_to_string(temp.path().join(RUN_LOG_FILE)).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(run_log.lines().next().unwrap()).unwrap();
        assert_eq!(value["payload_policy"], "none");
        assert!(value.get("log.dir").is_none());
    }

    #[test]
    fn sinks_apply_target_specific_log_filters() {
        let temp = tempfile::tempdir().unwrap();
        let console = SharedBuffer::default();
        let mut config = config(&temp.path().to_path_buf());
        config.targets = [LogTarget::File].into_iter().collect();
        config.level = LogLevel::Info;
        config.filter = Some("workhub::mcp=error,workhub::operations=debug".to_string());
        let mut sinks = ObservabilitySinks::new_with_console(config, Box::new(console));

        let mut mcp_info = event(
            LogLevel::Info,
            LogKind::Mcp,
            "mcp.tool_call.started",
            "MCP tool call started",
        );
        mcp_info.target = "workhub::mcp".to_string();
        let mut mcp_error = event(
            LogLevel::Error,
            LogKind::Mcp,
            "mcp.tool_call.failed",
            "MCP tool call failed",
        );
        mcp_error.target = "workhub::mcp".to_string();
        let mut operation_debug = event(
            LogLevel::Debug,
            LogKind::Operation,
            "operation.started",
            "operation started",
        );
        operation_debug.target = "workhub::operations".to_string();

        assert_eq!(sinks.write_event(&mcp_info), SinkWriteSummary::default());
        assert!(sinks.write_event(&mcp_error).run_file);
        assert!(sinks.write_event(&operation_debug).run_file);

        let run_log = fs::read_to_string(temp.path().join(RUN_LOG_FILE)).unwrap();
        assert!(!run_log.contains("mcp.tool_call.started"));
        assert!(run_log.contains("mcp.tool_call.failed"));
        assert!(run_log.contains("operation.started"));
    }

    #[test]
    fn storage_diagnostics_respect_console_target() {
        let temp = tempfile::tempdir().unwrap();
        let console = SharedBuffer::default();
        let handle = console.clone();
        let mut config = config(&temp.path().to_path_buf());
        config.targets = [LogTarget::File].into_iter().collect();
        let mut sinks = ObservabilitySinks::new_with_console(config, Box::new(console));

        sinks.emit_storage_diagnostic("log file target is unavailable");

        assert_eq!(handle.text(), "");
    }

    #[test]
    fn console_compact_writes_short_stderr_style_lines() {
        let temp = tempfile::tempdir().unwrap();
        let console = SharedBuffer::default();
        let handle = console.clone();
        let mut config = config(&temp.path().to_path_buf());
        config.targets = [LogTarget::Console].into_iter().collect();
        let mut sinks = ObservabilitySinks::new_with_console(config, Box::new(console));

        let summary = sinks.write_event(&event(
            LogLevel::Info,
            LogKind::Lifecycle,
            "process.started",
            "workhub process started",
        ));
        let output = handle.text();

        assert!(summary.console);
        assert!(output.contains(" INFO lifecycle workhub process started"));
        assert!(!output.trim_start().starts_with('{'));
        assert_eq!(output.chars().nth(4), Some('-'));
        assert_eq!(output.chars().nth(13), Some(':'));
    }

    #[test]
    fn file_init_failure_degrades_to_console_storage_diagnostic() {
        let temp = tempfile::tempdir().unwrap();
        let not_a_dir = temp.path().join("not-a-dir");
        fs::write(&not_a_dir, "file").unwrap();
        let console = SharedBuffer::default();
        let handle = console.clone();
        let mut sinks = ObservabilitySinks::new_with_console(config(&not_a_dir), Box::new(console));

        let summary = sinks.write_event(&event(
            LogLevel::Info,
            LogKind::Lifecycle,
            "process.started",
            "workhub process started",
        ));
        let output = handle.text();

        assert!(!summary.run_file);
        assert!(output.contains("WARN storage logging degraded"));
        assert!(!output.contains("stdout"));
    }
}
