//! 数据库连接选项规范化及部署目标身份；复用实际驱动解析，不连接数据库或暴露凭据。

#[cfg(feature = "sqlite")]
use std::path::Path;
use std::path::PathBuf;

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

/// 让 SQLx 按规范化后的最终 TCP IPv6 host 查找 pgpass；保留查询覆盖与密码优先级。
/// Unix socket、用户名、密码和 TLS 继续由驱动解析，不能用原 URI authority 覆盖最终目标。
#[cfg(feature = "postgres")]
pub(crate) fn postgres_connect_options(
    database_url: &str,
) -> Result<sqlx::postgres::PgConnectOptions, sqlx::Error> {
    let options = database_url.parse::<sqlx::postgres::PgConnectOptions>()?;
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
    // 只追加 ASCII host；fragment 不参与连接，原查询参数仍先经驱动逐项校验。
    let target = database_url.split_once('#').map_or(database_url, |(target, _)| target);
    let separator = if target.contains('?') { '&' } else { '?' };
    format!("{target}{separator}host={address}").parse()
}

/// 解析部署目标；调用方须先拒绝 PostgreSQL 的隐式目标和未知参数。
///
/// SQLite 必须是绝对磁盘路径，避免 CLI 与 systemd WorkingDirectory 不同而误判同库。
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
    #[cfg(feature = "sqlite")]
    if database_url.starts_with("sqlite:") {
        use sqlx::ConnectOptions;
        let options = database_url
            .parse::<sqlx::sqlite::SqliteConnectOptions>()
            .map_err(|_| DeploymentDatabaseIdentityError("SQLite 部署连接参数无效"))?;
        if options
            .to_url_lossy()
            .query_pairs()
            .any(|(key, value)| (key == "mode" && value == "memory") || key == "vfs")
        {
            return Err(DeploymentDatabaseIdentityError(
                "SQLite 部署不能使用内存数据库或自定义 VFS",
            ));
        }
        return Ok(DeploymentDatabaseIdentity {
            endpoint: canonical_database_path(options.get_filename())?,
            port: 0,
            database: String::new(),
        });
    }
    let _ = database_url;
    Err(DeploymentDatabaseIdentityError("当前构建不支持部署配置的数据库后端"))
}

#[cfg(feature = "sqlite")]
fn canonical_database_path(path: &Path) -> Result<PathBuf, DeploymentDatabaseIdentityError> {
    if !path.is_absolute() {
        return Err(DeploymentDatabaseIdentityError(
            "部署数据库文件或 Unix socket 目录必须使用绝对路径，不能依赖进程工作目录",
        ));
    }
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // 新建 SQLite 文件允许不存在，但父目录必须能证明实际身份。
            if std::fs::symlink_metadata(path).is_ok() {
                return Err(DeploymentDatabaseIdentityError("部署数据库路径不能是悬空符号链接"));
            }
            let parent =
                path.parent().ok_or(DeploymentDatabaseIdentityError("部署数据库路径无效"))?;
            let filename =
                path.file_name().ok_or(DeploymentDatabaseIdentityError("部署数据库路径无效"))?;
            let parent = std::fs::canonicalize(parent).map_err(|_| {
                DeploymentDatabaseIdentityError("无法确认部署数据库父目录的实际身份")
            })?;
            Ok(parent.join(filename))
        }
        Err(_) => Err(DeploymentDatabaseIdentityError("无法确认部署数据库路径的实际身份")),
    }
}
