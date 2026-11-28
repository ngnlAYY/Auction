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

/// 旧 URI 保留驱动的凭据、TLS 和查询覆盖语义，但 host/socket 与库名必须明确；同一目标键
/// 重复出现且取值冲突时按歧义目标拒绝，不依赖后写覆盖。
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
    let authority_host = url.host_str().filter(|host| !host.is_empty());
    let mut query_host: Option<String> = None;
    let mut hostaddr: Option<String> = None;
    let mut database = !url.path().trim_start_matches('/').is_empty();
    let mut port = url.port();
    // 查询参数里的目标键按出现顺序覆盖；重复键只允许取值一致，避免驱动与本校验分叉。
    let mut query_database: Option<String> = None;
    let mut query_port: Option<u16> = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "host" | "hostaddr" => {
                if value.is_empty() {
                    return Err(invalid());
                }
                let selected = if key == "host" { &mut query_host } else { &mut hostaddr };
                if selected.as_deref().is_some_and(|existing| existing != value) {
                    return Err(invalid());
                }
                *selected = Some(value.into_owned());
            }
            "dbname" => {
                if query_database.as_deref().is_some_and(|existing| existing != value.as_ref()) {
                    return Err(invalid());
                }
                database = !value.is_empty();
                query_database = Some(value.into_owned());
            }
            "port" => {
                let parsed = value.parse::<u16>().map_err(|_| invalid())?;
                if query_port.is_some_and(|existing| existing != parsed) {
                    return Err(invalid());
                }
                query_port = Some(parsed);
                port = Some(parsed);
            }
            _ => {}
        }
    }
    if authority_host
        .is_some_and(|authority| query_host.as_deref().is_some_and(|query| authority != query))
        || (authority_host.is_none() && query_host.is_none() && hostaddr.is_none())
        || !database
        || port == Some(0)
    {
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
    auction_application::core::strict_percent_decode(value)
        .is_some_and(|decoded| !decoded.chars().any(char::is_control))
}

fn validate_text(field: &'static str, value: &str, maximum: usize) -> Result<(), ConfigError> {
    if value.trim().is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(ConfigError::InvalidDatabaseConfig(field));
    }
    Ok(())
}

/// 从连接串提取非秘密的账号名，供离线预检区分迁移 owner 与服务运行账号。
/// 只接受与驱动一致的两种写法：URL userinfo 与 `user` 查询参数；两者都没有时返回 None，
/// 不读取 PGUSER 或当前系统账号，避免把本机身份误判为部署账号。
pub fn database_url_username(database_url: &str) -> Option<String> {
    let url = Url::parse(database_url).ok()?;
    // URL userinfo 是序列化的百分号编码文本；与 `user` 查询参数一样先解码，
    // 才能按真实登录名与配置中的账号分工比较。
    let userinfo = url.username();
    if !userinfo.is_empty() {
        return auction_application::core::strict_percent_decode(userinfo).map(Cow::into_owned);
    }
    url.query_pairs()
        .find(|(key, value)| key.as_ref() == "user" && !value.is_empty())
        .map(|(_, value)| value.into_owned())
}

#[cfg(test)]
mod tests {
    use super::{database_url_username, explicit_database_url};

    #[test]
    fn accepts_query_only_tcp_and_unix_socket_endpoints() {
        for url in [
            "postgresql:///auction?host=db.example&port=5432",
            "postgresql:///auction?hostaddr=127.0.0.1&port=5432",
            "postgresql:///auction?host=%2Fvar%2Frun%2Fpostgresql&port=5432",
        ] {
            assert!(explicit_database_url(url.to_owned()).is_ok(), "{url}");
        }
    }

    #[test]
    fn rejects_implicit_or_ambiguous_endpoints() {
        for url in [
            "postgresql:///auction",
            "postgresql:///auction?host=&port=5432",
            "postgresql://db.example/auction?host=other.example&port=5432",
            "postgresql:///auction?host=db.example&host=db.other&port=5432",
            "postgresql:///auction?dbname=auction&dbname=other",
            "postgresql:///auction?host=db.example&port=5432&port=5433",
        ] {
            assert!(explicit_database_url(url.to_owned()).is_err(), "{url}");
        }
    }
    #[test]
    fn accepts_host_and_hostaddr_as_distinct_endpoint_hints() {
        assert!(
            explicit_database_url(
                "postgresql:///auction?host=db.example&hostaddr=127.0.0.1&port=5432".to_owned()
            )
            .is_ok()
        );
    }

    #[test]
    fn decodes_percent_encoded_userinfo_for_account_comparison() {
        assert_eq!(
            database_url_username("postgresql://auction%40owner:secret@db:5432/auction").as_deref(),
            Some("auction@owner")
        );
    }
}
