mod mcp;

use std::net::SocketAddr;

use axum::Router;
use mcp::AtlassianMcpServer;
use rmcp::{
    ServiceExt,
    transport::{
        stdio,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

type AppResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8000;
const USAGE: &str = "\
Usage:
  mcp-atlassian-rs [stdio]
  mcp-atlassian-rs streamhttp [--host <host>] [--port <port>]

Commands:
  stdio       Run the MCP server over standard input/output.
  streamhttp  Run the MCP server over streamable HTTP at /mcp.
";

#[derive(Debug, Clone, PartialEq, Eq)]
enum RunMode {
    Stdio,
    StreamHttp(StreamHttpConfig),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StreamHttpConfig {
    host: String,
    port: u16,
}

#[tokio::main]
async fn main() -> AppResult<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        println!("{USAGE}");
        return Ok(());
    }

    let mode = match parse_args(args) {
        Ok(mode) => mode,
        Err(message) => {
            eprintln!("{message}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    init_tracing()?;

    match mode {
        RunMode::Stdio => run_stdio().await?,
        RunMode::StreamHttp(config) => run_streamhttp(config).await?,
    }

    Ok(())
}

async fn run_stdio() -> AppResult<()> {
    tracing::info!(server = mcp::SERVER_NAME, "starting MCP stdio server");

    let service = AtlassianMcpServer::new()
        .serve(stdio())
        .await
        .inspect_err(|error| {
            tracing::error!(?error, "MCP stdio server error");
        })?;

    service.waiting().await?;
    Ok(())
}

async fn run_streamhttp(config: StreamHttpConfig) -> AppResult<()> {
    let address: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let cancellation = CancellationToken::new();
    let service = StreamableHttpService::new(
        || Ok(AtlassianMcpServer::new()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default().with_cancellation_token(cancellation.child_token()),
    );
    let app = Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(address).await?;

    tracing::info!(
        server = mcp::SERVER_NAME,
        address = %address,
        endpoint = "/mcp",
        "starting MCP streamable HTTP server"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::error!(?error, "failed to listen for shutdown signal");
            }
            cancellation.cancel();
        })
        .await?;

    Ok(())
}

fn init_tracing() -> AppResult<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init()?;

    Ok(())
}

fn parse_args<I, S>(args: I) -> Result<RunMode, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    match args.first().map(String::as_str) {
        None => Ok(RunMode::Stdio),
        Some("stdio") if args.len() <= 1 => Ok(RunMode::Stdio),
        Some("stdio") => Err(format!("unexpected argument for stdio: `{}`", args[1])),
        Some("streamhttp") => parse_streamhttp_args(&args[1..]).map(RunMode::StreamHttp),
        Some(command) => Err(format!("unknown command `{command}`")),
    }
}

fn parse_streamhttp_args(args: &[String]) -> Result<StreamHttpConfig, String> {
    let mut config = StreamHttpConfig {
        host: DEFAULT_HOST.to_string(),
        port: DEFAULT_PORT,
    };
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--host" => {
                index += 1;
                config.host = args
                    .get(index)
                    .ok_or_else(|| "--host requires a value".to_string())?
                    .clone();
            }
            "--port" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--port requires a value".to_string())?;
                config.port = value
                    .parse()
                    .map_err(|_| format!("invalid --port value `{value}`"))?;
            }
            arg if arg.starts_with("--host=") => {
                config.host = arg
                    .strip_prefix("--host=")
                    .expect("prefix was just checked")
                    .to_string();
            }
            arg if arg.starts_with("--port=") => {
                let value = arg
                    .strip_prefix("--port=")
                    .expect("prefix was just checked");
                config.port = value
                    .parse()
                    .map_err(|_| format!("invalid --port value `{value}`"))?;
            }
            arg => return Err(format!("unexpected streamhttp argument `{arg}`")),
        }
        index += 1;
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_defaults_to_stdio() {
        assert_eq!(parse_args(Vec::<String>::new()).unwrap(), RunMode::Stdio);
        assert_eq!(parse_args(["stdio"]).unwrap(), RunMode::Stdio);
    }

    #[test]
    fn parse_args_accepts_streamhttp_defaults() {
        assert_eq!(
            parse_args(["streamhttp"]).unwrap(),
            RunMode::StreamHttp(StreamHttpConfig {
                host: DEFAULT_HOST.to_string(),
                port: DEFAULT_PORT,
            })
        );
    }

    #[test]
    fn parse_args_accepts_streamhttp_host_and_port() {
        assert_eq!(
            parse_args(["streamhttp", "--host", "0.0.0.0", "--port=9000"]).unwrap(),
            RunMode::StreamHttp(StreamHttpConfig {
                host: "0.0.0.0".to_string(),
                port: 9000,
            })
        );
    }

    #[test]
    fn parse_args_rejects_unknown_command() {
        assert!(parse_args(["http"]).is_err());
    }
}
