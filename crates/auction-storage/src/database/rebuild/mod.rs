//! PostgreSQL 数据库破坏性重建适配器。
//!
//! 本模块只拥有集群级的只读预检、advisory lock 和 `DROP DATABASE`/`CREATE DATABASE` SQL。
//! 调用方必须在进入本边界前完成精确目标确认、备份或一次性环境授权，并在重建后验证空库、
//! 恢复结果或迁移结果。由于 PostgreSQL 不允许在事务中执行建库/删库，本模块会在错误中明确
//! 报告目标是否已经删除或重新创建，不能把该过程描述成原子事务。

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{AssertSqlSafe, Connection, PgConnection, Row};
use thiserror::Error;

/// 数据库重建预检的机器可读格式标识。
pub const POSTGRES_REBUILD_PREFLIGHT_FORMAT: &str = "auction-postgres-rebuild-preflight.v1";

const MAINTENANCE_DATABASE_NAME: &str = "postgres";
const SUPPORTED_POSTGRES_MAJOR: i32 = 18;

/// 连接集群维护库后收集的重建预检证据。
#[derive(Debug, Clone, Serialize)]
pub struct PostgresRebuildPreflight {
    pub format: &'static str,
    pub collected_at: DateTime<Utc>,
    pub maintenance_database_name: String,
    pub server_version_num: i32,
    pub target_database_name: String,
    pub target_database_oid: i64,
    pub target_owner: String,
    pub target_encoding: String,
    pub target_locale_provider: String,
    pub target_tablespace: String,
    pub target_connection_limit: i32,
    pub target_allows_connections: bool,
    pub target_is_template: bool,
    pub target_has_custom_acl: bool,
    pub target_has_role_settings: bool,
    pub current_user: String,
    pub current_user_superuser: bool,
    pub current_user_createdb: bool,
    pub current_user_owns_target: bool,
    pub current_user_can_create_in_tablespace: bool,
    pub in_recovery: bool,
    pub default_transaction_read_only: bool,
    pub active_target_sessions: i64,
    pub prepared_transactions: i64,
    pub active_logical_replication_slots: i64,
    pub subscriptions: i64,
    pub advisory_lock_acquired: bool,
    pub rebuild_permitted: bool,
}

/// 已完成一次删库并重建空库后的结构化结果。
#[derive(Debug, Clone, Serialize)]
pub struct PostgresRebuildResult {
    pub database_name: String,
    pub database_owner: String,
    pub previous_database_oid: i64,
    pub new_database_oid: i64,
    pub encoding: String,
    pub locale_provider: String,
    pub tablespace: String,
    pub connection_limit: i32,
    pub template_database: &'static str,
    pub forced_drop: bool,
}

/// PostgreSQL 重建边界的安全错误分类。
#[derive(Debug, Error)]
pub enum PostgresRebuildError {
    #[error("confirmed PostgreSQL rebuild database name is invalid")]
    InvalidDatabaseName,

    #[error("refusing to rebuild a protected PostgreSQL maintenance database")]
    ProtectedDatabase,

    #[error("failed to connect to the PostgreSQL maintenance database")]
    Connect(#[source] sqlx::Error),

    #[error("failed to query PostgreSQL rebuild state at {stage}")]
    Query {
        stage: &'static str,
        #[source]
        source: sqlx::Error,
    },

    #[error("PostgreSQL rebuild preflight failed: {reason}")]
    UnsafePreflight { reason: &'static str },

    #[error(
        "PostgreSQL rebuild mutation failed at {stage}; database_dropped={database_dropped} database_recreated={database_recreated}"
    )]
    Mutation {
        stage: &'static str,
        database_dropped: bool,
        database_recreated: bool,
        #[source]
        source: sqlx::Error,
    },

    #[error(
        "PostgreSQL rebuild postcondition failed at {stage}; database_dropped=true database_recreated=true"
    )]
    Postcondition { stage: &'static str },
}

impl PostgresRebuildError {
    /// 返回已经发生的破坏性状态，供运维层决定是否必须发布失败回执。
    pub const fn mutation_state(&self) -> Option<(&'static str, bool, bool)> {
        match self {
            Self::Mutation { stage, database_dropped, database_recreated, .. } => {
                Some((stage, *database_dropped, *database_recreated))
            }
            Self::Postcondition { stage } => Some((stage, true, true)),
            _ => None,
        }
    }
}

/// 持有集群级 advisory lock 的破坏性重建会话。
///
/// guard 存活期间，同一数据库的其他 Auction 重建命令不能越过预检；外部数据库管理员仍需遵守
/// 同一维护窗口，advisory lock 不能替代部署停机和权限隔离。
pub struct PostgresDatabaseRebuildGuard {
    connection: PgConnection,
    preflight: PostgresRebuildPreflight,
    definition: TargetDatabaseDefinition,
}

impl PostgresDatabaseRebuildGuard {
    /// 连接固定的 `postgres` 维护库、取得 advisory lock，并完成不修改目标库的严格预检。
    pub async fn connect(
        maintenance_database_url: &str,
        confirmed_database_name: &str,
    ) -> Result<Self, PostgresRebuildError> {
        validate_database_name(confirmed_database_name)?;
        if matches!(confirmed_database_name, "postgres" | "template0" | "template1") {
            return Err(PostgresRebuildError::ProtectedDatabase);
        }

        let mut connection = PgConnection::connect(maintenance_database_url)
            .await
            .map_err(PostgresRebuildError::Connect)?;
        configure_session(&mut connection).await?;
        let advisory_lock_acquired = sqlx::query_scalar::<_, bool>(
            "SELECT pg_try_advisory_lock(hashtextextended('auction-postgres-rebuild:' || $1, 0))",
        )
        .bind(confirmed_database_name)
        .fetch_one(&mut connection)
        .await
        .map_err(|source| query_error("advisory_lock", source))?;
        if !advisory_lock_acquired {
            return Err(PostgresRebuildError::UnsafePreflight {
                reason: "another Auction rebuild command holds the database advisory lock",
            });
        }

        let collected =
            collect_state(&mut connection, confirmed_database_name, advisory_lock_acquired).await?;
        require_safe_preflight(&collected.preflight)?;
        Ok(Self { connection, preflight: collected.preflight, definition: collected.definition })
    }

    /// 返回已持锁的只读预检证据。
    pub const fn preflight(&self) -> &PostgresRebuildPreflight {
        &self.preflight
    }

    /// 删除已确认目标并以相同 owner、编码、locale provider、tablespace 和连接上限创建空库。
    ///
    /// `DROP DATABASE` 和 `CREATE DATABASE` 都不能放入事务；调用方必须检查错误中的
    /// `database_dropped`/`database_recreated`，并保持全部应用角色停止直到完成后置验证。
    pub async fn recreate_empty(&mut self) -> Result<PostgresRebuildResult, PostgresRebuildError> {
        let current =
            collect_state(&mut self.connection, &self.preflight.target_database_name, true).await?;
        require_safe_preflight(&current.preflight)?;
        if current.definition != self.definition
            || current.preflight.target_database_oid != self.preflight.target_database_oid
            || current.preflight.target_owner != self.preflight.target_owner
        {
            return Err(PostgresRebuildError::UnsafePreflight {
                reason: "target database identity changed after the initial rebuild preflight",
            });
        }

        // 建库 SQL 的 locale/provider/tablespace 校验必须在 DROP 前全部完成；否则本地定义错误也会
        // 让目标在已经删除后才失败，破坏“所有可预见错误先于 mutation”的边界。
        let create_sql = create_database_sql(
            &self.preflight.target_database_name,
            &self.preflight.target_owner,
            &self.definition,
        )?;
        let database_identifier = quote_identifier(&self.preflight.target_database_name);
        let drop_sql = format!("DROP DATABASE {database_identifier} WITH (FORCE)");
        // DDL 不能绑定数据库/角色标识符；所有动态标识符和 literal 都经本模块的 SQL quoting
        // 函数构造，数据库名还额外受保守 ASCII profile 限制，因此在这一最小边界显式审计。
        sqlx::query(AssertSqlSafe(drop_sql)).execute(&mut self.connection).await.map_err(
            |source| PostgresRebuildError::Mutation {
                stage: "drop_database",
                database_dropped: false,
                database_recreated: false,
                source,
            },
        )?;

        sqlx::query(AssertSqlSafe(create_sql)).execute(&mut self.connection).await.map_err(
            |source| PostgresRebuildError::Mutation {
                stage: "create_database",
                database_dropped: true,
                database_recreated: false,
                source,
            },
        )?;

        let recreated =
            collect_state(&mut self.connection, &self.preflight.target_database_name, true)
                .await
                .map_err(|error| match error {
                PostgresRebuildError::Query { source, .. } => PostgresRebuildError::Mutation {
                    stage: "verify_recreated_database",
                    database_dropped: true,
                    database_recreated: true,
                    source,
                },
                _ => PostgresRebuildError::Postcondition { stage: "verify_recreated_database" },
            })?;
        if recreated.preflight.target_database_oid == self.preflight.target_database_oid
            || recreated.preflight.target_owner != self.preflight.target_owner
            || recreated.definition != self.definition
            || recreated.preflight.active_target_sessions != 0
        {
            return Err(PostgresRebuildError::Postcondition {
                stage: "verify_recreated_database_identity",
            });
        }

        Ok(PostgresRebuildResult {
            database_name: self.preflight.target_database_name.clone(),
            database_owner: self.preflight.target_owner.clone(),
            previous_database_oid: self.preflight.target_database_oid,
            new_database_oid: recreated.preflight.target_database_oid,
            encoding: recreated.preflight.target_encoding,
            locale_provider: recreated.preflight.target_locale_provider,
            tablespace: recreated.preflight.target_tablespace,
            connection_limit: recreated.preflight.target_connection_limit,
            template_database: "template0",
            forced_drop: true,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetDatabaseDefinition {
    encoding: String,
    locale_provider: LocaleProvider,
    lc_collate: String,
    lc_ctype: String,
    provider_locale: Option<String>,
    icu_rules: Option<String>,
    tablespace: String,
    connection_limit: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocaleProvider {
    Builtin,
    Libc,
    Icu,
}

impl LocaleProvider {
    fn from_catalog(value: &str) -> Result<Self, PostgresRebuildError> {
        match value {
            "b" => Ok(Self::Builtin),
            "c" => Ok(Self::Libc),
            "i" => Ok(Self::Icu),
            _ => Err(PostgresRebuildError::UnsafePreflight {
                reason: "target database uses an unsupported locale provider",
            }),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Libc => "libc",
            Self::Icu => "icu",
        }
    }
}

struct CollectedState {
    preflight: PostgresRebuildPreflight,
    definition: TargetDatabaseDefinition,
}

async fn configure_session(connection: &mut PgConnection) -> Result<(), PostgresRebuildError> {
    for (stage, statement) in [
        ("application_name", "SET application_name = 'auctionctl/postgres-rebuild'"),
        ("search_path", "SET search_path = pg_catalog"),
        ("standard_conforming_strings", "SET standard_conforming_strings = on"),
        ("statement_timeout", "SET statement_timeout = '30s'"),
        ("lock_timeout", "SET lock_timeout = '5s'"),
    ] {
        sqlx::query(statement)
            .execute(&mut *connection)
            .await
            .map_err(|source| query_error(stage, source))?;
    }
    Ok(())
}

async fn collect_state(
    connection: &mut PgConnection,
    target_database_name: &str,
    advisory_lock_acquired: bool,
) -> Result<CollectedState, PostgresRebuildError> {
    let cluster = sqlx::query(
        "SELECT current_database() AS maintenance_database_name,
                current_user AS current_user,
                current_setting('server_version_num')::integer AS server_version_num,
                pg_is_in_recovery() AS in_recovery,
                current_setting('default_transaction_read_only') = 'on'
                    AS default_transaction_read_only,
                role.rolsuper AS current_user_superuser,
                role.rolcreatedb AS current_user_createdb
         FROM pg_catalog.pg_roles AS role
         WHERE role.rolname = current_user",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(|source| query_error("cluster_identity", source))?;
    let target = sqlx::query(
        "SELECT database.oid::bigint AS database_oid,
                pg_catalog.pg_get_userbyid(database.datdba) AS database_owner,
                pg_catalog.pg_encoding_to_char(database.encoding) AS encoding,
                database.datlocprovider::text AS locale_provider,
                database.datcollate AS lc_collate,
                database.datctype AS lc_ctype,
                database.datlocale AS provider_locale,
                database.daticurules AS icu_rules,
                tablespace.spcname AS tablespace,
                database.datconnlimit AS connection_limit,
                database.datallowconn AS allows_connections,
                database.datistemplate AS is_template,
                database.datacl IS NOT NULL AS has_custom_acl,
                EXISTS (
                    SELECT 1 FROM pg_catalog.pg_db_role_setting AS setting
                    WHERE setting.setdatabase = database.oid
                ) AS has_role_settings,
                has_tablespace_privilege(current_user, database.dattablespace, 'CREATE')
                    AS can_create_in_tablespace,
                (SELECT COUNT(*)::bigint FROM pg_catalog.pg_stat_activity
                 WHERE datname = database.datname) AS active_sessions,
                (SELECT COUNT(*)::bigint FROM pg_catalog.pg_prepared_xacts AS prepared
                 WHERE prepared.database = database.datname) AS prepared_transactions,
                (SELECT COUNT(*)::bigint FROM pg_catalog.pg_replication_slots AS slot
                 WHERE slot.database = database.datname AND slot.active) AS active_logical_slots,
                (SELECT COUNT(*)::bigint FROM pg_catalog.pg_subscription AS subscription
                 WHERE subscription.subdbid = database.oid) AS subscriptions
         FROM pg_catalog.pg_database AS database
         JOIN pg_catalog.pg_tablespace AS tablespace
           ON tablespace.oid = database.dattablespace
         WHERE database.datname = $1",
    )
    .bind(target_database_name)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|source| query_error("target_database", source))?
    .ok_or(PostgresRebuildError::UnsafePreflight {
        reason: "confirmed target database does not exist",
    })?;

    let maintenance_database_name: String = row_value(&cluster, "maintenance_database_name")?;
    let current_user: String = row_value(&cluster, "current_user")?;
    let server_version_num: i32 = row_value(&cluster, "server_version_num")?;
    let in_recovery: bool = row_value(&cluster, "in_recovery")?;
    let default_transaction_read_only: bool = row_value(&cluster, "default_transaction_read_only")?;
    let current_user_superuser: bool = row_value(&cluster, "current_user_superuser")?;
    let current_user_createdb: bool = row_value(&cluster, "current_user_createdb")?;
    let target_database_oid: i64 = row_value(&target, "database_oid")?;
    let target_owner: String = row_value(&target, "database_owner")?;
    let encoding: String = row_value(&target, "encoding")?;
    let locale_provider_code: String = row_value(&target, "locale_provider")?;
    let locale_provider = LocaleProvider::from_catalog(&locale_provider_code)?;
    let lc_collate: String = row_value(&target, "lc_collate")?;
    let lc_ctype: String = row_value(&target, "lc_ctype")?;
    let provider_locale: Option<String> = row_value(&target, "provider_locale")?;
    let icu_rules: Option<String> = row_value(&target, "icu_rules")?;
    let tablespace: String = row_value(&target, "tablespace")?;
    let connection_limit: i32 = row_value(&target, "connection_limit")?;
    let target_allows_connections: bool = row_value(&target, "allows_connections")?;
    let target_is_template: bool = row_value(&target, "is_template")?;
    let target_has_custom_acl: bool = row_value(&target, "has_custom_acl")?;
    let target_has_role_settings: bool = row_value(&target, "has_role_settings")?;
    let current_user_can_create_in_tablespace: bool =
        row_value(&target, "can_create_in_tablespace")?;
    let active_target_sessions: i64 = row_value(&target, "active_sessions")?;
    let prepared_transactions: i64 = row_value(&target, "prepared_transactions")?;
    let active_logical_replication_slots: i64 = row_value(&target, "active_logical_slots")?;
    let subscriptions: i64 = row_value(&target, "subscriptions")?;
    let current_user_owns_target = current_user == target_owner;
    let rebuild_permitted = maintenance_database_name == MAINTENANCE_DATABASE_NAME
        && server_version_num / 10_000 == SUPPORTED_POSTGRES_MAJOR
        && !in_recovery
        && !default_transaction_read_only
        && target_allows_connections
        && !target_is_template
        && !target_has_custom_acl
        && !target_has_role_settings
        && encoding == "UTF8"
        && connection_limit >= -1
        && (current_user_superuser || (current_user_owns_target && current_user_createdb))
        && (current_user_superuser || current_user_can_create_in_tablespace)
        && active_target_sessions == 0
        && prepared_transactions == 0
        && active_logical_replication_slots == 0
        && subscriptions == 0
        && advisory_lock_acquired;

    Ok(CollectedState {
        preflight: PostgresRebuildPreflight {
            format: POSTGRES_REBUILD_PREFLIGHT_FORMAT,
            collected_at: Utc::now(),
            maintenance_database_name,
            server_version_num,
            target_database_name: target_database_name.to_owned(),
            target_database_oid,
            target_owner,
            target_encoding: encoding.clone(),
            target_locale_provider: locale_provider.name().to_owned(),
            target_tablespace: tablespace.clone(),
            target_connection_limit: connection_limit,
            target_allows_connections,
            target_is_template,
            target_has_custom_acl,
            target_has_role_settings,
            current_user,
            current_user_superuser,
            current_user_createdb,
            current_user_owns_target,
            current_user_can_create_in_tablespace,
            in_recovery,
            default_transaction_read_only,
            active_target_sessions,
            prepared_transactions,
            active_logical_replication_slots,
            subscriptions,
            advisory_lock_acquired,
            rebuild_permitted,
        },
        definition: TargetDatabaseDefinition {
            encoding,
            locale_provider,
            lc_collate,
            lc_ctype,
            provider_locale,
            icu_rules,
            tablespace,
            connection_limit,
        },
    })
}

fn require_safe_preflight(
    preflight: &PostgresRebuildPreflight,
) -> Result<(), PostgresRebuildError> {
    let reason = if preflight.maintenance_database_name != MAINTENANCE_DATABASE_NAME {
        Some("maintenance connection is not attached to the postgres database")
    } else if preflight.server_version_num / 10_000 != SUPPORTED_POSTGRES_MAJOR {
        Some("server major version is outside the supported PostgreSQL 18 contract")
    } else if preflight.in_recovery || preflight.default_transaction_read_only {
        Some("maintenance connection is read-only or attached to a recovery replica")
    } else if !preflight.target_allows_connections || preflight.target_is_template {
        Some("target database is connection-disabled or marked as a template")
    } else if preflight.target_has_custom_acl {
        Some("target database has a custom ACL that this rebuild contract will not discard")
    } else if preflight.target_has_role_settings {
        Some(
            "target database has database/role settings that this rebuild contract will not discard",
        )
    } else if preflight.target_encoding != "UTF8" {
        Some("target database encoding is not UTF8")
    } else if preflight.target_connection_limit < -1 {
        Some("target database connection limit is invalid")
    } else if !(preflight.current_user_superuser
        || (preflight.current_user_owns_target && preflight.current_user_createdb))
    {
        Some("maintenance role must be superuser or the target owner with CREATEDB")
    } else if !(preflight.current_user_superuser || preflight.current_user_can_create_in_tablespace)
    {
        Some("maintenance role cannot recreate the database in its current tablespace")
    } else if preflight.active_target_sessions != 0 {
        Some("target database still has active sessions; stop every role and client first")
    } else if preflight.prepared_transactions != 0 {
        Some("target database has prepared transactions")
    } else if preflight.active_logical_replication_slots != 0 {
        Some("target database has active logical replication slots")
    } else if preflight.subscriptions != 0 {
        Some("target database has subscriptions")
    } else if !preflight.advisory_lock_acquired {
        Some("database rebuild advisory lock is not held")
    } else if !preflight.rebuild_permitted {
        Some("database rebuild preflight did not reach a permitted state")
    } else {
        None
    };
    match reason {
        Some(reason) => Err(PostgresRebuildError::UnsafePreflight { reason }),
        None => Ok(()),
    }
}

fn create_database_sql(
    database_name: &str,
    owner: &str,
    definition: &TargetDatabaseDefinition,
) -> Result<String, PostgresRebuildError> {
    let mut sql = format!(
        "CREATE DATABASE {} WITH OWNER = {} TEMPLATE = template0 ENCODING = {}",
        quote_identifier(database_name),
        quote_identifier(owner),
        quote_literal(&definition.encoding),
    );
    match definition.locale_provider {
        LocaleProvider::Builtin => {
            let locale = definition.provider_locale.as_deref().ok_or(
                PostgresRebuildError::UnsafePreflight {
                    reason: "builtin locale provider is missing its provider locale",
                },
            )?;
            sql.push_str(" LOCALE_PROVIDER = builtin BUILTIN_LOCALE = ");
            sql.push_str(&quote_literal(locale));
        }
        LocaleProvider::Libc => {
            sql.push_str(" LOCALE_PROVIDER = libc LC_COLLATE = ");
            sql.push_str(&quote_literal(&definition.lc_collate));
            sql.push_str(" LC_CTYPE = ");
            sql.push_str(&quote_literal(&definition.lc_ctype));
        }
        LocaleProvider::Icu => {
            let locale = definition.provider_locale.as_deref().ok_or(
                PostgresRebuildError::UnsafePreflight {
                    reason: "ICU locale provider is missing its provider locale",
                },
            )?;
            sql.push_str(" LOCALE_PROVIDER = icu ICU_LOCALE = ");
            sql.push_str(&quote_literal(locale));
            sql.push_str(" LC_COLLATE = ");
            sql.push_str(&quote_literal(&definition.lc_collate));
            sql.push_str(" LC_CTYPE = ");
            sql.push_str(&quote_literal(&definition.lc_ctype));
            if let Some(rules) = definition.icu_rules.as_deref().filter(|rules| !rules.is_empty()) {
                sql.push_str(" ICU_RULES = ");
                sql.push_str(&quote_literal(rules));
            }
        }
    }
    sql.push_str(" TABLESPACE = ");
    sql.push_str(&quote_identifier(&definition.tablespace));
    sql.push_str(" CONNECTION LIMIT = ");
    sql.push_str(&definition.connection_limit.to_string());
    Ok(sql)
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn validate_database_name(value: &str) -> Result<(), PostgresRebuildError> {
    if value.is_empty()
        || value.len() > 63
        || value.trim() != value
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(PostgresRebuildError::InvalidDatabaseName);
    }
    Ok(())
}

fn row_value<T>(
    row: &sqlx::postgres::PgRow,
    column: &'static str,
) -> Result<T, PostgresRebuildError>
where
    for<'decode> T: sqlx::Decode<'decode, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(column).map_err(|source| query_error("decode_catalog_row", source))
}

fn query_error(stage: &'static str, source: sqlx::Error) -> PostgresRebuildError {
    PostgresRebuildError::Query { stage, source }
}
