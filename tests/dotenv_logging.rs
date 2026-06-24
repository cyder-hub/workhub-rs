use std::{fs, process::Command};

const WORKHUB_LOG_ENV_KEYS: &[&str] = &[
    "WORKHUB_LOG_PROFILE",
    "WORKHUB_LOG_LEVEL",
    "WORKHUB_LOG_FILTER",
    "WORKHUB_LOG_PAYLOADS",
    "WORKHUB_LOG_TARGETS",
    "WORKHUB_LOG_FORMAT",
    "WORKHUB_LOG_DIR",
    "WORKHUB_LOG_ROTATION",
    "WORKHUB_LOG_MAX_BYTES",
    "WORKHUB_LOG_RETENTION_FILES",
    "WORKHUB_LOG_RETENTION_DAYS",
    "WORKHUB_LOG_COMPRESSION",
    "WORKHUB_LOG_BUNDLE_MAX_BYTES",
    "MCP_TOOL_CALL_DEBUG",
    "RUST_LOG",
];

#[test]
fn cli_env_file_logging_config_is_applied_before_startup_logging() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let log_dir = temp.path().join("logs");
    let env_file = temp.path().join("workhub.env");
    fs::create_dir_all(&home).unwrap();
    fs::write(
        &env_file,
        format!(
            "WORKHUB_LOG_DIR={}\nWORKHUB_LOG_TARGETS=file\n",
            log_dir.display()
        ),
    )
    .unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_workhub"));
    command
        .args(["cli", "--env-file"])
        .arg(&env_file)
        .args(["--json", "config", "path"])
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME");
    for key in WORKHUB_LOG_ENV_KEYS {
        command.env_remove(key);
    }

    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let run_log_path = log_dir.join("workhub.log");
    let run_log = fs::read_to_string(&run_log_path).unwrap();
    assert!(
        run_log.contains("process.started"),
        "expected startup log in {}",
        run_log_path.display()
    );
}
