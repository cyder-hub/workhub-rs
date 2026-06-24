use std::{sync::Arc, time::Instant};

#[cfg(test)]
use crate::upstream::error::UpstreamError;
use crate::{
    confluence::client::ConfluenceClient,
    context::AppContext,
    gitlab::client::GitlabClient,
    mcp_errors::upstream_error,
    observability::{
        context::{CorrelationIds, new_tool_call_id},
        events::{
            OperationLogContext, emit_operation_completed, emit_operation_failure_message,
            emit_operation_started, emit_security_rejection,
        },
        schema::{ErrorDiagnosticEnvelope, LogEvent, LogKind, LogLevel, Outcome, PayloadPolicy},
        sinks::{emit_global_event, global_context},
    },
    operations::error::OperationErrorCategory,
    tool_registry,
    upstream::redaction::redact_text,
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::tool::ToolCallContext,
    model::{
        CallToolRequestParams, CallToolResult, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    tool_handler,
};

mod confluence_handlers;
mod gitlab_handlers;
mod jira_handlers;
#[cfg(test)]
#[allow(dead_code)]
mod jira_payloads;
mod schema;
mod tool_log;

use schema::{sanitize_tool_for_clients, sanitize_tools_for_clients};
use tool_log::sanitize_tool_log_arguments;
#[cfg(test)]
use tool_log::{TOOL_LOG_MAX_STRING_CHARS, TOOL_LOG_REDACTED, TOOL_LOG_TRUNCATED};

pub const SERVER_NAME: &str = "workhub-rs";

#[derive(Clone)]
pub struct WorkhubMcpServer {
    context: Arc<AppContext>,
    tool_router: ToolRouter<Self>,
}

impl WorkhubMcpServer {
    pub fn new(context: Arc<AppContext>) -> Self {
        Self {
            context,
            tool_router: Self::tool_router(),
        }
    }

    fn tool_router() -> ToolRouter<Self> {
        Self::jira_tool_router() + Self::confluence_tool_router() + Self::gitlab_tool_router()
    }

    fn current_tools_result(&self) -> ListToolsResult {
        ListToolsResult {
            tools: sanitize_tools_for_clients(
                self.filtered_tools_from(self.tool_router.list_all()),
            ),
            ..Default::default()
        }
    }

    fn filtered_tools_from<I>(&self, tools: I) -> Vec<Tool>
    where
        I: IntoIterator<Item = Tool>,
    {
        tool_registry::visible_tools(tools, &self.context)
    }

    fn guard_registered_tool_call(&self, name: &str) -> Result<(), ErrorData> {
        if !self.tool_router.has_route(name) {
            return Err(ErrorData::invalid_params("tool not available", None));
        }

        tool_registry::guard_tool_call(name, &self.context)
    }

    #[allow(dead_code)]
    fn confluence_client(&self) -> Result<ConfluenceClient, ErrorData> {
        let Some(config) = self.context.confluence_config() else {
            return Err(ErrorData::invalid_params(
                "Confluence is not configured",
                None,
            ));
        };

        ConfluenceClient::new(config.clone()).map_err(upstream_error)
    }

    #[allow(dead_code)]
    fn gitlab_client(&self) -> Result<GitlabClient, ErrorData> {
        let Some(config) = self.context.gitlab_config() else {
            return Err(ErrorData::invalid_params("GitLab is not configured", None));
        };

        GitlabClient::new(config.clone()).map_err(upstream_error)
    }

    #[cfg(test)]
    fn guard_tool_call_with_metadata<F>(
        &self,
        name: &str,
        route_exists: bool,
        metadata_for: F,
    ) -> Result<(), ErrorData>
    where
        F: Fn(&str) -> Option<tool_registry::ToolMetadata>,
    {
        if !route_exists {
            return Err(ErrorData::invalid_params("tool not available", None));
        }

        tool_registry::guard_tool_call_with_metadata(name, &self.context, metadata_for)
    }

    #[cfg(test)]
    fn filtered_tools_from_with_metadata<I, F>(&self, tools: I, metadata_for: F) -> Vec<Tool>
    where
        I: IntoIterator<Item = Tool>,
        F: Fn(&str) -> Option<tool_registry::ToolMetadata>,
    {
        tool_registry::visible_tools_with_metadata(tools, &self.context, metadata_for)
    }
}

impl Default for WorkhubMcpServer {
    fn default() -> Self {
        Self::new(Arc::new(AppContext::default()))
    }
}
#[cfg(test)]
#[allow(dead_code)]
fn required_non_empty_arg(value: String, field_name: &'static str) -> Result<String, ErrorData> {
    let value = value.trim();
    if value.is_empty() {
        Err(upstream_error(UpstreamError::invalid_input(format!(
            "{field_name} must not be empty"
        ))))
    } else {
        Ok(value.to_string())
    }
}

#[cfg(test)]
#[allow(dead_code)]
fn optional_non_empty_arg(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn emit_mcp_tool_started(tool_call_id: &str, tool_name: &str, arguments: serde_json::Value) {
    let Some(context) = global_context() else {
        return;
    };
    let mut event = LogEvent::new(
        LogLevel::Info,
        LogKind::Mcp,
        "mcp.tool_call.started",
        "MCP tool call started",
        "workhub::mcp",
        context.mode.clone(),
        context.version.clone(),
        context.run_id.clone(),
        context.pid,
    )
    .with_correlation(CorrelationIds::for_tool_call(tool_call_id.to_string()))
    .with_outcome(Outcome::Started)
    .with_field("arguments", arguments);
    event.tool_name = Some(tool_name.to_string());
    event.payload_policy = PayloadPolicy::SanitizedArgs;
    emit_global_event(&event);
}

fn provider_from_tool_name(tool_name: &str) -> String {
    tool_name
        .split_once('_')
        .map(|(provider, _)| provider)
        .unwrap_or("unknown")
        .to_string()
}

fn emit_mcp_tool_completed(tool_call_id: &str, tool_name: &str, elapsed_ms: u128) {
    let Some(context) = global_context() else {
        return;
    };
    let mut event = LogEvent::new(
        LogLevel::Info,
        LogKind::Mcp,
        "mcp.tool_call.completed",
        "MCP tool call completed",
        "workhub::mcp",
        context.mode.clone(),
        context.version.clone(),
        context.run_id.clone(),
        context.pid,
    )
    .with_correlation(CorrelationIds::for_tool_call(tool_call_id.to_string()))
    .with_outcome(Outcome::Succeeded)
    .with_duration_ms(elapsed_ms.try_into().unwrap_or(u64::MAX));
    event.tool_name = Some(tool_name.to_string());
    emit_global_event(&event);
}

fn emit_mcp_tool_failed(tool_call_id: &str, tool_name: &str, error: &ErrorData, elapsed_ms: u128) {
    let Some(context) = global_context() else {
        return;
    };
    let error_message = redact_text(error.message.as_ref());
    let envelope = ErrorDiagnosticEnvelope {
        timestamp_utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        kind: LogKind::Mcp,
        event: "mcp.tool_call.failed".to_string(),
        message: "MCP tool call failed".to_string(),
        target: "workhub::mcp".to_string(),
        mode: context.mode.clone(),
        version: context.version.clone(),
        run_id: context.run_id.clone(),
        pid: context.pid,
        correlation: CorrelationIds::for_tool_call(tool_call_id.to_string()),
        provider: None,
        operation: None,
        tool_name: Some(tool_name.to_string()),
        command_path: None,
        duration_ms: Some(elapsed_ms.try_into().unwrap_or(u64::MAX)),
        exit_code: None,
        error_category: OperationErrorCategory::Business,
        error_kind: format!("mcp_error_{}", error.code.0),
        error_message: error_message.clone(),
        phase: "mcp_tool_call".to_string(),
        cause_summary: error_message.clone(),
        cause_chain: vec![format!("tool {tool_name}: {error_message}")],
        impact: "tool call failed before returning a successful MCP result".to_string(),
        remediation_action: "inspect_tool_arguments_or_provider_configuration".to_string(),
        remediation_evidence: format!("tool_name={tool_name} error_code={}", error.code.0),
        related_log_file: crate::observability::rotation::RUN_LOG_FILE.to_string(),
        related_line_hint: format!("run_id={} tool_call_id={tool_call_id}", context.run_id),
        support_bundle_hint: "workhub logs bundle --since 24h".to_string(),
        payload_policy: PayloadPolicy::Metadata,
        fields: Default::default(),
    };
    emit_global_event(&envelope.to_log_event());
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for WorkhubMcpServer {
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let tool_name = request.name.to_string();
        let tool_call_id = new_tool_call_id();
        let operation = OperationLogContext::new(tool_name.as_str())
            .with_provider(provider_from_tool_name(tool_name.as_str()))
            .with_correlation(CorrelationIds::for_tool_call(tool_call_id.clone()));
        let sanitized_arguments = sanitize_tool_log_arguments(request.arguments.as_ref());
        let started_at = Instant::now();

        emit_mcp_tool_started(&tool_call_id, &tool_name, sanitized_arguments);
        emit_operation_started(&operation);

        let result = match self.guard_registered_tool_call(tool_name.as_str()) {
            Ok(()) => {
                let tool_call_context = ToolCallContext::new(self, request, context);
                self.tool_router.call(tool_call_context).await
            }
            Err(error) => {
                emit_security_rejection(
                    "mcp_tool_filter_rejected",
                    "mcp_tool_visibility",
                    None,
                    format!("MCP tool call rejected by runtime controls: {tool_name}"),
                );
                Err(error)
            }
        };
        let elapsed_ms = started_at.elapsed().as_millis();

        match &result {
            Ok(_) => {
                emit_mcp_tool_completed(&tool_call_id, &tool_name, elapsed_ms);
                emit_operation_completed(&operation, elapsed_ms.try_into().unwrap_or(u64::MAX));
            }
            Err(error) => {
                emit_mcp_tool_failed(&tool_call_id, &tool_name, error, elapsed_ms);
                emit_operation_failure_message(
                    &operation,
                    OperationErrorCategory::Business,
                    format!("mcp_error_{}", error.code.0),
                    redact_text(error.message.as_ref()),
                    elapsed_ms.try_into().unwrap_or(u64::MAX),
                );
            }
        }

        result
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(self.current_tools_result())
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router
            .get(name)
            .cloned()
            .filter(|tool| !self.filtered_tools_from([tool.clone()]).is_empty())
            .map(sanitize_tool_for_clients)
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
            .with_instructions(format!(
                "workhub-rs exposes 101 Jira, Confluence, and GitLab business tools. MCP tool visibility is controlled by MCP_TOOL_PROFILE, MCP_TOOLSETS, MCP_ENABLED_TOOLS, and MCP_DISABLED_TOOLS. Jira, Confluence, and GitLab tools are available when their service configuration and authentication are complete. The resource CLI ignores MCP tool visibility controls. See docs/support-matrix.md for per-tool and runtime support status."
            ))
    }
}

#[cfg(test)]
mod tests;
