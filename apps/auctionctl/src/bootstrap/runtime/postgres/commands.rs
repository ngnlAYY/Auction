//! PostgreSQL 运维命令实现和非 PostgreSQL 能力缺失分支。

use super::*;

#[cfg(feature = "postgres")]
pub(crate) async fn bootstrap_mail_credentials(world: &str, label: &str) -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    if !config.worker.mooglemail.enabled {
        anyhow::bail!("Worker 的 MoogleMail 功能未启用");
    }
    let credential_world = match world {
        "main" => AdminCredentialWorld::Main,
        "isekai" => AdminCredentialWorld::Isekai,
        _ => anyhow::bail!("bootstrap mail world 必须是 main 或 isekai"),
    };
    if !config
        .worker
        .mooglemail
        .configured_worlds()
        .iter()
        .any(|configured| configured.world_id == world)
    {
        anyhow::bail!("bootstrap mail world 必须已在 Worker 配置中启用");
    }

    let database_url =
        config.database_url().context("database_url is required for credential bootstrap")?;
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("bootstrap-mail-credentials 只支持 PostgreSQL 完整档");
    }
    let data_key_id = config
        .credential_data_key_id()
        .context("credential_data_key_id is required for credential bootstrap")?;
    let data_key_material = config
        .credential_data_key()
        .context("credential_data_key is required for credential bootstrap")?;
    let data_key = DataKey::from_base64(data_key_id, data_key_material)
        .map_err(|_| anyhow::anyhow!("credential data key is invalid"))?;
    let cookie = read_bootstrap_cookie().context("read bootstrap mail Cookie from stdin")?;
    let secret = AdminCredentialSecret::new(cookie)
        .map_err(|_| anyhow::anyhow!("bootstrap mail Cookie is invalid"))?;
    let store = Arc::new(
        DatabaseStore::connect(database_url)
            .await
            .context("connect database for credential bootstrap")?,
    );
    let sealer = Arc::new(CredentialSealer::new(Arc::new(data_key)));
    let service = AdminCredentialService::new(store, sealer);
    let credential = service
        .import(AdminCredentialImportCommand {
            command_id: Uuid::new_v4(),
            kind: AdminCredentialKind::MailCookie,
            world: credential_world,
            label: label.to_owned(),
            secret,
            actor_id: "auctionctl/bootstrap-mail-credentials".to_owned(),
            request_id: None,
            now: Utc::now(),
        })
        .await
        .context("register bootstrap mail credential")?;
    println!(
        "credential_id={} kind={} world={} label={} status={}",
        credential.id,
        credential.kind.as_str(),
        credential.world.as_str(),
        credential.label,
        credential.status.as_str(),
    );
    Ok(())
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn bootstrap_mail_credentials(_world: &str, _label: &str) -> anyhow::Result<()> {
    anyhow::bail!("bootstrap-mail-credentials 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(feature = "postgres")]
pub(crate) async fn retention_inventory(
    output: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    validate_private_evidence_path(output)?;
    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url =
        config.database_url().context("database_url is required for retention inventory")?;
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("retention-inventory 只支持 PostgreSQL 完整档");
    }
    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect database for retention inventory")?;
    let inventory = match store {
        DatabaseStore::Postgres(postgres) => postgres
            .collect_retention_inventory(confirmed_database_name)
            .await
            .context("collect PostgreSQL retention inventory")?,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => anyhow::bail!("retention-inventory 只支持 PostgreSQL 完整档"),
    };
    let evidence =
        serde_json::to_vec_pretty(&inventory).context("encode retention inventory evidence")?;
    write_private_evidence(output, &evidence)?;
    println!(
        "retention-inventory written path={} database={} relations={} policy_tables={} policy_sha256={} migration_current={}",
        output.display(),
        inventory.database_name,
        inventory.relation_count,
        inventory.policy_table_count,
        inventory.policy_sha256,
        inventory.migration.current,
    );
    Ok(())
}

#[cfg(feature = "postgres")]
pub(crate) async fn recovery_preflight(
    output: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    validate_private_evidence_path(output)?;
    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url =
        config.database_url().context("database_url is required for recovery preflight")?;
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("recovery-preflight 只支持 PostgreSQL 完整档");
    }
    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect database for recovery preflight")?;
    let preflight = match store {
        DatabaseStore::Postgres(postgres) => postgres
            .collect_recovery_preflight(confirmed_database_name)
            .await
            .context("collect PostgreSQL recovery preflight")?,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => anyhow::bail!("recovery-preflight 只支持 PostgreSQL 完整档"),
    };
    let evidence =
        serde_json::to_vec_pretty(&preflight).context("encode recovery preflight evidence")?;
    write_private_evidence(output, &evidence)?;
    println!(
        "recovery-preflight written path={} database={} passed={} active_sessions={} user_objects={}",
        output.display(),
        preflight.database_name,
        preflight.restore_preflight_passed,
        preflight.active_other_sessions,
        preflight.user_object_count,
    );
    Ok(())
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn recovery_preflight(
    _output: &Path,
    _confirmed_database_name: &str,
) -> anyhow::Result<()> {
    anyhow::bail!("recovery-preflight 需要 PostgreSQL 完整档；请使用 --features postgres")
}

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

#[cfg(not(feature = "postgres"))]
pub(crate) async fn retention_inventory(
    _output: &Path,
    _confirmed_database_name: &str,
) -> anyhow::Result<()> {
    anyhow::bail!("retention-inventory 需要 PostgreSQL 完整档；请使用 --features postgres")
}
