use crate::observability::schema::RuntimeMode;
use uuid::Uuid;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CorrelationIds {
    pub command_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub session_id: Option<String>,
    pub request_id: Option<String>,
    pub upstream_request_id: Option<String>,
    pub operation_id: Option<String>,
}

impl CorrelationIds {
    pub(crate) fn for_command(command_id: impl Into<String>) -> Self {
        Self {
            command_id: Some(command_id.into()),
            ..Self::default()
        }
    }

    pub(crate) fn for_tool_call(tool_call_id: impl Into<String>) -> Self {
        Self {
            tool_call_id: Some(tool_call_id.into()),
            ..Self::default()
        }
    }

    pub(crate) fn with_operation_id(mut self, operation_id: impl Into<String>) -> Self {
        self.operation_id = Some(operation_id.into());
        self
    }

    pub(crate) fn with_upstream_request_id(
        mut self,
        upstream_request_id: impl Into<String>,
    ) -> Self {
        self.upstream_request_id = Some(upstream_request_id.into());
        self
    }

    pub(crate) fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    pub(crate) fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservabilityContext {
    pub run_id: String,
    pub mode: RuntimeMode,
    pub version: String,
    pub pid: u32,
}

impl ObservabilityContext {
    pub(crate) fn new(mode: RuntimeMode, version: impl Into<String>) -> Self {
        Self {
            run_id: new_run_id(),
            mode,
            version: version.into(),
            pid: std::process::id(),
        }
    }

    pub(crate) fn test(run_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            mode: RuntimeMode::Test,
            version: "test".to_string(),
            pid: 1,
        }
    }
}

pub(crate) fn new_run_id() -> String {
    new_prefixed_id("run")
}

pub(crate) fn new_command_id() -> String {
    new_prefixed_id("cmd")
}

pub(crate) fn new_tool_call_id() -> String {
    new_prefixed_id("tool")
}

pub(crate) fn new_operation_id() -> String {
    new_prefixed_id("op")
}

pub(crate) fn new_upstream_request_id() -> String {
    new_prefixed_id("http")
}

fn new_prefixed_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::now_v7().simple())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observability_context_generates_prefixed_correlation_ids() {
        let run_id = new_run_id();
        let command_id = new_command_id();
        let tool_call_id = new_tool_call_id();
        let operation_id = new_operation_id();
        let upstream_request_id = new_upstream_request_id();

        assert!(run_id.starts_with("run_"));
        assert!(command_id.starts_with("cmd_"));
        assert!(tool_call_id.starts_with("tool_"));
        assert!(operation_id.starts_with("op_"));
        assert!(upstream_request_id.starts_with("http_"));
        assert_ne!(run_id, new_run_id());
    }

    #[test]
    fn correlation_ids_chain_without_losing_existing_ids() {
        let ids = CorrelationIds::for_tool_call("tool_1")
            .with_operation_id("op_1")
            .with_upstream_request_id("http_1")
            .with_session_id("session_1")
            .with_request_id("request_1");

        assert_eq!(ids.tool_call_id.as_deref(), Some("tool_1"));
        assert_eq!(ids.operation_id.as_deref(), Some("op_1"));
        assert_eq!(ids.upstream_request_id.as_deref(), Some("http_1"));
        assert_eq!(ids.session_id.as_deref(), Some("session_1"));
        assert_eq!(ids.request_id.as_deref(), Some("request_1"));
    }
}
