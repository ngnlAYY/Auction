use sqlx::{AssertSqlSafe, PgConnection};

use super::{
    catalog::{collect_state, configure_session, query_error, require_safe_preflight},
    model::{
        PostgresRebuildError, PostgresRebuildPreflight, PostgresRebuildResult,
        TargetDatabaseDefinition,
    },
    sql::{create_database_sql, quote_identifier, validate_database_name},
};

pub(crate) const DATABASE_ADVISORY_LOCK_PREFIX: &str = "auction-postgres-database:";

/// 持有集群级 advisory lock 的破坏性重建会话。
/// guard 存活期间同一数据库的其他 Auction 重建命令不能越过预检；外部管理员仍须遵守同一维护窗口，
/// advisory lock 不能替代部署停机和权限隔离。
pub struct PostgresDatabaseRebuildGuard {
    connection: PgConnection,
    preflight: PostgresRebuildPreflight,
    definition: TargetDatabaseDefinition,
}

impl PostgresDatabaseRebuildGuard {
    /// 连接固定的 postgres 维护库、取得 advisory lock，并完成不修改目标库的严格预检。
    pub async fn connect(
        maintenance_database_url: &str,
        confirmed_database_name: &str,
        profile: crate::DatabaseConnectionProfile,
    ) -> Result<Self, PostgresRebuildError> {
        validate_database_name(confirmed_database_name)?;
        if matches!(confirmed_database_name, "postgres" | "template0" | "template1") {
            return Err(PostgresRebuildError::ProtectedDatabase);
        }

        let mut connection = profile
            .connect(maintenance_database_url)
            .await
            .map_err(PostgresRebuildError::Connect)?;
        configure_session(&mut connection).await?;
        let advisory_lock_acquired = sqlx::query_scalar::<_, bool>(
            "SELECT pg_try_advisory_lock(hashtextextended($1 || $2, 0))",
        )
        .bind(DATABASE_ADVISORY_LOCK_PREFIX)
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
    /// DROP/CREATE DATABASE 都不能放入事务；调用方必须检查错误中的 database_dropped/
    /// database_recreated，并保持全部应用角色停止直到完成后置验证。
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

        let create_sql = create_database_sql(
            &self.preflight.target_database_name,
            &self.preflight.target_owner,
            &self.definition,
        )?;
        let database_identifier = quote_identifier(&self.preflight.target_database_name);
        let drop_sql = format!("DROP DATABASE {database_identifier} WITH (FORCE)");
        sqlx::query(AssertSqlSafe(drop_sql)).execute(&mut self.connection).await.map_err(
            |source| PostgresRebuildError::Mutation {
                stage: "drop_database",
                // 响应丢失时 DROP 可能已经提交；此处没有独立 catalog 证明，必须保留未知结果。
                database_dropped: None,
                database_recreated: Some(false),
                source,
            },
        )?;

        sqlx::query(AssertSqlSafe(create_sql)).execute(&mut self.connection).await.map_err(
            |source| PostgresRebuildError::Mutation {
                stage: "create_database",
                database_dropped: Some(true),
                database_recreated: None,
                source,
            },
        )?;

        let recreated =
            collect_state(&mut self.connection, &self.preflight.target_database_name, true)
                .await
                .map_err(|error| match error {
                PostgresRebuildError::Query { source, .. } => PostgresRebuildError::Mutation {
                    stage: "verify_recreated_database",
                    database_dropped: Some(true),
                    database_recreated: Some(true),
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
