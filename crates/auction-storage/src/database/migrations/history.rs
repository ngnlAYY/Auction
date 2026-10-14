//! readiness 与灾备共用的完整迁移身份核验；旧备份只允许已知的完整前缀。

use sqlx::{Row, postgres::PgRow};

/// 核对完整前缀的版本、成功状态及兼容 checksum；未知记录失败关闭。
pub(in crate::database) fn postgres_history_matches(rows: &[PgRow], through_version: i64) -> bool {
    let Ok(migrator) = super::postgres_migrator() else {
        return false;
    };
    let expected = migrator
        .iter()
        .take_while(|migration| migration.version <= through_version)
        .collect::<Vec<_>>();
    if expected.last().is_none_or(|migration| migration.version != through_version)
        || rows.len() != expected.len()
    {
        return false;
    }
    rows.iter().zip(expected).all(|(row, migration)| {
        row.try_get::<i64, _>("version").is_ok_and(|version| version == migration.version)
            && row.try_get::<bool, _>("success").is_ok_and(|success| success)
            && row
                .try_get::<Vec<u8>, _>("checksum")
                .is_ok_and(|checksum| super::postgres_checksum_matches(migration, &checksum))
    })
}
