use std::collections::BTreeMap;

use reqwest::{Method, StatusCode, Url};
use serde_json::json;

use crate::{
    observability::{
        context::{CorrelationIds, new_operation_id, new_upstream_request_id},
        rotation::RUN_LOG_FILE,
        schema::{ErrorDiagnosticEnvelope, LogEvent, LogKind, LogLevel, Outcome, PayloadPolicy},
        sinks::{emit_global_event, global_context},
    },
    operations::{OperationError, OperationErrorCategory},
    upstream::{error::UpstreamError, redaction::redact_text},
};

#[derive(Debug, Clone)]
pub(crate) struct OperationLogContext {
    pub operation_id: String,
    pub provider: Option<String>,
    pub operation: String,
    pub correlation: CorrelationIds,
}

impl OperationLogContext {
    pub(crate) fn new(operation: impl Into<String>) -> Self {
        Self {
            operation_id: new_operation_id(),
            provider: None,
            operation: operation.into(),
            correlation: CorrelationIds::default(),
        }
    }

    pub(crate) fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    pub(crate) fn with_correlation(mut self, correlation: CorrelationIds) -> Self {
        self.correlation = correlation;
        self
    }

    fn correlation_with_operation(&self) -> CorrelationIds {
        let mut correlation = self.correlation.clone();
        correlation.operation_id = Some(self.operation_id.clone());
        correlation
    }
}

pub(crate) fn emit_operation_started(operation: &OperationLogContext) {
    let Some(context) = global_context() else {
        return;
    };
    let mut event = LogEvent::new(
        LogLevel::Info,
        LogKind::Operation,
        "operation.started",
        "operation started",
        "workhub::operations",
        context.mode.clone(),
        context.version.clone(),
        context.run_id.clone(),
        context.pid,
    )
    .with_correlation(operation.correlation_with_operation())
    .with_outcome(Outcome::Started);
    event.provider = operation.provider.clone();
    event.operation = Some(operation.operation.clone());
    emit_global_event(&event);
}

pub(crate) fn emit_operation_completed(operation: &OperationLogContext, duration_ms: u64) {
    let Some(context) = global_context() else {
        return;
    };
    let mut event = LogEvent::new(
        LogLevel::Info,
        LogKind::Operation,
        "operation.completed",
        "operation completed",
        "workhub::operations",
        context.mode.clone(),
        context.version.clone(),
        context.run_id.clone(),
        context.pid,
    )
    .with_correlation(operation.correlation_with_operation())
    .with_outcome(Outcome::Succeeded)
    .with_duration_ms(duration_ms);
    event.provider = operation.provider.clone();
    event.operation = Some(operation.operation.clone());
    emit_global_event(&event);
}

pub(crate) fn emit_operation_failed(
    operation: &OperationLogContext,
    error: &OperationError,
    duration_ms: u64,
) {
    emit_operation_failure_envelope(
        operation,
        error.category,
        "operation_failed",
        &error.message,
        duration_ms,
        Some(error.exit_code()),
    );
}

pub(crate) fn emit_operation_failure_message(
    operation: &OperationLogContext,
    category: OperationErrorCategory,
    error_kind: impl Into<String>,
    error_message: impl Into<String>,
    duration_ms: u64,
) {
    emit_operation_failure_envelope(
        operation,
        category,
        &error_kind.into(),
        &error_message.into(),
        duration_ms,
        None,
    );
}

fn emit_operation_failure_envelope(
    operation: &OperationLogContext,
    category: OperationErrorCategory,
    error_kind: &str,
    error_message: &str,
    duration_ms: u64,
    exit_code: Option<i32>,
) {
    let Some(context) = global_context() else {
        return;
    };
    let envelope = ErrorDiagnosticEnvelope {
        timestamp_utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        kind: LogKind::Operation,
        event: "operation.failed".to_string(),
        message: "operation failed".to_string(),
        target: "workhub::operations".to_string(),
        mode: context.mode.clone(),
        version: context.version.clone(),
        run_id: context.run_id.clone(),
        pid: context.pid,
        correlation: operation.correlation_with_operation(),
        provider: operation.provider.clone(),
        operation: Some(operation.operation.clone()),
        tool_name: None,
        command_path: None,
        duration_ms: Some(duration_ms),
        exit_code,
        error_category: category,
        error_kind: error_kind.to_string(),
        error_message: redact_text(error_message),
        phase: "operation".to_string(),
        cause_summary: redact_text(error_message),
        cause_chain: vec![format!(
            "operation {}: {}",
            operation.operation,
            redact_text(error_message)
        )],
        impact: "operation failed before producing a successful result".to_string(),
        remediation_action: remediation_for_category(category).to_string(),
        remediation_evidence: format!(
            "provider={} operation={}",
            operation.provider.as_deref().unwrap_or("unknown"),
            operation.operation
        ),
        related_log_file: RUN_LOG_FILE.to_string(),
        related_line_hint: format!(
            "run_id={} operation_id={}",
            context.run_id, operation.operation_id
        ),
        support_bundle_hint: "workhub logs bundle --since 24h".to_string(),
        payload_policy: PayloadPolicy::Metadata,
        fields: BTreeMap::new(),
    };
    emit_global_event(&envelope.to_log_event());
}

#[derive(Debug, Clone)]
pub(crate) struct UpstreamRequestLogContext {
    pub upstream_request_id: String,
    pub method: String,
    pub host: Option<String>,
    pub path_template: String,
}

impl UpstreamRequestLogContext {
    pub(crate) fn from_url(method: &Method, url: &Url) -> Self {
        Self {
            upstream_request_id: new_upstream_request_id(),
            method: method.as_str().to_string(),
            host: url.host_str().map(ToString::to_string),
            path_template: url.path().to_string(),
        }
    }
}

pub(crate) fn emit_upstream_started(request: &UpstreamRequestLogContext) {
    let Some(context) = global_context() else {
        return;
    };
    let event = upstream_event(
        &context,
        request,
        LogLevel::Info,
        "upstream.request.started",
        "upstream request started",
        Outcome::Started,
        None,
        0,
    );
    emit_global_event(&event);
}

pub(crate) fn emit_upstream_completed(
    request: &UpstreamRequestLogContext,
    status: StatusCode,
    duration_ms: u64,
) {
    let Some(context) = global_context() else {
        return;
    };
    let event = upstream_event(
        &context,
        request,
        LogLevel::Info,
        "upstream.request.completed",
        "upstream request completed",
        Outcome::Succeeded,
        Some(status.as_u16()),
        duration_ms,
    );
    emit_global_event(&event);
}

pub(crate) fn emit_upstream_failed(
    request: &UpstreamRequestLogContext,
    error: &UpstreamError,
    duration_ms: u64,
) {
    let Some(context) = global_context() else {
        return;
    };
    let category = operation_category_for_upstream(error);
    let envelope = ErrorDiagnosticEnvelope {
        timestamp_utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        kind: LogKind::UpstreamHttp,
        event: "upstream.request.failed".to_string(),
        message: "upstream request failed".to_string(),
        target: "workhub::upstream".to_string(),
        mode: context.mode.clone(),
        version: context.version.clone(),
        run_id: context.run_id.clone(),
        pid: context.pid,
        correlation: CorrelationIds::default()
            .with_upstream_request_id(request.upstream_request_id.clone()),
        provider: None,
        operation: None,
        tool_name: None,
        command_path: None,
        duration_ms: Some(duration_ms),
        exit_code: None,
        error_category: category,
        error_kind: category.as_str().to_string(),
        error_message: error.to_string(),
        phase: "upstream_request".to_string(),
        cause_summary: error.to_string(),
        cause_chain: vec![format!(
            "upstream {} {} failed: {}",
            request.method, request.path_template, error
        )],
        impact: "upstream request failed before response data could be used".to_string(),
        remediation_action: remediation_for_category(category).to_string(),
        remediation_evidence: format!(
            "host={} path={}",
            request.host.as_deref().unwrap_or("unknown"),
            request.path_template
        ),
        related_log_file: RUN_LOG_FILE.to_string(),
        related_line_hint: format!(
            "run_id={} upstream_request_id={}",
            context.run_id, request.upstream_request_id
        ),
        support_bundle_hint: "workhub logs bundle --since 24h".to_string(),
        payload_policy: PayloadPolicy::Metadata,
        fields: upstream_fields(request, None),
    };
    emit_global_event(&envelope.to_log_event());
}

fn upstream_event(
    context: &crate::observability::context::ObservabilityContext,
    request: &UpstreamRequestLogContext,
    level: LogLevel,
    event_name: &str,
    message: &str,
    outcome: Outcome,
    status: Option<u16>,
    duration_ms: u64,
) -> LogEvent {
    let mut event = LogEvent::new(
        level,
        LogKind::UpstreamHttp,
        event_name,
        message,
        "workhub::upstream",
        context.mode.clone(),
        context.version.clone(),
        context.run_id.clone(),
        context.pid,
    )
    .with_correlation(
        CorrelationIds::default().with_upstream_request_id(request.upstream_request_id.clone()),
    )
    .with_outcome(outcome)
    .with_duration_ms(duration_ms);
    event.fields = upstream_fields(request, status);
    event
}

fn upstream_fields(
    request: &UpstreamRequestLogContext,
    status: Option<u16>,
) -> BTreeMap<String, serde_json::Value> {
    let mut fields = BTreeMap::from([
        ("http.method".to_string(), json!(request.method)),
        (
            "url.path_template".to_string(),
            json!(request.path_template),
        ),
    ]);
    if let Some(host) = &request.host {
        fields.insert("url.host".to_string(), json!(host));
    }
    if let Some(status) = status {
        fields.insert("http.status".to_string(), json!(status));
    }
    fields
}

pub(crate) fn emit_security_rejection(
    reason: impl Into<String>,
    policy: impl Into<String>,
    host: Option<String>,
    message: impl Into<String>,
) {
    let Some(context) = global_context() else {
        return;
    };
    let mut event = LogEvent::new(
        LogLevel::Warn,
        LogKind::Security,
        "request.rejected",
        message.into(),
        "workhub::upstream",
        context.mode.clone(),
        context.version.clone(),
        context.run_id.clone(),
        context.pid,
    )
    .with_outcome(Outcome::Blocked)
    .with_field("security.reason", reason.into())
    .with_field("policy", policy.into());
    if let Some(host) = host {
        event = event.with_field("url.host", host);
    }
    emit_global_event(&event);
}

fn operation_category_for_upstream(error: &UpstreamError) -> OperationErrorCategory {
    match error {
        UpstreamError::InvalidInput { .. } => OperationErrorCategory::InvalidInput,
        UpstreamError::InvalidBaseUrl { .. } => OperationErrorCategory::Config,
        UpstreamError::HttpStatus { .. } => OperationErrorCategory::HttpStatus,
        UpstreamError::Transport { .. } => OperationErrorCategory::Transport,
        UpstreamError::JsonDecode { .. } => OperationErrorCategory::JsonDecode,
        UpstreamError::UnexpectedShape { .. } => OperationErrorCategory::UnexpectedShape,
    }
}

fn remediation_for_category(category: OperationErrorCategory) -> &'static str {
    match category {
        OperationErrorCategory::InvalidInput => "check_input_arguments",
        OperationErrorCategory::Config
        | OperationErrorCategory::ServiceUnavailable
        | OperationErrorCategory::DisabledTool => "fix_configuration_or_tool_filters",
        OperationErrorCategory::HttpStatus => "verify_credentials_permissions_or_upstream_status",
        OperationErrorCategory::Transport => "check_network_proxy_or_tls",
        OperationErrorCategory::JsonDecode | OperationErrorCategory::UnexpectedShape => {
            "report_upstream_shape"
        }
        OperationErrorCategory::Business => "inspect_business_error",
    }
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;

    use super::*;

    #[test]
    fn upstream_context_drops_query_from_path_template() {
        let url = Url::parse("https://jira.example/rest/api/3/issue/ABC-1?token=secret").unwrap();
        let request = UpstreamRequestLogContext::from_url(&Method::GET, &url);

        assert_eq!(request.host.as_deref(), Some("jira.example"));
        assert_eq!(request.path_template, "/rest/api/3/issue/ABC-1");
    }

    #[test]
    fn upstream_fields_include_status_without_query_or_body() {
        let request = UpstreamRequestLogContext {
            upstream_request_id: "http_1".to_string(),
            method: "GET".to_string(),
            host: Some("jira.example".to_string()),
            path_template: "/rest/api/3/issue/ABC-1".to_string(),
        };
        let fields = upstream_fields(&request, Some(StatusCode::UNAUTHORIZED.as_u16()));

        assert_eq!(fields["http.method"], "GET");
        assert_eq!(fields["url.host"], "jira.example");
        assert_eq!(fields["url.path_template"], "/rest/api/3/issue/ABC-1");
        assert_eq!(fields["http.status"], 401);
    }
}
