use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Duration, NaiveDate, Utc};
use flate2::{Compression, write::GzEncoder};

use crate::observability::config::{RetentionConfig, RotationConfig, RotationKind};

pub(crate) const RUN_LOG_FILE: &str = "workhub.log";
pub(crate) const ERROR_LOG_FILE: &str = "workhub-error.log";
pub(crate) const AUDIT_LOG_FILE: &str = "workhub-audit.log";

#[derive(Debug)]
pub(crate) struct RotatingLogFile {
    dir: PathBuf,
    active_name: &'static str,
    active_path: PathBuf,
    rotation: RotationConfig,
    retention: RetentionConfig,
    compression: bool,
    file: File,
    period: String,
}

impl RotatingLogFile {
    pub(crate) fn open(
        dir: impl Into<PathBuf>,
        active_name: &'static str,
        rotation: RotationConfig,
        retention: RetentionConfig,
        compression: bool,
        now: DateTime<Utc>,
    ) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let active_path = dir.join(active_name);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active_path)?;

        let metadata = file.metadata()?;
        let period_time = active_period_time(&metadata, now);
        let period = rotation_period(period_time, &rotation_period_kind(&rotation));

        Ok(Self {
            dir,
            active_name,
            active_path,
            rotation,
            retention,
            compression,
            file,
            period,
        })
    }

    pub(crate) fn write_line(&mut self, line: &str, now: DateTime<Utc>) -> io::Result<()> {
        if self.should_rotate(line, now)? {
            self.rotate(now)?;
        }

        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.file.flush()
    }

    pub(crate) fn cleanup(&self, now: DateTime<Utc>) -> io::Result<Vec<PathBuf>> {
        cleanup_rotated_files(&self.dir, self.active_name, &self.retention, now)
    }

    fn should_rotate(&self, line: &str, now: DateTime<Utc>) -> io::Result<bool> {
        let metadata = match self.file.metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        if metadata.len() == 0 {
            return Ok(false);
        }

        let period_kind = rotation_period_kind(&self.rotation);
        let period_changed = self.rotation.kinds.contains(&RotationKind::Daily)
            || self.rotation.kinds.contains(&RotationKind::Hourly);
        if period_changed && self.period != rotation_period(now, &period_kind) {
            return Ok(true);
        }

        let projected_size = metadata.len() + line.len() as u64 + 1;
        Ok(self.rotation.kinds.contains(&RotationKind::Size)
            && projected_size > self.rotation.max_bytes)
    }

    fn rotate(&mut self, now: DateTime<Utc>) -> io::Result<()> {
        self.file.flush()?;
        let rotated_path = next_rotated_path(&self.dir, self.active_name, &self.period)?;
        fs::rename(&self.active_path, &rotated_path)?;
        if self.compression {
            gzip_file(&rotated_path)?;
        }
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.active_path)?;
        self.period = rotation_period(now, &rotation_period_kind(&self.rotation));
        let _ = self.cleanup(now)?;
        Ok(())
    }
}

fn active_period_time(metadata: &fs::Metadata, now: DateTime<Utc>) -> DateTime<Utc> {
    if metadata.len() == 0 {
        return now;
    }

    metadata
        .modified()
        .map(DateTime::<Utc>::from)
        .unwrap_or(now)
}

pub(crate) fn cleanup_rotated_files(
    dir: &Path,
    active_name: &'static str,
    retention: &RetentionConfig,
    now: DateTime<Utc>,
) -> io::Result<Vec<PathBuf>> {
    let mut rotated = rotated_logs(dir, active_name)?;
    let mut removed = Vec::new();
    let keep_after = now.date_naive() - Duration::days(retention.days as i64);

    rotated.retain(|entry| {
        if let Some(date) = entry.date
            && date < keep_after
        {
            if fs::remove_file(&entry.path).is_ok() {
                removed.push(entry.path.clone());
            }
            return false;
        }
        true
    });

    rotated.sort_by(|left, right| {
        right
            .period
            .cmp(&left.period)
            .then_with(|| right.index.cmp(&left.index))
    });
    for entry in rotated.into_iter().skip(retention.files as usize) {
        if fs::remove_file(&entry.path).is_ok() {
            removed.push(entry.path);
        }
    }

    Ok(removed)
}

fn gzip_file(path: &Path) -> io::Result<PathBuf> {
    let gzip_path = path.with_file_name(format!(
        "{}.gz",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("rotated.log")
    ));
    let mut input = File::open(path)?;
    let output = File::create(&gzip_path)?;
    let mut encoder = GzEncoder::new(output, Compression::default());
    let mut buffer = Vec::new();
    input.read_to_end(&mut buffer)?;
    encoder.write_all(&buffer)?;
    encoder.finish()?;
    fs::remove_file(path)?;
    Ok(gzip_path)
}

fn next_rotated_path(dir: &Path, active_name: &'static str, period: &str) -> io::Result<PathBuf> {
    let next_index = rotated_logs(dir, active_name)?
        .into_iter()
        .filter(|entry| entry.period == period)
        .map(|entry| entry.index)
        .max()
        .map_or(0, |index| index + 1);
    Ok(dir.join(format!("{active_name}.{period}.{next_index}")))
}

fn rotation_period_kind(rotation: &RotationConfig) -> RotationKind {
    if rotation.kinds.contains(&RotationKind::Hourly) {
        RotationKind::Hourly
    } else {
        RotationKind::Daily
    }
}

fn rotation_period(now: DateTime<Utc>, kind: &RotationKind) -> String {
    match kind {
        RotationKind::Hourly => now.format("%Y-%m-%d-%H").to_string(),
        RotationKind::Daily | RotationKind::Size => now.format("%Y-%m-%d").to_string(),
    }
}

#[derive(Debug, Clone)]
struct RotatedLog {
    path: PathBuf,
    period: String,
    index: u32,
    date: Option<NaiveDate>,
}

fn rotated_logs(dir: &Path, active_name: &'static str) -> io::Result<Vec<RotatedLog>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let prefix = format!("{active_name}.");
    let mut logs = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(file_name) = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToString::to_string)
        else {
            continue;
        };
        let Some(rest) = file_name.strip_prefix(&prefix) else {
            continue;
        };
        let rest = rest.strip_suffix(".gz").unwrap_or(rest);
        let Some((period, index)) = rest.rsplit_once('.') else {
            continue;
        };
        let Ok(index) = index.parse::<u32>() else {
            continue;
        };
        logs.push(RotatedLog {
            path,
            period: period.to_string(),
            index,
            date: period
                .get(..10)
                .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()),
        });
    }

    Ok(logs)
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, FileTimes},
        time::{Duration as StdDuration, SystemTime, UNIX_EPOCH},
    };

    use chrono::TimeZone;

    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 22, 6, 0, 0).unwrap()
    }

    fn system_time(time: DateTime<Utc>) -> SystemTime {
        UNIX_EPOCH + StdDuration::from_secs(time.timestamp() as u64)
    }

    fn rotation(max_bytes: u64) -> RotationConfig {
        RotationConfig {
            kinds: [RotationKind::Daily, RotationKind::Size]
                .into_iter()
                .collect(),
            max_bytes,
        }
    }

    #[test]
    fn rotating_log_file_rotates_by_size_and_compresses_old_file() {
        let temp = tempfile::tempdir().unwrap();
        let mut file = RotatingLogFile::open(
            temp.path(),
            RUN_LOG_FILE,
            rotation(80),
            RetentionConfig {
                files: 20,
                days: 14,
            },
            true,
            now(),
        )
        .unwrap();

        file.write_line("{\"message\":\"first line with enough bytes\"}", now())
            .unwrap();
        file.write_line("{\"message\":\"second line forcing rotation\"}", now())
            .unwrap();

        assert!(temp.path().join(RUN_LOG_FILE).exists());
        let rotated = fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(
            rotated
                .iter()
                .any(|name| name.starts_with("workhub.log.2026-06-22.") && name.ends_with(".gz"))
        );
    }

    #[test]
    fn rotating_log_file_rotates_stale_active_file_on_first_write() {
        let temp = tempfile::tempdir().unwrap();
        let active_path = temp.path().join(RUN_LOG_FILE);
        fs::write(&active_path, "old line\n").unwrap();
        let old_time = Utc.with_ymd_and_hms(2026, 6, 21, 23, 0, 0).unwrap();
        let active = OpenOptions::new().write(true).open(&active_path).unwrap();
        active
            .set_times(FileTimes::new().set_modified(system_time(old_time)))
            .unwrap();
        drop(active);

        let mut file = RotatingLogFile::open(
            temp.path(),
            RUN_LOG_FILE,
            rotation(1024 * 1024),
            RetentionConfig {
                files: 20,
                days: 14,
            },
            false,
            now(),
        )
        .unwrap();

        file.write_line("new line", now()).unwrap();

        assert_eq!(
            fs::read_to_string(temp.path().join("workhub.log.2026-06-21.0")).unwrap(),
            "old line\n"
        );
        assert_eq!(fs::read_to_string(active_path).unwrap(), "new line\n");
    }

    #[test]
    fn cleanup_applies_retention_by_files_and_days() {
        let temp = tempfile::tempdir().unwrap();
        for name in [
            "workhub.log.2026-06-20.0.gz",
            "workhub.log.2026-06-21.0.gz",
            "workhub.log.2026-06-22.0.gz",
            "workhub.log.2000-01-01.0.gz",
        ] {
            fs::write(temp.path().join(name), "x").unwrap();
        }

        let removed = cleanup_rotated_files(
            temp.path(),
            RUN_LOG_FILE,
            &RetentionConfig { files: 2, days: 14 },
            now(),
        )
        .unwrap();
        let remaining = fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert_eq!(removed.len(), 2);
        assert!(remaining.contains(&"workhub.log.2026-06-22.0.gz".to_string()));
        assert!(remaining.contains(&"workhub.log.2026-06-21.0.gz".to_string()));
        assert!(!remaining.contains(&"workhub.log.2026-06-20.0.gz".to_string()));
        assert!(!remaining.contains(&"workhub.log.2000-01-01.0.gz".to_string()));
    }
}
