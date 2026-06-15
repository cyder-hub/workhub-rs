use std::{
    env,
    fmt::Display,
    path::{Path, PathBuf},
};

pub const ENV_FILE_VARIABLE: &str = "ENV_FILE";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DotenvSelection {
    Explicit(String),
    Environment(String),
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliDotenvSelection {
    Explicit(String),
    Environment(String),
    Global(PathBuf),
    CurrentDirectory(PathBuf),
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvLoadError {
    path: String,
    message: String,
}

impl EnvLoadError {
    #[cfg(test)]
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl Display for EnvLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "failed to load env file {}: {}",
            self.path, self.message
        )
    }
}

impl std::error::Error for EnvLoadError {}

pub fn load_dotenv(explicit_env_file: Option<&str>) -> Result<Option<PathBuf>, EnvLoadError> {
    load_dotenv_with(
        explicit_env_file,
        |key| env::var(key),
        DotenvyLoader::default(),
    )
}

pub fn load_cli_dotenv(explicit_env_file: Option<&str>) -> Result<Option<PathBuf>, EnvLoadError> {
    load_cli_dotenv_with(
        explicit_env_file,
        |key| env::var(key),
        DotenvyLoader::default(),
    )
}

pub fn select_dotenv<F, E>(explicit_env_file: Option<&str>, mut get_var: F) -> DotenvSelection
where
    F: FnMut(&str) -> Result<String, E>,
{
    explicit_env_file
        .map(|path| DotenvSelection::Explicit(path.to_string()))
        .or_else(|| {
            get_var(ENV_FILE_VARIABLE)
                .ok()
                .map(DotenvSelection::Environment)
        })
        .unwrap_or(DotenvSelection::Default)
}

pub fn load_dotenv_with<F, E, L>(
    explicit_env_file: Option<&str>,
    get_var: F,
    mut loader: L,
) -> Result<Option<PathBuf>, EnvLoadError>
where
    F: FnMut(&str) -> Result<String, E>,
    L: DotenvLoader,
{
    match select_dotenv(explicit_env_file, get_var) {
        DotenvSelection::Explicit(path) | DotenvSelection::Environment(path) => loader
            .load_filename(&path)
            .map(Some)
            .map_err(|message| EnvLoadError { path, message }),
        DotenvSelection::Default => Ok(loader.load_default().ok()),
    }
}

pub fn select_cli_dotenv<F, E, L>(
    explicit_env_file: Option<&str>,
    mut get_var: F,
    loader: &mut L,
) -> CliDotenvSelection
where
    F: FnMut(&str) -> Result<String, E>,
    L: DotenvLoader,
{
    if let Some(path) = explicit_env_file {
        return CliDotenvSelection::Explicit(path.to_string());
    }

    if let Ok(path) = get_var(ENV_FILE_VARIABLE) {
        return CliDotenvSelection::Environment(path);
    }

    if let Some(path) = cli_global_dotenv_path_with(&mut get_var)
        && loader.path_exists(&path)
    {
        return CliDotenvSelection::Global(path);
    }

    let current_dir_dotenv = loader.current_dir().join(".env");
    if loader.path_exists(&current_dir_dotenv) {
        return CliDotenvSelection::CurrentDirectory(current_dir_dotenv);
    }

    CliDotenvSelection::None
}

pub fn load_cli_dotenv_with<F, E, L>(
    explicit_env_file: Option<&str>,
    get_var: F,
    mut loader: L,
) -> Result<Option<PathBuf>, EnvLoadError>
where
    F: FnMut(&str) -> Result<String, E>,
    L: DotenvLoader,
{
    match select_cli_dotenv(explicit_env_file, get_var, &mut loader) {
        CliDotenvSelection::Explicit(path) | CliDotenvSelection::Environment(path) => loader
            .load_filename(&path)
            .map(Some)
            .map_err(|message| EnvLoadError { path, message }),
        CliDotenvSelection::Global(path) | CliDotenvSelection::CurrentDirectory(path) => {
            let error_path = path.display().to_string();
            loader
                .load_exact_path(&path)
                .map(Some)
                .map_err(|message| EnvLoadError {
                    path: error_path,
                    message,
                })
        }
        CliDotenvSelection::None => Ok(None),
    }
}

pub fn cli_global_dotenv_path() -> Option<PathBuf> {
    cli_global_dotenv_path_with(|key| env::var(key))
}

pub fn cli_global_dotenv_path_with<F, E>(mut get_var: F) -> Option<PathBuf>
where
    F: FnMut(&str) -> Result<String, E>,
{
    cli_global_dotenv_dir_with(&mut get_var).map(|path| path.join(".env"))
}

fn cli_global_dotenv_dir_with<F, E>(get_var: &mut F) -> Option<PathBuf>
where
    F: FnMut(&str) -> Result<String, E>,
{
    #[cfg(target_os = "windows")]
    {
        non_empty_var(get_var, "APPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("workhub"))
    }

    #[cfg(target_os = "macos")]
    {
        non_empty_var(get_var, "HOME")
            .map(PathBuf::from)
            .map(|path| {
                path.join("Library")
                    .join("Application Support")
                    .join("workhub")
            })
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        if let Some(path) = non_empty_var(get_var, "XDG_CONFIG_HOME") {
            return Some(PathBuf::from(path).join("workhub"));
        }

        non_empty_var(get_var, "HOME")
            .map(PathBuf::from)
            .map(|path| path.join(".config").join("workhub"))
    }
}

fn non_empty_var<F, E>(get_var: &mut F, key: &str) -> Option<String>
where
    F: FnMut(&str) -> Result<String, E>,
{
    get_var(key).ok().and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

pub trait DotenvLoader {
    fn load_filename(&mut self, path: &str) -> Result<PathBuf, String>;
    fn load_exact_path(&mut self, path: &Path) -> Result<PathBuf, String>;
    fn load_default(&mut self) -> Result<PathBuf, String>;
    fn path_exists(&mut self, path: &Path) -> bool;
    fn current_dir(&mut self) -> PathBuf;
}

#[derive(Default)]
struct DotenvyLoader;

impl DotenvLoader for DotenvyLoader {
    fn load_filename(&mut self, path: &str) -> Result<PathBuf, String> {
        dotenvy::from_filename(path).map_err(|error| error.to_string())
    }

    fn load_exact_path(&mut self, path: &Path) -> Result<PathBuf, String> {
        dotenvy::from_path(path)
            .map(|_| path.to_path_buf())
            .map_err(|error| error.to_string())
    }

    fn load_default(&mut self) -> Result<PathBuf, String> {
        dotenvy::dotenv().map_err(|error| error.to_string())
    }

    fn path_exists(&mut self, path: &Path) -> bool {
        path.exists()
    }

    fn current_dir(&mut self) -> PathBuf {
        env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct FakeLoader {
        loaded_filenames: Vec<String>,
        loaded_exact_paths: Vec<PathBuf>,
        existing_paths: Vec<PathBuf>,
        current_dir: PathBuf,
        default_loaded: bool,
        fail_filename: bool,
        fail_exact_path: bool,
        fail_default: bool,
    }

    impl DotenvLoader for FakeLoader {
        fn load_filename(&mut self, path: &str) -> Result<PathBuf, String> {
            self.loaded_filenames.push(path.to_string());
            if self.fail_filename {
                Err("missing file".to_string())
            } else {
                Ok(PathBuf::from(path))
            }
        }

        fn load_exact_path(&mut self, path: &Path) -> Result<PathBuf, String> {
            self.loaded_exact_paths.push(path.to_path_buf());
            if self.fail_exact_path {
                Err("invalid file".to_string())
            } else {
                Ok(path.to_path_buf())
            }
        }

        fn load_default(&mut self) -> Result<PathBuf, String> {
            self.default_loaded = true;
            if self.fail_default {
                Err("default missing".to_string())
            } else {
                Ok(PathBuf::from(".env"))
            }
        }

        fn path_exists(&mut self, path: &Path) -> bool {
            self.existing_paths.iter().any(|existing| existing == path)
        }

        fn current_dir(&mut self) -> PathBuf {
            if self.current_dir.as_os_str().is_empty() {
                PathBuf::from("/workspace")
            } else {
                self.current_dir.clone()
            }
        }
    }

    fn env_provider<'a>(
        pairs: &'a [(&'a str, &'a str)],
    ) -> impl FnMut(&str) -> Result<String, ()> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_string())
                .ok_or(())
        }
    }

    #[test]
    fn streamhttp_env_loader_prefers_explicit_env_file_over_env_file_variable() {
        let selected = select_dotenv(
            Some("explicit.env"),
            env_provider(&[(ENV_FILE_VARIABLE, "from-env.env")]),
        );

        assert_eq!(
            selected,
            DotenvSelection::Explicit("explicit.env".to_string())
        );
    }

    #[test]
    fn streamhttp_env_loader_prefers_env_file_variable_over_default_dotenv() {
        let selected = select_dotenv(None, env_provider(&[(ENV_FILE_VARIABLE, "from-env.env")]));

        assert_eq!(
            selected,
            DotenvSelection::Environment("from-env.env".to_string())
        );
    }

    #[test]
    fn streamhttp_env_loader_uses_default_dotenv_when_no_explicit_source_exists() {
        let loaded = load_dotenv_with(None, env_provider(&[]), FakeLoader::default()).unwrap();

        assert_eq!(loaded, Some(PathBuf::from(".env")));
    }

    #[test]
    fn streamhttp_env_loader_reports_explicit_env_file_failure() {
        let error = load_dotenv_with(
            Some("missing.env"),
            env_provider(&[]),
            FakeLoader {
                fail_filename: true,
                ..FakeLoader::default()
            },
        )
        .unwrap_err();

        assert_eq!(error.path(), "missing.env");
    }

    #[test]
    fn streamhttp_env_loader_ignores_missing_default_dotenv() {
        let loaded = load_dotenv_with(
            None,
            env_provider(&[]),
            FakeLoader {
                fail_default: true,
                ..FakeLoader::default()
            },
        )
        .unwrap();

        assert_eq!(loaded, None);
    }

    #[test]
    fn cli_env_loader_prefers_explicit_env_file_over_all_defaults() {
        let loaded = load_cli_dotenv_with(
            Some("explicit.env"),
            env_provider(&[
                (ENV_FILE_VARIABLE, "from-env.env"),
                ("XDG_CONFIG_HOME", "/home/user/.config"),
            ]),
            FakeLoader {
                existing_paths: vec![
                    PathBuf::from("/home/user/.config/workhub/.env"),
                    PathBuf::from("/workspace/.env"),
                ],
                ..FakeLoader::default()
            },
        )
        .unwrap();

        assert_eq!(loaded, Some(PathBuf::from("explicit.env")));
    }

    #[test]
    fn cli_env_loader_prefers_env_file_over_global_config() {
        let loaded = load_cli_dotenv_with(
            None,
            env_provider(&[
                (ENV_FILE_VARIABLE, "from-env.env"),
                ("XDG_CONFIG_HOME", "/home/user/.config"),
            ]),
            FakeLoader {
                existing_paths: vec![PathBuf::from("/home/user/.config/workhub/.env")],
                ..FakeLoader::default()
            },
        )
        .unwrap();

        assert_eq!(loaded, Some(PathBuf::from("from-env.env")));
    }

    #[test]
    fn cli_env_loader_uses_global_dotenv_before_current_directory_dotenv() {
        let loaded = load_cli_dotenv_with(
            None,
            env_provider(&[("XDG_CONFIG_HOME", "/home/user/.config")]),
            FakeLoader {
                existing_paths: vec![
                    PathBuf::from("/home/user/.config/workhub/.env"),
                    PathBuf::from("/workspace/.env"),
                ],
                current_dir: PathBuf::from("/workspace"),
                ..FakeLoader::default()
            },
        )
        .unwrap();

        assert_eq!(
            loaded,
            Some(PathBuf::from("/home/user/.config/workhub/.env"))
        );
    }

    #[test]
    fn cli_env_loader_uses_strict_current_directory_dotenv_as_fallback() {
        let loaded = load_cli_dotenv_with(
            None,
            env_provider(&[("HOME", "/home/user")]),
            FakeLoader {
                existing_paths: vec![PathBuf::from("/workspace/.env")],
                current_dir: PathBuf::from("/workspace"),
                ..FakeLoader::default()
            },
        )
        .unwrap();

        assert_eq!(loaded, Some(PathBuf::from("/workspace/.env")));
    }

    #[test]
    fn cli_env_loader_ignores_missing_defaults() {
        let loaded = load_cli_dotenv_with(
            None,
            env_provider(&[("HOME", "/home/user")]),
            FakeLoader {
                current_dir: PathBuf::from("/workspace"),
                ..FakeLoader::default()
            },
        )
        .unwrap();

        assert_eq!(loaded, None);
    }

    #[test]
    fn cli_env_loader_reports_existing_global_dotenv_failure() {
        let error = load_cli_dotenv_with(
            None,
            env_provider(&[("HOME", "/home/user")]),
            FakeLoader {
                existing_paths: vec![PathBuf::from("/home/user/.config/workhub/.env")],
                fail_exact_path: true,
                ..FakeLoader::default()
            },
        )
        .unwrap_err();

        assert_eq!(error.path(), "/home/user/.config/workhub/.env");
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    #[test]
    fn cli_global_dotenv_path_prefers_xdg_config_home_on_unix() {
        let path = cli_global_dotenv_path_with(env_provider(&[
            ("XDG_CONFIG_HOME", "/custom/config"),
            ("HOME", "/home/user"),
        ]))
        .unwrap();

        assert_eq!(path, PathBuf::from("/custom/config/workhub/.env"));
    }
}
