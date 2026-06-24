mod atlassian;
mod cli;
mod config;
mod confluence;
mod context;
mod env_loader;
mod error;
mod gitlab;
mod jira;
mod mcp;
mod mcp_confluence_helpers;
mod mcp_errors;
mod observability;
mod operations;
mod tool_registry;
mod upstream;

use std::{io, net::SocketAddr, path::PathBuf, sync::Arc, time::Instant};

use axum::{Json, Router, routing::get};
use clap::{Args, Command, Parser, Subcommand, ValueEnum};
use config::HttpConfigOverrides;
use context::AppContext;
use mcp::WorkhubMcpServer;
use observability::{
    config::{
        DeprecatedEnvWarning, LogDirSource, LogProfile, LogTarget, LogTargetsSource, LoggingConfig,
        ObservabilityConfig, ObservabilityConfigError,
    },
    context::{CorrelationIds, ObservabilityContext, new_command_id},
    events::{
        OperationLogContext, emit_operation_completed, emit_operation_failed,
        emit_operation_started,
    },
    panic::PanicHookGuard,
    rotation::RUN_LOG_FILE,
    schema::{
        ErrorDiagnosticEnvelope, LogEvent, LogKind, LogLevel, Outcome, PayloadPolicy, RuntimeMode,
    },
    sinks::{ObservabilityGuard, ObservabilitySinks},
    support_bundle::{BundleOptions, create_support_bundle, parse_since, summarize_log_path},
    usage::{UsageItem, UsageOptions, UsageReport, UsageSort, UsageSourceFilter, analyze_usage},
};
use operations::error::OperationErrorCategory;
use rmcp::{
    ServiceExt,
    transport::{
        stdio,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

type AppResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const USAGE: &str = "\
Usage:
  workhub -v
  workhub [stdio]
  workhub streamhttp [--host <host>] [--port <port>] [--path <path>] [--env-file <path>]
  workhub cli [--env-file <path>] [--json] [--pretty] [-v|--verbose] <provider> <resource> <action> ...
  workhub cli config <path|show|setup|set|unset> ...
  workhub logs path
  workhub logs bundle [--since <duration>] [--output <path>]
  workhub logs usage [--since <duration>] [--source <source>] [--limit <n>] [--sort <sort>] [--json] [--pretty]

Commands:
  stdio       Run the MCP server over standard input/output.
  streamhttp  Run the MCP server over streamable HTTP.
  cli         Run a resource-oriented Workhub CLI command.
  logs        Locate logs or create a redacted support bundle.

Options:
  -v                 Print the package version when used as `workhub -v`.
  cli -v, --verbose  Enable console log summaries for `workhub cli ...`.
  --env-file <path>  Load environment variables from the specified file (streamhttp and cli only).
                     Alternatively, set the ENV_FILE environment variable.

Defaults:
  host  127.0.0.1
  port  8000
  path  /mcp
";

#[derive(Debug, Clone, PartialEq, Eq)]
enum RunMode {
    Version,
    Stdio,
    StreamHttp {
        overrides: HttpConfigOverrides,
        env_file: Option<String>,
    },
    Cli(Box<cli::CliArgs>),
    Logs(LogsArgs),
}

#[derive(Debug, serde::Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Parser)]
#[command(name = "workhub", version, disable_help_subcommand = true)]
struct RootArgs {
    #[command(subcommand)]
    command: RootCommand,
}

#[derive(Debug, Subcommand)]
enum RootCommand {
    Streamhttp(StreamHttpArgs),
    Cli(cli::CliArgs),
    Logs(LogsArgs),
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
struct StreamHttpArgs {
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    path: Option<String>,
    #[arg(long, value_name = "path")]
    env_file: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
struct LogsArgs {
    #[command(subcommand)]
    command: LogsCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum LogsCommand {
    Path,
    Bundle(LogsBundleArgs),
    Usage(LogsUsageArgs),
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
struct LogsBundleArgs {
    #[arg(long, default_value = "24h")]
    since: String,
    #[arg(long, value_name = "path", default_value = "workhub-logs.zip")]
    output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
struct LogsUsageArgs {
    #[arg(long, default_value = "24h")]
    since: String,
    #[arg(long, value_enum, default_value_t = LogsUsageSourceArg::All)]
    source: LogsUsageSourceArg,
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[arg(long, value_enum, default_value_t = LogsUsageSortArg::Calls)]
    sort: LogsUsageSortArg,
    #[arg(long)]
    json: bool,
    #[arg(long, requires = "json")]
    pretty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LogsUsageSourceArg {
    All,
    Mcp,
    Cli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LogsUsageSortArg {
    Calls,
    Failures,
    #[value(alias = "success_rate")]
    SuccessRate,
    #[value(alias = "avg_duration")]
    AvgDuration,
    Name,
}

#[derive(Debug)]
enum ParseArgsError {
    Usage(String),
    Clap(clap::Error),
}

#[tokio::main]
async fn main() -> AppResult<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let raw_args = args.clone();

    let mode = match parse_args(args) {
        Ok(mode) => mode,
        Err(ParseArgsError::Usage(message)) => {
            eprintln!("{message}\n\n{USAGE}");
            std::process::exit(2);
        }
        Err(ParseArgsError::Clap(error)) => error.exit(),
    };

    let dotenv = load_startup_dotenv(&mode);
    let mut observability = init_observability(&mode);
    if observability.config_error.is_some() {
        observability.guard.flush();
        std::process::exit(3);
    }

    if mode == RunMode::Version {
        log_lifecycle_event(
            &mut observability.guard,
            &observability.context,
            LogLevel::Info,
            "version.completed",
            "version printed",
            Outcome::Succeeded,
        );
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if let RunMode::Logs(args) = &mode {
        if let Err(error) = run_logs_command(args, &mut observability) {
            let exit_code = logs_command_exit_code(error.as_ref());
            eprintln!("logs command failed: {error}");
            observability.guard.flush();
            std::process::exit(exit_code);
        }
        return Ok(());
    }

    let cli_command = match &mode {
        RunMode::Cli(args) => {
            let command = CliCommandLogContext::new(&raw_args, args);
            log_cli_command_started(&mut observability.guard, &observability.context, &command);
            Some(command)
        }
        _ => None,
    };

    handle_startup_dotenv_result(&mode, dotenv, &mut observability, cli_command.as_ref());

    if let RunMode::Cli(args) = &mode
        && args.is_config_command()
    {
        let RunMode::Cli(args) = mode else {
            unreachable!("checked CLI mode");
        };
        if let Err(error) = cli::run_config(*args) {
            log_cli_run_error(
                &mut observability.guard,
                &observability.context,
                "cli.config.failed",
                &error,
                cli_command.as_ref(),
            );
            exit_cli_error(error);
        }
        log_cli_command_completed(
            &mut observability.guard,
            &observability.context,
            cli_command.as_ref().expect("CLI command context exists"),
            Outcome::Succeeded,
            0,
        );
        return Ok(());
    }

    let runtime_config = match &mode {
        RunMode::Cli(args) => match config::RuntimeConfig::from_env_for_cli() {
            Ok(config) => config,
            Err(error) => {
                let message = error.to_string();
                log_config_failure(
                    &mut observability.guard,
                    &observability.context,
                    "runtime_config.failed",
                    "runtime configuration failed",
                    &message,
                    3,
                    cli_command.as_ref(),
                );
                exit_cli_error(cli::render_config_error(args, message));
            }
        },
        RunMode::StreamHttp { overrides, .. } => {
            match config::RuntimeConfig::from_env_with_http_overrides(overrides.clone()) {
                Ok(config) => config,
                Err(error) => {
                    let message = error.to_string();
                    log_config_failure(
                        &mut observability.guard,
                        &observability.context,
                        "runtime_config.failed",
                        "runtime configuration failed",
                        &message,
                        3,
                        cli_command.as_ref(),
                    );
                    std::process::exit(3);
                }
            }
        }
        RunMode::Version | RunMode::Stdio => match config::RuntimeConfig::from_env() {
            Ok(config) => config,
            Err(error) => {
                let message = error.to_string();
                log_config_failure(
                    &mut observability.guard,
                    &observability.context,
                    "runtime_config.failed",
                    "runtime configuration failed",
                    &message,
                    3,
                    cli_command.as_ref(),
                );
                std::process::exit(3);
            }
        },
        RunMode::Logs(_) => unreachable!("logs mode returned before runtime startup"),
    };
    let context = Arc::new(AppContext::from_config(&runtime_config));

    log_runtime_context(&context, &mut observability.guard, &observability.context);

    match mode {
        RunMode::Version => unreachable!("version mode returned before runtime startup"),
        RunMode::Stdio => {
            run_stdio(context, &mut observability.guard, &observability.context).await?
        }
        RunMode::StreamHttp { .. } => {
            run_streamhttp(
                runtime_config.http,
                context,
                &mut observability.guard,
                &observability.context,
            )
            .await?
        }
        RunMode::Cli(args) => {
            if let Err(error) = cli::run(*args, context).await {
                log_cli_run_error(
                    &mut observability.guard,
                    &observability.context,
                    "cli.failed",
                    &error,
                    cli_command.as_ref(),
                );
                exit_cli_error(error);
            }
            log_cli_command_completed(
                &mut observability.guard,
                &observability.context,
                cli_command.as_ref().expect("CLI command context exists"),
                Outcome::Succeeded,
                0,
            );
        }
        RunMode::Logs(_) => unreachable!("logs mode returned before runtime startup"),
    }

    log_lifecycle_event(
        &mut observability.guard,
        &observability.context,
        LogLevel::Info,
        "process.completed",
        "workhub process completed",
        Outcome::Succeeded,
    );

    Ok(())
}

fn exit_cli_error(error: cli::CliRunError) -> ! {
    eprintln!("{error}");
    std::process::exit(error.exit_code());
}

fn load_startup_dotenv(
    mode: &RunMode,
) -> Option<Result<Option<PathBuf>, env_loader::EnvLoadError>> {
    if !mode.loads_dotenv() {
        return None;
    }

    Some(match mode {
        RunMode::Cli(args) => env_loader::load_cli_dotenv(args.env_file.as_deref()),
        _ => env_loader::load_dotenv(mode.explicit_env_file()),
    })
}

fn handle_startup_dotenv_result(
    mode: &RunMode,
    loaded: Option<Result<Option<PathBuf>, env_loader::EnvLoadError>>,
    observability: &mut ObservabilityRuntime,
    cli_command: Option<&CliCommandLogContext>,
) {
    let Some(loaded) = loaded else {
        return;
    };

    match loaded {
        Ok(Some(path)) if mode.reports_dotenv_success() => {
            log_config_event(
                &mut observability.guard,
                &observability.context,
                LogLevel::Info,
                "dotenv.loaded",
                "environment file loaded",
                Outcome::Succeeded,
                json!({ "env_file": path.display().to_string() }),
            );
        }
        Ok(Some(path)) => {
            log_config_event(
                &mut observability.guard,
                &observability.context,
                LogLevel::Debug,
                "dotenv.loaded",
                "environment file loaded",
                Outcome::Succeeded,
                json!({ "env_file": path.display().to_string() }),
            );
        }
        Ok(None) => {}
        Err(error) => {
            let message = error.to_string();
            log_config_failure(
                &mut observability.guard,
                &observability.context,
                "dotenv.failed",
                "failed to load environment file",
                &message,
                3,
                cli_command,
            );
            if let RunMode::Cli(args) = mode {
                exit_cli_error(cli::render_config_error(args, message));
            }
            std::process::exit(3);
        }
    }
}

struct ObservabilityRuntime {
    context: ObservabilityContext,
    guard: ObservabilityGuard,
    logging_config: LoggingConfig,
    config_error: Option<ObservabilityConfigError>,
    _panic_hook: PanicHookGuard,
}

struct CliCommandLogContext {
    command_id: String,
    command_path: String,
    output_mode: &'static str,
    operation: OperationLogContext,
    started: Instant,
}

impl CliCommandLogContext {
    fn new(raw_args: &[String], args: &cli::CliArgs) -> Self {
        let command_id = new_command_id();
        let command_path = cli_command_path_from_raw(raw_args);
        let operation = OperationLogContext::new(command_path.clone())
            .with_provider(provider_from_command_path(&command_path))
            .with_correlation(CorrelationIds::for_command(command_id.clone()));
        Self {
            command_id,
            command_path,
            output_mode: cli_output_mode(args),
            operation,
            started: Instant::now(),
        }
    }

    fn correlation(&self) -> CorrelationIds {
        CorrelationIds::for_command(self.command_id.clone())
    }

    fn duration_ms(&self) -> u64 {
        self.started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

fn provider_from_command_path(command_path: &str) -> String {
    command_path
        .split_whitespace()
        .next()
        .unwrap_or("cli")
        .to_string()
}

fn cli_output_mode(args: &cli::CliArgs) -> &'static str {
    match (args.json, args.pretty) {
        (true, true) => "json_pretty",
        (true, false) => "json",
        (false, _) => "text",
    }
}

fn cli_command_path_from_raw(raw_args: &[String]) -> String {
    let args = if matches!(raw_args.first().map(String::as_str), Some("cli")) {
        &raw_args[1..]
    } else {
        raw_args
    };
    let command = <cli::CliArgs as Args>::augment_args(Command::new("cli").no_binary_name(true));
    let Ok(matches) = command.try_get_matches_from(args) else {
        return "cli".to_string();
    };

    let mut command_path = Vec::new();
    let mut current = &matches;
    while let Some((name, subcommand)) = current.subcommand() {
        command_path.push(name.to_string());
        current = subcommand;
    }

    if command_path.is_empty() {
        "cli".to_string()
    } else {
        command_path.join(" ")
    }
}

fn init_observability(mode: &RunMode) -> ObservabilityRuntime {
    let loaded_config = ObservabilityConfig::from_env_and_file();
    let (mut logging_config, config_path, config_error) = match loaded_config {
        Ok(config) => (config.logging, config.config_path, None),
        Err(error) => (
            LoggingConfig::for_profile(
                LogProfile::Production,
                PathBuf::from(".workhub").join("logs"),
                LogDirSource::WorkspaceFallback,
            ),
            None,
            Some(error),
        ),
    };
    apply_mode_console_defaults(mode, &mut logging_config);

    let context = ObservabilityContext::new(mode.observability_mode(), env!("CARGO_PKG_VERSION"));
    let mut guard = ObservabilityGuard::new(
        context.clone(),
        ObservabilitySinks::new(logging_config.clone()),
    );
    log_process_started(&mut guard, &context, &logging_config, config_path.as_ref());

    for warning in &logging_config.deprecated_env_warnings {
        log_deprecated_env_warning(&mut guard, &context, warning);
    }

    if let Some(error) = &config_error {
        log_config_failure(
            &mut guard,
            &context,
            "observability_config.failed",
            "observability configuration failed",
            &error.to_string(),
            3,
            None,
        );
    }

    ObservabilityRuntime {
        context,
        guard,
        logging_config,
        config_error,
        _panic_hook: PanicHookGuard::install(),
    }
}

fn apply_mode_console_defaults(mode: &RunMode, config: &mut LoggingConfig) {
    if config.targets_source != LogTargetsSource::ProfileDefault {
        return;
    }

    match mode {
        RunMode::StreamHttp { .. } => {}
        RunMode::Cli(args) if args.verbose => {
            config.targets.insert(LogTarget::Console);
        }
        RunMode::Version | RunMode::Stdio | RunMode::Cli(_) | RunMode::Logs(_) => {
            config.targets.remove(&LogTarget::Console);
        }
    }
}

fn run_logs_command(args: &LogsArgs, observability: &mut ObservabilityRuntime) -> AppResult<()> {
    let command_path = logs_command_path(args);
    let started = Instant::now();
    log_logs_command_event(
        &mut observability.guard,
        &observability.context,
        "logs.command.started",
        "logs command started",
        Outcome::Started,
        &command_path,
        None,
    );

    let result = match &args.command {
        LogsCommand::Path => print_logs_path_summary(&observability.logging_config),
        LogsCommand::Bundle(bundle) => print_logs_bundle(&observability.logging_config, bundle),
        LogsCommand::Usage(usage) => print_logs_usage(&observability.logging_config, usage),
    };

    match result {
        Ok(()) => {
            log_logs_command_event(
                &mut observability.guard,
                &observability.context,
                "logs.command.completed",
                "logs command completed",
                Outcome::Succeeded,
                &command_path,
                Some(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)),
            );
            Ok(())
        }
        Err(error) => {
            let exit_code = logs_command_exit_code(error.as_ref());
            log_logs_command_failed(
                &mut observability.guard,
                &observability.context,
                &command_path,
                started,
                error.as_ref(),
                exit_code,
            );
            Err(error)
        }
    }
}

fn print_logs_path_summary(config: &LoggingConfig) -> AppResult<()> {
    let summary = summarize_log_path(config)?;
    let output = json!({
        "log_dir": summary.dir.display().to_string(),
        "targets": summary.targets,
        "recent_files": summary.recent_files,
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_logs_bundle(config: &LoggingConfig, args: &LogsBundleArgs) -> AppResult<()> {
    let since = parse_since(&args.since)
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
    let result = create_support_bundle(
        config,
        &BundleOptions {
            since,
            output: args.output.clone(),
        },
        env!("CARGO_PKG_VERSION"),
    )?;
    let output = json!({
        "output": result.output.display().to_string(),
        "included_files": result.manifest.included_files,
        "omitted_files": result.manifest.omitted_files,
        "truncated": result.manifest.truncated,
        "manifest": "manifest.json",
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_logs_usage(config: &LoggingConfig, args: &LogsUsageArgs) -> AppResult<()> {
    let since = parse_since(&args.since)
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
    let report = analyze_usage(
        &config.dir,
        &UsageOptions {
            since,
            source: args.source.into(),
            limit: args.limit,
            sort: args.sort.into(),
        },
        chrono::Utc::now(),
    )?;
    let output = if args.json {
        if args.pretty {
            serde_json::to_string_pretty(&report)?
        } else {
            serde_json::to_string(&report)?
        }
    } else {
        render_usage_report_text(&report)
    };
    println!("{output}");
    Ok(())
}

fn logs_command_path(args: &LogsArgs) -> String {
    match &args.command {
        LogsCommand::Path => "logs path".to_string(),
        LogsCommand::Bundle(args) => {
            format!(
                "logs bundle --since {} --output {}",
                args.since,
                args.output.display()
            )
        }
        LogsCommand::Usage(args) => {
            format!(
                "logs usage --since {} --source {} --limit {} --sort {}",
                args.since,
                args.source.as_str(),
                args.limit,
                args.sort.as_str()
            )
        }
    }
}

fn render_usage_report_text(report: &UsageReport) -> String {
    let mut lines = vec![
        format!("window_start_utc: {}", report.window_start_utc),
        format!("window_end_utc: {}", report.window_end_utc),
        format!("files_scanned: {}", report.files_scanned.len()),
        format!("events_read: {}", report.events_read),
        format!("events_used: {}", report.events_used),
        format!("events_skipped: {}", report.events_skipped),
    ];
    if report.items.is_empty() {
        lines.push("no tool usage events found for the selected window".to_string());
        return lines.join("\n");
    }

    lines.push(String::new());
    lines.push(
        [
            "source",
            "name",
            "provider",
            "calls",
            "succeeded",
            "failed",
            "incomplete",
            "success_rate",
            "avg_ms",
            "p95_ms",
            "last_seen",
        ]
        .join("\t"),
    );
    for item in &report.items {
        lines.push(render_usage_item_row(item));
    }
    lines.join("\n")
}

fn render_usage_item_row(item: &UsageItem) -> String {
    [
        item.source.clone(),
        item.name.clone(),
        item.provider.clone(),
        item.calls.to_string(),
        item.succeeded.to_string(),
        item.failed.to_string(),
        item.incomplete.to_string(),
        format_ratio(item.success_rate),
        item.avg_duration_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        item.p95_duration_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        item.last_seen_utc
            .clone()
            .unwrap_or_else(|| "-".to_string()),
    ]
    .join("\t")
}

fn format_ratio(value: f64) -> String {
    format!("{:.1}%", value * 100.0)
}

impl From<LogsUsageSourceArg> for UsageSourceFilter {
    fn from(value: LogsUsageSourceArg) -> Self {
        match value {
            LogsUsageSourceArg::All => Self::All,
            LogsUsageSourceArg::Mcp => Self::Mcp,
            LogsUsageSourceArg::Cli => Self::Cli,
        }
    }
}

impl From<LogsUsageSortArg> for UsageSort {
    fn from(value: LogsUsageSortArg) -> Self {
        match value {
            LogsUsageSortArg::Calls => Self::Calls,
            LogsUsageSortArg::Failures => Self::Failures,
            LogsUsageSortArg::SuccessRate => Self::SuccessRate,
            LogsUsageSortArg::AvgDuration => Self::AvgDuration,
            LogsUsageSortArg::Name => Self::Name,
        }
    }
}

impl LogsUsageSourceArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Mcp => "mcp",
            Self::Cli => "cli",
        }
    }
}

impl LogsUsageSortArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Calls => "calls",
            Self::Failures => "failures",
            Self::SuccessRate => "success_rate",
            Self::AvgDuration => "avg_duration",
            Self::Name => "name",
        }
    }
}

fn log_logs_command_event(
    guard: &mut ObservabilityGuard,
    context: &ObservabilityContext,
    event_name: &'static str,
    message: &'static str,
    outcome: Outcome,
    command_path: &str,
    duration_ms: Option<u64>,
) {
    let mut event = LogEvent::new(
        LogLevel::Info,
        LogKind::Cli,
        event_name,
        message,
        "workhub::logs",
        context.mode.clone(),
        context.version.clone(),
        context.run_id.clone(),
        context.pid,
    )
    .with_outcome(outcome)
    .with_field("command.group", "logs");
    event.command_path = Some(command_path.to_string());
    event.duration_ms = duration_ms;
    guard.write_event(&event);
}

fn log_logs_command_failed(
    guard: &mut ObservabilityGuard,
    context: &ObservabilityContext,
    command_path: &str,
    started: Instant,
    error: &(dyn std::error::Error + Send + Sync + 'static),
    exit_code: i32,
) {
    let category = if exit_code == 2 {
        OperationErrorCategory::InvalidInput
    } else {
        OperationErrorCategory::Business
    };
    let envelope = ErrorDiagnosticEnvelope {
        timestamp_utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        kind: LogKind::Cli,
        event: "logs.command.failed".to_string(),
        message: "logs command failed".to_string(),
        target: "workhub::logs".to_string(),
        mode: context.mode.clone(),
        version: context.version.clone(),
        run_id: context.run_id.clone(),
        pid: context.pid,
        correlation: CorrelationIds::default(),
        provider: None,
        operation: Some("logs".to_string()),
        tool_name: None,
        command_path: Some(command_path.to_string()),
        duration_ms: Some(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)),
        exit_code: Some(exit_code),
        error_category: category,
        error_kind: "logs_command_failed".to_string(),
        error_message: error.to_string(),
        phase: "logs_command".to_string(),
        cause_summary: error.to_string(),
        cause_chain: vec![format!("logs command: {error}")],
        impact: "log location or support bundle output was not produced".to_string(),
        remediation_action: "check logs command arguments and filesystem permissions".to_string(),
        remediation_evidence: "run `workhub logs path` to confirm the log directory".to_string(),
        related_log_file: RUN_LOG_FILE.to_string(),
        related_line_hint: format!("run_id={}", context.run_id),
        support_bundle_hint: "workhub logs bundle --since 24h".to_string(),
        payload_policy: PayloadPolicy::Metadata,
        fields: Default::default(),
    };
    guard.write_event(&envelope.to_log_event());
}

fn logs_command_exit_code(error: &(dyn std::error::Error + 'static)) -> i32 {
    if let Some(error) = error.downcast_ref::<io::Error>()
        && error.kind() == io::ErrorKind::InvalidInput
    {
        return 2;
    }
    3
}

fn log_process_started(
    guard: &mut ObservabilityGuard,
    context: &ObservabilityContext,
    config: &LoggingConfig,
    config_path: Option<&PathBuf>,
) {
    let mut event = lifecycle_event(
        context,
        LogLevel::Info,
        "process.started",
        "workhub process started",
        Outcome::Started,
    )
    .with_field("log.dir", config.dir.display().to_string())
    .with_field("log.profile", profile_label(config.profile))
    .with_field("log.targets", json!(target_labels(&config.targets)))
    .with_field("log.dir_source", log_dir_source_label(config.dir_source));
    if let Some(config_path) = config_path {
        event = event.with_field("config.path", config_path.display().to_string());
    }
    guard.write_event(&event);
}

fn log_deprecated_env_warning(
    guard: &mut ObservabilityGuard,
    context: &ObservabilityContext,
    warning: &DeprecatedEnvWarning,
) {
    let event = LogEvent::new(
        LogLevel::Warn,
        LogKind::Config,
        "logging.deprecated_env",
        "deprecated logging environment variable ignored",
        "workhub::observability::config",
        context.mode.clone(),
        context.version.clone(),
        context.run_id.clone(),
        context.pid,
    )
    .with_outcome(Outcome::Skipped)
    .with_field("variable", warning.variable)
    .with_field("replacement", warning.replacement);
    guard.write_event(&event);
}

fn log_runtime_context(
    context: &AppContext,
    guard: &mut ObservabilityGuard,
    observability: &ObservabilityContext,
) {
    let mcp_enabled_tools = context.mcp_enabled_tools().map(|tools| tools.len());
    let availability = context.service_availability();

    let event = LogEvent::new(
        LogLevel::Info,
        LogKind::Config,
        "runtime_config.loaded",
        "runtime configuration loaded",
        "workhub::config",
        observability.mode.clone(),
        observability.version.clone(),
        observability.run_id.clone(),
        observability.pid,
    )
    .with_outcome(Outcome::Succeeded)
    .with_field("mcp_enabled_tools", json!(mcp_enabled_tools))
    .with_field(
        "mcp_disabled_tools",
        json!(context.mcp_disabled_tools().len()),
    )
    .with_field(
        "mcp_enabled_toolsets",
        json!(context.mcp_enabled_toolsets().len()),
    )
    .with_field(
        "services",
        json!({
            "jira": availability.jira,
            "confluence": availability.confluence,
            "gitlab": availability.gitlab,
        }),
    );
    guard.write_event(&event);
}

fn log_lifecycle_event(
    guard: &mut ObservabilityGuard,
    context: &ObservabilityContext,
    level: LogLevel,
    event_name: &'static str,
    message: &'static str,
    outcome: Outcome,
) {
    let event = lifecycle_event(context, level, event_name, message, outcome);
    guard.write_event(&event);
}

fn lifecycle_event(
    context: &ObservabilityContext,
    level: LogLevel,
    event_name: &'static str,
    message: &'static str,
    outcome: Outcome,
) -> LogEvent {
    LogEvent::new(
        level,
        LogKind::Lifecycle,
        event_name,
        message,
        "workhub::main",
        context.mode.clone(),
        context.version.clone(),
        context.run_id.clone(),
        context.pid,
    )
    .with_outcome(outcome)
}

fn log_config_event(
    guard: &mut ObservabilityGuard,
    context: &ObservabilityContext,
    level: LogLevel,
    event_name: &'static str,
    message: &'static str,
    outcome: Outcome,
    fields: serde_json::Value,
) {
    let mut event = LogEvent::new(
        level,
        LogKind::Config,
        event_name,
        message,
        "workhub::config",
        context.mode.clone(),
        context.version.clone(),
        context.run_id.clone(),
        context.pid,
    )
    .with_outcome(outcome);
    if let serde_json::Value::Object(fields) = fields {
        event.fields.extend(fields);
    }
    guard.write_event(&event);
}

fn log_config_failure(
    guard: &mut ObservabilityGuard,
    context: &ObservabilityContext,
    event_name: &'static str,
    message: &'static str,
    error_message: &str,
    exit_code: i32,
    command: Option<&CliCommandLogContext>,
) {
    log_error_event(
        guard,
        context,
        LogKind::Config,
        event_name,
        message,
        command,
        OperationErrorCategory::Config,
        "config_error",
        "configuration",
        error_message,
        "configuration could not be loaded",
        "fix the reported configuration value",
        exit_code,
    );
}

fn log_cli_command_started(
    guard: &mut ObservabilityGuard,
    context: &ObservabilityContext,
    command: &CliCommandLogContext,
) {
    let mut event = LogEvent::new(
        LogLevel::Info,
        LogKind::Cli,
        "cli.command.started",
        "CLI command started",
        "workhub::cli",
        context.mode.clone(),
        context.version.clone(),
        context.run_id.clone(),
        context.pid,
    )
    .with_correlation(command.correlation())
    .with_outcome(Outcome::Started)
    .with_field("output.mode", command.output_mode);
    event.command_path = Some(command.command_path.clone());
    guard.write_event(&event);
    emit_operation_started(&command.operation);
}

fn log_cli_command_completed(
    guard: &mut ObservabilityGuard,
    context: &ObservabilityContext,
    command: &CliCommandLogContext,
    outcome: Outcome,
    exit_code: i32,
) {
    let mut event = LogEvent::new(
        LogLevel::Info,
        LogKind::Cli,
        "cli.command.completed",
        "CLI command completed",
        "workhub::cli",
        context.mode.clone(),
        context.version.clone(),
        context.run_id.clone(),
        context.pid,
    )
    .with_correlation(command.correlation())
    .with_outcome(outcome)
    .with_duration_ms(command.duration_ms())
    .with_field("output.mode", command.output_mode);
    event.command_path = Some(command.command_path.clone());
    event.exit_code = Some(exit_code);
    guard.write_event(&event);
    if outcome == Outcome::Succeeded {
        emit_operation_completed(&command.operation, command.duration_ms());
    }
}

fn log_cli_run_error(
    guard: &mut ObservabilityGuard,
    context: &ObservabilityContext,
    event_name: &'static str,
    error: &cli::CliRunError,
    command: Option<&CliCommandLogContext>,
) {
    if let Some(command) = command {
        log_cli_command_completed(guard, context, command, Outcome::Failed, error.exit_code());
        emit_operation_failed(
            &command.operation,
            &operations::OperationError::business(error.to_string()),
            command.duration_ms(),
        );
    }
    log_error_event(
        guard,
        context,
        LogKind::Cli,
        event_name,
        "CLI command failed",
        command,
        OperationErrorCategory::Business,
        "cli_command_failed",
        "cli_command",
        &error.to_string(),
        "CLI command returned a non-zero exit code",
        "inspect the command arguments and provider configuration",
        error.exit_code(),
    );
}

fn log_error_event(
    guard: &mut ObservabilityGuard,
    context: &ObservabilityContext,
    kind: LogKind,
    event_name: &'static str,
    message: &'static str,
    command: Option<&CliCommandLogContext>,
    category: OperationErrorCategory,
    error_kind: &'static str,
    phase: &'static str,
    error_message: &str,
    impact: &'static str,
    remediation: &'static str,
    exit_code: i32,
) {
    let envelope = ErrorDiagnosticEnvelope {
        timestamp_utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        kind,
        event: event_name.to_string(),
        message: message.to_string(),
        target: "workhub::main".to_string(),
        mode: context.mode.clone(),
        version: context.version.clone(),
        run_id: context.run_id.clone(),
        pid: context.pid,
        correlation: command
            .map(CliCommandLogContext::correlation)
            .unwrap_or_default(),
        provider: None,
        operation: None,
        tool_name: None,
        command_path: command.map(|command| command.command_path.clone()),
        duration_ms: command.map(CliCommandLogContext::duration_ms),
        exit_code: Some(exit_code),
        error_category: category,
        error_kind: error_kind.to_string(),
        error_message: error_message.to_string(),
        phase: phase.to_string(),
        cause_summary: error_message.to_string(),
        cause_chain: vec![format!("{phase}: {error_message}")],
        impact: impact.to_string(),
        remediation_action: remediation.to_string(),
        remediation_evidence: remediation.to_string(),
        related_log_file: RUN_LOG_FILE.to_string(),
        related_line_hint: format!("run_id={}", context.run_id),
        support_bundle_hint: "workhub logs bundle --since 24h".to_string(),
        payload_policy: PayloadPolicy::Metadata,
        fields: Default::default(),
    };
    guard.write_event(&envelope.to_log_event());
}

fn profile_label(profile: LogProfile) -> &'static str {
    match profile {
        LogProfile::Production => "production",
        LogProfile::Support => "support",
        LogProfile::Development => "development",
        LogProfile::Quiet => "quiet",
        LogProfile::Test => "test",
    }
}

fn log_dir_source_label(source: LogDirSource) -> &'static str {
    match source {
        LogDirSource::Platform => "platform",
        LogDirSource::ConfigFile => "config_file",
        LogDirSource::Environment => "environment",
        LogDirSource::WorkspaceFallback => "workspace_fallback",
    }
}

fn target_labels(targets: &std::collections::BTreeSet<LogTarget>) -> Vec<&'static str> {
    targets
        .iter()
        .map(|target| match target {
            LogTarget::Console => "console",
            LogTarget::File => "file",
            LogTarget::ErrorFile => "error_file",
            LogTarget::AuditFile => "audit_file",
        })
        .collect()
}

impl RunMode {
    fn observability_mode(&self) -> RuntimeMode {
        match self {
            Self::Version => RuntimeMode::Version,
            Self::Stdio => RuntimeMode::Stdio,
            Self::StreamHttp { .. } => RuntimeMode::StreamHttp,
            Self::Cli(_) => RuntimeMode::Cli,
            Self::Logs(_) => RuntimeMode::Logs,
        }
    }

    fn explicit_env_file(&self) -> Option<&str> {
        match self {
            Self::StreamHttp { env_file, .. } => env_file.as_deref(),
            Self::Cli(args) => args.env_file.as_deref(),
            Self::Version | Self::Stdio | Self::Logs(_) => None,
        }
    }

    fn loads_dotenv(&self) -> bool {
        matches!(self, Self::StreamHttp { .. } | Self::Cli(_))
    }

    fn reports_dotenv_success(&self) -> bool {
        matches!(self, Self::StreamHttp { .. })
    }
}

async fn run_stdio(
    context: Arc<AppContext>,
    guard: &mut ObservabilityGuard,
    observability: &ObservabilityContext,
) -> AppResult<()> {
    let event = lifecycle_event(
        observability,
        LogLevel::Info,
        "stdio.started",
        "starting MCP stdio server",
        Outcome::Started,
    )
    .with_field("server", mcp::SERVER_NAME);
    guard.write_event(&event);

    let service = match WorkhubMcpServer::new(context).serve(stdio()).await {
        Ok(service) => service,
        Err(error) => {
            log_error_event(
                guard,
                observability,
                LogKind::Lifecycle,
                "stdio.failed",
                "MCP stdio server failed",
                None,
                OperationErrorCategory::Transport,
                "stdio_server_error",
                "stdio_serve",
                &error.to_string(),
                "MCP stdio server stopped before completing",
                "inspect stderr and MCP client transport configuration",
                4,
            );
            return Err(Box::new(error));
        }
    };

    if let Err(error) = service.waiting().await {
        log_error_event(
            guard,
            observability,
            LogKind::Lifecycle,
            "stdio.failed",
            "MCP stdio server failed",
            None,
            OperationErrorCategory::Transport,
            "stdio_wait_error",
            "stdio_wait",
            &error.to_string(),
            "MCP stdio server stopped with an error",
            "inspect stderr and MCP client transport configuration",
            4,
        );
        return Err(Box::new(error));
    }

    log_lifecycle_event(
        guard,
        observability,
        LogLevel::Info,
        "stdio.completed",
        "MCP stdio server completed",
        Outcome::Succeeded,
    );
    Ok(())
}

async fn run_streamhttp(
    config: config::HttpConfig,
    context: Arc<AppContext>,
    guard: &mut ObservabilityGuard,
    observability: &ObservabilityContext,
) -> AppResult<()> {
    let address: SocketAddr = match format!("{}:{}", config.host, config.port).parse() {
        Ok(address) => address,
        Err(error) => {
            log_error_event(
                guard,
                observability,
                LogKind::Lifecycle,
                "streamhttp.bind_address_failed",
                "failed to parse streamable HTTP bind address",
                None,
                OperationErrorCategory::Config,
                "invalid_bind_address",
                "streamhttp_bind",
                &error.to_string(),
                "streamable HTTP server did not start",
                "check MCP_HTTP_HOST and MCP_HTTP_PORT",
                3,
            );
            return Err(Box::new(error));
        }
    };
    let cancellation = CancellationToken::new();
    let server_context = context.clone();
    let service = StreamableHttpService::new(
        move || Ok(WorkhubMcpServer::new(server_context.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default().with_cancellation_token(cancellation.child_token()),
    );
    let app = Router::new()
        .nest_service(config.path.as_str(), service)
        .route("/healthz", get(healthz));
    let listener = match tokio::net::TcpListener::bind(address).await {
        Ok(listener) => listener,
        Err(error) => {
            log_error_event(
                guard,
                observability,
                LogKind::Lifecycle,
                "streamhttp.bind_failed",
                "failed to bind streamable HTTP listener",
                None,
                OperationErrorCategory::Transport,
                "bind_failed",
                "streamhttp_bind",
                &error.to_string(),
                "streamable HTTP server did not start",
                "check host, port, permissions, and existing listeners",
                4,
            );
            return Err(Box::new(error));
        }
    };

    let event = lifecycle_event(
        observability,
        LogLevel::Info,
        "streamhttp.listening",
        "starting MCP streamable HTTP server",
        Outcome::Started,
    )
    .with_field("server", mcp::SERVER_NAME)
    .with_field("address", address.to_string())
    .with_field("endpoint", config.path.clone());
    guard.write_event(&event);

    if let Err(error) = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            cancellation.cancel();
        })
        .await
    {
        log_error_event(
            guard,
            observability,
            LogKind::Lifecycle,
            "streamhttp.failed",
            "streamable HTTP server failed",
            None,
            OperationErrorCategory::Transport,
            "streamhttp_server_error",
            "streamhttp_serve",
            &error.to_string(),
            "streamable HTTP server stopped with an error",
            "inspect bind address, client connections, and network configuration",
            4,
        );
        return Err(Box::new(error));
    }

    log_lifecycle_event(
        guard,
        observability,
        LogLevel::Info,
        "streamhttp.completed",
        "MCP streamable HTTP server completed",
        Outcome::Succeeded,
    );

    Ok(())
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

fn parse_args<I, S>(args: I) -> Result<RunMode, ParseArgsError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    match args.first().map(String::as_str) {
        None => Ok(RunMode::Stdio),
        Some("stdio") if args.len() <= 1 => Ok(RunMode::Stdio),
        Some("stdio") => Err(ParseArgsError::Usage(format!(
            "unexpected argument for stdio: `{}`",
            args[1]
        ))),
        Some("-v") if args.len() == 1 => Ok(RunMode::Version),
        _ => {
            let argv = std::iter::once("workhub".to_string()).chain(args);
            let root = RootArgs::try_parse_from(argv).map_err(ParseArgsError::Clap)?;
            Ok(match root.command {
                RootCommand::Streamhttp(args) => RunMode::StreamHttp {
                    overrides: HttpConfigOverrides {
                        host: args.host,
                        port: args.port,
                        path: args.path,
                    },
                    env_file: args.env_file,
                },
                RootCommand::Cli(args) => RunMode::Cli(Box::new(args)),
                RootCommand::Logs(args) => RunMode::Logs(args),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DEFAULT_HTTP_HOST, DEFAULT_HTTP_PATH, DEFAULT_HTTP_PORT};
    use clap::error::ErrorKind;

    fn merge_http(overrides: HttpConfigOverrides) -> config::HttpConfig {
        config::HttpConfig::from_var_provider(|_| Err(()), overrides).unwrap()
    }

    #[test]
    fn parse_args_defaults_to_stdio() {
        assert_eq!(parse_args(Vec::<String>::new()).unwrap(), RunMode::Stdio);
        assert_eq!(parse_args(["stdio"]).unwrap(), RunMode::Stdio);
        assert!(!parse_args(Vec::<String>::new()).unwrap().loads_dotenv());
        assert!(!parse_args(["stdio"]).unwrap().loads_dotenv());
    }

    #[test]
    fn parse_args_accepts_streamhttp_env_defaults() {
        assert_eq!(
            parse_args(["streamhttp"]).unwrap(),
            RunMode::StreamHttp {
                overrides: HttpConfigOverrides::default(),
                env_file: None,
            }
        );
        assert_eq!(
            merge_http(HttpConfigOverrides::default()),
            config::HttpConfig {
                host: DEFAULT_HTTP_HOST.to_string(),
                port: DEFAULT_HTTP_PORT,
                path: DEFAULT_HTTP_PATH.to_string(),
            }
        );
    }

    #[test]
    fn parse_args_accepts_streamhttp_host_port_and_path() {
        let mode = parse_args([
            "streamhttp",
            "--host",
            "0.0.0.0",
            "--port=9000",
            "--path",
            "alt-mcp",
        ])
        .unwrap();

        assert_eq!(
            mode,
            RunMode::StreamHttp {
                overrides: HttpConfigOverrides {
                    host: Some("0.0.0.0".to_string()),
                    port: Some(9000),
                    path: Some("alt-mcp".to_string()),
                },
                env_file: None,
            }
        );

        assert_eq!(
            merge_http(match mode {
                RunMode::StreamHttp { overrides, .. } => overrides,
                _ => unreachable!("test parsed streamhttp"),
            }),
            config::HttpConfig {
                host: "0.0.0.0".to_string(),
                port: 9000,
                path: "/alt-mcp".to_string(),
            }
        );
    }

    #[test]
    fn parse_args_rejects_invalid_streamhttp_port() {
        for args in [
            ["streamhttp", "--port", "bad"].as_slice(),
            ["streamhttp", "--port=bad"].as_slice(),
        ] {
            let Err(ParseArgsError::Clap(error)) = parse_args(args.iter().copied()) else {
                panic!("expected clap error");
            };
            assert_eq!(error.kind(), ErrorKind::ValueValidation);
        }
    }

    #[test]
    fn parse_args_rejects_unknown_command() {
        for args in [
            ["http"].as_slice(),
            ["acceptance", "jira", "--preflight"].as_slice(),
            ["smoke", "jira", "restricted"].as_slice(),
        ] {
            let Err(ParseArgsError::Clap(error)) = parse_args(args.iter().copied()) else {
                panic!("expected clap error");
            };
            assert_eq!(error.kind(), ErrorKind::InvalidSubcommand);
        }
    }

    #[test]
    fn parse_args_rejects_sse_transport() {
        let Err(ParseArgsError::Clap(error)) = parse_args(["sse"]) else {
            panic!("expected clap error");
        };

        assert_eq!(error.kind(), ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn parse_args_keeps_stdio_fast_path_errors_out_of_clap() {
        let Err(ParseArgsError::Usage(error)) = parse_args(["stdio", "--env-file", ".env"]) else {
            panic!("expected fast-path usage error");
        };

        assert_eq!(error, "unexpected argument for stdio: `--env-file`");
    }

    #[test]
    fn parse_args_accepts_cli_mode_and_env_file() {
        let mode = parse_args([
            "cli",
            "--env-file",
            "workhub.env",
            "--json",
            "jira",
            "project",
            "list",
        ])
        .unwrap();

        assert!(mode.loads_dotenv());
        assert!(!mode.reports_dotenv_success());
        assert_eq!(mode.explicit_env_file(), Some("workhub.env"));
        assert!(matches!(mode, RunMode::Cli(_)));
    }

    #[test]
    fn parse_args_accepts_cli_config_mode() {
        let mode = parse_args(["cli", "config", "path"]).unwrap();

        assert!(mode.loads_dotenv());
        assert!(!mode.reports_dotenv_success());
        assert!(matches!(&mode, RunMode::Cli(args) if args.is_config_command()));
    }

    #[test]
    fn cli_logs_path_mode_is_top_level_and_does_not_load_dotenv() {
        let mode = parse_args(["logs", "path"]).unwrap();

        assert!(!mode.loads_dotenv());
        assert_eq!(mode.explicit_env_file(), None);
        assert_eq!(
            mode,
            RunMode::Logs(LogsArgs {
                command: LogsCommand::Path,
            })
        );
    }

    #[test]
    fn cli_logs_bundle_mode_accepts_since_and_output() {
        let mode = parse_args([
            "logs",
            "bundle",
            "--since",
            "12h",
            "--output",
            "/tmp/workhub-logs.zip",
        ])
        .unwrap();

        assert_eq!(
            mode,
            RunMode::Logs(LogsArgs {
                command: LogsCommand::Bundle(LogsBundleArgs {
                    since: "12h".to_string(),
                    output: PathBuf::from("/tmp/workhub-logs.zip"),
                }),
            })
        );
        let RunMode::Logs(args) = mode else {
            panic!("expected logs mode");
        };
        assert_eq!(
            logs_command_path(&args),
            "logs bundle --since 12h --output /tmp/workhub-logs.zip"
        );
    }

    #[test]
    fn cli_logs_usage_mode_accepts_filters_sort_and_json() {
        let mode = parse_args([
            "logs", "usage", "--since", "7d", "--source", "mcp", "--limit", "10", "--sort",
            "failures", "--json", "--pretty",
        ])
        .unwrap();

        assert_eq!(
            mode,
            RunMode::Logs(LogsArgs {
                command: LogsCommand::Usage(LogsUsageArgs {
                    since: "7d".to_string(),
                    source: LogsUsageSourceArg::Mcp,
                    limit: 10,
                    sort: LogsUsageSortArg::Failures,
                    json: true,
                    pretty: true,
                }),
            })
        );
        let RunMode::Logs(args) = mode else {
            panic!("expected logs mode");
        };
        assert_eq!(
            logs_command_path(&args),
            "logs usage --since 7d --source mcp --limit 10 --sort failures"
        );
    }

    #[test]
    fn parse_args_reports_dotenv_success_only_for_streamhttp() {
        assert!(
            parse_args(["streamhttp", "--env-file", "workhub.env"])
                .unwrap()
                .reports_dotenv_success()
        );
        assert!(
            !parse_args([
                "cli",
                "--env-file",
                "workhub.env",
                "jira",
                "project",
                "list"
            ])
            .unwrap()
            .reports_dotenv_success()
        );
    }

    #[test]
    fn parse_args_allows_nested_cli_help_without_top_level_help_scan() {
        let Err(ParseArgsError::Clap(error)) = parse_args(["cli", "jira", "issue", "--help"])
        else {
            panic!("expected clap display help");
        };

        assert_eq!(error.kind(), ErrorKind::DisplayHelp);
    }

    #[test]
    fn main_run_mode_maps_to_observability_runtime_mode() {
        assert_eq!(RunMode::Version.observability_mode(), RuntimeMode::Version);
        assert_eq!(RunMode::Stdio.observability_mode(), RuntimeMode::Stdio);
        assert_eq!(
            RunMode::StreamHttp {
                overrides: HttpConfigOverrides::default(),
                env_file: None,
            }
            .observability_mode(),
            RuntimeMode::StreamHttp
        );
        assert!(matches!(
            parse_args(["cli", "config", "path"])
                .unwrap()
                .observability_mode(),
            RuntimeMode::Cli
        ));
        assert_eq!(
            parse_args(["logs", "path"]).unwrap().observability_mode(),
            RuntimeMode::Logs
        );
    }

    #[test]
    fn main_lifecycle_event_contains_observability_context() {
        let context = ObservabilityContext::test("run_test");
        let event = lifecycle_event(
            &context,
            LogLevel::Info,
            "process.started",
            "workhub process started",
            Outcome::Started,
        );
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["run_id"], "run_test");
        assert_eq!(value["mode"], "test");
        assert_eq!(value["event"], "process.started");
        assert_eq!(value["outcome"], "started");
    }

    #[test]
    fn logs_usage_text_output_renders_summary_and_table() {
        let report = UsageReport {
            window_start_utc: "2026-06-22T08:00:00.000Z".to_string(),
            window_end_utc: "2026-06-23T08:00:00.000Z".to_string(),
            files_scanned: vec![],
            events_read: 4,
            events_used: 4,
            events_skipped: 0,
            items: vec![UsageItem {
                source: "mcp".to_string(),
                name: "jira_get_issue".to_string(),
                provider: "jira".to_string(),
                calls: 2,
                started: 2,
                succeeded: 1,
                failed: 1,
                incomplete: 0,
                success_rate: 0.5,
                failure_rate: 0.5,
                avg_duration_ms: Some(42),
                p95_duration_ms: Some(80),
                max_duration_ms: Some(80),
                last_seen_utc: Some("2026-06-23T07:00:00.000Z".to_string()),
                last_failure_utc: Some("2026-06-23T07:00:00.000Z".to_string()),
                last_error_kind: Some("mcp_error_-32603".to_string()),
            }],
        };

        let output = render_usage_report_text(&report);

        assert!(output.contains("events_used: 4"));
        assert!(output.contains("source\tname\tprovider\tcalls"));
        assert!(output.contains("mcp\tjira_get_issue\tjira\t2\t1\t1\t0\t50.0%\t42\t80"));
    }

    #[test]
    fn logs_usage_text_output_handles_empty_report() {
        let report = UsageReport {
            window_start_utc: "2026-06-22T08:00:00.000Z".to_string(),
            window_end_utc: "2026-06-23T08:00:00.000Z".to_string(),
            files_scanned: vec![],
            events_read: 0,
            events_used: 0,
            events_skipped: 0,
            items: vec![],
        };

        let output = render_usage_report_text(&report);

        assert!(output.contains("no tool usage events found for the selected window"));
        assert!(!output.contains("source\tname"));
    }

    fn logging_config() -> LoggingConfig {
        LoggingConfig::for_profile(
            LogProfile::Production,
            PathBuf::from("/tmp/logs"),
            LogDirSource::Platform,
        )
    }

    #[test]
    fn mode_console_defaults_match_runtime_output_contract() {
        let mut streamhttp = logging_config();
        apply_mode_console_defaults(
            &RunMode::StreamHttp {
                overrides: HttpConfigOverrides::default(),
                env_file: None,
            },
            &mut streamhttp,
        );
        assert!(streamhttp.targets.contains(&LogTarget::Console));

        let mut quiet_streamhttp = LoggingConfig::for_profile(
            LogProfile::Quiet,
            PathBuf::from("/tmp/logs"),
            LogDirSource::Platform,
        );
        apply_mode_console_defaults(
            &RunMode::StreamHttp {
                overrides: HttpConfigOverrides::default(),
                env_file: None,
            },
            &mut quiet_streamhttp,
        );
        assert!(!quiet_streamhttp.targets.contains(&LogTarget::Console));

        let mut stdio = logging_config();
        apply_mode_console_defaults(&RunMode::Stdio, &mut stdio);
        assert!(!stdio.targets.contains(&LogTarget::Console));

        let mut version = logging_config();
        apply_mode_console_defaults(&RunMode::Version, &mut version);
        assert!(!version.targets.contains(&LogTarget::Console));

        let mut logs = logging_config();
        apply_mode_console_defaults(
            &RunMode::Logs(LogsArgs {
                command: LogsCommand::Path,
            }),
            &mut logs,
        );
        assert!(!logs.targets.contains(&LogTarget::Console));

        let cli = parse_args(["cli", "jira", "project", "list"]).unwrap();
        let mut cli_config = logging_config();
        apply_mode_console_defaults(&cli, &mut cli_config);
        assert!(!cli_config.targets.contains(&LogTarget::Console));

        let cli_verbose = parse_args(["cli", "-v", "jira", "project", "list"]).unwrap();
        let mut cli_verbose_config = logging_config();
        apply_mode_console_defaults(&cli_verbose, &mut cli_verbose_config);
        assert!(cli_verbose_config.targets.contains(&LogTarget::Console));
    }

    #[test]
    fn explicit_log_targets_are_not_changed_by_runtime_mode_defaults() {
        let mut config = logging_config();
        config.targets = [LogTarget::File].into_iter().collect();
        config.targets_source = LogTargetsSource::Environment;
        apply_mode_console_defaults(
            &RunMode::StreamHttp {
                overrides: HttpConfigOverrides::default(),
                env_file: None,
            },
            &mut config,
        );
        assert!(!config.targets.contains(&LogTarget::Console));

        let mut config = logging_config();
        config.targets = [LogTarget::Console].into_iter().collect();
        config.targets_source = LogTargetsSource::ConfigFile;
        apply_mode_console_defaults(&RunMode::Stdio, &mut config);
        assert!(config.targets.contains(&LogTarget::Console));
    }

    #[test]
    fn cli_command_log_context_uses_command_path_and_output_mode() {
        let args = parse_args([
            "cli",
            "--env-file",
            "workhub.env",
            "--json",
            "--pretty",
            "jira",
            "issue",
            "get",
            "ABC-123",
        ])
        .unwrap();
        let RunMode::Cli(cli_args) = &args else {
            panic!("expected CLI mode");
        };
        let raw = [
            "cli",
            "--env-file",
            "workhub.env",
            "--json",
            "--pretty",
            "jira",
            "issue",
            "get",
            "ABC-123",
        ]
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
        let command = CliCommandLogContext::new(&raw, cli_args);

        assert!(command.command_id.starts_with("cmd_"));
        assert_eq!(command.command_path, "jira issue get");
        assert_eq!(command.output_mode, "json_pretty");
    }

    #[test]
    fn cli_command_log_context_omits_config_values_from_command_path() {
        let args = parse_args([
            "cli",
            "config",
            "set",
            "JIRA_PERSONAL_TOKEN",
            "secret-token",
        ])
        .unwrap();
        let RunMode::Cli(cli_args) = &args else {
            panic!("expected CLI mode");
        };
        let raw = [
            "cli",
            "config",
            "set",
            "JIRA_PERSONAL_TOKEN",
            "secret-token",
        ]
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
        let command = CliCommandLogContext::new(&raw, cli_args);

        assert_eq!(command.command_path, "config set");
        assert!(!command.command_path.contains("JIRA_PERSONAL_TOKEN"));
        assert!(!command.command_path.contains("secret-token"));
    }

    #[tokio::test]
    async fn healthz_reports_ok_without_runtime_details() {
        let Json(response) = healthz().await;

        assert_eq!(response.status, "ok");
    }
}
