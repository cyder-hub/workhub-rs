use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRouter,
    model::{Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};

pub const SERVER_NAME: &str = "mcp-atlassian-rs";

const MIGRATION_STATUS: &str = "mcp-atlassian-rs Stage 0 migration baseline. \
Jira and Confluence tools have not been migrated yet. \
Next stages establish the shared runtime, Jira client/models, Jira MCP tools, \
Jira acceptance gate, and then Confluence migration.";

#[derive(Clone)]
pub struct AtlassianMcpServer {
    tool_router: ToolRouter<Self>,
}

impl AtlassianMcpServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

impl Default for AtlassianMcpServer {
    fn default() -> Self {
        Self::new()
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
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Rust MCP Atlassian migration baseline. Jira and Confluence tools are not available until later migration stages.",
            )
    }
}

#[cfg(test)]
mod tests {
    use rmcp::ServerHandler;

    use super::*;

    #[test]
    fn server_info_advertises_tools() {
        let info = AtlassianMcpServer::new().get_info();

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
            "migration_status"
        );
    }

    #[test]
    fn migration_status_reports_stage_scope() {
        let server = AtlassianMcpServer::new();
        let status = server.migration_status();

        assert!(status.contains("Stage 0 migration baseline"));
        assert!(status.contains("Jira and Confluence tools have not been migrated yet"));
    }
}
