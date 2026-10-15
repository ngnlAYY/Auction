//! PostgreSQL 独立连接字段到进程内部敏感 URL 的唯一转换边界。

use std::{fmt, net::IpAddr};

use serde::{Deserialize, Serialize};
use url::{Host, Url};

use super::{super::ConfigError, model::SecretString};

/// 敏感 TOML 的 PostgreSQL 连接参数；账号和密码按原文填写，不能预先做 URL 编码。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PostgresConnectionConfig {
    /// 主机名或 IP 地址，不包含协议、端口、账号或路径。
    pub host: String,
    /// 显式 TCP 端口，范围 1-65535。
    pub port: u16,
    /// PostgreSQL 登录账号。
    pub username: String,
    /// PostgreSQL 登录密码；不得输出到日志或诊断。
    pub password: String,
    /// 明确的数据库名称，不从环境或账号推导。
    pub name: String,
}

impl PostgresConnectionConfig {
    /// 生成只供数据库适配器使用的敏感连接串；错误仅包含字段名，返回值不得记录或回传。
    pub fn connection_url(&self) -> Result<String, ConfigError> {
        validate_text("database.host", &self.host, 253)?;
        validate_text("database.username", &self.username, 128)?;
        validate_text("database.password", &self.password, 4096)?;
        validate_text("database.name", &self.name, 128)?;
        if self.host != self.host.trim() || self.host.contains('%') {
            return Err(ConfigError::InvalidDatabaseConfig("database.host"));
        }
        if self.port == 0 {
            return Err(ConfigError::InvalidDatabaseConfig("database.port"));
        }
        // URL 路径会规范化点段；不能因此把指定数据库静默替换成驱动的默认库。
        if matches!(self.name.as_str(), "." | "..") {
            return Err(ConfigError::InvalidDatabaseConfig("database.name"));
        }

        let mut url = Url::parse("postgresql://localhost")
            .map_err(|_| ConfigError::InvalidDatabaseConfig("database.host"))?;
        if let Ok(address) = self.host.parse::<IpAddr>() {
            url.set_ip_host(address)
                .map_err(|_| ConfigError::InvalidDatabaseConfig("database.host"))?;
        } else {
            let host = Host::parse(&self.host)
                .map_err(|_| ConfigError::InvalidDatabaseConfig("database.host"))?;
            url.set_host(Some(&host.to_string()))
                .map_err(|_| ConfigError::InvalidDatabaseConfig("database.host"))?;
        }
        url.set_port(Some(self.port))
            .map_err(|_| ConfigError::InvalidDatabaseConfig("database.port"))?;
        // URL 的 userinfo setter 保留已有百分号；先保护原文中的 %，防止 SQLx 多解释一层转义。
        url.set_username(&self.username.replace('%', "%25"))
            .map_err(|_| ConfigError::InvalidDatabaseConfig("database.username"))?;
        url.set_password(Some(&self.password.replace('%', "%25")))
            .map_err(|_| ConfigError::InvalidDatabaseConfig("database.password"))?;
        url.path_segments_mut()
            .map_err(|_| ConfigError::InvalidDatabaseConfig("database.name"))?
            .clear()
            .push(&self.name);
        Ok(url.into())
    }
}

impl fmt::Debug for PostgresConnectionConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PostgresConnectionConfig([REDACTED])")
    }
}

/// 兼容旧 URL，但禁止两个来源并存，避免静默连接到非预期数据库。
pub(super) fn resolve_database_url(
    database: Option<PostgresConnectionConfig>,
    database_url: Option<String>,
) -> Result<Option<SecretString>, ConfigError> {
    let value = match (database, database_url) {
        (Some(_), Some(_)) => return Err(ConfigError::ConflictingDatabaseConfiguration),
        (Some(database), None) => Some(database.connection_url()?),
        (None, database_url) => database_url,
    };
    value
        .map(|value| {
            // 仅接受 PostgreSQL 协议；拒绝未知协议且不回显连接串，不静默切换数据库。
            let postgres = value.split_once(':').is_some_and(|(scheme, _)| {
                scheme.eq_ignore_ascii_case("postgres") || scheme.eq_ignore_ascii_case("postgresql")
            });
            if !postgres {
                return Err(ConfigError::InvalidDatabaseConfig("database_url"));
            }
            SecretString::new("database_url", value)
        })
        .transpose()
}

fn validate_text(field: &'static str, value: &str, maximum: usize) -> Result<(), ConfigError> {
    if value.trim().is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(ConfigError::InvalidDatabaseConfig(field));
    }
    Ok(())
}
