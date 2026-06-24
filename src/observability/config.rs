use std::{
    collections::BTreeSet,
    fmt::{Display, Formatter},
    path::PathBuf,
};

use serde::Deserialize;

use crate::observability::schema::{LogLevel, PayloadPolicy};

pub(crate) const ENV_WORKHUB_LOG_PROFILE: &str = "WORKHUB_LOG_PROFILE";
pub(crate) const ENV_WORKHUB_LOG_LEVEL: &str = "WORKHUB_LOG_LEVEL";
pub(crate) const ENV_WORKHUB_LOG_FILTER: &str = "WORKHUB_LOG_FILTER";
pub(crate) const ENV_WORKHUB_LOG_PAYLOADS: &str = "WORKHUB_LOG_PAYLOADS";
pub(crate) const ENV_WORKHUB_LOG_TARGETS: &str = "WORKHUB_LOG_TARGETS";
pub(crate) const ENV_WORKHUB_LOG_FORMAT: &str = "WORKHUB_LOG_FORMAT";
pub(crate) const ENV_WORKHUB_LOG_DIR: &str = "WORKHUB_LOG_DIR";
pub(crate) const ENV_WORKHUB_LOG_ROTATION: &str = "WORKHUB_LOG_ROTATION";
pub(crate) const ENV_WORKHUB_LOG_MAX_BYTES: &str = "WORKHUB_LOG_MAX_BYTES";
pub(crate) const ENV_WORKHUB_LOG_RETENTION_FILES: &str = "WORKHUB_LOG_RETENTION_FILES";
pub(crate) const ENV_WORKHUB_LOG_RETENTION_DAYS: &str = "WORKHUB_LOG_RETENTION_DAYS";
pub(crate) const ENV_WORKHUB_LOG_COMPRESSION: &str = "WORKHUB_LOG_COMPRESSION";
pub(crate) const ENV_WORKHUB_LOG_BUNDLE_MAX_BYTES: &str = "WORKHUB_LOG_BUNDLE_MAX_BYTES";

pub(crate) const ENV_RUST_LOG_DEPRECATED: &str = "RUST_LOG";
pub(crate) const ENV_MCP_TOOL_CALL_DEBUG_DEPRECATED: &str = "MCP_TOOL_CALL_DEBUG";

pub(crate) const DEFAULT_LOG_MAX_BYTES: u64 = 20 * 1024 * 1024;
pub(crate) const DEFAULT_RETENTION_FILES: u32 = 20;
pub(crate) const DEFAULT_RETENTION_DAYS: u32 = 14;
pub(crate) const DEFAULT_BUNDLE_MAX_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservabilityConfig {
    pub logging: LoggingConfig,
    pub config_path: Option<PathBuf>,
}

impl ObservabilityConfig {
    pub(crate) fn from_env_and_file() -> Result<Self, ObservabilityConfigError> {
        let config_path = workhub_config_path_with(|key| std::env::var(key));
        let file_toml = match &config_path {
            Some(path) if path.exists() => {
                Some(std::fs::read_to_string(path).map_err(|error| {
                    ObservabilityConfigError::ReadConfig {
                        path: path.clone(),
                        message: error.to_string(),
                    }
                })?)
            }
            _ => None,
        };
        let logging = LoggingConfig::from_var_provider_and_toml(
            |key| std::env::var(key),
            file_toml.as_deref(),
        )?;

        Ok(Self {
            logging,
            config_path,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoggingConfig {
    pub profile: LogProfile,
    pub level: LogLevel,
    pub filter: Option<String>,
    pub payloads: PayloadPolicy,
    pub targets: BTreeSet<LogTarget>,
    pub targets_source: LogTargetsSource,
    pub format: ConsoleFormat,
    pub dir: PathBuf,
    pub dir_source: LogDirSource,
    pub rotation: RotationConfig,
    pub retention: RetentionConfig,
    pub compression: bool,
    pub support_bundle: SupportBundleConfig,
    pub deprecated_env_warnings: Vec<DeprecatedEnvWarning>,
}

impl LoggingConfig {
    pub(crate) fn from_env() -> Result<Self, ObservabilityConfigError> {
        Self::from_var_provider_and_toml(|key| std::env::var(key), None)
    }

    pub(crate) fn from_var_provider_and_toml<F, E>(
        mut get_var: F,
        file_toml: Option<&str>,
    ) -> Result<Self, ObservabilityConfigError>
    where
        F: FnMut(&str) -> Result<String, E>,
    {
        let file = parse_file_config(file_toml)?;
        let file_logging = file
            .observability
            .and_then(|observability| observability.logging)
            .unwrap_or_default();

        let env_profile = optional_env(&mut get_var, ENV_WORKHUB_LOG_PROFILE)
            .as_deref()
            .map(|value| parse_profile(ENV_WORKHUB_LOG_PROFILE, value))
            .transpose()?;
        let file_profile = file_logging
            .profile
            .as_deref()
            .map(|value| parse_profile_file("observability.logging.profile", value))
            .transpose()?;
        let profile = env_profile
            .or(file_profile)
            .unwrap_or(LogProfile::Production);

        let (dir, dir_source) = default_log_dir_with(&mut get_var);
        let mut config = Self::for_profile(profile, dir, dir_source);
        apply_file_logging(&mut config, file_logging)?;
        apply_env_logging(&mut config, &mut get_var)?;
        config.deprecated_env_warnings = deprecated_env_warnings(&mut get_var);
        validate_rotation(&config.rotation)?;

        Ok(config)
    }

    pub(crate) fn for_profile(profile: LogProfile, dir: PathBuf, dir_source: LogDirSource) -> Self {
        let mut config = Self {
            profile,
            level: LogLevel::Info,
            filter: None,
            payloads: PayloadPolicy::Metadata,
            targets: default_targets(),
            targets_source: LogTargetsSource::ProfileDefault,
            format: ConsoleFormat::Compact,
            dir,
            dir_source,
            rotation: RotationConfig::default(),
            retention: RetentionConfig::default(),
            compression: true,
            support_bundle: SupportBundleConfig::default(),
            deprecated_env_warnings: Vec::new(),
        };

        match profile {
            LogProfile::Production => {}
            LogProfile::Support => {
                config.level = LogLevel::Debug;
                config.payloads = PayloadPolicy::SanitizedArgs;
            }
            LogProfile::Development => {
                config.level = LogLevel::Trace;
                config.payloads = PayloadPolicy::SanitizedArgs;
            }
            LogProfile::Quiet => {
                config.level = LogLevel::Warn;
                config.payloads = PayloadPolicy::None;
                config.targets = [LogTarget::File, LogTarget::ErrorFile, LogTarget::AuditFile]
                    .into_iter()
                    .collect();
            }
            LogProfile::Test => {
                config.level = LogLevel::Trace;
                config.payloads = PayloadPolicy::SanitizedArgs;
                config.targets = BTreeSet::new();
                config.compression = false;
            }
        }

        config
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum LogProfile {
    Production,
    Support,
    Development,
    Quiet,
    Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum LogTarget {
    Console,
    File,
    ErrorFile,
    AuditFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogTargetsSource {
    ProfileDefault,
    ConfigFile,
    Environment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConsoleFormat {
    Compact,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RotationKind {
    Daily,
    Hourly,
    Size,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RotationConfig {
    pub kinds: BTreeSet<RotationKind>,
    pub max_bytes: u64,
}

impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            kinds: [RotationKind::Daily, RotationKind::Size]
                .into_iter()
                .collect(),
            max_bytes: DEFAULT_LOG_MAX_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetentionConfig {
    pub files: u32,
    pub days: u32,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            files: DEFAULT_RETENTION_FILES,
            days: DEFAULT_RETENTION_DAYS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SupportBundleConfig {
    pub max_bytes: u64,
}

impl Default for SupportBundleConfig {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_BUNDLE_MAX_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogDirSource {
    Platform,
    ConfigFile,
    Environment,
    WorkspaceFallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeprecatedEnvWarning {
    pub variable: &'static str,
    pub replacement: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObservabilityConfigError {
    InvalidValue {
        key: &'static str,
        value: String,
        expected: &'static str,
    },
    InvalidFileValue {
        key: &'static str,
        value: String,
        expected: &'static str,
    },
    InvalidToml {
        message: String,
    },
    ReadConfig {
        path: PathBuf,
        message: String,
    },
}

impl Display for ObservabilityConfigError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidValue {
                key,
                value,
                expected,
            } => write!(
                formatter,
                "invalid {key} value `{value}`; expected {expected}"
            ),
            Self::InvalidFileValue {
                key,
                value,
                expected,
            } => write!(
                formatter,
                "invalid {key} config value `{value}`; expected {expected}"
            ),
            Self::InvalidToml { message } => {
                write!(formatter, "invalid Workhub observability config: {message}")
            }
            Self::ReadConfig { path, message } => {
                write!(
                    formatter,
                    "failed to read Workhub config {}: {message}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for ObservabilityConfigError {}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    observability: Option<FileObservabilityConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct FileObservabilityConfig {
    logging: Option<FileLoggingConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct FileLoggingConfig {
    profile: Option<String>,
    level: Option<String>,
    filter: Option<String>,
    payloads: Option<String>,
    targets: Option<StringList>,
    format: Option<String>,
    dir: Option<PathBuf>,
    rotation: Option<FileRotationConfig>,
    retention: Option<FileRetentionConfig>,
    compression: Option<bool>,
    support_bundle: Option<FileSupportBundleConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct FileRotationConfig {
    kind: Option<StringList>,
    max_bytes: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct FileRetentionConfig {
    files: Option<u32>,
    days: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
struct FileSupportBundleConfig {
    max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum StringList {
    String(String),
    List(Vec<String>),
}

impl StringList {
    fn values(self) -> Vec<String> {
        match self {
            Self::String(value) => parse_csv_tokens(&value),
            Self::List(values) => values,
        }
    }
}

fn parse_file_config(file_toml: Option<&str>) -> Result<FileConfig, ObservabilityConfigError> {
    match file_toml {
        Some(value) if !value.trim().is_empty() => {
            toml::from_str(value).map_err(|error| ObservabilityConfigError::InvalidToml {
                message: error.to_string(),
            })
        }
        _ => Ok(FileConfig::default()),
    }
}

fn apply_file_logging(
    config: &mut LoggingConfig,
    file: FileLoggingConfig,
) -> Result<(), ObservabilityConfigError> {
    if let Some(level) = file.level {
        config.level = parse_level_file("observability.logging.level", &level)?;
    }
    if let Some(filter) = file.filter.and_then(non_empty_trimmed) {
        config.filter = Some(filter);
    }
    if let Some(payloads) = file.payloads {
        config.payloads = parse_payload_policy_file("observability.logging.payloads", &payloads)?;
    }
    if let Some(targets) = file.targets {
        config.targets = parse_targets_file("observability.logging.targets", targets.values())?;
        config.targets_source = LogTargetsSource::ConfigFile;
    }
    if let Some(format) = file.format {
        config.format = parse_console_format_file("observability.logging.format", &format)?;
    }
    if let Some(dir) = file.dir {
        config.dir = dir;
        config.dir_source = LogDirSource::ConfigFile;
    }
    if let Some(rotation) = file.rotation {
        if let Some(kind) = rotation.kind {
            config.rotation.kinds =
                parse_rotation_kinds_file("observability.logging.rotation.kind", kind.values())?;
        }
        if let Some(max_bytes) = rotation.max_bytes {
            config.rotation.max_bytes =
                validate_positive_u64_file("observability.logging.rotation.max_bytes", max_bytes)?;
        }
    }
    if let Some(retention) = file.retention {
        if let Some(files) = retention.files {
            config.retention.files =
                validate_positive_u32_file("observability.logging.retention.files", files)?;
        }
        if let Some(days) = retention.days {
            config.retention.days =
                validate_positive_u32_file("observability.logging.retention.days", days)?;
        }
    }
    if let Some(compression) = file.compression {
        config.compression = compression;
    }
    if let Some(support_bundle) = file.support_bundle
        && let Some(max_bytes) = support_bundle.max_bytes
    {
        config.support_bundle.max_bytes = validate_positive_u64_file(
            "observability.logging.support_bundle.max_bytes",
            max_bytes,
        )?;
    }

    Ok(())
}

fn apply_env_logging<F, E>(
    config: &mut LoggingConfig,
    get_var: &mut F,
) -> Result<(), ObservabilityConfigError>
where
    F: FnMut(&str) -> Result<String, E>,
{
    if let Some(level) = optional_env(get_var, ENV_WORKHUB_LOG_LEVEL) {
        config.level = parse_level(ENV_WORKHUB_LOG_LEVEL, &level)?;
    }
    if let Some(filter) = optional_env(get_var, ENV_WORKHUB_LOG_FILTER) {
        config.filter = Some(filter);
    }
    if let Some(payloads) = optional_env(get_var, ENV_WORKHUB_LOG_PAYLOADS) {
        config.payloads = parse_payload_policy(ENV_WORKHUB_LOG_PAYLOADS, &payloads)?;
    }
    if let Some(targets) = optional_env(get_var, ENV_WORKHUB_LOG_TARGETS) {
        config.targets = parse_targets(ENV_WORKHUB_LOG_TARGETS, parse_csv_tokens(&targets))?;
        config.targets_source = LogTargetsSource::Environment;
    }
    if let Some(format) = optional_env(get_var, ENV_WORKHUB_LOG_FORMAT) {
        config.format = parse_console_format(ENV_WORKHUB_LOG_FORMAT, &format)?;
    }
    if let Some(dir) = optional_env(get_var, ENV_WORKHUB_LOG_DIR) {
        config.dir = PathBuf::from(dir);
        config.dir_source = LogDirSource::Environment;
    }
    if let Some(rotation) = optional_env(get_var, ENV_WORKHUB_LOG_ROTATION) {
        config.rotation.kinds =
            parse_rotation_kinds(ENV_WORKHUB_LOG_ROTATION, parse_csv_tokens(&rotation))?;
    }
    if let Some(max_bytes) = optional_env(get_var, ENV_WORKHUB_LOG_MAX_BYTES) {
        config.rotation.max_bytes = parse_positive_u64(ENV_WORKHUB_LOG_MAX_BYTES, &max_bytes)?;
    }
    if let Some(files) = optional_env(get_var, ENV_WORKHUB_LOG_RETENTION_FILES) {
        config.retention.files = parse_positive_u32(ENV_WORKHUB_LOG_RETENTION_FILES, &files)?;
    }
    if let Some(days) = optional_env(get_var, ENV_WORKHUB_LOG_RETENTION_DAYS) {
        config.retention.days = parse_positive_u32(ENV_WORKHUB_LOG_RETENTION_DAYS, &days)?;
    }
    if let Some(compression) = optional_env(get_var, ENV_WORKHUB_LOG_COMPRESSION) {
        config.compression = parse_bool(ENV_WORKHUB_LOG_COMPRESSION, &compression)?;
    }
    if let Some(max_bytes) = optional_env(get_var, ENV_WORKHUB_LOG_BUNDLE_MAX_BYTES) {
        config.support_bundle.max_bytes =
            parse_positive_u64(ENV_WORKHUB_LOG_BUNDLE_MAX_BYTES, &max_bytes)?;
    }

    Ok(())
}

fn default_targets() -> BTreeSet<LogTarget> {
    [
        LogTarget::Console,
        LogTarget::File,
        LogTarget::ErrorFile,
        LogTarget::AuditFile,
    ]
    .into_iter()
    .collect()
}

fn parse_profile(key: &'static str, value: &str) -> Result<LogProfile, ObservabilityConfigError> {
    parse_profile_inner(value).ok_or_else(|| {
        invalid_value(
            key,
            value,
            "one of production, support, development, quiet, test",
        )
    })
}

fn parse_profile_file(
    key: &'static str,
    value: &str,
) -> Result<LogProfile, ObservabilityConfigError> {
    parse_profile_inner(value).ok_or_else(|| {
        invalid_file_value(
            key,
            value,
            "one of production, support, development, quiet, test",
        )
    })
}

fn parse_profile_inner(value: &str) -> Option<LogProfile> {
    match normalize(value).as_str() {
        "production" => Some(LogProfile::Production),
        "support" => Some(LogProfile::Support),
        "development" | "dev" => Some(LogProfile::Development),
        "quiet" => Some(LogProfile::Quiet),
        "test" => Some(LogProfile::Test),
        _ => None,
    }
}

fn parse_level(key: &'static str, value: &str) -> Result<LogLevel, ObservabilityConfigError> {
    parse_level_inner(value)
        .ok_or_else(|| invalid_value(key, value, "one of trace, debug, info, warn, error"))
}

fn parse_level_file(key: &'static str, value: &str) -> Result<LogLevel, ObservabilityConfigError> {
    parse_level_inner(value)
        .ok_or_else(|| invalid_file_value(key, value, "one of trace, debug, info, warn, error"))
}

fn parse_level_inner(value: &str) -> Option<LogLevel> {
    match normalize(value).as_str() {
        "trace" => Some(LogLevel::Trace),
        "debug" => Some(LogLevel::Debug),
        "info" => Some(LogLevel::Info),
        "warn" | "warning" => Some(LogLevel::Warn),
        "error" => Some(LogLevel::Error),
        _ => None,
    }
}

fn parse_payload_policy(
    key: &'static str,
    value: &str,
) -> Result<PayloadPolicy, ObservabilityConfigError> {
    parse_payload_policy_inner(value)
        .ok_or_else(|| invalid_value(key, value, "one of none, metadata, sanitized_args"))
}

fn parse_payload_policy_file(
    key: &'static str,
    value: &str,
) -> Result<PayloadPolicy, ObservabilityConfigError> {
    parse_payload_policy_inner(value)
        .ok_or_else(|| invalid_file_value(key, value, "one of none, metadata, sanitized_args"))
}

fn parse_payload_policy_inner(value: &str) -> Option<PayloadPolicy> {
    match normalize(value).as_str() {
        "none" => Some(PayloadPolicy::None),
        "metadata" => Some(PayloadPolicy::Metadata),
        "sanitized_args" | "sanitized-args" => Some(PayloadPolicy::SanitizedArgs),
        _ => None,
    }
}

fn parse_targets<I, S>(
    key: &'static str,
    values: I,
) -> Result<BTreeSet<LogTarget>, ObservabilityConfigError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    parse_targets_inner(values).map_err(|value| {
        invalid_value(
            key,
            &value,
            "comma-separated console,file,error_file,audit_file,all,none",
        )
    })
}

fn parse_targets_file<I, S>(
    key: &'static str,
    values: I,
) -> Result<BTreeSet<LogTarget>, ObservabilityConfigError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    parse_targets_inner(values).map_err(|value| {
        invalid_file_value(
            key,
            &value,
            "console, file, error_file, audit_file, all, or none",
        )
    })
}

fn parse_targets_inner<I, S>(values: I) -> Result<BTreeSet<LogTarget>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut targets = BTreeSet::new();
    let values = values.into_iter().collect::<Vec<_>>();
    if values.is_empty() {
        return Ok(targets);
    }

    for value in values {
        match normalize(value.as_ref()).as_str() {
            "all" => return Ok(default_targets()),
            "none" => targets.clear(),
            "console" => {
                targets.insert(LogTarget::Console);
            }
            "file" => {
                targets.insert(LogTarget::File);
            }
            "error_file" | "error-file" => {
                targets.insert(LogTarget::ErrorFile);
            }
            "audit_file" | "audit-file" => {
                targets.insert(LogTarget::AuditFile);
            }
            "" => {}
            _ => return Err(value.as_ref().to_string()),
        }
    }

    Ok(targets)
}

fn parse_console_format(
    key: &'static str,
    value: &str,
) -> Result<ConsoleFormat, ObservabilityConfigError> {
    parse_console_format_inner(value)
        .ok_or_else(|| invalid_value(key, value, "one of compact, json"))
}

fn parse_console_format_file(
    key: &'static str,
    value: &str,
) -> Result<ConsoleFormat, ObservabilityConfigError> {
    parse_console_format_inner(value)
        .ok_or_else(|| invalid_file_value(key, value, "one of compact, json"))
}

fn parse_console_format_inner(value: &str) -> Option<ConsoleFormat> {
    match normalize(value).as_str() {
        "compact" => Some(ConsoleFormat::Compact),
        "json" => Some(ConsoleFormat::Json),
        _ => None,
    }
}

fn parse_rotation_kinds<I, S>(
    key: &'static str,
    values: I,
) -> Result<BTreeSet<RotationKind>, ObservabilityConfigError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let kinds = parse_rotation_kinds_inner(values)
        .map_err(|value| invalid_value(key, &value, "comma-separated daily,hourly,size"))?;
    validate_rotation_kinds(key, kinds, false)
}

fn parse_rotation_kinds_file<I, S>(
    key: &'static str,
    values: I,
) -> Result<BTreeSet<RotationKind>, ObservabilityConfigError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let kinds = parse_rotation_kinds_inner(values)
        .map_err(|value| invalid_file_value(key, &value, "daily, hourly, size"))?;
    validate_rotation_kinds(key, kinds, true)
}

fn parse_rotation_kinds_inner<I, S>(values: I) -> Result<BTreeSet<RotationKind>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut kinds = BTreeSet::new();
    for value in values {
        match normalize(value.as_ref()).as_str() {
            "daily" => {
                kinds.insert(RotationKind::Daily);
            }
            "hourly" => {
                kinds.insert(RotationKind::Hourly);
            }
            "size" => {
                kinds.insert(RotationKind::Size);
            }
            "" => {}
            _ => return Err(value.as_ref().to_string()),
        }
    }

    if kinds.is_empty() {
        kinds.insert(RotationKind::Daily);
    }

    Ok(kinds)
}

fn validate_rotation_kinds(
    key: &'static str,
    kinds: BTreeSet<RotationKind>,
    file_value: bool,
) -> Result<BTreeSet<RotationKind>, ObservabilityConfigError> {
    if kinds.contains(&RotationKind::Daily) && kinds.contains(&RotationKind::Hourly) {
        let value = "daily,hourly";
        return if file_value {
            Err(invalid_file_value(
                key,
                value,
                "daily or hourly, not both; size may be combined with either",
            ))
        } else {
            Err(invalid_value(
                key,
                value,
                "daily or hourly, not both; size may be combined with either",
            ))
        };
    }
    Ok(kinds)
}

fn validate_rotation(rotation: &RotationConfig) -> Result<(), ObservabilityConfigError> {
    validate_rotation_kinds(ENV_WORKHUB_LOG_ROTATION, rotation.kinds.clone(), false)?;
    Ok(())
}

fn parse_positive_u64(key: &'static str, value: &str) -> Result<u64, ObservabilityConfigError> {
    let parsed = value
        .trim()
        .parse::<u64>()
        .map_err(|_| invalid_value(key, value, "a positive integer"))?;
    if parsed == 0 {
        return Err(invalid_value(key, value, "a positive integer"));
    }
    Ok(parsed)
}

fn parse_positive_u32(key: &'static str, value: &str) -> Result<u32, ObservabilityConfigError> {
    let parsed = value
        .trim()
        .parse::<u32>()
        .map_err(|_| invalid_value(key, value, "a positive integer"))?;
    if parsed == 0 {
        return Err(invalid_value(key, value, "a positive integer"));
    }
    Ok(parsed)
}

fn validate_positive_u64_file(
    key: &'static str,
    value: u64,
) -> Result<u64, ObservabilityConfigError> {
    if value == 0 {
        return Err(invalid_file_value(key, "0", "a positive integer"));
    }
    Ok(value)
}

fn validate_positive_u32_file(
    key: &'static str,
    value: u32,
) -> Result<u32, ObservabilityConfigError> {
    if value == 0 {
        return Err(invalid_file_value(key, "0", "a positive integer"));
    }
    Ok(value)
}

fn parse_bool(key: &'static str, value: &str) -> Result<bool, ObservabilityConfigError> {
    match normalize(value).as_str() {
        "true" | "1" | "yes" | "y" | "on" => Ok(true),
        "false" | "0" | "no" | "n" | "off" => Ok(false),
        _ => Err(invalid_value(
            key,
            value,
            "one of true,false,1,0,yes,no,on,off",
        )),
    }
}

fn optional_env<F, E>(get_var: &mut F, key: &'static str) -> Option<String>
where
    F: FnMut(&str) -> Result<String, E>,
{
    get_var(key).ok().and_then(non_empty_trimmed)
}

fn deprecated_env_warnings<F, E>(get_var: &mut F) -> Vec<DeprecatedEnvWarning>
where
    F: FnMut(&str) -> Result<String, E>,
{
    let mut warnings = Vec::new();
    if optional_env(get_var, ENV_RUST_LOG_DEPRECATED).is_some() {
        warnings.push(DeprecatedEnvWarning {
            variable: ENV_RUST_LOG_DEPRECATED,
            replacement: ENV_WORKHUB_LOG_FILTER,
        });
    }
    if optional_env(get_var, ENV_MCP_TOOL_CALL_DEBUG_DEPRECATED).is_some() {
        warnings.push(DeprecatedEnvWarning {
            variable: ENV_MCP_TOOL_CALL_DEBUG_DEPRECATED,
            replacement: ENV_WORKHUB_LOG_PAYLOADS,
        });
    }
    warnings
}

fn default_log_dir_with<F, E>(mut get_var: F) -> (PathBuf, LogDirSource)
where
    F: FnMut(&str) -> Result<String, E>,
{
    #[cfg(target_os = "windows")]
    {
        if let Some(path) = non_empty_env(&mut get_var, "LOCALAPPDATA") {
            return (
                PathBuf::from(path).join("workhub").join("logs"),
                LogDirSource::Platform,
            );
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(path) = non_empty_env(&mut get_var, "HOME") {
            return (
                PathBuf::from(path)
                    .join("Library")
                    .join("Logs")
                    .join("workhub"),
                LogDirSource::Platform,
            );
        }
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        if let Some(path) = non_empty_env(&mut get_var, "XDG_STATE_HOME") {
            return (
                PathBuf::from(path).join("workhub").join("logs"),
                LogDirSource::Platform,
            );
        }
        if let Some(path) = non_empty_env(&mut get_var, "HOME") {
            return (
                PathBuf::from(path)
                    .join(".local")
                    .join("state")
                    .join("workhub")
                    .join("logs"),
                LogDirSource::Platform,
            );
        }
    }

    (
        PathBuf::from(".workhub").join("logs"),
        LogDirSource::WorkspaceFallback,
    )
}

pub(crate) fn workhub_config_path_with<F, E>(mut get_var: F) -> Option<PathBuf>
where
    F: FnMut(&str) -> Result<String, E>,
{
    #[cfg(target_os = "windows")]
    {
        return non_empty_env(&mut get_var, "APPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("workhub").join("config.toml"));
    }

    #[cfg(target_os = "macos")]
    {
        return non_empty_env(&mut get_var, "HOME").map(|path| {
            PathBuf::from(path)
                .join("Library")
                .join("Application Support")
                .join("workhub")
                .join("config.toml")
        });
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        if let Some(path) = non_empty_env(&mut get_var, "XDG_CONFIG_HOME") {
            return Some(PathBuf::from(path).join("workhub").join("config.toml"));
        }

        non_empty_env(&mut get_var, "HOME").map(|path| {
            PathBuf::from(path)
                .join(".config")
                .join("workhub")
                .join("config.toml")
        })
    }
}

fn non_empty_env<F, E>(get_var: &mut F, key: &str) -> Option<String>
where
    F: FnMut(&str) -> Result<String, E>,
{
    get_var(key).ok().and_then(non_empty_trimmed)
}

fn non_empty_trimmed(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn parse_csv_tokens(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('-', "_")
}

fn invalid_value(
    key: &'static str,
    value: &str,
    expected: &'static str,
) -> ObservabilityConfigError {
    ObservabilityConfigError::InvalidValue {
        key,
        value: value.to_string(),
        expected,
    }
}

fn invalid_file_value(
    key: &'static str,
    value: &str,
    expected: &'static str,
) -> ObservabilityConfigError {
    ObservabilityConfigError::InvalidFileValue {
        key,
        value: value.to_string(),
        expected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_provider<'a>(
        pairs: &'a [(&'a str, &'a str)],
    ) -> impl FnMut(&str) -> Result<String, ()> + 'a {
        move |key| {
            pairs
                .iter()
                .find_map(|(env_key, value)| (*env_key == key).then(|| (*value).to_string()))
                .ok_or(())
        }
    }

    fn parse_with(
        env: &[(&str, &str)],
        file_toml: Option<&str>,
    ) -> Result<LoggingConfig, ObservabilityConfigError> {
        LoggingConfig::from_var_provider_and_toml(env_provider(env), file_toml)
    }

    #[test]
    fn profile_defaults_are_fixed() {
        let base_dir = PathBuf::from("/tmp/logs");
        let production = LoggingConfig::for_profile(
            LogProfile::Production,
            base_dir.clone(),
            LogDirSource::Platform,
        );
        let support = LoggingConfig::for_profile(
            LogProfile::Support,
            base_dir.clone(),
            LogDirSource::Platform,
        );
        let development = LoggingConfig::for_profile(
            LogProfile::Development,
            base_dir.clone(),
            LogDirSource::Platform,
        );
        let quiet =
            LoggingConfig::for_profile(LogProfile::Quiet, base_dir.clone(), LogDirSource::Platform);
        let test = LoggingConfig::for_profile(LogProfile::Test, base_dir, LogDirSource::Platform);

        assert_eq!(production.level, LogLevel::Info);
        assert_eq!(production.payloads, PayloadPolicy::Metadata);
        assert!(production.targets.contains(&LogTarget::Console));
        assert_eq!(production.targets_source, LogTargetsSource::ProfileDefault);
        assert_eq!(support.level, LogLevel::Debug);
        assert_eq!(support.payloads, PayloadPolicy::SanitizedArgs);
        assert_eq!(development.level, LogLevel::Trace);
        assert_eq!(quiet.level, LogLevel::Warn);
        assert!(!quiet.targets.contains(&LogTarget::Console));
        assert_eq!(test.level, LogLevel::Trace);
        assert!(test.targets.is_empty());
        assert!(!test.compression);
    }

    #[test]
    fn toml_config_is_loaded_and_env_overrides_it() {
        let file_toml = r#"
            [observability.logging]
            profile = "quiet"
            level = "error"
            filter = "workhub=error"
            payloads = "none"
            targets = ["file"]
            format = "json"
            dir = "/from-file"
            compression = true

            [observability.logging.rotation]
            kind = ["hourly", "size"]
            max_bytes = 1024

            [observability.logging.retention]
            files = 2
            days = 3

            [observability.logging.support_bundle]
            max_bytes = 4096
        "#;
        let config = parse_with(
            &[
                ("HOME", "/home/user"),
                (ENV_WORKHUB_LOG_PROFILE, "support"),
                (ENV_WORKHUB_LOG_LEVEL, "warn"),
                (ENV_WORKHUB_LOG_FILTER, "workhub=debug"),
                (ENV_WORKHUB_LOG_PAYLOADS, "sanitized_args"),
                (ENV_WORKHUB_LOG_TARGETS, "console,error_file"),
                (ENV_WORKHUB_LOG_FORMAT, "compact"),
                (ENV_WORKHUB_LOG_DIR, "/from-env"),
                (ENV_WORKHUB_LOG_ROTATION, "daily,size"),
                (ENV_WORKHUB_LOG_MAX_BYTES, "2048"),
                (ENV_WORKHUB_LOG_RETENTION_FILES, "4"),
                (ENV_WORKHUB_LOG_RETENTION_DAYS, "5"),
                (ENV_WORKHUB_LOG_COMPRESSION, "false"),
                (ENV_WORKHUB_LOG_BUNDLE_MAX_BYTES, "8192"),
            ],
            Some(file_toml),
        )
        .unwrap();

        assert_eq!(config.profile, LogProfile::Support);
        assert_eq!(config.level, LogLevel::Warn);
        assert_eq!(config.filter.as_deref(), Some("workhub=debug"));
        assert_eq!(config.payloads, PayloadPolicy::SanitizedArgs);
        assert_eq!(
            config.targets,
            [LogTarget::Console, LogTarget::ErrorFile]
                .into_iter()
                .collect()
        );
        assert_eq!(config.targets_source, LogTargetsSource::Environment);
        assert_eq!(config.format, ConsoleFormat::Compact);
        assert_eq!(config.dir, PathBuf::from("/from-env"));
        assert_eq!(config.dir_source, LogDirSource::Environment);
        assert_eq!(
            config.rotation.kinds,
            [RotationKind::Daily, RotationKind::Size]
                .into_iter()
                .collect()
        );
        assert_eq!(config.rotation.max_bytes, 2048);
        assert_eq!(config.retention.files, 4);
        assert_eq!(config.retention.days, 5);
        assert!(!config.compression);
        assert_eq!(config.support_bundle.max_bytes, 8192);
    }

    #[test]
    fn file_config_can_drive_values_without_env_overrides() {
        let file_toml = r#"
            [observability.logging]
            profile = "support"
            level = "debug"
            targets = "file,error_file,audit_file"
            dir = "/from-file"
        "#;
        let config = parse_with(&[("HOME", "/home/user")], Some(file_toml)).unwrap();

        assert_eq!(config.profile, LogProfile::Support);
        assert_eq!(config.level, LogLevel::Debug);
        assert_eq!(config.dir, PathBuf::from("/from-file"));
        assert_eq!(config.dir_source, LogDirSource::ConfigFile);
        assert_eq!(config.targets_source, LogTargetsSource::ConfigFile);
        assert!(!config.targets.contains(&LogTarget::Console));
        assert!(config.targets.contains(&LogTarget::AuditFile));
    }

    #[test]
    fn invalid_env_values_are_config_errors() {
        let error = parse_with(&[(ENV_WORKHUB_LOG_LEVEL, "verbose")], None).unwrap_err();
        assert!(matches!(
            error,
            ObservabilityConfigError::InvalidValue {
                key: ENV_WORKHUB_LOG_LEVEL,
                ..
            }
        ));

        let error = parse_with(&[(ENV_WORKHUB_LOG_MAX_BYTES, "0")], None).unwrap_err();
        assert!(matches!(
            error,
            ObservabilityConfigError::InvalidValue {
                key: ENV_WORKHUB_LOG_MAX_BYTES,
                ..
            }
        ));

        let error = parse_with(&[(ENV_WORKHUB_LOG_ROTATION, "daily,hourly")], None).unwrap_err();
        assert!(matches!(
            error,
            ObservabilityConfigError::InvalidValue {
                key: ENV_WORKHUB_LOG_ROTATION,
                ..
            }
        ));
    }

    #[test]
    fn deprecated_old_logging_env_vars_are_reported_but_not_used() {
        let config = parse_with(
            &[
                ("HOME", "/home/user"),
                (ENV_RUST_LOG_DEPRECATED, "trace"),
                (ENV_MCP_TOOL_CALL_DEBUG_DEPRECATED, "true"),
            ],
            None,
        )
        .unwrap();

        assert_eq!(config.level, LogLevel::Info);
        assert_eq!(config.filter, None);
        assert_eq!(
            config.deprecated_env_warnings,
            vec![
                DeprecatedEnvWarning {
                    variable: ENV_RUST_LOG_DEPRECATED,
                    replacement: ENV_WORKHUB_LOG_FILTER,
                },
                DeprecatedEnvWarning {
                    variable: ENV_MCP_TOOL_CALL_DEBUG_DEPRECATED,
                    replacement: ENV_WORKHUB_LOG_PAYLOADS,
                },
            ]
        );
    }

    #[test]
    fn default_paths_follow_platform_rules() {
        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            let (dir, source) = default_log_dir_with(env_provider(&[("XDG_STATE_HOME", "/state")]));
            assert_eq!(dir, PathBuf::from("/state/workhub/logs"));
            assert_eq!(source, LogDirSource::Platform);

            let path =
                workhub_config_path_with(env_provider(&[("XDG_CONFIG_HOME", "/config")])).unwrap();
            assert_eq!(path, PathBuf::from("/config/workhub/config.toml"));
        }
    }

    #[test]
    fn missing_platform_dir_falls_back_to_workspace_logs() {
        let (dir, source) = default_log_dir_with(env_provider(&[]));

        assert_eq!(dir, PathBuf::from(".workhub/logs"));
        assert_eq!(source, LogDirSource::WorkspaceFallback);
    }
}
