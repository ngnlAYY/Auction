//! PostgreSQL 备份归档、manifest 和完整性校验命令。

#[cfg(feature = "postgres")]
use super::super::*;

#[cfg(feature = "postgres")]
pub(crate) const POSTGRES_BACKUP_FORMAT: &str = "auction-postgres-backup.v1";

#[cfg(feature = "postgres")]
pub(crate) const MAX_BACKUP_MANIFEST_BYTES: u64 = 64 * 1024;

#[cfg(feature = "postgres")]
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct PostgresBackupManifest {
    pub(crate) format: String,
    pub(crate) archive_file_name: String,
    pub(crate) database_name_sha256: String,
    pub(crate) archive_sha256: String,
    pub(crate) schema_version: i64,
    pub(crate) created_at: DateTime<Utc>,
}

#[cfg(feature = "postgres")]
pub(crate) async fn postgres_backup(
    output: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    validate_private_evidence_path(output)?;
    require_dump_extension(output)?;
    let manifest_path = backup_manifest_path(output)?;
    validate_private_evidence_path(&manifest_path)?;
    require_absent_artifact(output, "backup archive")?;
    require_absent_artifact(&manifest_path, "backup manifest")?;

    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url =
        config.database_url().context("database_url is required for PostgreSQL backup")?;
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("postgres-backup 只支持 PostgreSQL 完整档");
    }
    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect database for PostgreSQL backup")?;
    let postgres = match store {
        DatabaseStore::Postgres(postgres) => postgres,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => anyhow::bail!("postgres-backup 只支持 PostgreSQL 完整档"),
    };
    let before = postgres
        .collect_recovery_preflight(confirmed_database_name)
        .await
        .context("inspect backup source before pg_dump")?;
    let schema_version = require_backup_source(&before)?;
    let command_environment =
        PostgresCommandEnvironment::from_database_url(database_url, confirmed_database_name)?;
    let temporary_archive = create_private_temp_path(output, "auction-postgres-backup")?;
    let backup_result = async {
        run_pg_dump(&command_environment, &temporary_archive)?;
        verify_archive(&temporary_archive)?;
        let after = postgres
            .collect_recovery_preflight(confirmed_database_name)
            .await
            .context("inspect backup source after pg_dump")?;
        let after_schema_version = require_backup_source(&after)?;
        if after_schema_version != schema_version {
            anyhow::bail!(
                "database migration version changed during backup: before={schema_version} after={after_schema_version}"
            );
        }

        publish_private_artifact(&temporary_archive, output)?;
        let manifest = PostgresBackupManifest {
            format: POSTGRES_BACKUP_FORMAT.to_owned(),
            archive_file_name: output
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| anyhow::anyhow!("backup archive filename is not valid UTF-8"))?
                .to_owned(),
            database_name_sha256: sha256_text(confirmed_database_name),
            archive_sha256: sha256_file(output)?,
            schema_version,
            created_at: Utc::now(),
        };
        let manifest_bytes =
            serde_json::to_vec_pretty(&manifest).context("encode PostgreSQL backup manifest")?;
        write_private_evidence(&manifest_path, &manifest_bytes)?;
        Ok::<PostgresBackupManifest, anyhow::Error>(manifest)
    }
    .await;
    if backup_result.is_err() {
        remove_owned_temp(&temporary_archive);
    }
    let manifest = backup_result?;
    println!(
        "postgres-backup written archive={} manifest={} schema_version={} archive_sha256={}",
        output.display(),
        manifest_path.display(),
        manifest.schema_version,
        manifest.archive_sha256,
    );
    Ok(())
}

#[cfg(feature = "postgres")]
pub(crate) async fn postgres_backup_verify(
    manifest_path: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    validate_private_evidence_path(manifest_path)?;
    let manifest = read_backup_manifest(manifest_path)?;
    validate_backup_manifest(&manifest, confirmed_database_name)?;
    let archive_path = manifest_archive_path(manifest_path, &manifest)?;
    validate_private_evidence_path(&archive_path)?;
    verify_private_regular_file(&archive_path, "backup archive")?;
    let actual_sha256 = sha256_file(&archive_path)?;
    if actual_sha256 != manifest.archive_sha256 {
        anyhow::bail!("backup archive SHA-256 does not match its manifest");
    }
    verify_archive(&archive_path)?;
    println!(
        "postgres-backup-verify passed archive={} manifest={} schema_version={} archive_sha256={}",
        archive_path.display(),
        manifest_path.display(),
        manifest.schema_version,
        manifest.archive_sha256,
    );
    Ok(())
}
