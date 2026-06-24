use std::collections::BTreeMap;

use chrono::{SecondsFormat, Utc};
use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::{observability::context::CorrelationIds, operations::error::OperationErrorCategory};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LogKind {
    Lifecycle,
    Config,
    Cli,
    Mcp,
    Operation,
    UpstreamHttp,
    Security,
    Audit,
    Performance,
    Diagnostic,
    Storage,
    Panic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Started,
    Succeeded,
    Failed,
    Blocked,
    Degraded,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PayloadPolicy {
    None,
    Metadata,
    SanitizedArgs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeMode {
    Stdio,
    StreamHttp,
    Cli,
    Logs,
    Version,
    Test,
    Unknown(String),
}

impl RuntimeMode {
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Stdio => "stdio",
            Self::StreamHttp => "streamhttp",
            Self::Cli => "cli",
            Self::Logs => "logs",
            Self::Version => "version",
            Self::Test => "test",
            Self::Unknown(value) => value.as_str(),
        }
    }
}

impl Serialize for RuntimeMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct LogEvent {
    pub timestamp_utc: String,
    pub level: LogLevel,
    pub kind: LogKind,
    pub event: String,
    pub message: String,
    pub target: String,
    pub mode: RuntimeMode,
    pub version: String,
    pub run_id: String,
    pub pid: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(rename = "error.category", skip_serializing_if = "Option::is_none")]
    pub error_category: Option<OperationErrorCategory>,
    #[serde(rename = "error.kind", skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(rename = "error.message", skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(rename = "cause.summary", skip_serializing_if = "Option::is_none")]
    pub cause_summary: Option<String>,
    #[serde(rename = "cause.chain", skip_serializing_if = "Vec::is_empty")]
    pub cause_chain: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub impact: Option<String>,
    #[serde(rename = "remediation.action", skip_serializing_if = "Option::is_none")]
    pub remediation_action: Option<String>,
    #[serde(
        rename = "remediation.evidence",
        skip_serializing_if = "Option::is_none"
    )]
    pub remediation_evidence: Option<String>,
    #[serde(rename = "related.log_file", skip_serializing_if = "Option::is_none")]
    pub related_log_file: Option<String>,
    #[serde(rename = "related.line_hint", skip_serializing_if = "Option::is_none")]
    pub related_line_hint: Option<String>,
    #[serde(
        rename = "support_bundle.hint",
        skip_serializing_if = "Option::is_none"
    )]
    pub support_bundle_hint: Option<String>,
    pub payload_policy: PayloadPolicy,
    #[serde(rename = "redaction.applied")]
    pub redaction_applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

impl LogEvent {
    pub(crate) fn new(
        level: LogLevel,
        kind: LogKind,
        event: impl Into<String>,
        message: impl Into<String>,
        target: impl Into<String>,
        mode: RuntimeMode,
        version: impl Into<String>,
        run_id: impl Into<String>,
        pid: u32,
    ) -> Self {
        Self::new_at(
            Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            level,
            kind,
            event,
            message,
            target,
            mode,
            version,
            run_id,
            pid,
        )
    }

    pub(crate) fn new_at(
        timestamp_utc: impl Into<String>,
        level: LogLevel,
        kind: LogKind,
        event: impl Into<String>,
        message: impl Into<String>,
        target: impl Into<String>,
        mode: RuntimeMode,
        version: impl Into<String>,
        run_id: impl Into<String>,
        pid: u32,
    ) -> Self {
        Self {
            timestamp_utc: timestamp_utc.into(),
            level,
            kind,
            event: event.into(),
            message: message.into(),
            target: target.into(),
            mode,
            version: version.into(),
            run_id: run_id.into(),
            pid,
            command_id: None,
            tool_call_id: None,
            session_id: None,
            request_id: None,
            upstream_request_id: None,
            operation_id: None,
            outcome: None,
            duration_ms: None,
            exit_code: None,
            error_category: None,
            error_kind: None,
            error_message: None,
            provider: None,
            operation: None,
            tool_name: None,
            command_path: None,
            phase: None,
            cause_summary: None,
            cause_chain: Vec::new(),
            impact: None,
            remediation_action: None,
            remediation_evidence: None,
            related_log_file: None,
            related_line_hint: None,
            support_bundle_hint: None,
            payload_policy: PayloadPolicy::Metadata,
            redaction_applied: false,
            truncated: None,
            fields: BTreeMap::new(),
        }
    }

    pub(crate) fn with_correlation(mut self, ids: CorrelationIds) -> Self {
        self.command_id = ids.command_id;
        self.tool_call_id = ids.tool_call_id;
        self.session_id = ids.session_id;
        self.request_id = ids.request_id;
        self.upstream_request_id = ids.upstream_request_id;
        self.operation_id = ids.operation_id;
        self
    }

    pub(crate) fn with_outcome(mut self, outcome: Outcome) -> Self {
        self.outcome = Some(outcome);
        self
    }

    pub(crate) fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    pub(crate) fn with_field(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ErrorDiagnosticEnvelope {
    pub timestamp_utc: String,
    pub kind: LogKind,
    pub event: String,
    pub message: String,
    pub target: String,
    pub mode: RuntimeMode,
    pub version: String,
    pub run_id: String,
    pub pid: u32,
    pub correlation: CorrelationIds,
    pub provider: Option<String>,
    pub operation: Option<String>,
    pub tool_name: Option<String>,
    pub command_path: Option<String>,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub error_category: OperationErrorCategory,
    pub error_kind: String,
    pub error_message: String,
    pub phase: String,
    pub cause_summary: String,
    pub cause_chain: Vec<String>,
    pub impact: String,
    pub remediation_action: String,
    pub remediation_evidence: String,
    pub related_log_file: String,
    pub related_line_hint: String,
    pub support_bundle_hint: String,
    pub payload_policy: PayloadPolicy,
    pub fields: BTreeMap<String, Value>,
}

impl ErrorDiagnosticEnvelope {
    pub(crate) fn to_log_event(&self) -> LogEvent {
        let mut event = LogEvent::new_at(
            self.timestamp_utc.clone(),
            LogLevel::Error,
            self.kind,
            self.event.clone(),
            self.message.clone(),
            self.target.clone(),
            self.mode.clone(),
            self.version.clone(),
            self.run_id.clone(),
            self.pid,
        )
        .with_correlation(self.correlation.clone());

        event.provider = self.provider.clone();
        event.operation = self.operation.clone();
        event.tool_name = self.tool_name.clone();
        event.command_path = self.command_path.clone();
        event.duration_ms = self.duration_ms;
        event.exit_code = self.exit_code;
        event.error_category = Some(self.error_category);
        event.error_kind = Some(self.error_kind.clone());
        event.error_message = Some(self.error_message.clone());
        event.phase = Some(self.phase.clone());
        event.cause_summary = Some(self.cause_summary.clone());
        event.cause_chain = self.cause_chain.clone();
        event.impact = Some(self.impact.clone());
        event.remediation_action = Some(self.remediation_action.clone());
        event.remediation_evidence = Some(self.remediation_evidence.clone());
        event.related_log_file = Some(self.related_log_file.clone());
        event.related_line_hint = Some(self.related_line_hint.clone());
        event.support_bundle_hint = Some(self.support_bundle_hint.clone());
        event.payload_policy = self.payload_policy;
        event.outcome = Some(Outcome::Failed);
        event.fields = self.fields.clone();
        event
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RuntimeStateEvent {
    pub timestamp_utc: String,
    pub level: LogLevel,
    pub kind: LogKind,
    pub event: String,
    pub message: String,
    pub target: String,
    pub mode: RuntimeMode,
    pub version: String,
    pub run_id: String,
    pub pid: u32,
    pub outcome: Outcome,
    pub duration_ms: Option<u64>,
    pub fields: BTreeMap<String, Value>,
}

impl RuntimeStateEvent {
    pub(crate) fn to_log_event(&self) -> LogEvent {
        let mut event = LogEvent::new_at(
            self.timestamp_utc.clone(),
            self.level,
            self.kind,
            self.event.clone(),
            self.message.clone(),
            self.target.clone(),
            self.mode.clone(),
            self.version.clone(),
            self.run_id.clone(),
            self.pid,
        )
        .with_outcome(self.outcome);
        event.duration_ms = self.duration_ms;
        event.fields = self.fields.clone();
        event
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ConsoleSummaryEvent {
    pub timestamp_utc: String,
    pub level: LogLevel,
    pub kind: LogKind,
    pub event: String,
    pub message: String,
    pub target: String,
    pub mode: RuntimeMode,
    pub version: String,
    pub run_id: String,
    pub pid: u32,
    pub next_action: Option<String>,
    pub details_file: Option<String>,
}

impl ConsoleSummaryEvent {
    pub(crate) fn to_log_event(&self) -> LogEvent {
        let mut event = LogEvent::new_at(
            self.timestamp_utc.clone(),
            self.level,
            self.kind,
            self.event.clone(),
            self.message.clone(),
            self.target.clone(),
            self.mode.clone(),
            self.version.clone(),
            self.run_id.clone(),
            self.pid,
        );
        if let Some(next_action) = &self.next_action {
            event.fields.insert(
                "console.next_action".to_string(),
                Value::String(next_action.clone()),
            );
        }
        if let Some(details_file) = &self.details_file {
            event.fields.insert(
                "console.details_file".to_string(),
                Value::String(details_file.clone()),
            );
        }
        event
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn test_event() -> LogEvent {
        LogEvent::new_at(
            "2026-06-22T06:03:12.481Z",
            LogLevel::Info,
            LogKind::Lifecycle,
            "process.started",
            "workhub process started",
            "workhub::main",
            RuntimeMode::StreamHttp,
            "0.5.0",
            "run_01",
            43120,
        )
    }

    #[test]
    fn log_event_serializes_stable_ndjson_fields() {
        let event = test_event()
            .with_outcome(Outcome::Started)
            .with_duration_ms(0)
            .with_field("log.dir", "/tmp/workhub/logs");

        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["timestamp_utc"], "2026-06-22T06:03:12.481Z");
        assert_eq!(value["level"], "info");
        assert_eq!(value["kind"], "lifecycle");
        assert_eq!(value["event"], "process.started");
        assert_eq!(value["mode"], "streamhttp");
        assert_eq!(value["outcome"], "started");
        assert_eq!(value["duration_ms"], 0);
        assert_eq!(value["payload_policy"], "metadata");
        assert_eq!(value["redaction.applied"], false);
        assert_eq!(value["log.dir"], "/tmp/workhub/logs");
    }

    #[test]
    fn error_diagnostic_envelope_converts_to_log_event() {
        let envelope = ErrorDiagnosticEnvelope {
            timestamp_utc: "2026-06-22T06:04:02.118Z".to_string(),
            kind: LogKind::Operation,
            event: "operation.failed".to_string(),
            message: "jira issue get failed".to_string(),
            target: "workhub::operations".to_string(),
            mode: RuntimeMode::Cli,
            version: "0.5.0".to_string(),
            run_id: "run_err".to_string(),
            pid: 42,
            correlation: CorrelationIds::for_command("cmd_1").with_operation_id("op_1"),
            provider: Some("jira".to_string()),
            operation: Some("issue.get".to_string()),
            tool_name: None,
            command_path: Some("jira issue get".to_string()),
            duration_ms: Some(842),
            exit_code: Some(4),
            error_category: OperationErrorCategory::HttpStatus,
            error_kind: "upstream_auth_failed".to_string(),
            error_message: "Upstream error category=http_status status=401".to_string(),
            phase: "upstream_request".to_string(),
            cause_summary: "Jira returned HTTP 401 for issue lookup".to_string(),
            cause_chain: vec![
                "cli command jira issue get".to_string(),
                "operation jira.issue.get".to_string(),
            ],
            impact: "command failed before reading issue data".to_string(),
            remediation_action: "verify_credentials".to_string(),
            remediation_evidence: "check JIRA_URL, JIRA_USERNAME and JIRA_API_TOKEN".to_string(),
            related_log_file: "workhub.log".to_string(),
            related_line_hint: "run_id=run_err operation_id=op_1".to_string(),
            support_bundle_hint: "workhub logs bundle --since 24h".to_string(),
            payload_policy: PayloadPolicy::Metadata,
            fields: BTreeMap::from([("http.status".to_string(), json!(401))]),
        };

        let value = serde_json::to_value(envelope.to_log_event()).unwrap();

        assert_eq!(value["level"], "error");
        assert_eq!(value["kind"], "operation");
        assert_eq!(value["outcome"], "failed");
        assert_eq!(value["command_id"], "cmd_1");
        assert_eq!(value["operation_id"], "op_1");
        assert_eq!(value["error.category"], "http_status");
        assert_eq!(value["error.kind"], "upstream_auth_failed");
        assert_eq!(value["remediation.action"], "verify_credentials");
        assert_eq!(value["related.log_file"], "workhub.log");
        assert_eq!(value["http.status"], 401);
    }

    #[test]
    fn runtime_and_console_events_convert_to_log_event() {
        let runtime = RuntimeStateEvent {
            timestamp_utc: "2026-06-22T06:03:12.690Z".to_string(),
            level: LogLevel::Info,
            kind: LogKind::Config,
            event: "runtime_config.loaded".to_string(),
            message: "runtime configuration loaded".to_string(),
            target: "workhub::config".to_string(),
            mode: RuntimeMode::Stdio,
            version: "0.5.0".to_string(),
            run_id: "run_1".to_string(),
            pid: 7,
            outcome: Outcome::Succeeded,
            duration_ms: Some(42),
            fields: BTreeMap::from([("services.jira".to_string(), json!(true))]),
        };
        let console = ConsoleSummaryEvent {
            timestamp_utc: "2026-06-22T06:04:02.118Z".to_string(),
            level: LogLevel::Error,
            kind: LogKind::Operation,
            event: "operation.failed.console".to_string(),
            message: "operation failed".to_string(),
            target: "workhub::console".to_string(),
            mode: RuntimeMode::Cli,
            version: "0.5.0".to_string(),
            run_id: "run_1".to_string(),
            pid: 7,
            next_action: Some("verify_credentials".to_string()),
            details_file: Some("workhub-error.log".to_string()),
        };

        let runtime_value = serde_json::to_value(runtime.to_log_event()).unwrap();
        let console_value = serde_json::to_value(console.to_log_event()).unwrap();

        assert_eq!(runtime_value["kind"], "config");
        assert_eq!(runtime_value["outcome"], "succeeded");
        assert_eq!(runtime_value["services.jira"], true);
        assert_eq!(console_value["console.next_action"], "verify_credentials");
        assert_eq!(console_value["console.details_file"], "workhub-error.log");
    }
}
