//! 数据库后端枚举、统一存储错误与多个 owner 共用的 PostgreSQL 规则（冲突谓词、命令锁、运维库名与只读快照上限）。

use std::fmt;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseBackend {
    Postgres,
}

impl DatabaseBackend {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
        }
    }

    pub fn from_url(database_url: &str) -> Result<Self, StorageError> {
        // 后端选择也拒绝缩略 URI 和坏编码；具体目标由连接边界继续校验。
        if database_url != database_url.trim() || !valid_url_encoding(database_url) {
            return Err(StorageError::InvalidPostgresUrl);
        }
        let url = url::Url::parse(database_url).map_err(|_| StorageError::InvalidPostgresUrl)?;
        match url.scheme() {
            "postgres" | "postgresql" => {
                let hierarchical = database_url
                    .split_once(':')
                    .is_some_and(|(_, remainder)| remainder.starts_with("//"));
                if !hierarchical || url.cannot_be_a_base() || url.fragment().is_some() {
                    return Err(StorageError::InvalidPostgresUrl);
                }
                Ok(Self::Postgres)
            }
            scheme => Err(StorageError::UnsupportedScheme(scheme.to_owned())),
        }
    }
}

/// 存储适配器统一的仓储失败；消息只描述固定分类，不携带 SQL、连接串或驱动原文。
#[cfg(feature = "postgres")]
pub(crate) fn storage_error(message: &str) -> auction_application::ApplicationError {
    auction_application::RepositoryError::Failed(message.to_owned()).into()
}

/// 仅增强失败诊断；不改变校验、重试或缓存语义，保护不变量 6、7、9。
/// 原始驱动错误不进入应用错误的正文或 source。
#[cfg(feature = "postgres")]
pub(crate) fn storage_error_from(
    error: sqlx::Error,
    context: &'static str,
) -> auction_application::ApplicationError {
    auction_application::RepositoryError::Failed(sql_error_context(&error, context)).into()
}

/// SQL 错误的安全投影；只读取 SQLSTATE、约束名和表名，缺失或异常字段均用固定值。
#[cfg(feature = "postgres")]
pub(crate) fn sql_error_context(error: &sqlx::Error, context: &'static str) -> String {
    super::pool::observe_sql_error(error, auction_observability::DatabaseConflictSite::Other);
    let sqlx_kind = sql_error_kind(error);
    let Some(database) = error.as_database_error() else {
        return format!(
            "{context} [sqlx_kind={sqlx_kind} sqlstate_class=none sqlstate=none constraint=none table=none]"
        );
    };
    let code = database.code();
    let (sqlstate_class, sqlstate) = match code.as_deref() {
        Some(value)
            if value.len() == 5
                && value.bytes().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()) =>
        {
            // 已验证五个 ASCII 字节，按两位类别切片不会切断字符或越界。
            (&value[..2], value)
        }
        Some(_) => ("invalid", "invalid"),
        None => ("none", "none"),
    };
    let constraint = sql_schema_identifier(database.constraint());
    let table = sql_schema_identifier(database.table());
    format!(
        "{context} [sqlx_kind={sqlx_kind} sqlstate_class={sqlstate_class} sqlstate={sqlstate} constraint={constraint} table={table}]"
    )
}

/// 不读取任何变体负载；连接、编码、协议和配置错误可能携带秘密。
#[cfg(feature = "postgres")]
fn sql_error_kind(error: &sqlx::Error) -> &'static str {
    match error {
        sqlx::Error::Configuration(_) => "Configuration",
        sqlx::Error::InvalidArgument(_) => "InvalidArgument",
        sqlx::Error::Database(_) => "Database",
        sqlx::Error::Io(_) => "Io",
        sqlx::Error::Tls(_) => "Tls",
        sqlx::Error::Protocol(_) => "Protocol",
        sqlx::Error::RowNotFound => "RowNotFound",
        sqlx::Error::TypeNotFound { .. } => "TypeNotFound",
        sqlx::Error::ColumnIndexOutOfBounds { .. } => "ColumnIndexOutOfBounds",
        sqlx::Error::ColumnNotFound(_) => "ColumnNotFound",
        sqlx::Error::ColumnDecode { .. } => "ColumnDecode",
        sqlx::Error::Encode(_) => "Encode",
        sqlx::Error::Decode(_) => "Decode",
        sqlx::Error::AnyDriverError(_) => "AnyDriverError",
        sqlx::Error::PoolTimedOut => "PoolTimedOut",
        sqlx::Error::PoolClosed => "PoolClosed",
        sqlx::Error::WorkerCrashed => "WorkerCrashed",
        sqlx::Error::Migrate(_) => "Migrate",
        sqlx::Error::InvalidSavePointStatement => "InvalidSavePointStatement",
        sqlx::Error::BeginFailed => "BeginFailed",
        sqlx::Error::ConfigFile(_) => "ConfigFile",
        _ => "Other",
    }
}

/// 当前迁移仅使用静态 ASCII schema 标识；拒绝控制字符、分隔符与超长诊断字段。
#[cfg(feature = "postgres")]
fn sql_schema_identifier(value: Option<&str>) -> &str {
    match value {
        Some(value)
            if !value.is_empty()
                && value.len() <= 63
                && (value.as_bytes()[0].is_ascii_alphabetic() || value.as_bytes()[0] == b'_')
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') =>
        {
            value
        }
        None => "none",
        Some(_) => "invalid",
    }
}

/// PostgreSQL 明确中止、可由事务 owner 有界重试的冲突：序列化失败（40001）与死锁（40P01）。
/// 全部重试判断共用此谓词，避免各模块手写 SQLSTATE 闭集漂移。
#[cfg(feature = "postgres")]
pub(crate) fn is_serialization_conflict(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| matches!(code.as_ref(), "40001" | "40P01"))
}

/// 独立 Serializable 写事务的默认总尝试次数，与竞价 `TransactionPolicy::serializable()` 一致。
#[cfg(feature = "postgres")]
pub(crate) const SERIALIZABLE_ATTEMPTS: usize = 3;

/// 整事务有界重试：每次调用 `operation` 都必须开启新事务；只有 `retryable` 认定的明确中止才重跑，
/// 其他错误与最后一次结果原样交给调用方映射。重试不重建命令身份、事件 ID 或证据时间，
/// 结果未知的提交也不能被判定为可重试。
#[cfg(feature = "postgres")]
pub(crate) async fn retry_conflicts<T, E, F>(
    pool: &super::TrackedPool<sqlx::Postgres>,
    owner: auction_observability::DatabaseTransactionOwner,
    attempts: usize,
    retryable: impl Fn(&E) -> bool,
    mut operation: impl FnMut() -> F,
) -> Result<T, E>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    pool.observe_transaction(owner, async {
        let mut attempt = 1;
        loop {
            match operation().await {
                Err(error) if attempt < attempts && retryable(&error) => attempt += 1,
                result => {
                    if attempt >= attempts && attempts > 1 && result.as_ref().is_err_and(&retryable)
                    {
                        auction_observability::database_transaction_exhausted();
                    }
                    return result;
                }
            }
        }
    })
    .await
}

/// 以命令 ID 取事务级 advisory 锁，串行化同一幂等身份的并发入口；锁随事务结束释放。
/// 各命令 owner 共用这一键空间（规范 UUID 文本经 `hashtextextended(_, 0)`），不得各自改写哈希或种子。
#[cfg(feature = "postgres")]
pub(crate) async fn lock_command_id(
    connection: &mut sqlx::PgConnection,
    command_id: impl Into<uuid::Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(command_id.into().to_string())
        .execute(connection)
        .await
        .map(drop)
}

/// 运维确认库名的共同边界：非空、无首尾空白与控制字符，且不超过 PostgreSQL 63 字节标识符上限。
/// 只用于和数据库实际名称比对；需拼入 DDL 的名称改用 [`valid_ddl_database_name`]。
#[cfg(feature = "postgres")]
pub(crate) fn valid_confirmed_database_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

/// 创建或重建目标库时拼入 DDL 的库名，额外限定为 ASCII 字母数字与 `_-.`。
#[cfg(feature = "postgres")]
pub(crate) fn valid_ddl_database_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

/// 以双引号包裹并转义内嵌双引号，使库名、owner 与 tablespace 作为单个 PostgreSQL 标识符拼入 DDL。
#[cfg(feature = "postgres")]
pub(crate) fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// 为 `begin_read_snapshot` 开启的运维只读快照设置语句与锁等待上限（5s/1s）；
/// 恢复预检与保留盘点共用同一阈值。失败时返回对应设置名，由调用方映射为自身错误。
#[cfg(feature = "postgres")]
pub(crate) async fn set_read_only_limits(
    connection: &mut sqlx::PgConnection,
) -> Result<(), (&'static str, sqlx::Error)> {
    sqlx::query("SET LOCAL statement_timeout = '5s'")
        .execute(&mut *connection)
        .await
        .map_err(|source| ("statement_timeout", source))?;
    sqlx::query("SET LOCAL lock_timeout = '1s'")
        .execute(connection)
        .await
        .map_err(|source| ("lock_timeout", source))?;
    Ok(())
}

/// 不向驱动传入会被容忍或有损解码的 URI，错误只报告固定分类。
fn valid_url_encoding(value: &str) -> bool {
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

impl fmt::Display for DatabaseBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("unsupported database URL scheme {0:?}; expected postgres:// or postgresql://")]
    UnsupportedScheme(String),

    #[error("PostgreSQL URL 必须是无 fragment、编码有效的 postgres:// 或 postgresql:// URI")]
    InvalidPostgresUrl,

    #[error("failed to connect to {backend} database")]
    Connection {
        backend: DatabaseBackend,
        #[source]
        source: sqlx::Error,
    },

    #[error("failed to migrate {backend} database")]
    Migration {
        backend: DatabaseBackend,
        #[source]
        source: sqlx::migrate::MigrateError,
    },

    #[error("{backend} migration source invariant failed: {detail}")]
    MigrationSourceInvariant { backend: DatabaseBackend, detail: &'static str },

    #[error("failed to inspect {backend} migration target")]
    MigrationTarget {
        backend: DatabaseBackend,
        #[source]
        source: sqlx::Error,
    },

    #[error("refusing to migrate an unknown non-empty {backend} database")]
    UnknownDatabase { backend: DatabaseBackend },

    #[error("failed to apply runtime database privileges")]
    RuntimePrivileges {
        #[source]
        source: sqlx::Error,
    },
}

/// 映射前保存冲突种类；统计在尝试结束时去重，纯冲突谓词不承担计数副作用。
#[cfg(feature = "postgres")]
pub(crate) fn observe_transaction_error(error: &sqlx::Error) {
    super::pool::observe_sql_error(error, auction_observability::DatabaseConflictSite::Other);
}

/// 映射前附加元数据后调用纯冲突谓词；不改变旧白名单或布尔结果。
#[cfg(feature = "postgres")]
pub(crate) fn observed_serialization_conflict(error: &sqlx::Error) -> bool {
    observe_transaction_error(error);
    is_serialization_conflict(error)
}

/// 只有原 owner 的终止分支确认用尽多次尝试预算时，才记录耗尽。
#[cfg(feature = "postgres")]
pub(crate) fn observe_retry_exhaustion(attempt: usize, attempts: usize, retryable: bool) {
    if attempts > 1 && attempt >= attempts && retryable {
        auction_observability::database_transaction_exhausted();
    }
}
