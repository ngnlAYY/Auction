//! 数据库后端枚举、统一存储错误与多个 owner 共用的 PostgreSQL 规则（冲突谓词、命令锁、运维库名与只读快照上限）。

use std::{borrow::Cow, fmt};

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
/// 各 owner 的 `mapping` 模块直接复用本构造，不再复制函数体。
pub(crate) fn repository_error(message: &str) -> auction_application::core::RepositoryError {
    auction_application::core::RepositoryError::Failed(message.to_owned())
}

/// [`repository_error`] 的应用错误形态；与 `RepositoryError` 之间的转换完全一致。
pub(crate) fn repository_failure(message: &str) -> auction_application::core::ApplicationError {
    repository_error(message).into()
}

/// 存储适配器统一的仓储失败；消息只描述固定分类，不携带 SQL、连接串或驱动原文。
pub(crate) fn storage_error(message: &str) -> auction_application::core::ApplicationError {
    repository_failure(message)
}

/// 仅增强失败诊断；不改变校验、重试或缓存语义，保护不变量 6、7、9。
/// 原始驱动错误不进入应用错误的正文或 source。
pub(crate) fn storage_error_from(
    error: sqlx::Error,
    context: &'static str,
) -> auction_application::core::ApplicationError {
    auction_application::core::RepositoryError::Failed(sql_error_context(&error, context)).into()
}

/// 读取路径的固定分类：`RowNotFound` 与其余失败各自投影，变体与重试分类不变。
pub(crate) fn storage_failure(
    error: &sqlx::Error,
    not_found: &'static str,
    other: &'static str,
) -> auction_application::core::RepositoryError {
    let context = match error {
        sqlx::Error::RowNotFound => not_found,
        _ => other,
    };
    repository_error(&sql_error_context(error, context))
}

/// 写路径的固定分类：40001 序列化冲突使用专用文案；重试白名单仍由事务 owner 判断。
pub(crate) fn serialization_failure(
    error: &sqlx::Error,
    conflict: &'static str,
    other: &'static str,
) -> auction_application::core::RepositoryError {
    let context = match error {
        sqlx::Error::Database(database) if database.code().as_deref() == Some("40001") => conflict,
        _ => other,
    };
    repository_error(&sql_error_context(error, context))
}

/// SQL 错误的安全投影；只读取 SQLSTATE、约束名和表名，缺失或异常字段均用固定值。
pub(crate) fn sql_error_context(error: &sqlx::Error, context: &'static str) -> String {
    super::pool::observe_sql_error(error, auction_observability::DatabaseConflictSite::Other);
    let sqlx_kind = sql_error_kind(error);
    let sqlstate = sql_error_sqlstate(error);
    // 合法 SQLSTATE 恰为五个 ASCII 字节，前两位是类别；固定词 `invalid`/`none` 同时充当类别。
    let sqlstate_class = if sqlstate.len() == 5 { &sqlstate[..2] } else { &*sqlstate };
    let database = error.as_database_error();
    let constraint = sql_schema_identifier(database.and_then(|database| database.constraint()));
    let table = sql_schema_identifier(database.and_then(|database| database.table()));
    format!(
        "{context} [sqlx_kind={sqlx_kind} sqlstate_class={sqlstate_class} sqlstate={sqlstate} constraint={constraint} table={table}]"
    )
}

/// 经校验的 SQLSTATE：五位大写字母或数字原样返回；格式异常为 `invalid`，
/// 非数据库错误或缺失为 `none`。
pub(crate) fn sql_error_sqlstate(error: &sqlx::Error) -> Cow<'_, str> {
    match error.as_database_error().and_then(sqlx::error::DatabaseError::code) {
        Some(code)
            if code.len() == 5
                && code.bytes().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()) =>
        {
            code
        }
        Some(_) => Cow::Borrowed("invalid"),
        None => Cow::Borrowed("none"),
    }
}

/// SQLx 错误的变体名。不读取任何变体负载；连接、编码、协议和配置错误可能携带秘密。
pub(crate) fn sql_error_kind(error: &sqlx::Error) -> &'static str {
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
pub(crate) fn is_serialization_conflict(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .is_some_and(|code| matches!(code.as_ref(), "40001" | "40P01"))
}

/// 独立 Serializable 写事务的默认总尝试次数，与竞价 `TransactionPolicy::serializable()` 一致。
pub(crate) const SERIALIZABLE_ATTEMPTS: usize = 3;

/// Serializable 冲突重试的退避窗口基数：第 n 次失败的窗口为 `基数 * 2^(n-1)`，并以
/// [`SERIALIZABLE_RETRY_WINDOW_CAP`] 封顶。三次尝试下额外等待不超过 18ms，远低于请求与
/// Worker item 预算，不改变重试预算本身。
const SERIALIZABLE_RETRY_WINDOW_BASE: std::time::Duration = std::time::Duration::from_millis(6);

/// 退避窗口上限；指数增长到此为止，避免重试把延迟推到用户可见。
const SERIALIZABLE_RETRY_WINDOW_CAP: std::time::Duration = std::time::Duration::from_millis(24);

/// 带抖动的 Serializable 冲突退避：只有 owner 已决定重跑（预算未耗尽）时才调用。
///
/// 多个事务同时因同一行序号或同一谓词范围冲突时，固定等待会让它们在相同时刻同步重试并
/// 再次碰撞；这里把等待取窗口的一半再加一半的均匀抖动，窗口随尝试次数指数增长并封顶。
/// 等待只影响时序，不改变命令身份、隔离级别、锁序、重试白名单、重试次数或错误返回；
/// `entropy` 只用于抖动，必须保持调用方的重试预算不变。
pub(crate) fn serializable_retry_delay(attempt: usize, entropy: u64) -> std::time::Duration {
    let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(u32::MAX).min(16);
    let window = SERIALIZABLE_RETRY_WINDOW_BASE
        .saturating_mul(1_u32 << exponent)
        .min(SERIALIZABLE_RETRY_WINDOW_CAP);
    let half = window / 2;
    let span_micros = u64::try_from((window - half).as_micros()).unwrap_or(u64::MAX);
    let jitter = entropy % span_micros.saturating_add(1);
    half + std::time::Duration::from_micros(jitter)
}

/// 每次重试前调用一次；抖动熵只取单调计数器与系统时钟，不读取业务身份或配置。
pub(crate) async fn serializable_retry_backoff(attempt: usize) {
    tokio::time::sleep(serializable_retry_delay(attempt, retry_entropy())).await;
}

/// 抖动熵，不需要密码学质量：只用于打散同时到达的重试，不参与业务判定。
fn retry_entropy() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    let clock = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64 ^ elapsed.subsec_nanos() as u64);
    (counter ^ clock.rotate_left(17)).wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// 整事务有界重试：每次调用 `operation` 都必须开启新事务；只有 `retryable` 认定的明确中止才重跑，
/// 其他错误与最后一次结果原样交给调用方映射。重试不重建命令身份、事件 ID 或证据时间，
/// 结果未知的提交也不能被判定为可重试。
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
                Err(error) if attempt < attempts && retryable(&error) => {
                    // 只有确定重跑才退避；尝试次数与错误返回保持原状。
                    serializable_retry_backoff(attempt + 1).await;
                    attempt += 1;
                }
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
pub(crate) fn valid_confirmed_database_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

/// 创建或重建目标库时拼入 DDL 的库名，额外限定为 ASCII 字母数字与 `_-.`。
pub(crate) fn valid_ddl_database_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

/// 以双引号包裹并转义内嵌双引号，使库名、owner 与 tablespace 作为单个 PostgreSQL 标识符拼入 DDL。
pub(crate) fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// 为 `begin_read_snapshot` 开启的运维只读快照设置语句与锁等待上限（5s/1s）；
/// 恢复预检与保留盘点共用同一阈值。失败时返回对应设置名，由调用方映射为自身错误。
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
    auction_application::core::strict_percent_decode(value)
        .is_some_and(|decoded| !decoded.chars().any(char::is_control))
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
pub(crate) fn observe_transaction_error(error: &sqlx::Error) {
    super::pool::observe_sql_error(error, auction_observability::DatabaseConflictSite::Other);
}

/// 映射前附加元数据后调用纯冲突谓词；不改变旧白名单或布尔结果。
pub(crate) fn observed_serialization_conflict(error: &sqlx::Error) -> bool {
    observe_transaction_error(error);
    is_serialization_conflict(error)
}

/// 只有原 owner 的终止分支确认用尽多次尝试预算时，才记录耗尽。
pub(crate) fn observe_retry_exhaustion(attempt: usize, attempts: usize, retryable: bool) {
    if attempts > 1 && attempt >= attempts && retryable {
        auction_observability::database_transaction_exhausted();
    }
}

#[cfg(test)]
mod serializable_retry_tests {
    use super::{SERIALIZABLE_ATTEMPTS, serializable_retry_delay};
    use std::time::Duration;

    /// 抖动只允许落在「窗口一半 + [0, 一半]」内，且窗口随尝试次数增长并封顶；重试预算不变。
    #[test]
    fn serializable_retry_delay_stays_inside_the_jitter_window() {
        assert_eq!(SERIALIZABLE_ATTEMPTS, 3, "退避不得改变尝试次数");
        let expected_windows = [
            Duration::from_millis(6),
            Duration::from_millis(12),
            Duration::from_millis(24),
            Duration::from_millis(24),
            Duration::from_millis(24),
        ];
        for (offset, window) in expected_windows.into_iter().enumerate() {
            let attempt = offset + 1;
            let half = window / 2;
            for entropy in [0_u64, 1, 7, u64::MAX, 0x0123_4567_89AB_CDEF] {
                let delay = serializable_retry_delay(attempt, entropy);
                assert!(
                    delay >= half && delay <= window,
                    "attempt {attempt} entropy {entropy} 的等待 {delay:?} 应在 [{half:?}, {window:?}]"
                );
            }
        }
    }

    /// 第一次重试的窗口不超过第二次，保证退避单调增强而不是随机漂移。
    #[test]
    fn serializable_retry_window_grows_before_it_is_capped() {
        let first = serializable_retry_delay(1, 0);
        let second = serializable_retry_delay(2, 0);
        let capped = serializable_retry_delay(64, u64::MAX);
        assert!(first < second, "第二次重试应等待更久");
        assert!(
            capped <= Duration::from_millis(24),
            "极端尝试次数也必须落在封顶窗口内：{capped:?}"
        );
    }
}
