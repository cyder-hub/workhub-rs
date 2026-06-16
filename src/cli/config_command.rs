use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use clap::{Args, Subcommand, ValueEnum};
use dialoguer::{Confirm, Input, Password, Select, theme::ColorfulTheme};
use reqwest::Url;
use serde_json::json;

use crate::{
    atlassian::compat::{
        ENV_ATLASSIAN_API_TOKEN, ENV_ATLASSIAN_PASSWORD, ENV_ATLASSIAN_PERSONAL_TOKEN,
        ENV_ATLASSIAN_SSL_VERIFY, ENV_ATLASSIAN_TIMEOUT, ENV_ATLASSIAN_USERNAME,
    },
    confluence::config::{
        ENV_CONFLUENCE_API_TOKEN, ENV_CONFLUENCE_PASSWORD, ENV_CONFLUENCE_PERSONAL_TOKEN,
        ENV_CONFLUENCE_SPACES_FILTER, ENV_CONFLUENCE_SSL_VERIFY, ENV_CONFLUENCE_TIMEOUT,
        ENV_CONFLUENCE_URL, ENV_CONFLUENCE_USERNAME,
    },
    env_loader,
    gitlab::config::{
        ENV_GITLAB_PERSONAL_TOKEN, ENV_GITLAB_PROJECTS_FILTER, ENV_GITLAB_SSL_VERIFY,
        ENV_GITLAB_TIMEOUT, ENV_GITLAB_TOKEN, ENV_GITLAB_URL,
    },
    jira::config::{
        ENV_JIRA_API_TOKEN, ENV_JIRA_PASSWORD, ENV_JIRA_PERSONAL_TOKEN, ENV_JIRA_PROJECTS_FILTER,
        ENV_JIRA_SSL_VERIFY, ENV_JIRA_TIMEOUT, ENV_JIRA_URL, ENV_JIRA_USERNAME,
    },
    operations::{OperationError, OperationErrorCategory, OperationResult, OutputPresentation},
    upstream::redaction::{
        REDACTED, env_secret_values_from_pairs, is_secret_env_key, redact_text_with_secrets,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum ConfigCommand {
    Path,
    Show,
    Setup(SetupArgs),
    Set(SetArgs),
    Unset(UnsetArgs),
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct SetupArgs {
    #[arg(value_enum, default_value_t = ConfigScope::All)]
    pub scope: ConfigScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ConfigScope {
    Atlassian,
    Jira,
    Confluence,
    Gitlab,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct SetArgs {
    #[arg(value_name = "KEY")]
    pub key: String,
    #[arg(value_name = "VALUE", allow_hyphen_values = true)]
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct UnsetArgs {
    #[arg(value_name = "KEY")]
    pub key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetOutcome {
    Created,
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DotenvDocument {
    lines: Vec<String>,
}

#[derive(Debug, Default)]
struct SetupChanges {
    values: BTreeMap<String, String>,
    removals: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeploymentKind {
    Cloud,
    ServerDataCenter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerAuthMethod {
    PersonalToken,
    UsernamePassword,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtlassianAuthMethod {
    CloudApiToken,
    ServerPersonalToken,
    ServerUsernamePassword,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigPresence {
    Missing,
    Partial,
    Configured,
}

#[derive(Debug, Clone)]
struct SelectChoice<T> {
    label: String,
    value: T,
    aliases: &'static [&'static str],
}

pub fn execute(args: ConfigArgs) -> Result<OperationResult, OperationError> {
    let path = global_config_path()?;
    let stdin = io::stdin();
    let mut prompt_output = io::stderr();
    let interactive = stdin.is_terminal() && prompt_output.is_terminal();
    if interactive {
        let mut input = io::Cursor::new(Vec::<u8>::new());
        execute_at_path(args, &path, &mut input, &mut prompt_output, true)
    } else {
        let mut input = stdin.lock();
        execute_at_path(args, &path, &mut input, &mut prompt_output, false)
    }
}

fn global_config_path() -> Result<PathBuf, OperationError> {
    env_loader::cli_global_dotenv_path().ok_or_else(|| {
        OperationError::config(
            "could not determine global CLI config path; set HOME, XDG_CONFIG_HOME, or APPDATA",
        )
    })
}

fn execute_at_path<R, W>(
    args: ConfigArgs,
    path: &Path,
    input: &mut R,
    prompt_output: &mut W,
    interactive: bool,
) -> Result<OperationResult, OperationError>
where
    R: BufRead,
    W: Write,
{
    match args.command {
        ConfigCommand::Path => Ok(path_result(path)),
        ConfigCommand::Show => show_config(path),
        ConfigCommand::Setup(args) => {
            setup_config_with_mode(path, args.scope, input, prompt_output, interactive)
        }
        ConfigCommand::Set(args) => set_config(path, &args.key, &args.value),
        ConfigCommand::Unset(args) => unset_config(path, &args.key),
    }
}

fn path_result(path: &Path) -> OperationResult {
    OperationResult::success(json!({
        "path": path.display().to_string(),
        "exists": path.exists(),
    }))
}

fn show_config(path: &Path) -> Result<OperationResult, OperationError> {
    let values = parse_config_values(path)?;
    let secret_values = env_secret_values_from_pairs(values.iter());
    let entries = values
        .iter()
        .map(|(key, value)| {
            let value = if is_secret_env_key(key) {
                REDACTED.to_string()
            } else {
                redact_text_with_secrets(value, &secret_values)
            };
            json!({
                "key": key,
                "value": value,
                "secret": is_secret_env_key(key),
            })
        })
        .collect::<Vec<_>>();

    Ok(OperationResult::success(json!({
        "path": path.display().to_string(),
        "exists": path.exists(),
        "entries": entries,
    })))
}

#[cfg(test)]
fn setup_config<R, W>(
    path: &Path,
    scope: ConfigScope,
    input: &mut R,
    prompt_output: &mut W,
) -> Result<OperationResult, OperationError>
where
    R: BufRead,
    W: Write,
{
    setup_config_with_mode(path, scope, input, prompt_output, false)
}

fn setup_config_with_mode<R, W>(
    path: &Path,
    scope: ConfigScope,
    input: &mut R,
    prompt_output: &mut W,
    interactive: bool,
) -> Result<OperationResult, OperationError>
where
    R: BufRead,
    W: Write,
{
    let values = parse_config_values(path)?;
    let mut changes = SetupChanges::default();

    writeln!(prompt_output, "Config file: {}", path.display()).map_err(prompt_error)?;

    match scope {
        ConfigScope::Atlassian => {
            setup_atlassian(&values, &mut changes, input, prompt_output, interactive)?
        }
        ConfigScope::Jira => setup_jira(
            &values,
            &mut changes,
            input,
            prompt_output,
            true,
            interactive,
        )?,
        ConfigScope::Confluence => setup_confluence(
            &values,
            &mut changes,
            input,
            prompt_output,
            true,
            interactive,
        )?,
        ConfigScope::Gitlab => setup_gitlab(
            &values,
            &mut changes,
            input,
            prompt_output,
            true,
            interactive,
        )?,
        ConfigScope::All => {
            setup_jira(
                &values,
                &mut changes,
                input,
                prompt_output,
                false,
                interactive,
            )?;
            setup_confluence(
                &values,
                &mut changes,
                input,
                prompt_output,
                false,
                interactive,
            )?;
            setup_gitlab(
                &values,
                &mut changes,
                input,
                prompt_output,
                false,
                interactive,
            )?;
        }
    }

    if changes.values.is_empty() && changes.removals.is_empty() {
        return Ok(mutation_result(path, "no changes"));
    }

    let mut document = DotenvDocument::read(path)?;
    let mut removed_count = 0;
    for key in &changes.removals {
        if !changes.values.contains_key(key) {
            removed_count += document.unset(key);
        }
    }
    for (key, value) in &changes.values {
        document.set(key, value)?;
    }

    let changed = changes.values.len() + removed_count;
    if changed == 0 {
        return Ok(mutation_result(path, "no changes"));
    }

    document.write(path)?;

    Ok(mutation_result(path, format!("updated {changed} values")))
}

fn setup_jira<R, W>(
    values: &BTreeMap<String, String>,
    changes: &mut SetupChanges,
    input: &mut R,
    output: &mut W,
    explicit_scope: bool,
    interactive: bool,
) -> Result<(), OperationError>
where
    R: BufRead,
    W: Write,
{
    let configured = has_any_value(values, JIRA_SUMMARY_FIELDS);
    if configured {
        write_service_summary("Jira", values, JIRA_SUMMARY_FIELDS, output)?;
    }

    if !should_configure_service(
        "Jira",
        configured,
        explicit_scope,
        input,
        output,
        interactive,
    )? {
        return Ok(());
    }

    let url = prompt_required_http_url(
        ENV_JIRA_URL,
        values.get(ENV_JIRA_URL),
        input,
        output,
        interactive,
    )?;
    let deployment = deployment_from_url(&url, ENV_JIRA_URL)?;
    changes.set(ENV_JIRA_URL, url);

    match deployment {
        DeploymentKind::Cloud => {
            let username = prompt_required_value(
                ENV_JIRA_USERNAME,
                values.get(ENV_JIRA_USERNAME),
                input,
                output,
                interactive,
            )?;
            let token = prompt_required_value(
                ENV_JIRA_API_TOKEN,
                values.get(ENV_JIRA_API_TOKEN),
                input,
                output,
                interactive,
            )?;
            changes.set(ENV_JIRA_USERNAME, username);
            changes.set(ENV_JIRA_API_TOKEN, token);
            changes.remove(ENV_JIRA_PERSONAL_TOKEN);
            changes.remove(ENV_JIRA_PASSWORD);
        }
        DeploymentKind::ServerDataCenter => {
            match prompt_server_auth_method(
                "Jira",
                ENV_JIRA_PERSONAL_TOKEN,
                ENV_JIRA_USERNAME,
                ENV_JIRA_PASSWORD,
                values,
                input,
                output,
                interactive,
            )? {
                ServerAuthMethod::PersonalToken => {
                    let token = prompt_required_value(
                        ENV_JIRA_PERSONAL_TOKEN,
                        values.get(ENV_JIRA_PERSONAL_TOKEN),
                        input,
                        output,
                        interactive,
                    )?;
                    changes.set(ENV_JIRA_PERSONAL_TOKEN, token);
                    changes.remove(ENV_JIRA_USERNAME);
                    changes.remove(ENV_JIRA_API_TOKEN);
                    changes.remove(ENV_JIRA_PASSWORD);
                }
                ServerAuthMethod::UsernamePassword => {
                    let username = prompt_required_value(
                        ENV_JIRA_USERNAME,
                        values.get(ENV_JIRA_USERNAME),
                        input,
                        output,
                        interactive,
                    )?;
                    let password = prompt_required_value(
                        ENV_JIRA_PASSWORD,
                        values.get(ENV_JIRA_PASSWORD),
                        input,
                        output,
                        interactive,
                    )?;
                    changes.set(ENV_JIRA_USERNAME, username);
                    changes.set(ENV_JIRA_PASSWORD, password);
                    changes.remove(ENV_JIRA_API_TOKEN);
                    changes.remove(ENV_JIRA_PERSONAL_TOKEN);
                }
            }
        }
    }

    prompt_existing_optional_values(
        values,
        changes,
        JIRA_OPTIONAL_FIELDS,
        input,
        output,
        interactive,
    )
}

fn setup_confluence<R, W>(
    values: &BTreeMap<String, String>,
    changes: &mut SetupChanges,
    input: &mut R,
    output: &mut W,
    explicit_scope: bool,
    interactive: bool,
) -> Result<(), OperationError>
where
    R: BufRead,
    W: Write,
{
    let configured = has_any_value(values, CONFLUENCE_SUMMARY_FIELDS);
    if configured {
        write_service_summary("Confluence", values, CONFLUENCE_SUMMARY_FIELDS, output)?;
    }

    if !should_configure_service(
        "Confluence",
        configured,
        explicit_scope,
        input,
        output,
        interactive,
    )? {
        return Ok(());
    }

    let url = prompt_required_http_url(
        ENV_CONFLUENCE_URL,
        values.get(ENV_CONFLUENCE_URL),
        input,
        output,
        interactive,
    )?;
    let deployment = deployment_from_url(&url, ENV_CONFLUENCE_URL)?;
    changes.set(ENV_CONFLUENCE_URL, url);

    match deployment {
        DeploymentKind::Cloud => {
            let username = prompt_required_value(
                ENV_CONFLUENCE_USERNAME,
                values.get(ENV_CONFLUENCE_USERNAME),
                input,
                output,
                interactive,
            )?;
            let token = prompt_required_value(
                ENV_CONFLUENCE_API_TOKEN,
                values.get(ENV_CONFLUENCE_API_TOKEN),
                input,
                output,
                interactive,
            )?;
            changes.set(ENV_CONFLUENCE_USERNAME, username);
            changes.set(ENV_CONFLUENCE_API_TOKEN, token);
            changes.remove(ENV_CONFLUENCE_PERSONAL_TOKEN);
            changes.remove(ENV_CONFLUENCE_PASSWORD);
        }
        DeploymentKind::ServerDataCenter => {
            match prompt_server_auth_method(
                "Confluence",
                ENV_CONFLUENCE_PERSONAL_TOKEN,
                ENV_CONFLUENCE_USERNAME,
                ENV_CONFLUENCE_PASSWORD,
                values,
                input,
                output,
                interactive,
            )? {
                ServerAuthMethod::PersonalToken => {
                    let token = prompt_required_value(
                        ENV_CONFLUENCE_PERSONAL_TOKEN,
                        values.get(ENV_CONFLUENCE_PERSONAL_TOKEN),
                        input,
                        output,
                        interactive,
                    )?;
                    changes.set(ENV_CONFLUENCE_PERSONAL_TOKEN, token);
                    changes.remove(ENV_CONFLUENCE_USERNAME);
                    changes.remove(ENV_CONFLUENCE_API_TOKEN);
                    changes.remove(ENV_CONFLUENCE_PASSWORD);
                }
                ServerAuthMethod::UsernamePassword => {
                    let username = prompt_required_value(
                        ENV_CONFLUENCE_USERNAME,
                        values.get(ENV_CONFLUENCE_USERNAME),
                        input,
                        output,
                        interactive,
                    )?;
                    let password = prompt_required_value(
                        ENV_CONFLUENCE_PASSWORD,
                        values.get(ENV_CONFLUENCE_PASSWORD),
                        input,
                        output,
                        interactive,
                    )?;
                    changes.set(ENV_CONFLUENCE_USERNAME, username);
                    changes.set(ENV_CONFLUENCE_PASSWORD, password);
                    changes.remove(ENV_CONFLUENCE_API_TOKEN);
                    changes.remove(ENV_CONFLUENCE_PERSONAL_TOKEN);
                }
            }
        }
    }

    prompt_existing_optional_values(
        values,
        changes,
        CONFLUENCE_OPTIONAL_FIELDS,
        input,
        output,
        interactive,
    )
}

fn setup_gitlab<R, W>(
    values: &BTreeMap<String, String>,
    changes: &mut SetupChanges,
    input: &mut R,
    output: &mut W,
    explicit_scope: bool,
    interactive: bool,
) -> Result<(), OperationError>
where
    R: BufRead,
    W: Write,
{
    let configured = has_any_value(values, GITLAB_SUMMARY_FIELDS);
    if configured {
        write_service_summary("GitLab", values, GITLAB_SUMMARY_FIELDS, output)?;
    }

    if !should_configure_service(
        "GitLab",
        configured,
        explicit_scope,
        input,
        output,
        interactive,
    )? {
        return Ok(());
    }

    let url = prompt_required_http_url(
        ENV_GITLAB_URL,
        values.get(ENV_GITLAB_URL),
        input,
        output,
        interactive,
    )?;
    let token = prompt_required_value(
        ENV_GITLAB_TOKEN,
        values.get(ENV_GITLAB_TOKEN),
        input,
        output,
        interactive,
    )?;
    changes.set(ENV_GITLAB_URL, url);
    changes.set(ENV_GITLAB_TOKEN, token);
    changes.remove(ENV_GITLAB_PERSONAL_TOKEN);

    prompt_existing_optional_values(
        values,
        changes,
        GITLAB_OPTIONAL_FIELDS,
        input,
        output,
        interactive,
    )
}

fn setup_atlassian<R, W>(
    values: &BTreeMap<String, String>,
    changes: &mut SetupChanges,
    input: &mut R,
    output: &mut W,
    interactive: bool,
) -> Result<(), OperationError>
where
    R: BufRead,
    W: Write,
{
    let configured = has_any_value(values, ATLASSIAN_SUMMARY_FIELDS);
    if configured {
        write_service_summary("Shared Atlassian", values, ATLASSIAN_SUMMARY_FIELDS, output)?;
    }

    if !should_configure_service(
        "shared Atlassian credentials",
        configured,
        true,
        input,
        output,
        interactive,
    )? {
        return Ok(());
    }

    match prompt_atlassian_auth_method(values, input, output, interactive)? {
        AtlassianAuthMethod::CloudApiToken => {
            let username = prompt_required_value(
                ENV_ATLASSIAN_USERNAME,
                values.get(ENV_ATLASSIAN_USERNAME),
                input,
                output,
                interactive,
            )?;
            let token = prompt_required_value(
                ENV_ATLASSIAN_API_TOKEN,
                values.get(ENV_ATLASSIAN_API_TOKEN),
                input,
                output,
                interactive,
            )?;
            changes.set(ENV_ATLASSIAN_USERNAME, username);
            changes.set(ENV_ATLASSIAN_API_TOKEN, token);
            changes.remove(ENV_ATLASSIAN_PERSONAL_TOKEN);
            changes.remove(ENV_ATLASSIAN_PASSWORD);
        }
        AtlassianAuthMethod::ServerPersonalToken => {
            let token = prompt_required_value(
                ENV_ATLASSIAN_PERSONAL_TOKEN,
                values.get(ENV_ATLASSIAN_PERSONAL_TOKEN),
                input,
                output,
                interactive,
            )?;
            changes.set(ENV_ATLASSIAN_PERSONAL_TOKEN, token);
            changes.remove(ENV_ATLASSIAN_USERNAME);
            changes.remove(ENV_ATLASSIAN_API_TOKEN);
            changes.remove(ENV_ATLASSIAN_PASSWORD);
        }
        AtlassianAuthMethod::ServerUsernamePassword => {
            let username = prompt_required_value(
                ENV_ATLASSIAN_USERNAME,
                values.get(ENV_ATLASSIAN_USERNAME),
                input,
                output,
                interactive,
            )?;
            let password = prompt_required_value(
                ENV_ATLASSIAN_PASSWORD,
                values.get(ENV_ATLASSIAN_PASSWORD),
                input,
                output,
                interactive,
            )?;
            changes.set(ENV_ATLASSIAN_USERNAME, username);
            changes.set(ENV_ATLASSIAN_PASSWORD, password);
            changes.remove(ENV_ATLASSIAN_API_TOKEN);
            changes.remove(ENV_ATLASSIAN_PERSONAL_TOKEN);
        }
    }

    prompt_existing_optional_values(
        values,
        changes,
        ATLASSIAN_OPTIONAL_FIELDS,
        input,
        output,
        interactive,
    )
}

fn should_configure_service<R, W>(
    service: &str,
    configured: bool,
    explicit_scope: bool,
    input: &mut R,
    output: &mut W,
    interactive: bool,
) -> Result<bool, OperationError>
where
    R: BufRead,
    W: Write,
{
    if configured {
        prompt_yes_no(
            &format!("Modify {service}?"),
            false,
            input,
            output,
            interactive,
        )
    } else if explicit_scope {
        prompt_yes_no(
            &format!("Configure {service}?"),
            true,
            input,
            output,
            interactive,
        )
    } else {
        prompt_yes_no(
            &format!("Configure {service}?"),
            false,
            input,
            output,
            interactive,
        )
    }
}

fn prompt_server_auth_method<R, W>(
    service: &str,
    personal_token_key: &'static str,
    username_key: &'static str,
    password_key: &'static str,
    values: &BTreeMap<String, String>,
    input: &mut R,
    output: &mut W,
    interactive_select: bool,
) -> Result<ServerAuthMethod, OperationError>
where
    R: BufRead,
    W: Write,
{
    let choices = vec![
        SelectChoice {
            label: auth_method_label(
                &format!("Personal access token ({personal_token_key})"),
                token_presence(values, personal_token_key),
            ),
            value: ServerAuthMethod::PersonalToken,
            aliases: &["pat", "token", "personal-token", "personal-access-token"],
        },
        SelectChoice {
            label: auth_method_label(
                &format!("Username/password ({username_key} + {password_key})"),
                username_secret_presence(values, username_key, password_key),
            ),
            value: ServerAuthMethod::UsernamePassword,
            aliases: &["basic", "password", "username-password", "user-password"],
        },
    ];
    let default_index = if has_non_empty_value(values, personal_token_key) {
        0
    } else if has_non_empty_value(values, password_key) {
        1
    } else {
        0
    };

    prompt_select(
        &format!("{service} Server/Data Center auth method"),
        &choices,
        default_index,
        input,
        output,
        interactive_select,
    )
}

fn prompt_atlassian_auth_method<R, W>(
    values: &BTreeMap<String, String>,
    input: &mut R,
    output: &mut W,
    interactive_select: bool,
) -> Result<AtlassianAuthMethod, OperationError>
where
    R: BufRead,
    W: Write,
{
    let default = default_atlassian_auth_method(values);
    let choices = vec![
        SelectChoice {
            label: auth_method_label(
                &format!(
                    "Cloud username/API token ({ENV_ATLASSIAN_USERNAME} + {ENV_ATLASSIAN_API_TOKEN})"
                ),
                username_secret_presence(values, ENV_ATLASSIAN_USERNAME, ENV_ATLASSIAN_API_TOKEN),
            ),
            value: AtlassianAuthMethod::CloudApiToken,
            aliases: &["cloud", "api-token", "api"],
        },
        SelectChoice {
            label: auth_method_label(
                &format!(
                    "Server/Data Center personal access token ({ENV_ATLASSIAN_PERSONAL_TOKEN})"
                ),
                token_presence(values, ENV_ATLASSIAN_PERSONAL_TOKEN),
            ),
            value: AtlassianAuthMethod::ServerPersonalToken,
            aliases: &["pat", "token", "personal-token", "personal-access-token"],
        },
        SelectChoice {
            label: auth_method_label(
                &format!(
                    "Server/Data Center username/password ({ENV_ATLASSIAN_USERNAME} + {ENV_ATLASSIAN_PASSWORD})"
                ),
                username_secret_presence(values, ENV_ATLASSIAN_USERNAME, ENV_ATLASSIAN_PASSWORD),
            ),
            value: AtlassianAuthMethod::ServerUsernamePassword,
            aliases: &["basic", "password", "username-password", "user-password"],
        },
    ];
    let default_index = choices
        .iter()
        .position(|choice| choice.value == default)
        .unwrap_or(0);

    prompt_select(
        "Shared Atlassian auth method",
        &choices,
        default_index,
        input,
        output,
        interactive_select,
    )
}

fn default_atlassian_auth_method(values: &BTreeMap<String, String>) -> AtlassianAuthMethod {
    if has_non_empty_value(values, ENV_ATLASSIAN_PERSONAL_TOKEN) {
        AtlassianAuthMethod::ServerPersonalToken
    } else if has_non_empty_value(values, ENV_ATLASSIAN_PASSWORD) {
        AtlassianAuthMethod::ServerUsernamePassword
    } else {
        AtlassianAuthMethod::CloudApiToken
    }
}

fn token_presence(values: &BTreeMap<String, String>, token_key: &str) -> ConfigPresence {
    if has_non_empty_value(values, token_key) {
        ConfigPresence::Configured
    } else {
        ConfigPresence::Missing
    }
}

fn username_secret_presence(
    values: &BTreeMap<String, String>,
    username_key: &str,
    secret_key: &str,
) -> ConfigPresence {
    match (
        has_non_empty_value(values, username_key),
        has_non_empty_value(values, secret_key),
    ) {
        (true, true) => ConfigPresence::Configured,
        (true, false) | (false, true) => ConfigPresence::Partial,
        (false, false) => ConfigPresence::Missing,
    }
}

fn auth_method_label(label: &str, presence: ConfigPresence) -> String {
    match presence {
        ConfigPresence::Configured => format!("{label} [configured]"),
        ConfigPresence::Partial => format!("{label} [partial]"),
        ConfigPresence::Missing => label.to_string(),
    }
}

fn prompt_select<T, R, W>(
    prompt: &str,
    choices: &[SelectChoice<T>],
    default_index: usize,
    input: &mut R,
    output: &mut W,
    interactive: bool,
) -> Result<T, OperationError>
where
    T: Copy,
    R: BufRead,
    W: Write,
{
    if choices.is_empty() {
        return Err(OperationError::config("auth method menu has no choices"));
    }
    let default_index = default_index.min(choices.len().saturating_sub(1));
    if interactive {
        return prompt_select_terminal(prompt, choices, default_index);
    }

    prompt_select_line(prompt, choices, default_index, input, output)
}

fn prompt_select_terminal<T>(
    prompt: &str,
    choices: &[SelectChoice<T>],
    default_index: usize,
) -> Result<T, OperationError>
where
    T: Copy,
{
    let labels = choices
        .iter()
        .map(|choice| choice.label.as_str())
        .collect::<Vec<_>>();
    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .items(&labels)
        .default(default_index)
        .interact()
        .map_err(dialoguer_error)?;

    Ok(choices[selection].value)
}

fn prompt_select_line<T, R, W>(
    prompt: &str,
    choices: &[SelectChoice<T>],
    default_index: usize,
    input: &mut R,
    output: &mut W,
) -> Result<T, OperationError>
where
    T: Copy,
    R: BufRead,
    W: Write,
{
    loop {
        writeln!(output, "{prompt}:").map_err(prompt_error)?;
        for (index, choice) in choices.iter().enumerate() {
            let default_marker = if index == default_index {
                " (default)"
            } else {
                ""
            };
            writeln!(output, "  {}. {}{default_marker}", index + 1, choice.label)
                .map_err(prompt_error)?;
        }
        write!(output, "Select auth method: ").map_err(prompt_error)?;
        output.flush().map_err(prompt_error)?;

        let mut line = String::new();
        if input.read_line(&mut line).map_err(prompt_error)? == 0 {
            return Ok(choices[default_index].value);
        }
        let answer = normalize_prompt_choice(&line);
        if answer.is_empty() {
            return Ok(choices[default_index].value);
        }
        if let Ok(index) = answer.parse::<usize>()
            && (1..=choices.len()).contains(&index)
        {
            return Ok(choices[index - 1].value);
        }
        if let Some(choice) = choices
            .iter()
            .find(|choice| choice.aliases.iter().any(|alias| *alias == answer))
        {
            return Ok(choice.value);
        }
        writeln!(output, "Please choose one of the listed auth methods.").map_err(prompt_error)?;
    }
}

fn normalize_prompt_choice(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace([' ', '_'], "-")
}

fn prompt_existing_optional_values<R, W>(
    values: &BTreeMap<String, String>,
    changes: &mut SetupChanges,
    fields: &[&'static str],
    input: &mut R,
    output: &mut W,
    interactive: bool,
) -> Result<(), OperationError>
where
    R: BufRead,
    W: Write,
{
    for field in fields {
        if values.contains_key(*field)
            && let Some(value) =
                prompt_optional_value(field, values.get(*field), input, output, interactive)?
        {
            changes.set(field, value);
        }
    }
    Ok(())
}

fn prompt_required_value<R, W>(
    key: &'static str,
    existing: Option<&String>,
    input: &mut R,
    output: &mut W,
    interactive: bool,
) -> Result<String, OperationError>
where
    R: BufRead,
    W: Write,
{
    prompt_required_value_with_validation(key, existing, input, output, interactive, |_| Ok(()))
}

fn prompt_required_http_url<R, W>(
    key: &'static str,
    existing: Option<&String>,
    input: &mut R,
    output: &mut W,
    interactive: bool,
) -> Result<String, OperationError>
where
    R: BufRead,
    W: Write,
{
    prompt_required_value_with_validation(key, existing, input, output, interactive, |value| {
        parse_http_url(value, key).map(|_| ())
    })
}

fn prompt_required_value_with_validation<R, W, F>(
    key: &'static str,
    existing: Option<&String>,
    input: &mut R,
    output: &mut W,
    interactive: bool,
    mut validate: F,
) -> Result<String, OperationError>
where
    R: BufRead,
    W: Write,
    F: FnMut(&str) -> Result<(), OperationError>,
{
    if interactive {
        return prompt_required_value_dialoguer(key, existing, validate);
    }

    loop {
        match prompt_value_status(key, existing, input, output)? {
            PromptValue::Entered(value) => {
                if !validate_prompt_value(&value, &mut validate, output)? {
                    continue;
                }
                return Ok(value);
            }
            PromptValue::Blank => {
                if let Some(value) = existing.filter(|value| !value.is_empty()) {
                    if !validate_prompt_value(value, &mut validate, output)? {
                        continue;
                    }
                    return Ok(value.clone());
                }
                writeln!(output, "{key} is required.").map_err(prompt_error)?;
            }
            PromptValue::Eof => {
                if let Some(value) = existing.filter(|value| !value.is_empty()) {
                    validate(value)?;
                    return Ok(value.clone());
                }
                return Err(OperationError::invalid_input(format!("{key} is required")));
            }
        }
    }
}

fn prompt_required_value_dialoguer<F>(
    key: &'static str,
    existing: Option<&String>,
    validate: F,
) -> Result<String, OperationError>
where
    F: FnMut(&str) -> Result<(), OperationError>,
{
    if is_secret_env_key(key) {
        prompt_required_secret_dialoguer(key, existing, validate)
    } else {
        prompt_required_text_dialoguer(key, existing, validate)
    }
}

fn prompt_required_text_dialoguer<F>(
    key: &'static str,
    existing: Option<&String>,
    mut validate: F,
) -> Result<String, OperationError>
where
    F: FnMut(&str) -> Result<(), OperationError>,
{
    let theme = ColorfulTheme::default();
    let mut prompt = Input::<String>::with_theme(&theme).with_prompt(key);
    if let Some(existing) = existing.filter(|value| !value.is_empty()) {
        prompt = prompt.default(existing.clone());
    }
    prompt
        .validate_with(move |value: &String| validate_dialoguer_value(value, &mut validate))
        .interact_text()
        .map_err(dialoguer_error)
}

fn prompt_required_secret_dialoguer<F>(
    key: &'static str,
    existing: Option<&String>,
    validate: F,
) -> Result<String, OperationError>
where
    F: FnMut(&str) -> Result<(), OperationError>,
{
    let theme = ColorfulTheme::default();
    let has_existing = existing.is_some_and(|value| !value.is_empty());
    let validator = RefCell::new(validate);
    let prompt = if has_existing {
        format!("{key} [set, press Enter to keep]")
    } else {
        key.to_string()
    };
    let value = Password::with_theme(&theme)
        .with_prompt(&prompt)
        .allow_empty_password(has_existing)
        .validate_with(|value: &String| -> Result<(), String> {
            if value.is_empty() && has_existing {
                return Ok(());
            }
            validate_dialoguer_value(value, &mut *validator.borrow_mut())
        })
        .interact()
        .map_err(dialoguer_error)?;

    if value.is_empty()
        && let Some(existing) = existing.filter(|value| !value.is_empty())
    {
        return Ok(existing.clone());
    }
    Ok(value)
}

fn validate_dialoguer_value<F>(value: &str, validate: &mut F) -> Result<(), String>
where
    F: FnMut(&str) -> Result<(), OperationError>,
{
    validate_config_value(value)
        .and_then(|_| validate(value))
        .map_err(|error| error.message)
}

fn validate_prompt_value<W, F>(
    value: &str,
    validate: &mut F,
    output: &mut W,
) -> Result<bool, OperationError>
where
    W: Write,
    F: FnMut(&str) -> Result<(), OperationError>,
{
    match validate(value) {
        Ok(()) => Ok(true),
        Err(error) if error.category == OperationErrorCategory::InvalidInput => {
            writeln!(output, "{}", error.message).map_err(prompt_error)?;
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

fn prompt_optional_value<R, W>(
    key: &'static str,
    existing: Option<&String>,
    input: &mut R,
    output: &mut W,
    interactive: bool,
) -> Result<Option<String>, OperationError>
where
    R: BufRead,
    W: Write,
{
    if interactive {
        prompt_optional_value_dialoguer(key, existing)
    } else {
        prompt_value(key, existing, input, output)
    }
}

fn prompt_optional_value_dialoguer(
    key: &'static str,
    existing: Option<&String>,
) -> Result<Option<String>, OperationError> {
    let value = if is_secret_env_key(key) {
        let theme = ColorfulTheme::default();
        let prompt = if existing.is_some_and(|value| !value.is_empty()) {
            format!("{key} [set, press Enter to keep]")
        } else {
            key.to_string()
        };
        Password::with_theme(&theme)
            .with_prompt(prompt)
            .allow_empty_password(true)
            .validate_with(|value: &String| -> Result<(), String> {
                if value.is_empty() {
                    Ok(())
                } else {
                    validate_config_value(value).map_err(|error| error.message)
                }
            })
            .interact()
            .map_err(dialoguer_error)?
    } else {
        let theme = ColorfulTheme::default();
        let prompt = if let Some(existing) = existing.filter(|value| !value.is_empty()) {
            format!("{key} [{existing}]")
        } else {
            key.to_string()
        };
        Input::<String>::with_theme(&theme)
            .with_prompt(prompt)
            .allow_empty(true)
            .validate_with(|value: &String| {
                if value.is_empty() {
                    Ok(())
                } else {
                    validate_config_value(value).map_err(|error| error.message)
                }
            })
            .interact_text()
            .map_err(dialoguer_error)?
    };

    if value.is_empty() {
        Ok(None)
    } else {
        validate_config_value(&value)?;
        Ok(Some(value))
    }
}

fn prompt_value<R, W>(
    key: &'static str,
    existing: Option<&String>,
    input: &mut R,
    output: &mut W,
) -> Result<Option<String>, OperationError>
where
    R: BufRead,
    W: Write,
{
    match prompt_value_status(key, existing, input, output)? {
        PromptValue::Entered(value) => Ok(Some(value)),
        PromptValue::Blank | PromptValue::Eof => Ok(None),
    }
}

enum PromptValue {
    Entered(String),
    Blank,
    Eof,
}

fn prompt_value_status<R, W>(
    key: &'static str,
    existing: Option<&String>,
    input: &mut R,
    output: &mut W,
) -> Result<PromptValue, OperationError>
where
    R: BufRead,
    W: Write,
{
    let secret = is_secret_env_key(key);
    if secret {
        if existing.is_some_and(|value| !value.is_empty()) {
            write!(output, "{key} [set, press Enter to keep]: ").map_err(prompt_error)?;
        } else {
            write!(output, "{key}: ").map_err(prompt_error)?;
        }
    } else if let Some(existing) = existing.filter(|value| !value.is_empty()) {
        write!(output, "{key} [{existing}]: ").map_err(prompt_error)?;
    } else {
        write!(output, "{key}: ").map_err(prompt_error)?;
    }
    output.flush().map_err(prompt_error)?;

    let mut line = String::new();
    let echo_guard = EchoGuard::disable_if(secret);
    let bytes_read = input.read_line(&mut line).map_err(prompt_error)?;
    let echo_disabled = echo_guard.disabled();
    drop(echo_guard);
    if echo_disabled {
        writeln!(output).map_err(prompt_error)?;
    }
    if bytes_read == 0 {
        return Ok(PromptValue::Eof);
    }
    let value = line.trim_end_matches(['\r', '\n']);
    if value.is_empty() {
        return Ok(PromptValue::Blank);
    }
    validate_config_value(value)?;
    Ok(PromptValue::Entered(value.to_string()))
}

fn prompt_yes_no<R, W>(
    prompt: &str,
    default: bool,
    input: &mut R,
    output: &mut W,
    interactive: bool,
) -> Result<bool, OperationError>
where
    R: BufRead,
    W: Write,
{
    if interactive {
        return Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(prompt)
            .default(default)
            .interact()
            .map_err(dialoguer_error);
    }

    let suffix = if default { "[Y/n]" } else { "[y/N]" };
    loop {
        write!(output, "{prompt} {suffix}: ").map_err(prompt_error)?;
        output.flush().map_err(prompt_error)?;

        let mut line = String::new();
        if input.read_line(&mut line).map_err(prompt_error)? == 0 {
            return Ok(default);
        }
        let answer = line.trim().to_ascii_lowercase();
        match answer.as_str() {
            "" => return Ok(default),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => writeln!(output, "Please answer y or n.").map_err(prompt_error)?,
        }
    }
}

fn set_config(path: &Path, key: &str, value: &str) -> Result<OperationResult, OperationError> {
    validate_config_entry(key, value)?;
    let mut document = DotenvDocument::read(path)?;
    let outcome = document.set(key, value)?;
    document.write(path)?;

    let message = match outcome {
        SetOutcome::Created => format!("created {key}"),
        SetOutcome::Updated => format!("updated {key}"),
    };
    Ok(mutation_result(path, message))
}

fn unset_config(path: &Path, key: &str) -> Result<OperationResult, OperationError> {
    validate_config_key(key)?;
    let mut document = DotenvDocument::read(path)?;
    let removed = document.unset(key);
    if removed > 0 {
        document.write(path)?;
    }

    let message = if removed == 0 {
        format!("{key} was not set")
    } else {
        format!("removed {key}")
    };
    Ok(mutation_result(path, message))
}

fn mutation_result(path: &Path, message: impl Into<String>) -> OperationResult {
    OperationResult::success(json!({
        "success": true,
        "path": path.display().to_string(),
        "message": message.into(),
    }))
    .with_presentation(OutputPresentation::MutationSummary { label: "config" })
}

impl SetupChanges {
    fn set(&mut self, key: &'static str, value: String) {
        self.removals.remove(key);
        self.values.insert(key.to_string(), value);
    }

    fn remove(&mut self, key: &'static str) {
        if !self.values.contains_key(key) {
            self.removals.insert(key.to_string());
        }
    }
}

fn has_any_value(values: &BTreeMap<String, String>, fields: &[&str]) -> bool {
    fields
        .iter()
        .any(|field| has_non_empty_value(values, field))
}

fn has_non_empty_value(values: &BTreeMap<String, String>, field: &str) -> bool {
    values.get(field).is_some_and(|value| !value.is_empty())
}

fn write_service_summary<W>(
    service: &str,
    values: &BTreeMap<String, String>,
    fields: &[&str],
    output: &mut W,
) -> Result<(), OperationError>
where
    W: Write,
{
    writeln!(output, "{service}:").map_err(prompt_error)?;
    for field in fields {
        let Some(value) = values.get(*field).filter(|value| !value.is_empty()) else {
            continue;
        };
        writeln!(output, "  {field}: {}", display_config_value(field, value))
            .map_err(prompt_error)?;
    }
    if let Some(url_field) = fields.iter().find(|field| field.ends_with("_URL"))
        && let Some(url) = values.get(*url_field)
        && let Ok(deployment) = deployment_from_url(url, url_field)
    {
        let label = match deployment {
            DeploymentKind::Cloud => "Cloud",
            DeploymentKind::ServerDataCenter => "Server/Data Center",
        };
        writeln!(output, "  deployment: {label}").map_err(prompt_error)?;
    }
    Ok(())
}

fn display_config_value(key: &str, value: &str) -> String {
    if is_secret_env_key(key) {
        "set".to_string()
    } else {
        value.to_string()
    }
}

fn deployment_from_url(value: &str, key: &str) -> Result<DeploymentKind, OperationError> {
    let url = parse_http_url(value, key)?;
    let deployment = if url
        .host_str()
        .is_some_and(|host| host.to_ascii_lowercase().ends_with(".atlassian.net"))
    {
        DeploymentKind::Cloud
    } else {
        DeploymentKind::ServerDataCenter
    };
    Ok(deployment)
}

fn parse_http_url(value: &str, key: &str) -> Result<Url, OperationError> {
    let url = Url::parse(value.trim()).map_err(|_| invalid_setup_url(value, key))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(invalid_setup_url(value, key));
    }
    Ok(url)
}

fn invalid_setup_url(value: &str, key: &str) -> OperationError {
    OperationError::invalid_input(format!(
        "invalid {key} value `{value}`; expected http or https URL with host"
    ))
}

const ATLASSIAN_SUMMARY_FIELDS: &[&str] = &[
    ENV_ATLASSIAN_USERNAME,
    ENV_ATLASSIAN_API_TOKEN,
    ENV_ATLASSIAN_PASSWORD,
    ENV_ATLASSIAN_PERSONAL_TOKEN,
    ENV_ATLASSIAN_SSL_VERIFY,
    ENV_ATLASSIAN_TIMEOUT,
];

const JIRA_SUMMARY_FIELDS: &[&str] = &[
    ENV_JIRA_URL,
    ENV_JIRA_USERNAME,
    ENV_JIRA_API_TOKEN,
    ENV_JIRA_PASSWORD,
    ENV_JIRA_PERSONAL_TOKEN,
    ENV_JIRA_PROJECTS_FILTER,
    ENV_JIRA_SSL_VERIFY,
    ENV_JIRA_TIMEOUT,
];

const CONFLUENCE_SUMMARY_FIELDS: &[&str] = &[
    ENV_CONFLUENCE_URL,
    ENV_CONFLUENCE_USERNAME,
    ENV_CONFLUENCE_API_TOKEN,
    ENV_CONFLUENCE_PASSWORD,
    ENV_CONFLUENCE_PERSONAL_TOKEN,
    ENV_CONFLUENCE_SPACES_FILTER,
    ENV_CONFLUENCE_SSL_VERIFY,
    ENV_CONFLUENCE_TIMEOUT,
];

const GITLAB_SUMMARY_FIELDS: &[&str] = &[
    ENV_GITLAB_URL,
    ENV_GITLAB_TOKEN,
    ENV_GITLAB_PERSONAL_TOKEN,
    ENV_GITLAB_PROJECTS_FILTER,
    ENV_GITLAB_SSL_VERIFY,
    ENV_GITLAB_TIMEOUT,
];

const ATLASSIAN_OPTIONAL_FIELDS: &[&str] = &[ENV_ATLASSIAN_SSL_VERIFY, ENV_ATLASSIAN_TIMEOUT];

const JIRA_OPTIONAL_FIELDS: &[&str] = &[
    ENV_JIRA_PROJECTS_FILTER,
    ENV_JIRA_SSL_VERIFY,
    ENV_JIRA_TIMEOUT,
];

const CONFLUENCE_OPTIONAL_FIELDS: &[&str] = &[
    ENV_CONFLUENCE_SPACES_FILTER,
    ENV_CONFLUENCE_SSL_VERIFY,
    ENV_CONFLUENCE_TIMEOUT,
];

const GITLAB_OPTIONAL_FIELDS: &[&str] = &[
    ENV_GITLAB_PROJECTS_FILTER,
    ENV_GITLAB_SSL_VERIFY,
    ENV_GITLAB_TIMEOUT,
];

fn parse_config_values(path: &Path) -> Result<BTreeMap<String, String>, OperationError> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }

    let iter = dotenvy::from_path_iter(path).map_err(|error| {
        OperationError::config(format!(
            "failed to read global CLI config {}: {error}",
            path.display()
        ))
    })?;
    let mut values = BTreeMap::new();
    for item in iter {
        let (key, value) = item.map_err(|error| {
            OperationError::config(format!(
                "failed to parse global CLI config {}: {error}",
                path.display()
            ))
        })?;
        values.entry(key).or_insert(value);
    }
    Ok(values)
}

impl DotenvDocument {
    fn read(path: &Path) -> Result<Self, OperationError> {
        if !path.exists() {
            return Ok(Self { lines: Vec::new() });
        }

        let content = fs::read_to_string(path).map_err(|error| {
            OperationError::config(format!(
                "failed to read global CLI config {}: {error}",
                path.display()
            ))
        })?;
        Ok(Self {
            lines: content
                .lines()
                .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
                .collect(),
        })
    }

    fn set(&mut self, key: &str, value: &str) -> Result<SetOutcome, OperationError> {
        validate_config_entry(key, value)?;
        let formatted = format_dotenv_line(key, value);
        for line in &mut self.lines {
            if dotenv_line_key(line).as_deref() == Some(key) {
                *line = formatted;
                return Ok(SetOutcome::Updated);
            }
        }

        if self
            .lines
            .last()
            .is_some_and(|line| !line.trim().is_empty())
        {
            self.lines.push(String::new());
        }
        self.lines.push(formatted);
        Ok(SetOutcome::Created)
    }

    fn unset(&mut self, key: &str) -> usize {
        let previous_len = self.lines.len();
        self.lines
            .retain(|line| dotenv_line_key(line).as_deref() != Some(key));
        previous_len - self.lines.len()
    }

    fn write(&self, path: &Path) -> Result<(), OperationError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                OperationError::config(format!(
                    "failed to create global CLI config directory {}: {error}",
                    parent.display()
                ))
            })?;
            secure_config_dir(parent)?;
        }

        let mut content = self.lines.join("\n");
        if !content.is_empty() {
            content.push('\n');
        }

        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(path).map_err(|error| {
            OperationError::config(format!(
                "failed to write global CLI config {}: {error}",
                path.display()
            ))
        })?;
        file.write_all(content.as_bytes()).map_err(|error| {
            OperationError::config(format!(
                "failed to write global CLI config {}: {error}",
                path.display()
            ))
        })?;
        secure_config_file(path)
    }
}

fn secure_config_dir(path: &Path) -> Result<(), OperationError> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
            OperationError::config(format!(
                "failed to secure global CLI config directory {}: {error}",
                path.display()
            ))
        })?;
    }

    Ok(())
}

fn secure_config_file(path: &Path) -> Result<(), OperationError> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            OperationError::config(format!(
                "failed to secure global CLI config {}: {error}",
                path.display()
            ))
        })?;
    }

    Ok(())
}

fn dotenv_line_key(line: &str) -> Option<String> {
    let mut value = line.trim_start();
    if value.is_empty() || value.starts_with('#') {
        return None;
    }

    if let Some(rest) = value.strip_prefix("export")
        && rest.chars().next().is_some_and(char::is_whitespace)
    {
        value = rest.trim_start();
    }

    let mut chars = value.char_indices();
    let (_, first) = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }

    let mut end = first.len_utf8();
    for (index, character) in chars {
        if character.is_ascii_alphanumeric() || character == '_' || character == '.' {
            end = index + character.len_utf8();
        } else {
            break;
        }
    }

    let key = &value[..end];
    value[end..]
        .trim_start()
        .starts_with('=')
        .then(|| key.to_string())
}

fn validate_config_entry(key: &str, value: &str) -> Result<(), OperationError> {
    validate_config_key(key)?;
    validate_config_value(value)
}

fn validate_config_key(key: &str) -> Result<(), OperationError> {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return Err(OperationError::invalid_input("config key cannot be empty"));
    };
    if !(first.is_ascii_uppercase() || first == '_')
        || !chars.all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
    {
        return Err(OperationError::invalid_input(format!(
            "invalid config key `{key}`; expected uppercase dotenv variable name"
        )));
    }
    Ok(())
}

fn validate_config_value(value: &str) -> Result<(), OperationError> {
    if value
        .chars()
        .any(|character| matches!(character, '\n' | '\r' | '\0'))
    {
        return Err(OperationError::invalid_input(
            "config value must be a single line",
        ));
    }
    Ok(())
}

fn format_dotenv_line(key: &str, value: &str) -> String {
    format!("{key}={}", quote_dotenv_value(value))
}

fn quote_dotenv_value(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '$' => output.push_str("\\$"),
            _ => output.push(character),
        }
    }
    output.push('"');
    output
}

fn prompt_error(error: io::Error) -> OperationError {
    OperationError::config(format!("failed to read CLI config input: {error}"))
}

fn dialoguer_error(error: dialoguer::Error) -> OperationError {
    OperationError::config(format!("failed to read CLI config selection: {error}"))
}

struct EchoGuard {
    disabled: bool,
}

impl EchoGuard {
    fn disable_if(secret: bool) -> Self {
        if !secret || !io::stdin().is_terminal() {
            return Self { disabled: false };
        }

        let disabled = Command::new("stty")
            .arg("-echo")
            .status()
            .is_ok_and(|status| status.success());
        Self { disabled }
    }

    fn disabled(&self) -> bool {
        self.disabled
    }
}

impl Drop for EchoGuard {
    fn drop(&mut self) {
        if self.disabled {
            let _ = Command::new("stty").arg("echo").status();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Cursor,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::operations::OperationErrorCategory;

    use super::*;

    fn temp_config_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("workhub-cli-config-{name}-{nonce}"))
            .join(".env")
    }

    #[test]
    fn config_set_creates_global_dotenv_and_quotes_values() {
        let path = temp_config_path("set");
        let result = set_config(&path, ENV_JIRA_URL, "https://example.atlassian.net").unwrap();

        assert!(result.value["success"].as_bool().unwrap());
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains(r#"JIRA_URL="https://example.atlassian.net""#));
    }

    #[test]
    fn config_set_updates_first_existing_declaration() {
        let path = temp_config_path("update");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "JIRA_URL=\"old\"\n# keep me\nJIRA_URL=\"duplicate\"\n",
        )
        .unwrap();

        set_config(&path, ENV_JIRA_URL, "new").unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "JIRA_URL=\"new\"\n# keep me\nJIRA_URL=\"duplicate\"\n"
        );
    }

    #[test]
    fn config_unset_removes_existing_declarations() {
        let path = temp_config_path("unset");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "JIRA_URL=\"old\"\n# JIRA_URL=\"commented\"\nJIRA_URL=\"duplicate\"\n",
        )
        .unwrap();

        unset_config(&path, ENV_JIRA_URL).unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# JIRA_URL=\"commented\"\n"
        );
    }

    #[test]
    fn config_show_redacts_secret_values() {
        let path = temp_config_path("show");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "JIRA_URL=\"https://example.atlassian.net\"\nJIRA_API_TOKEN=\"secret-token\"\n",
        )
        .unwrap();

        let result = show_config(&path).unwrap();
        let entries = result.value["entries"].as_array().unwrap();

        assert!(entries.iter().any(|entry| {
            entry["key"] == ENV_JIRA_API_TOKEN
                && entry["value"] == REDACTED
                && entry["secret"] == true
        }));
        assert!(
            !serde_json::to_string(&result.value)
                .unwrap()
                .contains("secret-token")
        );
    }

    #[test]
    fn config_show_redacts_custom_header_values() {
        let path = temp_config_path("show-custom-headers");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "JIRA_CUSTOM_HEADERS=\"X-Token=secret-token\"\n").unwrap();

        let result = show_config(&path).unwrap();
        let entries = result.value["entries"].as_array().unwrap();

        assert!(entries.iter().any(|entry| {
            entry["key"] == "JIRA_CUSTOM_HEADERS"
                && entry["value"] == REDACTED
                && entry["secret"] == true
        }));
        let output = serde_json::to_string(&result.value).unwrap();
        assert!(!output.contains("secret-token"));
        assert!(!output.contains("X-Token"));
    }

    #[test]
    fn config_setup_keeps_existing_secret_on_blank_and_updates_new_values() {
        let path = temp_config_path("setup");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            concat!(
                "JIRA_URL=\"old\"\n",
                "JIRA_API_TOKEN=\"keep-token\"\n",
                "JIRA_PROJECTS_FILTER=\"OLD\"\n",
                "JIRA_SSL_VERIFY=\"false\"\n",
                "JIRA_TIMEOUT=\"75\"\n",
            ),
        )
        .unwrap();
        let mut input =
            Cursor::new("y\nhttps://example.atlassian.net\nuser@example.com\n\nABC\ntrue\n90\n");
        let mut output = Vec::new();

        setup_config(&path, ConfigScope::Jira, &mut input, &mut output).unwrap();
        let values = parse_config_values(&path).unwrap();

        assert_eq!(
            values.get(ENV_JIRA_URL).unwrap(),
            "https://example.atlassian.net"
        );
        assert_eq!(values.get(ENV_JIRA_USERNAME).unwrap(), "user@example.com");
        assert_eq!(values.get(ENV_JIRA_API_TOKEN).unwrap(), "keep-token");
        assert_eq!(values.get(ENV_JIRA_PROJECTS_FILTER).unwrap(), "ABC");
        assert_eq!(values.get(ENV_JIRA_SSL_VERIFY).unwrap(), "true");
        assert_eq!(values.get(ENV_JIRA_TIMEOUT).unwrap(), "90");
        assert!(!String::from_utf8(output).unwrap().contains("keep-token"));
    }

    #[test]
    fn config_setup_jira_cloud_removes_server_credentials() {
        let path = temp_config_path("setup-cloud");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "JIRA_URL=\"https://jira.example\"\nJIRA_PERSONAL_TOKEN=\"server-token\"\nJIRA_PASSWORD=\"legacy\"\n",
        )
        .unwrap();
        let mut input =
            Cursor::new("y\nhttps://example.atlassian.net\nuser@example.com\ncloud-token\n");
        let mut output = Vec::new();

        setup_config(&path, ConfigScope::Jira, &mut input, &mut output).unwrap();
        let values = parse_config_values(&path).unwrap();

        assert_eq!(values.get(ENV_JIRA_API_TOKEN).unwrap(), "cloud-token");
        assert!(!values.contains_key(ENV_JIRA_PERSONAL_TOKEN));
        assert!(!values.contains_key(ENV_JIRA_PASSWORD));
    }

    #[test]
    fn config_setup_jira_server_removes_cloud_credentials() {
        let path = temp_config_path("setup-server");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "JIRA_URL=\"https://example.atlassian.net\"\nJIRA_USERNAME=\"user@example.com\"\nJIRA_API_TOKEN=\"cloud-token\"\n",
        )
        .unwrap();
        let mut input = Cursor::new("y\nhttps://jira.example\n\nserver-token\n");
        let mut output = Vec::new();

        setup_config(&path, ConfigScope::Jira, &mut input, &mut output).unwrap();
        let values = parse_config_values(&path).unwrap();

        assert_eq!(values.get(ENV_JIRA_PERSONAL_TOKEN).unwrap(), "server-token");
        assert!(!values.contains_key(ENV_JIRA_USERNAME));
        assert!(!values.contains_key(ENV_JIRA_API_TOKEN));
    }

    #[test]
    fn config_setup_jira_server_can_choose_username_password_auth() {
        let path = temp_config_path("setup-jira-server-password");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            concat!(
                "JIRA_URL=\"https://old.example\"\n",
                "JIRA_API_TOKEN=\"cloud-token\"\n",
                "JIRA_PERSONAL_TOKEN=\"server-token\"\n",
            ),
        )
        .unwrap();
        let mut input = Cursor::new("y\nhttps://jira.example\n2\njira-user\njira-password\n");
        let mut output = Vec::new();

        setup_config(&path, ConfigScope::Jira, &mut input, &mut output).unwrap();
        let values = parse_config_values(&path).unwrap();

        assert_eq!(values.get(ENV_JIRA_USERNAME).unwrap(), "jira-user");
        assert_eq!(values.get(ENV_JIRA_PASSWORD).unwrap(), "jira-password");
        assert!(!values.contains_key(ENV_JIRA_API_TOKEN));
        assert!(!values.contains_key(ENV_JIRA_PERSONAL_TOKEN));
    }

    #[test]
    fn config_setup_auth_method_menu_marks_existing_configured_paths() {
        let path = temp_config_path("setup-auth-method-configured");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            concat!(
                "JIRA_URL=\"https://jira.example\"\n",
                "JIRA_PERSONAL_TOKEN=\"server-token\"\n",
                "JIRA_USERNAME=\"jira-user\"\n",
                "JIRA_PASSWORD=\"jira-password\"\n",
            ),
        )
        .unwrap();
        let mut input = Cursor::new("y\n\n\n\n");
        let mut output = Vec::new();

        setup_config(&path, ConfigScope::Jira, &mut input, &mut output).unwrap();
        let output = String::from_utf8(output).unwrap();

        assert!(
            output.contains("Personal access token (JIRA_PERSONAL_TOKEN) [configured]"),
            "{output}"
        );
        assert!(
            output.contains("Username/password (JIRA_USERNAME + JIRA_PASSWORD) [configured]"),
            "{output}"
        );
        assert!(!output.contains("server-token"));
        assert!(!output.contains("jira-password"));
    }

    #[test]
    fn config_setup_confluence_server_can_choose_username_password_auth() {
        let path = temp_config_path("setup-confluence-server-password");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            concat!(
                "CONFLUENCE_URL=\"https://old.example\"\n",
                "CONFLUENCE_API_TOKEN=\"cloud-token\"\n",
                "CONFLUENCE_PERSONAL_TOKEN=\"server-token\"\n",
            ),
        )
        .unwrap();
        let mut input =
            Cursor::new("y\nhttps://confluence.example\n2\nconfluence-user\nconfluence-password\n");
        let mut output = Vec::new();

        setup_config(&path, ConfigScope::Confluence, &mut input, &mut output).unwrap();
        let values = parse_config_values(&path).unwrap();

        assert_eq!(
            values.get(ENV_CONFLUENCE_USERNAME).unwrap(),
            "confluence-user"
        );
        assert_eq!(
            values.get(ENV_CONFLUENCE_PASSWORD).unwrap(),
            "confluence-password"
        );
        assert!(!values.contains_key(ENV_CONFLUENCE_API_TOKEN));
        assert!(!values.contains_key(ENV_CONFLUENCE_PERSONAL_TOKEN));
    }

    #[test]
    fn config_setup_all_skips_unconfigured_services_by_default() {
        let path = temp_config_path("setup-all-skip");
        let mut input = Cursor::new("\n\n\n");
        let mut output = Vec::new();

        let result = setup_config(&path, ConfigScope::All, &mut input, &mut output).unwrap();

        assert_eq!(result.value["message"], "no changes");
        assert!(!path.exists());
    }

    #[test]
    fn config_setup_atlassian_can_choose_shared_server_username_password_auth() {
        let path = temp_config_path("setup-atlassian-server-password");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            concat!(
                "ATLASSIAN_USERNAME=\"cloud@example.com\"\n",
                "ATLASSIAN_API_TOKEN=\"cloud-token\"\n",
                "ATLASSIAN_PERSONAL_TOKEN=\"server-token\"\n",
            ),
        )
        .unwrap();
        let mut input = Cursor::new("y\n3\nshared-user\nshared-password\n");
        let mut output = Vec::new();

        setup_config(&path, ConfigScope::Atlassian, &mut input, &mut output).unwrap();
        let values = parse_config_values(&path).unwrap();

        assert_eq!(values.get(ENV_ATLASSIAN_USERNAME).unwrap(), "shared-user");
        assert_eq!(
            values.get(ENV_ATLASSIAN_PASSWORD).unwrap(),
            "shared-password"
        );
        assert!(!values.contains_key(ENV_ATLASSIAN_API_TOKEN));
        assert!(!values.contains_key(ENV_ATLASSIAN_PERSONAL_TOKEN));
    }

    #[test]
    fn config_setup_gitlab_uses_single_primary_token() {
        let path = temp_config_path("setup-gitlab");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "GITLAB_URL=\"https://gitlab.old\"\nGITLAB_PERSONAL_TOKEN=\"legacy-token\"\n",
        )
        .unwrap();
        let mut input = Cursor::new("y\nhttps://gitlab.example\nprimary-token\n");
        let mut output = Vec::new();

        setup_config(&path, ConfigScope::Gitlab, &mut input, &mut output).unwrap();
        let values = parse_config_values(&path).unwrap();

        assert_eq!(
            values.get(ENV_GITLAB_URL).unwrap(),
            "https://gitlab.example"
        );
        assert_eq!(values.get(ENV_GITLAB_TOKEN).unwrap(), "primary-token");
        assert!(!values.contains_key(ENV_GITLAB_PERSONAL_TOKEN));
    }

    #[test]
    fn config_setup_gitlab_retries_runtime_invalid_urls() {
        for invalid_url in ["gitlab.example", "ftp://gitlab.example"] {
            let path = temp_config_path("setup-gitlab-invalid-url");
            let mut input = Cursor::new(format!(
                "y\n{invalid_url}\nhttps://gitlab.example\nprimary-token\n"
            ));
            let mut output = Vec::new();

            setup_config(&path, ConfigScope::Gitlab, &mut input, &mut output).unwrap();
            let values = parse_config_values(&path).unwrap();
            let output = String::from_utf8(output).unwrap();

            assert_eq!(
                values.get(ENV_GITLAB_URL).unwrap(),
                "https://gitlab.example"
            );
            assert!(output.contains("invalid GITLAB_URL value"));
        }
    }

    #[test]
    fn config_setup_atlassian_services_retry_non_http_urls() {
        for (scope, invalid_url, key, valid_url, token_key) in [
            (
                ConfigScope::Jira,
                "ftp://jira.example",
                ENV_JIRA_URL,
                "https://jira.example",
                ENV_JIRA_PERSONAL_TOKEN,
            ),
            (
                ConfigScope::Confluence,
                "file:///tmp",
                ENV_CONFLUENCE_URL,
                "https://confluence.example",
                ENV_CONFLUENCE_PERSONAL_TOKEN,
            ),
        ] {
            let path = temp_config_path("setup-atlassian-invalid-url");
            let mut input = Cursor::new(format!("y\n{invalid_url}\n{valid_url}\n\nserver-token\n"));
            let mut output = Vec::new();

            setup_config(&path, scope, &mut input, &mut output).unwrap();
            let values = parse_config_values(&path).unwrap();
            let output = String::from_utf8(output).unwrap();

            assert_eq!(values.get(token_key).unwrap(), "server-token");
            assert!(output.contains(&format!("invalid {key} value")));
        }
    }

    #[test]
    fn config_rejects_invalid_key_and_multiline_value() {
        let path = temp_config_path("invalid");

        assert!(set_config(&path, "bad-key", "value").is_err());
        assert!(set_config(&path, ENV_JIRA_URL, "one\ntwo").is_err());
    }

    #[test]
    fn dotenv_line_key_handles_export_and_comments() {
        assert_eq!(
            dotenv_line_key("export JIRA_URL=\"https://example\"").as_deref(),
            Some(ENV_JIRA_URL)
        );
        assert_eq!(dotenv_line_key("# JIRA_URL=\"https://example\""), None);
        assert_eq!(
            dotenv_line_key("exported=value").as_deref(),
            Some("exported")
        );
    }

    #[test]
    fn quote_dotenv_value_escapes_substitution_fragments() {
        let line = format_dotenv_line("TOKEN", r#"abc"$HOME\xyz"#);
        let parsed = dotenvy::from_read_iter(line.as_bytes())
            .next()
            .unwrap()
            .unwrap();

        assert_eq!(parsed.1, r#"abc"$HOME\xyz"#);
    }
}
