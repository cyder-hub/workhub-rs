use std::sync::Arc;

use crate::{context::AppContext, tool_registry};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::tool::ToolCallContext,
    model::{
        CallToolRequestParams, CallToolResult, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};

pub const SERVER_NAME: &str = "mcp-atlassian-rs";

const MIGRATION_STATUS: &str = "mcp-atlassian-rs Stage 1 shared MCP runtime and control plane is complete. \
Jira and Confluence business tools have not been migrated yet. \
Next stages migrate Jira client/models, Jira MCP tools, the Jira acceptance gate, \
and then Confluence support.";

#[derive(Clone)]
pub struct AtlassianMcpServer {
    context: Arc<AppContext>,
    tool_router: ToolRouter<Self>,
}

impl AtlassianMcpServer {
    pub fn new(context: Arc<AppContext>) -> Self {
        Self {
            context,
            tool_router: Self::tool_router(),
        }
    }

    fn current_tools_result(&self) -> ListToolsResult {
        ListToolsResult {
            tools: self.filtered_tools_from(self.tool_router.list_all()),
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

impl Default for AtlassianMcpServer {
    fn default() -> Self {
        Self::new(Arc::new(AppContext::default()))
    }
}

#[tool_router(router = tool_router)]
impl AtlassianMcpServer {
    #[tool(description = "Report the current Rust migration status for MCP Atlassian")]
    fn migration_status(&self) -> String {
        MIGRATION_STATUS.to_string()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AtlassianMcpServer {
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.guard_registered_tool_call(request.name.as_ref())?;

        let tool_call_context = ToolCallContext::new(self, request, context);
        self.tool_router.call(tool_call_context).await
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
    }

    fn get_info(&self) -> ServerInfo {
        let access_mode = if self.context.read_only() {
            "read-only"
        } else {
            "read/write"
        };

        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
            .with_instructions(format!(
                "Rust MCP Atlassian migration baseline. The Stage 1 control plane is initialized in {access_mode} mode. Jira and Confluence tools are not available until later migration stages."
            ))
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, sync::Arc};

    use crate::{
        config::{HttpConfig, RuntimeConfig},
        context::AppContext,
        tool_registry::{MIGRATION_STATUS_TOOL_NAME, ToolAccess, ToolMetadata, ToolService},
    };
    use rmcp::ServerHandler;
    use rmcp::model::{JsonObject, Tool};

    use super::*;

    fn server_with_config(config: RuntimeConfig) -> AtlassianMcpServer {
        AtlassianMcpServer::new(Arc::new(AppContext::from_config(&config)))
    }

    const SYNTHETIC_JIRA_READ: ToolMetadata = ToolMetadata {
        name: "stage1_synthetic_jira_read",
        service: ToolService::Jira,
        access: ToolAccess::Read,
        toolset: Some("jira_issues"),
        title: "Synthetic Jira read",
        description: "Test-only Jira read metadata.",
    };

    const SYNTHETIC_JIRA_WRITE: ToolMetadata = ToolMetadata {
        name: "stage1_synthetic_jira_write",
        service: ToolService::Jira,
        access: ToolAccess::Write,
        toolset: Some("jira_issues"),
        title: "Synthetic Jira write",
        description: "Test-only Jira write metadata.",
    };

    const SYNTHETIC_CONFLUENCE_READ: ToolMetadata = ToolMetadata {
        name: "stage1_synthetic_confluence_read",
        service: ToolService::Confluence,
        access: ToolAccess::Read,
        toolset: Some("confluence_pages"),
        title: "Synthetic Confluence read",
        description: "Test-only Confluence read metadata.",
    };

    fn metadata_for_test_tool(name: &str) -> Option<ToolMetadata> {
        match name {
            "stage1_synthetic_jira_read" => Some(SYNTHETIC_JIRA_READ),
            "stage1_synthetic_jira_write" => Some(SYNTHETIC_JIRA_WRITE),
            "stage1_synthetic_confluence_read" => Some(SYNTHETIC_CONFLUENCE_READ),
            _ => tool_registry::metadata_for(name),
        }
    }

    fn runtime_config() -> RuntimeConfig {
        RuntimeConfig {
            http: HttpConfig::default(),
            ..RuntimeConfig::default()
        }
    }

    fn tool(name: &'static str) -> Tool {
        Tool::new(name, "", Arc::<JsonObject>::new(Default::default()))
    }

    fn current_tool_names(server: &AtlassianMcpServer) -> Vec<String> {
        tool_names(server.current_tools_result().tools)
    }

    fn tool_names(tools: Vec<Tool>) -> Vec<String> {
        tools
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    #[test]
    fn server_info_advertises_tools() {
        let info = AtlassianMcpServer::default().get_info();

        assert_eq!(info.server_info.name, SERVER_NAME);
        assert_eq!(info.server_info.version, env!("CARGO_PKG_VERSION"));
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.prompts.is_none());
        assert!(info.capabilities.resources.is_none());
    }

    #[test]
    fn tool_metadata_is_generated() {
        assert_eq!(
            AtlassianMcpServer::migration_status_tool_attr().name,
            MIGRATION_STATUS_TOOL_NAME
        );
    }

    #[test]
    fn migration_status_reports_stage_scope() {
        let server = AtlassianMcpServer::default();
        let status = server.migration_status();

        assert!(status.contains("Stage 1 shared MCP runtime and control plane is complete"));
        assert!(status.contains("Jira and Confluence business tools have not been migrated yet"));
    }

    #[test]
    fn server_info_uses_app_context() {
        let config = RuntimeConfig {
            read_only: true,
            ..RuntimeConfig::default()
        };
        let server = AtlassianMcpServer::new(Arc::new(AppContext::from_config(&config)));
        let info = server.get_info();
        let instructions = info.instructions.unwrap_or_default();

        assert!(instructions.contains("read-only mode"));
    }

    #[test]
    fn tool_discovery_uses_registry_and_keeps_migration_status_visible_by_default() {
        let server = AtlassianMcpServer::default();

        assert_eq!(
            current_tool_names(&server),
            vec![MIGRATION_STATUS_TOOL_NAME.to_string()]
        );
        assert!(server.get_tool(MIGRATION_STATUS_TOOL_NAME).is_some());
    }

    #[test]
    fn tool_discovery_applies_enabled_tools_filter_to_migration_status() {
        let server = server_with_config(RuntimeConfig {
            enabled_tools: Some(BTreeSet::from(["some_other_tool".to_string()])),
            ..runtime_config()
        });

        assert!(current_tool_names(&server).is_empty());
        assert!(server.get_tool(MIGRATION_STATUS_TOOL_NAME).is_none());
        assert!(
            server
                .guard_registered_tool_call(MIGRATION_STATUS_TOOL_NAME)
                .is_err()
        );
    }

    #[test]
    fn tool_discovery_does_not_apply_toolsets_to_migration_status() {
        let server = server_with_config(RuntimeConfig {
            enabled_toolsets: BTreeSet::new(),
            ..runtime_config()
        });

        assert_eq!(
            current_tool_names(&server),
            vec![MIGRATION_STATUS_TOOL_NAME.to_string()]
        );
    }

    #[test]
    fn tool_discovery_fails_closed_for_unmapped_tools() {
        let server = AtlassianMcpServer::default();
        let tools =
            server.filtered_tools_from([tool(MIGRATION_STATUS_TOOL_NAME), tool("unmapped_tool")]);
        let names: Vec<_> = tools
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();

        assert_eq!(names, vec![MIGRATION_STATUS_TOOL_NAME.to_string()]);
    }

    #[test]
    fn tool_discovery_applies_future_service_and_toolset_policy_at_server_boundary() {
        let unavailable = AtlassianMcpServer::default();
        let available = server_with_config(RuntimeConfig {
            jira_url: Some("https://jira.example".to_string()),
            confluence_url: Some("https://confluence.example".to_string()),
            ..runtime_config()
        });
        let jira_fields_only = server_with_config(RuntimeConfig {
            jira_url: Some("https://jira.example".to_string()),
            enabled_toolsets: BTreeSet::from(["jira_fields".to_string()]),
            ..runtime_config()
        });

        assert_eq!(
            tool_names(unavailable.filtered_tools_from_with_metadata(
                [
                    tool("stage1_synthetic_jira_read"),
                    tool("stage1_synthetic_confluence_read"),
                ],
                metadata_for_test_tool,
            )),
            Vec::<String>::new()
        );
        assert_eq!(
            tool_names(available.filtered_tools_from_with_metadata(
                [
                    tool("stage1_synthetic_jira_read"),
                    tool("stage1_synthetic_confluence_read"),
                ],
                metadata_for_test_tool,
            )),
            vec![
                "stage1_synthetic_confluence_read".to_string(),
                "stage1_synthetic_jira_read".to_string(),
            ]
        );
        assert!(
            jira_fields_only
                .filtered_tools_from_with_metadata(
                    [tool("stage1_synthetic_jira_read")],
                    metadata_for_test_tool,
                )
                .is_empty()
        );
    }

    #[test]
    fn direct_call_guard_applies_future_read_only_policy_at_server_boundary() {
        let read_only_server = server_with_config(RuntimeConfig {
            read_only: true,
            jira_url: Some("https://jira.example".to_string()),
            ..runtime_config()
        });
        let read_write_server = server_with_config(RuntimeConfig {
            jira_url: Some("https://jira.example".to_string()),
            ..runtime_config()
        });

        let error = read_only_server
            .guard_tool_call_with_metadata(
                "stage1_synthetic_jira_write",
                true,
                metadata_for_test_tool,
            )
            .unwrap_err();

        assert_eq!(error.message, "tool is disabled in read-only mode");
        assert!(
            read_write_server
                .guard_tool_call_with_metadata(
                    "stage1_synthetic_jira_write",
                    true,
                    metadata_for_test_tool,
                )
                .is_ok()
        );
        assert!(
            read_write_server
                .guard_tool_call_with_metadata(
                    "stage1_synthetic_jira_write",
                    false,
                    metadata_for_test_tool,
                )
                .is_err()
        );
    }
}
