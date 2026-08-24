//! PostgreSQL v28 专用备份生成与源库稳定性检查。

use std::path::Path;

use anyhow::Context;
use auction_config::{AppConfig, Role};
use auction_storage::{DatabaseStore, LEGACY_V28_MIGRATION_VERSION};
use chrono::Utc;

use super::super::{
    PostgresCommandEnvironment, backup_manifest_path, create_private_temp_path,
    publish_private_artifact, remove_owned_temp, require_absent_artifact, require_dump_extension,
    run_pg_dump, sha256_file, sha256_text, validate_private_evidence_path, verify_archive,
    write_private_evidence,
};
use super::common::{preflight_sha256, require_adoption_ready, require_postgres_backend};
use super::model::{LEGACY_V28_BACKUP_FORMAT, LegacyV28BackupManifest};

/// 为严格通过预检的 canonical Auction-BF v28 创建专用备份。
pub(in crate::bootstrap) async fn backup(
    output: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    validate_private_evidence_path(output)?;
    require_dump_extension(output)?;
    let manifest_path = backup_manifest_path(output)?;
    validate_private_evidence_path(&manifest_path)?;
    require_absent_artifact(output, "legacy v28 backup archive")?;
    require_absent_artifact(&manifest_path, "legacy v28 backup manifest")?;

    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url =
        config.database_url().context("database_url is required for legacy v28 backup")?;
    require_postgres_backend(database_url)?;
    let command_environment =
        PostgresCommandEnvironment::from_database_url(database_url, confirmed_database_name)?;
    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect database for legacy v28 backup")?;
    let postgres = match store {
        DatabaseStore::Postgres(postgres) => postgres,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => {
            anyhow::bail!("postgres-legacy-v28-backup 只支持 PostgreSQL 完整档")
        }
    };

    let before = postgres
        .collect_legacy_v28_preflight(confirmed_database_name)
        .await
        .context("inspect legacy v28 source before pg_dump")?;
    require_adoption_ready(&before)?;
    let source_preflight_sha256 = preflight_sha256(&before)?;
    let temporary_archive = create_private_temp_path(output, "auction-postgres-legacy-v28-backup")?;
    let backup_result = async {
        run_pg_dump(&command_environment, &temporary_archive)?;
        verify_archive(&temporary_archive)?;

        let after = postgres
            .collect_legacy_v28_preflight(confirmed_database_name)
            .await
            .context("inspect legacy v28 source after pg_dump")?;
        require_adoption_ready(&after)?;
        let after_preflight_sha256 = preflight_sha256(&after)?;
        if after_preflight_sha256 != source_preflight_sha256 {
            anyhow::bail!(
                "legacy v28 source shape changed during pg_dump; discard this backup attempt"
            );
        }

        publish_private_artifact(&temporary_archive, output)?;
        let archive_file_name = output
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("legacy backup archive filename is not valid UTF-8"))?
            .to_owned();
        let manifest = LegacyV28BackupManifest {
            format: LEGACY_V28_BACKUP_FORMAT.to_owned(),
            archive_file_name,
            database_name_sha256: sha256_text(confirmed_database_name),
            archive_sha256: sha256_file(output)?,
            legacy_migration_version: LEGACY_V28_MIGRATION_VERSION,
            created_at: Utc::now(),
            preflight_sha256: source_preflight_sha256,
        };
        let manifest_bytes =
            serde_json::to_vec_pretty(&manifest).context("encode legacy v28 backup manifest")?;
        write_private_evidence(&manifest_path, &manifest_bytes)?;
        Ok::<LegacyV28BackupManifest, anyhow::Error>(manifest)
    }
    .await;
    postgres.close().await;
    if backup_result.is_err() {
        remove_owned_temp(&temporary_archive);
    }
    let manifest = backup_result?;

    println!(
        "postgres-legacy-v28-backup written archive={} manifest={} legacy_version={} archive_sha256={} preflight_sha256={}",
        output.display(),
        manifest_path.display(),
        manifest.legacy_migration_version,
        manifest.archive_sha256,
        manifest.preflight_sha256,
    );
    Ok(())
}
