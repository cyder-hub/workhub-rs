#![allow(dead_code)]

use rmcp::{ErrorData, model::CallToolResult};
use serde_json::{Map, Value, json};

use crate::{
    confluence::client::ConfluenceClient, context::AppContext, gitlab::client::GitlabClient,
    jira::client::JiraClient, observability::events::emit_security_rejection, tool_registry,
    upstream::error::UpstreamError,
};

pub mod confluence;
pub mod error;
pub mod gitlab;
pub mod jira;
pub mod output;

pub use error::{OperationError, OperationErrorCategory};
pub use output::OutputPresentation;

#[derive(Debug, Clone, PartialEq)]
pub struct OperationResult {
    pub value: Value,
    pub is_error: bool,
    pub presentation: OutputPresentation,
}

impl OperationResult {
    pub fn success(value: Value) -> Self {
        Self {
            value,
            is_error: false,
            presentation: OutputPresentation::KeyValue,
        }
    }

    pub fn structured_error(value: Value) -> Self {
        Self {
            value,
            is_error: true,
            presentation: OutputPresentation::KeyValue,
        }
    }

    pub fn with_presentation(mut self, presentation: OutputPresentation) -> Self {
        self.presentation = presentation;
        self
    }
}

pub fn guard_operation(tool_name: &str, context: &AppContext) -> Result<(), OperationError> {
    tool_registry::guard_operation_access(tool_name, context)
        .map(|_| ())
        .map_err(|error| {
            emit_security_rejection(
                "operation_filter_rejected",
                "runtime_tool_controls",
                None,
                format!("operation rejected by runtime controls: {tool_name}"),
            );
            OperationError::from_tool_guard(error)
        })
}

pub fn jira_client(context: &AppContext) -> Result<JiraClient, OperationError> {
    let config = context
        .jira_config()
        .cloned()
        .ok_or_else(|| OperationError::config("Jira is not configured"))?;
    JiraClient::new(config).map_err(OperationError::from_upstream)
}

pub fn confluence_client(context: &AppContext) -> Result<ConfluenceClient, OperationError> {
    let config = context
        .confluence_config()
        .cloned()
        .ok_or_else(|| OperationError::config("Confluence is not configured"))?;
    ConfluenceClient::new(config).map_err(OperationError::from_upstream)
}

pub fn gitlab_client(context: &AppContext) -> Result<GitlabClient, OperationError> {
    let config = context
        .gitlab_config()
        .cloned()
        .ok_or_else(|| OperationError::config("GitLab is not configured"))?;
    GitlabClient::new(config).map_err(OperationError::from_upstream)
}

pub fn operation_result_to_mcp(result: OperationResult) -> CallToolResult {
    if result.is_error {
        CallToolResult::structured_error(result.value)
    } else {
        CallToolResult::structured(result.value)
    }
}

pub fn operation_error_to_mcp(error: OperationError) -> ErrorData {
    match error.category {
        OperationErrorCategory::InvalidInput
        | OperationErrorCategory::Config
        | OperationErrorCategory::ServiceUnavailable
        | OperationErrorCategory::DisabledTool => {
            ErrorData::invalid_params(error.to_string(), None)
        }
        OperationErrorCategory::HttpStatus
        | OperationErrorCategory::Transport
        | OperationErrorCategory::JsonDecode
        | OperationErrorCategory::UnexpectedShape
        | OperationErrorCategory::Business => ErrorData::internal_error(error.to_string(), None),
    }
}

pub fn mutation_success(message: impl Into<String>, data: Value) -> Value {
    let mut object = Map::new();
    object.insert("success".to_string(), Value::Bool(true));
    object.insert("message".to_string(), Value::String(message.into()));
    object.insert("data".to_string(), non_null_data(data));
    object.insert("warnings".to_string(), Value::Array(Vec::new()));
    Value::Object(object)
}

pub fn mutation_success_with_fields(
    message: impl Into<String>,
    data: Value,
    fields: impl IntoIterator<Item = (&'static str, Value)>,
) -> Value {
    let mut value = mutation_success(message, data);
    let object = value
        .as_object_mut()
        .expect("mutation_success must return an object");
    for (key, field_value) in fields {
        object.insert(key.to_string(), field_value);
    }
    value
}

pub fn mutation_failure(
    message: impl Into<String>,
    category: impl Into<String>,
    error: impl Into<String>,
    cleanup_hint: Option<Value>,
) -> Value {
    let mut object = Map::new();
    object.insert("success".to_string(), Value::Bool(false));
    object.insert("message".to_string(), Value::String(message.into()));
    object.insert(
        "error".to_string(),
        json!({
            "category": category.into(),
            "message": error.into(),
        }),
    );
    object.insert("warnings".to_string(), Value::Array(Vec::new()));
    if let Some(cleanup_hint) = cleanup_hint {
        object.insert("cleanup_hint".to_string(), cleanup_hint);
    }
    Value::Object(object)
}

pub fn mutation_failure_from_upstream(
    message: impl Into<String>,
    error: &UpstreamError,
    cleanup_hint: Option<Value>,
) -> Value {
    mutation_failure(
        message,
        upstream_failure_category(error),
        error.to_string(),
        cleanup_hint,
    )
}

pub fn non_null_data(value: Value) -> Value {
    if value.is_null() { json!({}) } else { value }
}

pub fn upstream_failure_category(error: &UpstreamError) -> &'static str {
    match error {
        UpstreamError::InvalidInput { .. } => "invalid_input",
        UpstreamError::InvalidBaseUrl { .. } => "config",
        UpstreamError::HttpStatus { status, .. } => match *status {
            401 | 403 => "permission_denied",
            404 => "not_found",
            405 => "unsupported_or_auth_required",
            _ => "http_status",
        },
        UpstreamError::Transport { .. } => "transport",
        UpstreamError::JsonDecode { .. } => "json_decode",
        UpstreamError::UnexpectedShape { .. } => "unexpected_shape",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::json;

    use crate::{
        config::{HttpConfig, RuntimeConfig},
        gitlab::config::GitlabConfig,
        gitlab::tools::GITLAB_GET_PROJECT_TOOL_NAME,
        tool_registry::default_toolsets,
        upstream::{auth::UpstreamAuth, custom_headers::CustomHeaders, proxy::ProxyConfig},
    };

    use super::*;

    #[test]
    fn operations_guard_allows_enabled_tool_with_available_service() {
        let context = AppContext::from_config(&RuntimeConfig {
            gitlab: Some(gitlab_config()),
            mcp_enabled_toolsets: default_toolsets(),
            http: HttpConfig::default(),
            ..RuntimeConfig::default()
        });

        assert!(guard_operation(GITLAB_GET_PROJECT_TOOL_NAME, &context).is_ok());
    }

    #[test]
    fn operations_guard_ignores_mcp_disabled_tools() {
        let context = AppContext::from_config(&RuntimeConfig {
            gitlab: Some(gitlab_config()),
            mcp_disabled_tools: BTreeSet::from([GITLAB_GET_PROJECT_TOOL_NAME.to_string()]),
            mcp_enabled_toolsets: default_toolsets(),
            http: HttpConfig::default(),
            ..RuntimeConfig::default()
        });

        assert!(guard_operation(GITLAB_GET_PROJECT_TOOL_NAME, &context).is_ok());
    }

    #[test]
    fn operations_guard_blocks_missing_service() {
        let context = AppContext::from_config(&RuntimeConfig {
            mcp_enabled_toolsets: default_toolsets(),
            http: HttpConfig::default(),
            ..RuntimeConfig::default()
        });
        let error = guard_operation(GITLAB_GET_PROJECT_TOOL_NAME, &context).unwrap_err();

        assert_eq!(error.category, OperationErrorCategory::ServiceUnavailable);
        assert_eq!(error.exit_code(), 3);
    }

    #[test]
    fn operations_mcp_adapter_uses_structured_error_flag() {
        let result = operation_result_to_mcp(OperationResult::structured_error(json!({
            "success": false,
            "error": "not available"
        })));

        assert_eq!(
            result.structured_content.as_ref().unwrap()["success"],
            json!(false)
        );
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn mutation_success_replaces_no_content_data_with_empty_object() {
        let value = mutation_success("done", Value::Null);

        assert_eq!(value["success"], json!(true));
        assert_eq!(value["data"], json!({}));
        assert_eq!(value["warnings"], json!([]));
    }

    #[test]
    fn mutation_failure_includes_category_message_and_cleanup_hint() {
        let value = mutation_failure(
            "delete failed",
            "permission_denied",
            "forbidden",
            Some(json!({"manual": "remove object in UI"})),
        );

        assert_eq!(value["success"], json!(false));
        assert_eq!(value["error"]["category"], json!("permission_denied"));
        assert_eq!(
            value["cleanup_hint"]["manual"],
            json!("remove object in UI")
        );
    }

    #[test]
    fn upstream_failure_category_maps_common_compatibility_statuses() {
        assert_eq!(
            upstream_failure_category(&UpstreamError::http_status(
                reqwest::StatusCode::FORBIDDEN,
                "forbidden",
            )),
            "permission_denied"
        );
        assert_eq!(
            upstream_failure_category(&UpstreamError::http_status(
                reqwest::StatusCode::NOT_FOUND,
                "missing",
            )),
            "not_found"
        );
        assert_eq!(
            upstream_failure_category(&UpstreamError::http_status(
                reqwest::StatusCode::METHOD_NOT_ALLOWED,
                "no method",
            )),
            "unsupported_or_auth_required"
        );
    }

    fn gitlab_config() -> GitlabConfig {
        GitlabConfig {
            base_url: "https://gitlab.example".to_string(),
            auth: UpstreamAuth::HeaderToken {
                header_name: reqwest::header::HeaderName::from_static("private-token"),
                token: "test-token".to_string(),
            },
            ssl_verify: true,
            proxy: ProxyConfig::default(),
            custom_headers: CustomHeaders::default(),
            mtls: None,
            projects_filter: BTreeSet::new(),
            timeout_seconds: 75,
        }
    }
}
