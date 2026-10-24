//! readiness 与灾备共用的完整迁移身份核验；旧备份只允许已知的完整前缀。

use std::sync::LazyLock;

use sqlx::{Row, migrate::Migrator, postgres::PgRow};

// 仅缓存二进制内嵌的执行视图，不缓存数据库 history；每次 readiness 仍核对当前持久事实。
// 构造失败只可能来自固定迁移源码不变量，保留失败，不能退回未经校验的迁移集。
static EXPECTED_MIGRATIONS: LazyLock<Option<Migrator>> =
    LazyLock::new(|| super::postgres_migrator().ok());

/// 核对完整前缀的版本、成功状态及兼容 checksum；未知记录失败关闭。
pub(in crate::database) fn postgres_history_matches(rows: &[PgRow], through_version: i64) -> bool {
    let Some(migrator) = EXPECTED_MIGRATIONS.as_ref() else {
        return false;
    };
    let expected = &migrator.migrations
        [..migrator.migrations.partition_point(|migration| migration.version <= through_version)];
    if expected.last().is_none_or(|migration| migration.version != through_version)
        || rows.len() != expected.len()
    {
        return false;
    }
    rows.iter().zip(expected).all(|(row, migration)| {
        row.try_get::<i64, _>("version").is_ok_and(|version| version == migration.version)
            && row.try_get::<bool, _>("success").is_ok_and(|success| success)
            && row
                .try_get::<&[u8], _>("checksum")
                .is_ok_and(|checksum| super::postgres_checksum_matches(migration, checksum))
    })
}
