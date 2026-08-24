//! PostgreSQL rebuild 的执行流程。

use anyhow::Context;
use auction_config::{AppConfig, Role, load_postgres_maintenance_secrets};
use auction_storage::{DatabaseBackend, DatabaseStore, PostgresDatabaseRebuildGuard};

use super::{
    model::{Mode, Options, ReceiptContext},
    postconditions::{migrate_and_verify, require_rebuild_source, verify_empty_target},
    receipt::{RebuildReceiptReservation, publish_postcondition_failure, publish_receipt},
    validation::{
        load_authorization, resolve_maintenance_database_url, validate_execution_options,
    },
};

/// 执行只读演练，或在全部显式授权成立后执行破坏性重建。
pub async fn execute(options: Options<'_>) -> anyhow::Result<()> {
    validate_execution_options(&options)?;
    let authorization = load_authorization(&options)?;
    let mut receipt_reservation = if options.execute {
        Some(RebuildReceiptReservation::reserve(
            options
                .receipt
                .ok_or_else(|| anyhow::anyhow!("实际 postgres-rebuild 必须提供 --receipt"))?,
        )?)
    } else {
        None
    };

    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url =
        config.database_url().context("database_url is required for PostgreSQL rebuild")?;
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("postgres-rebuild 只支持 PostgreSQL 完整档");
    }
    let maintenance_secrets = options
        .maintenance_secrets_file
        .map(load_postgres_maintenance_secrets)
        .transpose()
        .context("load PostgreSQL maintenance sensitive TOML")?;
    let maintenance_database_url = resolve_maintenance_database_url(
        database_url,
        maintenance_secrets.as_ref().map(|secrets| secrets.database_url()),
        options.confirmed_database_name,
    )?;

    let source_store = DatabaseStore::connect(database_url)
        .await
        .context("connect target database for rebuild source preflight")?;
    let source_postgres = match source_store {
        DatabaseStore::Postgres(postgres) => postgres,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => anyhow::bail!("postgres-rebuild 只支持 PostgreSQL 完整档"),
    };
    let source_preflight = source_postgres
        .collect_recovery_preflight(options.confirmed_database_name)
        .await
        .context("collect PostgreSQL rebuild source preflight")?;
    let source_schema_version = require_rebuild_source(&source_preflight)?;
    // 维护库预检必须观察到零目标连接，所以先显式释放本命令自己的源库连接。
    source_postgres.close().await;

    let mut guard = PostgresDatabaseRebuildGuard::connect(
        &maintenance_database_url,
        options.confirmed_database_name,
    )
    .await
    .context("collect locked PostgreSQL rebuild preflight")?;
    let maintenance_preflight = guard.preflight().clone();

    if !options.execute {
        println!(
            "postgres-rebuild dry-run passed database={} mode={} schema_version={} owner={} active_sessions={} backup_evidence={} rebuild_permitted={}",
            options.confirmed_database_name,
            options.mode.name(),
            source_schema_version,
            maintenance_preflight.target_owner,
            maintenance_preflight.active_target_sessions,
            authorization.receipt().kind,
            maintenance_preflight.rebuild_permitted,
        );
        return Ok(());
    }

    // 已持有集群 advisory lock 后再次读取 archive/manifest，拒绝预检期间发生的替换。
    authorization.reverify(options.confirmed_database_name)?;
    let rebuild_result = match guard.recreate_empty().await {
        Ok(result) => result,
        Err(error) => {
            if let Some((stage, database_dropped, database_recreated)) = error.mutation_state()
                && database_dropped
            {
                publish_receipt(
                    &mut receipt_reservation,
                    ReceiptContext {
                        options: &options,
                        authorization: authorization.receipt(),
                        source_schema_version,
                        preflight: &maintenance_preflight,
                        rebuild: None,
                        status: "mutation_failed",
                        failure_stage: Some(stage),
                        database_dropped,
                        database_recreated,
                        postcondition: None,
                        migration_version: None,
                    },
                )
                .context(
                    "database changed but publishing PostgreSQL rebuild failure receipt failed",
                )?;
            }
            return Err(error).context("recreate empty PostgreSQL database");
        }
    };

    let postcondition = match verify_empty_target(database_url, options.confirmed_database_name)
        .await
    {
        Ok(()) if matches!(options.mode, Mode::EmptyForRestore) => "empty_for_restore",
        Ok(()) => match migrate_and_verify(&config, options.confirmed_database_name).await {
            Ok(version) => {
                publish_receipt(
                    &mut receipt_reservation,
                    ReceiptContext {
                        options: &options,
                        authorization: authorization.receipt(),
                        source_schema_version,
                        preflight: &maintenance_preflight,
                        rebuild: Some(&rebuild_result),
                        status: "completed",
                        failure_stage: None,
                        database_dropped: true,
                        database_recreated: true,
                        postcondition: Some("fresh_schema"),
                        migration_version: Some(version),
                    },
                )?;
                println!(
                    "postgres-rebuild completed database={} mode={} previous_oid={} new_oid={} migration_version={} receipt={}",
                    options.confirmed_database_name,
                    options.mode.name(),
                    rebuild_result.previous_database_oid,
                    rebuild_result.new_database_oid,
                    version,
                    options
                        .receipt
                        .map_or_else(|| "<missing>".into(), |path| path.display().to_string()),
                );
                return Ok(());
            }
            Err(error) => {
                publish_postcondition_failure(
                    &mut receipt_reservation,
                    &options,
                    authorization.receipt(),
                    source_schema_version,
                    &maintenance_preflight,
                    &rebuild_result,
                    "migrate_fresh_schema",
                )?;
                return Err(error).context(
                    "database was recreated but fresh schema migration failed; keep all roles stopped",
                );
            }
        },
        Err(error) => {
            publish_postcondition_failure(
                &mut receipt_reservation,
                &options,
                authorization.receipt(),
                source_schema_version,
                &maintenance_preflight,
                &rebuild_result,
                "verify_empty_database",
            )?;
            return Err(error).context(
                "database was recreated but empty target verification failed; keep all roles stopped",
            );
        }
    };

    publish_receipt(
        &mut receipt_reservation,
        ReceiptContext {
            options: &options,
            authorization: authorization.receipt(),
            source_schema_version,
            preflight: &maintenance_preflight,
            rebuild: Some(&rebuild_result),
            status: "completed",
            failure_stage: None,
            database_dropped: true,
            database_recreated: true,
            postcondition: Some(postcondition),
            migration_version: None,
        },
    )?;
    println!(
        "postgres-rebuild completed database={} mode={} previous_oid={} new_oid={} receipt={}",
        options.confirmed_database_name,
        options.mode.name(),
        rebuild_result.previous_database_oid,
        rebuild_result.new_database_oid,
        options.receipt.map_or_else(|| "<missing>".into(), |path| path.display().to_string()),
    );
    Ok(())
}
