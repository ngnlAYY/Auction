//! PostgreSQL 备份来源 schema 与迁移状态的安全预检。

pub(crate) fn require_backup_source(
    preflight: &auction_storage::PostgresRecoveryPreflight,
) -> anyhow::Result<i64> {
    let _identity = preflight
        .schema_identity
        .as_ref()
        .filter(|identity| identity.application == "auction" && identity.baseline_version > 0)
        .ok_or_else(|| {
            anyhow::anyhow!("backup source does not have a valid Auction schema identity")
        })?;
    let migration = preflight
        .migration
        .as_ref()
        .filter(|migration| {
            migration.all_success && migration.current && migration.latest_version > 0
        })
        .ok_or_else(|| {
            anyhow::anyhow!("backup source migration history is not current and clean")
        })?;
    Ok(migration.latest_version)
}
