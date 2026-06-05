use std::collections::BTreeSet;

use reqwest::Url;

use crate::{atlassian::auth::AtlassianAuth, error::ConfigError};

pub const ENV_CONFLUENCE_URL: &str = "CONFLUENCE_URL";
pub const ENV_CONFLUENCE_USERNAME: &str = "CONFLUENCE_USERNAME";
pub const ENV_CONFLUENCE_API_TOKEN: &str = "CONFLUENCE_API_TOKEN";
pub const ENV_CONFLUENCE_PERSONAL_TOKEN: &str = "CONFLUENCE_PERSONAL_TOKEN";
pub const ENV_CONFLUENCE_SSL_VERIFY: &str = "CONFLUENCE_SSL_VERIFY";
pub const ENV_CONFLUENCE_SPACES_FILTER: &str = "CONFLUENCE_SPACES_FILTER";
pub const ENV_CONFLUENCE_TIMEOUT: &str = "CONFLUENCE_TIMEOUT";

pub const DEFAULT_CONFLUENCE_TIMEOUT_SECONDS: u64 = 75;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfluenceConfig {
    pub base_url: String,
    pub deployment: ConfluenceDeployment,
    pub auth: AtlassianAuth,
    pub ssl_verify: bool,
    pub spaces_filter: BTreeSet<String>,
    pub timeout_seconds: u64,
}

impl ConfluenceConfig {
    pub fn from_var_provider<F, E>(get_var: &mut F) -> Result<Option<Self>, ConfigError>
    where
        F: FnMut(&str) -> Result<String, E>,
    {
        let base_url = optional_var(get_var, ENV_CONFLUENCE_URL);
        let username = optional_var(get_var, ENV_CONFLUENCE_USERNAME);
        let api_token = optional_var(get_var, ENV_CONFLUENCE_API_TOKEN);
        let personal_token = optional_var(get_var, ENV_CONFLUENCE_PERSONAL_TOKEN);

        let Some(base_url) = base_url else {
            let credential_variables = present_variables([
                (ENV_CONFLUENCE_USERNAME, username.as_ref()),
                (ENV_CONFLUENCE_API_TOKEN, api_token.as_ref()),
                (ENV_CONFLUENCE_PERSONAL_TOKEN, personal_token.as_ref()),
            ]);

            if credential_variables.is_empty() {
                return Ok(None);
            }

            return Err(ConfigError::MissingConfluenceUrl {
                credential_variables,
            });
        };

        let parsed_url = parse_base_url(&base_url)?;
        let deployment = ConfluenceDeployment::from_base_url(&parsed_url);
        let auth = match deployment {
            ConfluenceDeployment::Cloud => {
                let missing_variables = missing_variables([
                    (ENV_CONFLUENCE_USERNAME, username.as_ref()),
                    (ENV_CONFLUENCE_API_TOKEN, api_token.as_ref()),
                ]);

                if !missing_variables.is_empty() {
                    return Err(ConfigError::MissingConfluenceCloudCredentials {
                        missing_variables,
                    });
                }

                AtlassianAuth::Basic {
                    username: username.expect("missing variables were checked"),
                    api_token: api_token.expect("missing variables were checked"),
                }
            }
            ConfluenceDeployment::ServerDataCenter => {
                let Some(personal_token) = personal_token else {
                    return Err(ConfigError::MissingConfluencePersonalToken {
                        variable: ENV_CONFLUENCE_PERSONAL_TOKEN,
                    });
                };

                AtlassianAuth::Pat { personal_token }
            }
        };

        Ok(Some(Self {
            base_url: normalize_base_url(parsed_url),
            deployment,
            auth,
            ssl_verify: parse_ssl_verify(
                optional_var(get_var, ENV_CONFLUENCE_SSL_VERIFY).as_deref(),
            ),
            spaces_filter: parse_spaces_filter(optional_var(get_var, ENV_CONFLUENCE_SPACES_FILTER)),
            timeout_seconds: parse_timeout_seconds(optional_var(get_var, ENV_CONFLUENCE_TIMEOUT))?,
        }))
    }

    pub fn is_auth_configured(&self) -> bool {
        matches!(
            self.auth,
            AtlassianAuth::Basic { .. } | AtlassianAuth::Pat { .. }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfluenceDeployment {
    Cloud,
    ServerDataCenter,
}

impl ConfluenceDeployment {
    fn from_base_url(url: &Url) -> Self {
        if url
            .host_str()
            .is_some_and(|host| host.to_ascii_lowercase().ends_with(".atlassian.net"))
        {
            Self::Cloud
        } else {
            Self::ServerDataCenter
        }
    }
}

fn optional_var<F, E>(get_var: &mut F, key: &'static str) -> Option<String>
where
    F: FnMut(&str) -> Result<String, E>,
{
    get_var(key).ok().and_then(non_empty_trimmed)
}

fn non_empty_trimmed(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_base_url(value: &str) -> Result<Url, ConfigError> {
    let url = Url::parse(value).map_err(|_| ConfigError::InvalidConfluenceUrl {
        variable: ENV_CONFLUENCE_URL,
    })?;

    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ConfigError::InvalidConfluenceUrl {
            variable: ENV_CONFLUENCE_URL,
        });
    }

    Ok(url)
}

fn normalize_base_url(mut url: Url) -> String {
    url.set_query(None);
    url.set_fragment(None);
    url.to_string().trim_end_matches('/').to_string()
}

fn parse_ssl_verify(value: Option<&str>) -> bool {
    !matches!(
        value.map(|value| value.trim().to_ascii_lowercase()),
        Some(value) if matches!(value.as_str(), "false" | "0" | "no" | "off")
    )
}

fn parse_spaces_filter(value: Option<String>) -> BTreeSet<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_timeout_seconds(value: Option<String>) -> Result<u64, ConfigError> {
    let Some(value) = value else {
        return Ok(DEFAULT_CONFLUENCE_TIMEOUT_SECONDS);
    };

    let seconds: u64 = value
        .parse()
        .map_err(|_| ConfigError::InvalidConfluenceTimeout {
            variable: ENV_CONFLUENCE_TIMEOUT,
            value: value.clone(),
        })?;

    if seconds == 0 {
        return Err(ConfigError::InvalidConfluenceTimeout {
            variable: ENV_CONFLUENCE_TIMEOUT,
            value,
        });
    }

    Ok(seconds)
}

fn present_variables<const N: usize>(
    variables: [(&'static str, Option<&String>); N],
) -> Vec<&'static str> {
    variables
        .into_iter()
        .filter_map(|(name, value)| value.map(|_| name))
        .collect()
}

fn missing_variables<const N: usize>(
    variables: [(&'static str, Option<&String>); N],
) -> Vec<&'static str> {
    variables
        .into_iter()
        .filter_map(|(name, value)| if value.is_none() { Some(name) } else { None })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    fn config_from_pairs(pairs: &[(&str, &str)]) -> Result<Option<ConfluenceConfig>, ConfigError> {
        let vars: BTreeMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();

        ConfluenceConfig::from_var_provider(&mut |key| vars.get(key).cloned().ok_or(()))
    }

    #[test]
    fn confluence_config_is_disabled_without_url_or_credentials() {
        assert_eq!(config_from_pairs(&[]).unwrap(), None);
    }

    #[test]
    fn credentials_without_url_are_rejected_without_secret_leakage() {
        let error =
            config_from_pairs(&[(ENV_CONFLUENCE_PERSONAL_TOKEN, "test-pat-value")]).unwrap_err();

        assert_eq!(
            error,
            ConfigError::MissingConfluenceUrl {
                credential_variables: vec![ENV_CONFLUENCE_PERSONAL_TOKEN],
            }
        );
        assert!(!error.to_string().contains("test-pat-value"));
    }

    #[test]
    fn cloud_config_requires_username_and_api_token() {
        let error =
            config_from_pairs(&[(ENV_CONFLUENCE_URL, "https://example.atlassian.net/wiki")])
                .unwrap_err();

        assert_eq!(
            error,
            ConfigError::MissingConfluenceCloudCredentials {
                missing_variables: vec![ENV_CONFLUENCE_USERNAME, ENV_CONFLUENCE_API_TOKEN],
            }
        );
    }

    #[test]
    fn cloud_config_builds_basic_auth_and_preserves_wiki_base_path() {
        let config = config_from_pairs(&[
            (ENV_CONFLUENCE_URL, " https://example.atlassian.net/wiki/ "),
            (ENV_CONFLUENCE_USERNAME, "user@example.com"),
            (ENV_CONFLUENCE_API_TOKEN, "test-api-token"),
            (ENV_CONFLUENCE_SSL_VERIFY, "off"),
            (ENV_CONFLUENCE_SPACES_FILTER, " ENG, DOCS,ENG "),
            (ENV_CONFLUENCE_TIMEOUT, "30"),
        ])
        .unwrap()
        .unwrap();

        assert_eq!(config.base_url, "https://example.atlassian.net/wiki");
        assert_eq!(config.deployment, ConfluenceDeployment::Cloud);
        assert_eq!(
            config.auth,
            AtlassianAuth::Basic {
                username: "user@example.com".to_string(),
                api_token: "test-api-token".to_string(),
            }
        );
        assert!(!config.ssl_verify);
        assert_eq!(
            config.spaces_filter,
            BTreeSet::from(["DOCS".to_string(), "ENG".to_string()])
        );
        assert_eq!(config.timeout_seconds, 30);
        assert!(config.is_auth_configured());
    }

    #[test]
    fn server_config_requires_pat() {
        let error =
            config_from_pairs(&[(ENV_CONFLUENCE_URL, "https://confluence.example")]).unwrap_err();

        assert_eq!(
            error,
            ConfigError::MissingConfluencePersonalToken {
                variable: ENV_CONFLUENCE_PERSONAL_TOKEN,
            }
        );
    }

    #[test]
    fn server_config_builds_pat_auth_with_defaults() {
        let config = config_from_pairs(&[
            (ENV_CONFLUENCE_URL, "https://confluence.example"),
            (ENV_CONFLUENCE_PERSONAL_TOKEN, "test-pat-value"),
        ])
        .unwrap()
        .unwrap();

        assert_eq!(config.base_url, "https://confluence.example");
        assert_eq!(config.deployment, ConfluenceDeployment::ServerDataCenter);
        assert_eq!(
            config.auth,
            AtlassianAuth::Pat {
                personal_token: "test-pat-value".to_string(),
            }
        );
        assert!(config.ssl_verify);
        assert!(config.spaces_filter.is_empty());
        assert_eq!(config.timeout_seconds, DEFAULT_CONFLUENCE_TIMEOUT_SECONDS);
        assert!(config.is_auth_configured());
    }

    #[test]
    fn invalid_timeout_is_rejected() {
        let error = config_from_pairs(&[
            (ENV_CONFLUENCE_URL, "https://confluence.example"),
            (ENV_CONFLUENCE_PERSONAL_TOKEN, "test-pat-value"),
            (ENV_CONFLUENCE_TIMEOUT, "0"),
        ])
        .unwrap_err();

        assert_eq!(
            error,
            ConfigError::InvalidConfluenceTimeout {
                variable: ENV_CONFLUENCE_TIMEOUT,
                value: "0".to_string(),
            }
        );
    }

    #[test]
    fn invalid_url_is_rejected() {
        let error = config_from_pairs(&[(ENV_CONFLUENCE_URL, "not-a-url")]).unwrap_err();

        assert_eq!(
            error,
            ConfigError::InvalidConfluenceUrl {
                variable: ENV_CONFLUENCE_URL,
            }
        );
    }
}
