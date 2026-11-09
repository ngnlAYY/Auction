//! readiness 专用迁移兼容判定；恢复、盘点和迁移执行继续使用完整历史的严格校验。

use crate::database::TrackedTransaction as Transaction;
use std::time::Duration;

use auction_application::core::{ApplicationError, RepositoryError};
use auction_application::operations::MigrationCompatibilityMode;
use auction_observability::MigrationReadinessBranch;
use sqlx::{Postgres, Row, postgres::PgRow};

use crate::database::{
    DatabaseStore, postgres_expected_migration_version, set_read_only_limits, storage_error,
};

// 与 HTTP 底层检查采用同一上限；heartbeat 和 supervisor 的独立调用也不能无限等待。
const CHECK_TIMEOUT: Duration = Duration::from_millis(750);

struct Assessment {
    strict: bool,
    compatible: bool,
    branch: MigrationReadinessBranch,
    database_version: i64,
}

impl Assessment {
    fn rejected(branch: MigrationReadinessBranch, database_version: i64) -> Self {
        Self { strict: false, compatible: false, branch, database_version }
    }
}

/// 在有界只读快照内核对 schema；仅显式 compatible 模式接受完整兼容后缀。
pub(in crate::database) async fn check_postgres_readiness(
    store: &DatabaseStore,
) -> Result<(), ApplicationError> {
    let expected_version = postgres_expected_migration_version();
    // 超时只证明预算内没有完成检查；不得投影为 repository_failed，否则与真实数据库错误混淆。
    let result = tokio::time::timeout(CHECK_TIMEOUT, read_snapshot(store, expected_version))
        .await
        .map_err(|error| {
            let _ = error;
            ApplicationError::from(RepositoryError::Timeout)
        })
        .and_then(std::convert::identity);
    if result.is_err() {
        store.migration_readiness_recorder.observe(MigrationReadinessBranch::QueryFailed, false);
    }
    let assessment = result?;
    let shadow_mismatch = store.migration_compatibility_mode == MigrationCompatibilityMode::Shadow
        && assessment.strict != assessment.compatible;
    store.migration_readiness_recorder.observe(assessment.branch, shadow_mismatch);
    if shadow_mismatch {
        // 暂沿用既有就绪影子观测，后续统一。版本号不进入指标 label。
        tracing::warn!(
            target: "auction::storage::migration_readiness",
            class = assessment.branch.label(),
            expected_version,
            database_version = assessment.database_version,
            strict_ready = assessment.strict,
            compatible_ready = assessment.compatible,
            "迁移就绪影子判定出现预期差异"
        );
    }
    let ready = match store.migration_compatibility_mode {
        MigrationCompatibilityMode::Strict | MigrationCompatibilityMode::Shadow => {
            assessment.strict
        }
        MigrationCompatibilityMode::Compatible => assessment.compatible,
    };
    if ready {
        Ok(())
    } else {
        Err(storage_error("数据库迁移历史不满足 readiness 策略"))
    }
}

/// 有界预算内的饱和/取消是暂时不可用，不是仓储故障；schema 与其他驱动错误保留原诊断。
/// 与审计分类共用同一组瞬时 SQLSTATE，避免把预期负载下的超时记成 repository_failed。
fn readiness_error(error: sqlx::Error, context: &'static str) -> ApplicationError {
    if is_transient(&error) {
        RepositoryError::Unavailable.into()
    } else {
        crate::database::storage_error_from(error, context)
    }
}

fn is_transient(error: &sqlx::Error) -> bool {
    if matches!(error, sqlx::Error::PoolTimedOut) {
        return true;
    }
    let Some(code) = error.as_database_error().and_then(|database| database.code()) else {
        return false;
    };
    code.starts_with("08")
        || matches!(
            code.as_ref(),
            "40001" | "40P01" | "53300" | "55P03" | "57014" | "57P01" | "57P02" | "57P03"
        )
}

async fn read_snapshot(
    store: &DatabaseStore,
    expected_version: i64,
) -> Result<Assessment, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin_read_snapshot_reserved()
        .await
        .map_err(|error| readiness_error(error, "开启迁移 readiness 只读快照失败"))?;
    set_read_only_limits(&mut transaction)
        .await
        .map_err(|(stage, error)| readiness_error(error, stage))?;
    let rows = sqlx::query(
        "SELECT version, success, checksum FROM public._sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| readiness_error(error, "读取 readiness 迁移历史失败"))?;
    let assessment = assess_history(
        &mut transaction,
        &rows,
        expected_version,
        store.migration_compatibility_mode,
    )
    .await?;
    transaction
        .commit()
        .await
        .map_err(|error| readiness_error(error, "结束迁移 readiness 只读快照失败"))?;
    Ok(assessment)
}

async fn assess_history(
    transaction: &mut Transaction<'_, Postgres>,
    rows: &[PgRow],
    expected_version: i64,
    mode: MigrationCompatibilityMode,
) -> Result<Assessment, ApplicationError> {
    let versions = rows
        .iter()
        .map(|row| row.try_get::<i64, _>("version"))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| readiness_error(error, "解码 readiness 迁移版本失败"))?;
    let database_version = versions.last().copied().unwrap_or(0);
    if super::postgres_history_matches(rows, expected_version) {
        return Ok(Assessment {
            strict: true,
            compatible: true,
            branch: MigrationReadinessBranch::Current,
            database_version,
        });
    }
    if database_version < expected_version {
        return Ok(Assessment::rejected(MigrationReadinessBranch::Behind, database_version));
    }
    let prefix_len = versions.partition_point(|version| *version <= expected_version);
    if !super::postgres_history_matches(&rows[..prefix_len], expected_version)
        || database_version == expected_version
    {
        return Ok(Assessment::rejected(
            MigrationReadinessBranch::InvalidHistory,
            database_version,
        ));
    }
    if mode == MigrationCompatibilityMode::Strict {
        return Ok(Assessment::rejected(MigrationReadinessBranch::Ahead, database_version));
    }
    let compatible =
        compatible_suffix(transaction, expected_version, database_version, rows.len() - prefix_len)
            .await?;
    Ok(Assessment {
        strict: false,
        compatible,
        branch: if compatible {
            MigrationReadinessBranch::CompatibleAhead
        } else {
            MigrationReadinessBranch::IncompatibleAhead
        },
        database_version,
    })
}

async fn compatible_suffix(
    transaction: &mut Transaction<'_, Postgres>,
    expected_version: i64,
    database_version: i64,
    expected_count: usize,
) -> Result<bool, ApplicationError> {
    // 必须由实际迁移 LEFT JOIN 登记驱动；缺表报错、缺登记为 NULL，均不得变成空集放行。
    // SQLx 在迁移 SQL 之后、同事务内写入成功记录；未知摘要不能冒充旧二进制已验证的文件。
    let rows = sqlx::query(
        "SELECT migration.version, migration.success, compatibility.level
         FROM public._sqlx_migrations AS migration
         LEFT JOIN public.schema_migration_compatibility AS compatibility
           ON compatibility.version = migration.version
         WHERE migration.version > $1
         ORDER BY migration.version",
    )
    .bind(expected_version)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|error| readiness_error(error, "读取 readiness 迁移兼容登记失败"))?;
    if rows.is_empty() || rows.len() != expected_count {
        return Ok(false);
    }
    let mut previous_version = expected_version;
    for row in rows {
        let version = row
            .try_get::<i64, _>("version")
            .map_err(|error| readiness_error(error, "解码兼容迁移版本失败"))?;
        let success = row
            .try_get::<bool, _>("success")
            .map_err(|error| readiness_error(error, "解码兼容迁移状态失败"))?;
        let level = row
            .try_get::<Option<String>, _>("level")
            .map_err(|error| readiness_error(error, "解码迁移兼容等级失败"))?;
        if previous_version.checked_add(1) != Some(version)
            || !success
            || level.as_deref() != Some("compatible")
        {
            return Ok(false);
        }
        previous_version = version;
    }
    Ok(previous_version == database_version)
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::database::migrations::readiness as parent;

    #[test]
    fn saturated_pool_is_not_reported_as_repository_failure() {
        let error = readiness_error(sqlx::Error::PoolTimedOut, "readiness");
        assert_eq!(error.safe_kind(), "dependency_unavailable");
        // 连接错误仍保留仓储故障类别，避免把真实依赖故障说成暂时饱和。
        let error = readiness_error(
            sqlx::Error::Io(std::io::Error::other("connection reset")),
            "readiness",
        );
        assert_eq!(error.safe_kind(), "repository_failed");
    }
}
