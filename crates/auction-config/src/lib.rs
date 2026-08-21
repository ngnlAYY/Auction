use std::{
    env, fmt, fs, io,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::Deserialize;
use thiserror::Error;

const CONFIG_FILE_ENV: &str = "AUCTION_CONFIG_FILE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Public,
    Admin,
    Worker,
}

impl Role {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Admin => "admin",
            Self::Worker => "worker",
        }
    }

    const fn default_listen(self) -> Option<&'static str> {
        match self {
            Self::Public => Some("127.0.0.1:8090"),
            Self::Admin => Some("127.0.0.1:8091"),
            Self::Worker => None,
        }
    }
}

impl FromStr for Role {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "public" => Ok(Self::Public),
            "admin" => Ok(Self::Admin),
            "worker" => Ok(Self::Worker),
            _ => Err(ConfigError::UnknownRole(value.to_owned())),
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("unknown process role: {0}")]
    UnknownRole(String),

    #[error("failed to read TOML configuration {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to parse TOML configuration {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("invalid {field} value {value:?}: {source}")]
    InvalidAddress { field: &'static str, value: String, source: std::net::AddrParseError },
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    listen_address: Option<String>,
    database_url: Option<String>,
    log_filter: Option<String>,
}

#[derive(Clone)]
pub struct AppConfig {
    pub role: Role,
    pub listen_addr: Option<SocketAddr>,
    pub database_url: Option<String>,
    pub log_filter: Option<String>,
    pub config_path: Option<PathBuf>,
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppConfig")
            .field("role", &self.role)
            .field("listen_addr", &self.listen_addr)
            .field("database_configured", &self.database_url.is_some())
            .field("log_filter", &self.log_filter)
            .field("config_path", &self.config_path)
            .finish()
    }
}

impl AppConfig {
    pub fn load(role: Role) -> Result<Self, ConfigError> {
        if let Some(path) = env::var_os(CONFIG_FILE_ENV).map(PathBuf::from) {
            return Self::load_from_path(role, path);
        }

        let default_path = PathBuf::from("ops/config").join(format!("{}.toml", role.name()));
        if default_path.is_file() {
            return Self::load_from_path(role, default_path);
        }

        Self::from_file(role, None, FileConfig::default())
    }

    pub fn load_from_path(role: Role, path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref().to_owned();
        let contents = fs::read_to_string(&path)
            .map_err(|source| ConfigError::Read { path: path.clone(), source })?;
        let file = toml::from_str(&contents)
            .map_err(|source| ConfigError::Parse { path: path.clone(), source })?;

        Self::from_file(role, Some(path), file)
    }

    fn from_file(
        role: Role,
        config_path: Option<PathBuf>,
        file: FileConfig,
    ) -> Result<Self, ConfigError> {
        let listen_value = file.listen_address.or_else(|| role.default_listen().map(str::to_owned));
        let listen_addr = listen_value
            .map(|value| {
                value.parse().map_err(|source| ConfigError::InvalidAddress {
                    field: "listen_address",
                    value,
                    source,
                })
            })
            .transpose()?;

        Ok(Self {
            role,
            listen_addr,
            database_url: file.database_url,
            log_filter: file.log_filter,
            config_path,
        })
    }
}
