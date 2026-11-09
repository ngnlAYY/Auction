//! PostgreSQL 历史迁移的不可变 checksum 与受限执行兼容层。

use std::borrow::Cow;

use sha2::{Digest, Sha384};
use sqlx::{
    AssertSqlSafe, SqlSafeStr,
    migrate::{Migration, Migrator},
};

use super::{DatabaseBackend, StorageError};

mod history;
mod readiness;
pub(super) use history::postgres_history_matches;
pub(super) use readiness::check_postgres_readiness;

static POSTGRES_MIGRATOR: Migrator = sqlx::migrate!("../../migrations/postgres");

const MIGRATION_FIVE_DESCRIPTION: &str = "recovery observability forum";
const MIGRATION_FIFTEEN_DESCRIPTION: &str = "operations alert state invariants";
const MIGRATION_FORTY_FOUR_DESCRIPTION: &str = "user timestamp order";
const MIGRATION_FORTY_SEVEN_DESCRIPTION: &str = "settlement funding";

/// 返回 PostgreSQL 执行视图：保留历史文件 checksum，仅应用固定版本的执行兼容。
/// `000005`/`000015` 修正未加引号的保留字别名；`000044` 在改 users 前执行已排队的延迟身份约束，
/// 避免同事务修复用户后 ALTER TABLE 被待处理事件拒绝，失败仍回滚整个迁移；`000047` 仅为保留字
/// 列名 authorization 加双引号。替换限定描述、原片段及出现次数，已发布 SQL 文件、列名及迁移
/// 事务边界不变，checksum 默认保持原文件值。
pub(super) fn postgres_migrator() -> Result<Migrator, StorageError> {
    postgres_migrator_with_applied_checksums(&[])
}

/// 为已经运行过早期别名修正版的数据库选择已记录 checksum，不改写 migration history；兼容值只允许
/// 等于历史原文件或固定替换后 SQL 的 SHA-384，其它差异仍交 SQLx 以 `VersionMismatch` 失败关闭。
pub(super) fn postgres_migrator_with_applied_checksums(
    applied_checksums: &[(i64, Vec<u8>)],
) -> Result<Migrator, StorageError> {
    let mut migrations = POSTGRES_MIGRATOR.iter().cloned().collect::<Vec<_>>();
    let mut patched_five = false;
    let mut patched_fifteen = false;
    let mut patched_forty_four = false;
    let mut patched_forty_seven = false;

    for migration in &mut migrations {
        match migration.version {
            5 => {
                patch_migration(
                    migration,
                    MIGRATION_FIVE_DESCRIPTION,
                    &[
                        ("authorization.", "proxy_authorization.", 17),
                        ("AS authorization", "AS proxy_authorization", 1),
                    ],
                )?;
                patched_five = true;
            }
            15 => {
                patch_migration(
                    migration,
                    MIGRATION_FIFTEEN_DESCRIPTION,
                    &[
                        ("constraint.", "catalog_constraint.", 19),
                        ("AS constraint", "AS catalog_constraint", 3),
                    ],
                )?;
                patched_fifteen = true;
            }
            44 => {
                patch_migration(
                    migration,
                    MIGRATION_FORTY_FOUR_DESCRIPTION,
                    &[(
                        "ALTER TABLE public.users\n",
                        "SET CONSTRAINTS public.user_forum_identity_account_id_check IMMEDIATE;\n\nALTER TABLE public.users\n",
                        1,
                    )],
                )?;
                patched_forty_four = true;
            }
            47 => {
                patch_migration(
                    migration,
                    MIGRATION_FORTY_SEVEN_DESCRIPTION,
                    &[
                        ("authorization jsonb", "\"authorization\" jsonb", 1),
                        ("jsonb_typeof(authorization)", "jsonb_typeof(\"authorization\")", 1),
                    ],
                )?;
                patched_forty_seven = true;
            }
            _ => {}
        }
    }

    if !patched_five || !patched_fifteen || !patched_forty_four || !patched_forty_seven {
        return Err(source_invariant("required PostgreSQL compatibility migration is missing"));
    }

    for migration in &mut migrations {
        let Some((_, applied_checksum)) =
            applied_checksums.iter().find(|(version, _)| *version == migration.version)
        else {
            continue;
        };
        if migration.checksum.as_ref() != applied_checksum.as_slice()
            && alternate_checksum_matches(migration, applied_checksum)
        {
            migration.checksum = Cow::Owned(applied_checksum.clone());
        }
    }

    Ok(Migrator {
        migrations: Cow::Owned(migrations),
        ignore_missing: POSTGRES_MIGRATOR.ignore_missing,
        locking: POSTGRES_MIGRATOR.locking,
        no_tx: POSTGRES_MIGRATOR.no_tx,
        table_name: POSTGRES_MIGRATOR.table_name.clone(),
        create_schemas: POSTGRES_MIGRATOR.create_schemas.clone(),
    })
}

/// 兼容声明验证专用：只把测试库迁移到指定历史版本（含）。
/// 与正式 `migrate` 共用同一执行视图（历史别名修正与 checksum 语义），目标之后不执行，
/// 以便重建“上一版程序面对的 schema”。生产装配不调用该入口。
#[cfg(feature = "test-db")]
pub async fn postgres_migrate_to(pool: &sqlx::PgPool, version: i64) -> Result<(), StorageError> {
    let migrator = postgres_migrator()?;
    let mut connection = pool.acquire().await.map_err(|source| StorageError::Migration {
        backend: DatabaseBackend::Postgres,
        source: source.into(),
    })?;
    // 与正式迁移路径一致：会话可能仍持有 advisory lock，不能带着锁归池。
    connection.close_on_drop();
    migrator
        .run_to(version, &mut *connection)
        .await
        .map_err(|source| StorageError::Migration { backend: DatabaseBackend::Postgres, source })
}

/// Readiness 同时接受历史原文件和已知别名修正版，不接受任意其它 checksum。
pub(super) fn postgres_checksum_matches(migration: &Migration, applied_checksum: &[u8]) -> bool {
    migration.checksum.as_ref() == applied_checksum
        || alternate_checksum_matches(migration, applied_checksum)
}

fn alternate_checksum_matches(migration: &Migration, applied_checksum: &[u8]) -> bool {
    // 仅兼容曾登记的两个别名修正版；000044/000047 始终只接受原文件 checksum。
    matches!(migration.version, 5 | 15)
        && Sha384::digest(migration.sql.as_str().as_bytes()).as_slice() == applied_checksum
}

fn patch_migration(
    migration: &mut Migration,
    expected_description: &str,
    replacements: &[(&str, &str, usize)],
) -> Result<(), StorageError> {
    if migration.description.as_ref() != expected_description {
        return Err(source_invariant("PostgreSQL compatibility migration description changed"));
    }

    let mut sql = migration.sql.as_str().to_owned();
    for &(source, replacement, expected_count) in replacements {
        if sql.matches(source).count() != expected_count {
            return Err(source_invariant("PostgreSQL compatibility migration SQL changed"));
        }
        sql = sql.replace(source, replacement);
    }
    migration.sql = AssertSqlSafe(sql).into_sql_str();
    Ok(())
}

const fn source_invariant(detail: &'static str) -> StorageError {
    StorageError::MigrationSourceInvariant { backend: DatabaseBackend::Postgres, detail }
}
