use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    InvalidHttpPort {
        variable: &'static str,
        value: String,
    },
}

impl Display for ConfigError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHttpPort { variable, value } => {
                write!(formatter, "invalid {variable} value `{value}`")
            }
        }
    }
}

impl std::error::Error for ConfigError {}
