use std::sync::Arc;

use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use tokio::sync::Mutex;

pub const SERVER_NAME: &str = "cyder-mcp-template";

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EchoRequest {
    #[schemars(description = "Text to echo back to the caller")]
    pub message: String,
}

#[derive(Clone)]
pub struct TemplateMcpServer {
    counter: Arc<Mutex<i32>>,
    tool_router: ToolRouter<Self>,
}

impl TemplateMcpServer {
    pub fn new() -> Self {
        Self {
            counter: Arc::new(Mutex::new(0)),
            tool_router: Self::tool_router(),
        }
    }
}

impl Default for TemplateMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router(router = tool_router)]
impl TemplateMcpServer {
    #[tool(description = "Increment the in-memory counter by 1")]
    async fn increment(&self) -> String {
        let mut counter = self.counter.lock().await;
        *counter += 1;
        counter.to_string()
    }

    #[tool(description = "Decrement the in-memory counter by 1")]
    async fn decrement(&self) -> String {
        let mut counter = self.counter.lock().await;
        *counter -= 1;
        counter.to_string()
    }

    #[tool(description = "Get the current in-memory counter value")]
    async fn get_value(&self) -> String {
        self.counter.lock().await.to_string()
    }

    #[tool(description = "Echo a message back to the caller")]
    fn echo(&self, Parameters(EchoRequest { message }): Parameters<EchoRequest>) -> String {
        message
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for TemplateMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Starter MCP server with a small in-memory counter and echo tool. Replace these tools with your own application capabilities.",
            )
    }
}

#[cfg(test)]
mod tests {
    use rmcp::ServerHandler;

    use super::*;

    #[test]
    fn server_info_advertises_tools() {
        let info = TemplateMcpServer::new().get_info();

        assert_eq!(info.server_info.name, SERVER_NAME);
        assert_eq!(info.server_info.version, env!("CARGO_PKG_VERSION"));
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.prompts.is_none());
        assert!(info.capabilities.resources.is_none());
    }

    #[test]
    fn tool_metadata_is_generated() {
        assert_eq!(TemplateMcpServer::increment_tool_attr().name, "increment");
        assert_eq!(TemplateMcpServer::decrement_tool_attr().name, "decrement");
        assert_eq!(TemplateMcpServer::get_value_tool_attr().name, "get_value");
        assert_eq!(TemplateMcpServer::echo_tool_attr().name, "echo");
    }

    #[tokio::test]
    async fn counter_tools_update_value() {
        let server = TemplateMcpServer::new();

        assert_eq!(server.get_value().await, "0");
        assert_eq!(server.increment().await, "1");
        assert_eq!(server.increment().await, "2");
        assert_eq!(server.decrement().await, "1");
        assert_eq!(server.get_value().await, "1");
    }

    #[test]
    fn echo_returns_message() {
        let server = TemplateMcpServer::new();

        assert_eq!(
            server.echo(Parameters(EchoRequest {
                message: "hello mcp".to_string(),
            })),
            "hello mcp"
        );
    }
}
