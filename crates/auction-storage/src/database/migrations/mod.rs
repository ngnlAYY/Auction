//! PostgreSQL 历史迁移的不可变 checksum 与当前解析器兼容层。

use std::borrow::Cow;

use sha2::{Digest, Sha384};
use sqlx::{
    AssertSqlSafe, SqlSafeStr,
    migrate::{Migration, Migrator},
};

use super::{DatabaseBackend, StorageError};

mod history;
pub(super) use history::postgres_history_matches;

static POSTGRES_MIGRATOR: Migrator = sqlx::migrate!("../../migrations/postgres");

const MIGRATION_FIVE_DESCRIPTION: &str = "recovery observability forum";
const MIGRATION_FIFTEEN_DESCRIPTION: &str = "operations alert state invariants";

/// 返回 PostgreSQL 执行视图：保留历史文件 checksum，只替换 PostgreSQL 保留字别名。
///
/// `000005` 和 `000015` 已经进入 SQLx 历史，文件字节不能改变；但其中两个未加引号的别名
/// 会被当前 PostgreSQL 当作保留字拒绝。这里仅对编译期内嵌的固定 SQL 做逐字、计数受限替换，
/// `Migration::checksum` 默认保持原文件值，使未应用迁移的数据库继续写入不可变身份。
pub(super) fn postgres_migrator() -> Result<Migrator, StorageError> {
    postgres_migrator_with_applied_checksums(&[])
}

/// 为已经运行过早期别名修正版的数据库选择已记录 checksum，不改写 migration history。
///
/// 兼容值只允许等于历史原文件或本模块固定替换后 SQL 的 SHA-384；其它差异仍交给 SQLx
/// 以 `VersionMismatch` 失败关闭。
pub(super) fn postgres_migrator_with_applied_checksums(
    applied_checksums: &[(i64, Vec<u8>)],
) -> Result<Migrator, StorageError> {
    let mut migrations = POSTGRES_MIGRATOR.iter().cloned().collect::<Vec<_>>();
    let mut patched_five = false;
    let mut patched_fifteen = false;

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
            _ => {}
        }
    }

    if !patched_five || !patched_fifteen {
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

/// Readiness 同时接受历史原文件和已知别名修正版，不接受任意其它 checksum。
pub(super) fn postgres_checksum_matches(migration: &Migration, applied_checksum: &[u8]) -> bool {
    migration.checksum.as_ref() == applied_checksum
        || alternate_checksum_matches(migration, applied_checksum)
}

fn alternate_checksum_matches(migration: &Migration, applied_checksum: &[u8]) -> bool {
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
