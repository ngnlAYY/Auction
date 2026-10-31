//! 数据库连接选项规范化及部署目标身份；复用实际驱动解析，不连接数据库或暴露凭据。

use std::path::PathBuf;

#[cfg(feature = "postgres")]
use std::borrow::Cow;

mod strict;
pub use strict::strict_deployment_database_identity;

/// 可比较但不输出内部字段的部署目标身份；用户名、密码不参与数据库身份。
#[derive(PartialEq, Eq)]
pub struct DeploymentDatabaseIdentity {
    endpoint: PathBuf,
    port: u16,
    database: String,
}

/// 返回安全的部署目标解析错误，不保留可能含凭据的驱动错误链。
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct DeploymentDatabaseIdentityError(&'static str);

/// 连接前拒绝未知查询参数，并要求 URI 明确提供 host/socket 和库名。
/// 省略端口固定为 5432，不继承 PGPORT；凭据、Unix socket 和 TLS 仍由 SQLx 解析。
/// CLI 预检与 libpq 参数投影也复用此入口，确保 host/hostaddr 顺序和 Unix socket 优先级一致。
#[cfg(feature = "postgres")]
pub fn postgres_connect_options(
    database_url: &str,
) -> Result<sqlx::postgres::PgConnectOptions, sqlx::Error> {
    let port = explicit_postgres_port(database_url)?;
    // SQLx 在 FromStr 返回前已经查找 pgpass；必须在首次解析前固定端口，避免先选中 PGPORT 的密码。
    let connection_url = if port.is_some() {
        Cow::Borrowed(database_url)
    } else {
        let separator = if database_url.contains('?') { '&' } else { '?' };
        Cow::Owned(format!("{database_url}{separator}port=5432"))
    };
    let options = parse_postgres_options(&connection_url)?;
    if options.get_socket().is_some() {
        return Ok(options);
    }
    let address = options
        .get_host()
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .and_then(|host| host.parse::<std::net::Ipv6Addr>().ok());
    let Some(address) = address else {
        return Ok(options);
    };
    // 丢弃首次解析取得的密码，重新让驱动按最终裸 IPv6 查找 pgpass；显式密码优先级不变。
    // 只追加 ASCII host；原查询键已在首次驱动解析前校验，二次解析保留已固定的端口。
    let separator = if connection_url.contains('?') { '&' } else { '?' };
    parse_postgres_options(&format!("{connection_url}{separator}host={address}"))
}

#[cfg(feature = "postgres")]
fn parse_postgres_options(
    database_url: &str,
) -> Result<sqlx::postgres::PgConnectOptions, sqlx::Error> {
    database_url.parse().map_err(|_| {
        sqlx::Error::Configuration(Box::new(DeploymentDatabaseIdentityError(
            "PostgreSQL 连接参数无效",
        )))
    })
}

/// 与 SQLx 0.9 PostgreSQL URI 解析器保持同一闭集；strict 部署入口额外禁止 options。
/// 未知键和值不能交给驱动，否则 SQLx 会把它们写入 warning。
fn supported_postgres_query_key(key: &str, allow_options: bool) -> bool {
    matches!(
        key,
        "host"
            | "hostaddr"
            | "port"
            | "dbname"
            | "sslmode"
            | "ssl-mode"
            | "sslrootcert"
            | "ssl-root-cert"
            | "ssl-ca"
            | "sslcert"
            | "ssl-cert"
            | "sslkey"
            | "ssl-key"
            | "statement-cache-capacity"
            | "user"
            | "password"
            | "application_name"
    ) || (allow_options
        && (key == "options"
            || key
                .strip_prefix("options[")
                .and_then(|name| name.strip_suffix(']'))
                .is_some_and(|name| !name.is_empty())))
}

/// 适配器也守住显式目标边界，不能依赖调用方一定经过配置 loader 或部署预检。
#[cfg(feature = "postgres")]
fn explicit_postgres_port(database_url: &str) -> Result<Option<u16>, sqlx::Error> {
    crate::database::DatabaseBackend::from_url(database_url)
        .map_err(|error| sqlx::Error::Configuration(Box::new(error)))?;
    let invalid = || {
        sqlx::Error::Configuration(Box::new(DeploymentDatabaseIdentityError(
            "PostgreSQL 连接必须显式指定 host/socket 和数据库，端口必须为非零 u16，且不得包含未知查询参数",
        )))
    };
    let url = url::Url::parse(database_url).map_err(|_| invalid())?;
    let mut host = url.host_str().is_some_and(|host| !host.is_empty());
    let mut database = !url.path().trim_start_matches('/').is_empty();
    let mut port = url.port();
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "host" | "hostaddr" => host = !value.is_empty(),
            "dbname" => database = !value.is_empty(),
            "port" => port = Some(value.parse::<u16>().map_err(|_| invalid())?),
            _ if supported_postgres_query_key(&key, true) => {}
            _ => return Err(invalid()),
        }
    }
    if !host || !database || port == Some(0) {
        return Err(invalid());
    }
    Ok(port)
}

/// 解析部署目标；与实际连接共用显式目标、查询闭集和脱敏错误边界。
pub fn deployment_database_identity(
    database_url: &str,
) -> Result<DeploymentDatabaseIdentity, DeploymentDatabaseIdentityError> {
    #[cfg(feature = "postgres")]
    if database_url.starts_with("postgres://") || database_url.starts_with("postgresql://") {
        let options = postgres_connect_options(database_url)
            .map_err(|_| DeploymentDatabaseIdentityError("PostgreSQL 部署连接参数无效"))?;
        let endpoint = if let Some(socket) = options.get_socket() {
            if !socket.is_absolute() {
                return Err(DeploymentDatabaseIdentityError(
                    "PostgreSQL 部署 socket 必须使用绝对路径",
                ));
            }
            std::fs::canonicalize(socket).map_err(|_| {
                DeploymentDatabaseIdentityError("无法确认 PostgreSQL socket 目录实际身份")
            })?
        } else {
            let host = options.get_host();
            if host.is_empty() || host.contains(',') || host.chars().any(char::is_control) {
                return Err(DeploymentDatabaseIdentityError("PostgreSQL 部署目标 host 无效"));
            }
            PathBuf::from(
                host.parse::<std::net::IpAddr>()
                    .map_or_else(|_| host.to_ascii_lowercase(), |address| address.to_string()),
            )
        };
        let database = options
            .get_database()
            .filter(|value| !value.is_empty())
            .ok_or(DeploymentDatabaseIdentityError("PostgreSQL 部署必须显式指定数据库"))?;
        return Ok(DeploymentDatabaseIdentity {
            endpoint,
            port: options.get_port(),
            database: database.to_owned(),
        });
    }
    let _ = database_url;
    Err(DeploymentDatabaseIdentityError("当前构建不支持部署配置的数据库后端"))
}
