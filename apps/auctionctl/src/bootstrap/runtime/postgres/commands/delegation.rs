//! PostgreSQL restore、legacy v28 和 rebuild 专用 owner 的命令桥接。

use super::super::*;

#[cfg(feature = "postgres")]
pub(crate) async fn execute_postgres_restore(
    manifest_path: &Path,
    receipt_path: Option<&Path>,
    confirmed_database_name: &str,
    dry_run: bool,
) -> anyhow::Result<()> {
    postgres_restore::execute(manifest_path, receipt_path, confirmed_database_name, dry_run).await
}

#[cfg(feature = "postgres")]
pub(crate) async fn execute_postgres_legacy_v28_backup(
    output: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    postgres_legacy_v28::backup(output, confirmed_database_name).await
}

#[cfg(feature = "postgres")]
pub(crate) fn verify_postgres_legacy_v28_backup(
    manifest_path: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    postgres_legacy_v28::verify_backup_cli(manifest_path, confirmed_database_name)
}

#[cfg(feature = "postgres")]
pub(crate) async fn execute_postgres_legacy_v28_adoption(
    confirmed_database_name: &str,
    backup_manifest: Option<&Path>,
    execute: bool,
    confirm_adoption: Option<&str>,
    receipt: Option<&Path>,
) -> anyhow::Result<()> {
    postgres_legacy_v28::adopt(
        confirmed_database_name,
        backup_manifest,
        execute,
        confirm_adoption,
        receipt,
    )
    .await
}

#[cfg(feature = "postgres")]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_postgres_rebuild(
    confirmed_database_name: &str,
    mode: PostgresRebuildModeArg,
    backup_manifest: Option<&Path>,
    allow_unrecoverable_ephemeral: bool,
    confirm_ephemeral: Option<&str>,
    maintenance_secrets_file: Option<&Path>,
    execute: bool,
    confirm_rebuild: Option<&str>,
    receipt: Option<&Path>,
) -> anyhow::Result<()> {
    postgres_rebuild::execute(postgres_rebuild::Options {
        confirmed_database_name,
        mode: match mode {
            PostgresRebuildModeArg::EmptyForRestore => postgres_rebuild::Mode::EmptyForRestore,
            PostgresRebuildModeArg::FreshSchema => postgres_rebuild::Mode::FreshSchema,
        },
        backup_manifest,
        allow_unrecoverable_ephemeral,
        confirm_ephemeral,
        maintenance_secrets_file,
        execute,
        confirm_rebuild,
        receipt,
    })
    .await
}

#[cfg(not(feature = "postgres"))]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_postgres_rebuild(
    _confirmed_database_name: &str,
    _mode: PostgresRebuildModeArg,
    _backup_manifest: Option<&Path>,
    _allow_unrecoverable_ephemeral: bool,
    _confirm_ephemeral: Option<&str>,
    _maintenance_secrets_file: Option<&Path>,
    _execute: bool,
    _confirm_rebuild: Option<&str>,
    _receipt: Option<&Path>,
) -> anyhow::Result<()> {
    anyhow::bail!("postgres-rebuild 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn execute_postgres_restore(
    _manifest_path: &Path,
    _receipt_path: Option<&Path>,
    _confirmed_database_name: &str,
    _dry_run: bool,
) -> anyhow::Result<()> {
    anyhow::bail!("postgres-restore 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn execute_postgres_legacy_v28_backup(
    _output: &Path,
    _confirmed_database_name: &str,
) -> anyhow::Result<()> {
    anyhow::bail!("postgres-legacy-v28-backup 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(not(feature = "postgres"))]
pub(crate) fn verify_postgres_legacy_v28_backup(
    _manifest_path: &Path,
    _confirmed_database_name: &str,
) -> anyhow::Result<()> {
    anyhow::bail!(
        "postgres-legacy-v28-backup-verify 需要 PostgreSQL 完整档；请使用 --features postgres"
    )
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn execute_postgres_legacy_v28_adoption(
    _confirmed_database_name: &str,
    _backup_manifest: Option<&Path>,
    _execute: bool,
    _confirm_adoption: Option<&str>,
    _receipt: Option<&Path>,
) -> anyhow::Result<()> {
    anyhow::bail!("postgres-adopt-legacy-v28 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn postgres_backup(
    _output: &Path,
    _confirmed_database_name: &str,
) -> anyhow::Result<()> {
    anyhow::bail!("postgres-backup 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn postgres_backup_verify(
    _manifest_path: &Path,
    _confirmed_database_name: &str,
) -> anyhow::Result<()> {
    anyhow::bail!("postgres-backup-verify 需要 PostgreSQL 完整档；请使用 --features postgres")
}
