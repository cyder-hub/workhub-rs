use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use serde::Serialize;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::{
    observability::{
        config::{LogTarget, LoggingConfig},
        rotation::{AUDIT_LOG_FILE, ERROR_LOG_FILE, RUN_LOG_FILE},
    },
    upstream::redaction::redact_text,
};

use flate2::read::GzDecoder;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LogPathSummary {
    pub dir: PathBuf,
    pub targets: Vec<String>,
    pub recent_files: Vec<LogFileSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LogFileSummary {
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BundleOptions {
    pub since: Duration,
    pub output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct BundleManifest {
    pub generated_at_utc: String,
    pub version: String,
    pub log_dir: String,
    pub since_seconds: u64,
    pub included_files: Vec<LogFileSummary>,
    pub omitted_files: Vec<String>,
    pub redaction: String,
    pub max_bytes: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BundleResult {
    pub output: PathBuf,
    pub manifest: BundleManifest,
}

pub(crate) fn summarize_log_path(config: &LoggingConfig) -> io::Result<LogPathSummary> {
    Ok(LogPathSummary {
        dir: config.dir.clone(),
        targets: target_labels(&config.targets),
        recent_files: recent_log_files(&config.dir, None)?
            .into_iter()
            .take(10)
            .map(|entry| LogFileSummary {
                name: entry.name,
                bytes: entry.bytes,
            })
            .collect(),
    })
}

pub(crate) fn create_support_bundle(
    config: &LoggingConfig,
    options: &BundleOptions,
    version: &str,
) -> io::Result<BundleResult> {
    let now = SystemTime::now();
    let cutoff = now
        .checked_sub(options.since)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut files = recent_log_files(&config.dir, Some(cutoff))?;
    files.sort_by(|left, right| left.name.cmp(&right.name));

    let mut manifest = BundleManifest {
        generated_at_utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        version: version.to_string(),
        log_dir: config.dir.display().to_string(),
        since_seconds: options.since.as_secs(),
        included_files: Vec::new(),
        omitted_files: Vec::new(),
        redaction: "secondary redaction via Workhub secret/query/header text redactor".to_string(),
        max_bytes: config.support_bundle.max_bytes,
        truncated: false,
    };

    if let Some(parent) = options.output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let output = File::create(&options.output)?;
    let mut zip = ZipWriter::new(output);
    let file_options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut written_bytes = 0_u64;

    for file in files {
        if written_bytes >= config.support_bundle.max_bytes {
            manifest.truncated = true;
            manifest.omitted_files.push(file.name);
            continue;
        }
        let content = match read_log_text(&file.path) {
            Ok(content) => content,
            Err(error) => {
                manifest
                    .omitted_files
                    .push(format!("{}: {}", file.name, error));
                continue;
            }
        };
        let redacted = redact_text(&content);
        let bytes = redacted.as_bytes();
        if written_bytes + bytes.len() as u64 > config.support_bundle.max_bytes {
            manifest.truncated = true;
            manifest.omitted_files.push(file.name);
            continue;
        }
        zip_io(zip.start_file(format!("logs/{}", file.name), file_options))?;
        zip.write_all(bytes)?;
        written_bytes += bytes.len() as u64;
        manifest.included_files.push(LogFileSummary {
            name: file.name,
            bytes: bytes.len() as u64,
        });
    }

    let runtime_summary = serde_json::json!({
        "log_dir": config.dir.display().to_string(),
        "targets": target_labels(&config.targets),
        "included_files": manifest.included_files,
    });
    zip_io(zip.start_file("runtime-summary.json", file_options))?;
    zip.write_all(json_pretty_io(&runtime_summary)?.as_bytes())?;

    zip_io(zip.start_file("manifest.json", file_options))?;
    zip.write_all(json_pretty_io(&manifest)?.as_bytes())?;
    zip_io(zip.finish())?;

    Ok(BundleResult {
        output: options.output.clone(),
        manifest,
    })
}

pub(crate) fn read_log_text(path: &Path) -> io::Result<String> {
    let mut content = String::new();
    if path.extension().and_then(|extension| extension.to_str()) == Some("gz") {
        GzDecoder::new(File::open(path)?).read_to_string(&mut content)?;
    } else {
        File::open(path)?.read_to_string(&mut content)?;
    }
    Ok(content)
}

fn zip_io<T>(result: zip::result::ZipResult<T>) -> io::Result<T> {
    result.map_err(|error| io::Error::other(error.to_string()))
}

fn json_pretty_io<T: Serialize>(value: &T) -> io::Result<String> {
    serde_json::to_string_pretty(value).map_err(|error| io::Error::other(error.to_string()))
}

pub(crate) fn parse_since(value: &str) -> Result<Duration, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("since must not be empty".to_string());
    }
    let (number, unit) = value.split_at(
        value
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(value.len()),
    );
    let amount = number
        .parse::<u64>()
        .map_err(|_| format!("invalid since value `{value}`"))?;
    if amount == 0 {
        return Err("since must be positive".to_string());
    }
    match unit {
        "h" | "hour" | "hours" => Ok(Duration::from_secs(amount * 60 * 60)),
        "d" | "day" | "days" => Ok(Duration::from_secs(amount * 24 * 60 * 60)),
        "m" | "min" | "mins" | "minute" | "minutes" => Ok(Duration::from_secs(amount * 60)),
        "" | "s" | "sec" | "secs" | "second" | "seconds" => Ok(Duration::from_secs(amount)),
        _ => Err(format!("invalid since unit `{unit}`")),
    }
}

pub(crate) fn recent_log_files(
    dir: &Path,
    cutoff: Option<SystemTime>,
) -> io::Result<Vec<LogFileEntry>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_workhub_log_name(name) {
            continue;
        }
        let name = name.to_string();
        let metadata = entry.metadata()?;
        if let Some(cutoff) = cutoff
            && metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH) < cutoff
        {
            continue;
        }
        entries.push(LogFileEntry {
            path,
            name,
            bytes: metadata.len(),
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        });
    }
    entries.sort_by(|left, right| right.modified.cmp(&left.modified));
    Ok(entries)
}

fn is_workhub_log_name(name: &str) -> bool {
    name == RUN_LOG_FILE
        || name == ERROR_LOG_FILE
        || name == AUDIT_LOG_FILE
        || name.starts_with("workhub.log.")
        || name.starts_with("workhub-error.log.")
        || name.starts_with("workhub-audit.log.")
}

fn target_labels(targets: &std::collections::BTreeSet<LogTarget>) -> Vec<String> {
    targets
        .iter()
        .map(|target| match target {
            LogTarget::Console => "console",
            LogTarget::File => "file",
            LogTarget::ErrorFile => "error_file",
            LogTarget::AuditFile => "audit_file",
        })
        .map(ToString::to_string)
        .collect()
}

#[derive(Debug, Clone)]
pub(crate) struct LogFileEntry {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) bytes: u64,
    pub(crate) modified: SystemTime,
}

#[cfg(test)]
mod tests {
    use crate::observability::config::{LogDirSource, LogProfile};

    use super::*;

    #[test]
    fn parse_since_accepts_common_units() {
        assert_eq!(parse_since("24h").unwrap(), Duration::from_secs(24 * 3600));
        assert_eq!(
            parse_since("2d").unwrap(),
            Duration::from_secs(2 * 24 * 3600)
        );
        assert_eq!(parse_since("30m").unwrap(), Duration::from_secs(30 * 60));
    }

    #[test]
    fn support_bundle_includes_redacted_logs_and_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let log_dir = temp.path().join("logs");
        fs::create_dir_all(&log_dir).unwrap();
        fs::write(
            log_dir.join(RUN_LOG_FILE),
            "token=super-secret\nAuthorization: Bearer abcdef\nvisible",
        )
        .unwrap();
        fs::write(log_dir.join(ERROR_LOG_FILE), "error line").unwrap();
        let mut config =
            LoggingConfig::for_profile(LogProfile::Production, log_dir, LogDirSource::ConfigFile);
        config.support_bundle.max_bytes = 1024 * 1024;
        let output = temp.path().join("bundle.zip");

        let result = create_support_bundle(
            &config,
            &BundleOptions {
                since: Duration::from_secs(24 * 3600),
                output: output.clone(),
            },
            "test",
        )
        .unwrap();

        assert_eq!(result.output, output);
        assert_eq!(result.manifest.included_files.len(), 2);
        let bundle = File::open(output).unwrap();
        let mut archive = zip::ZipArchive::new(bundle).unwrap();
        let mut log_content = String::new();
        archive
            .by_name("logs/workhub.log")
            .unwrap()
            .read_to_string(&mut log_content)
            .unwrap();

        assert!(!log_content.contains("super-secret"));
        assert!(!log_content.contains("Bearer abcdef"));
        assert!(log_content.contains("visible"));
        assert!(archive.by_name("manifest.json").is_ok());
    }
}
