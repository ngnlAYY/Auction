//! PostgreSQL 独立连接字段到进程内部敏感 URL 的唯一转换边界。

use std::{borrow::Cow, fmt, net::IpAddr};

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
        url.set_username(&escape_userinfo(&self.username))
            .map_err(|_| ConfigError::InvalidDatabaseConfig("database.username"))?;
        url.set_password(Some(&escape_userinfo(&self.password)))
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

/// 未含百分号的凭据直接借用，避免生成无用的敏感字符串副本。
fn escape_userinfo(value: &str) -> Cow<'_, str> {
    if value.contains('%') { Cow::Owned(value.replace('%', "%25")) } else { Cow::Borrowed(value) }
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
            // 不让驱动以 PG* 环境变量或当前账号补齐遗漏的目标；保留原文凭据编码。
            SecretString::new("database_url", explicit_database_url(value)?)
        })
        .transpose()
}

/// 旧 URI 保留驱动的凭据、TLS 和查询覆盖语义，但 host/socket 与库名必须明确。
/// 省略端口使用 PostgreSQL 的固定默认值，不继承进程环境中的 PGPORT。
fn explicit_database_url(value: String) -> Result<String, ConfigError> {
    let invalid = || ConfigError::InvalidDatabaseConfig("database_url");
    if value != value.trim() || !valid_url_encoding(&value) {
        return Err(invalid());
    }
    let Some((scheme, _)) = value.split_once("://") else { return Err(invalid()) };
    if !scheme.eq_ignore_ascii_case("postgres") && !scheme.eq_ignore_ascii_case("postgresql") {
        return Err(invalid());
    }
    let url = Url::parse(&value).map_err(|_| invalid())?;
    if url.cannot_be_a_base() || url.fragment().is_some() {
        return Err(invalid());
    }
    let mut host = url.host_str().is_some_and(|host| !host.is_empty());
    let mut database = !url.path().trim_start_matches('/').is_empty();
    let mut port = url.port();
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "host" | "hostaddr" => host = !value.is_empty(),
            "dbname" => database = !value.is_empty(),
            "port" => port = Some(value.parse::<u16>().map_err(|_| invalid())?),
            _ => {}
        }
    }
    if !host || !database || port == Some(0) {
        return Err(invalid());
    }
    if port.is_none() {
        let separator = if value.contains('?') { '&' } else { '?' };
        Ok(format!("{value}{separator}port=5432"))
    } else {
        Ok(value)
    }
}

/// URL 解析会容忍坏转义，查询解码还会替换非法 UTF-8；在此拒绝，避免目标被静默改写。
fn valid_url_encoding(value: &str) -> bool {
    // 原文已经是 UTF-8；没有转义时直接检查控制字符，不复制完整连接串。
    if !value.as_bytes().contains(&b'%') {
        return !value.chars().any(char::is_control);
    }
    let mut bytes = value.bytes();
    let mut decoded = Vec::with_capacity(value.len());
    while let Some(byte) = bytes.next() {
        let byte = if byte == b'%' {
            let Some(high) = bytes.next().and_then(|byte| char::from(byte).to_digit(16)) else {
                return false;
            };
            let Some(low) = bytes.next().and_then(|byte| char::from(byte).to_digit(16)) else {
                return false;
            };
            (high * 16 + low) as u8
        } else {
            byte
        };
        decoded.push(byte);
    }
    std::str::from_utf8(&decoded).is_ok_and(|value| !value.chars().any(char::is_control))
}

fn validate_text(field: &'static str, value: &str, maximum: usize) -> Result<(), ConfigError> {
    if value.trim().is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(ConfigError::InvalidDatabaseConfig(field));
    }
    Ok(())
}
