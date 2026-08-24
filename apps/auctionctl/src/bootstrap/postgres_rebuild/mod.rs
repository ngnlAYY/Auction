//! PostgreSQL 数据库破坏性重建编排。
//!
//! 命令默认只读；实际执行必须具备精确数据库确认、第二确认短语、可验证 backup 或一次性环境
//! 豁免，以及预先可写的不可覆盖回执。目标必须是当前、干净且可识别的 Auction schema，全部
//! 外部连接必须先停止。删库后若建库或后置验证失败，会尽可能发布带破坏性状态的失败回执。

mod receipt;

use std::{collections::HashSet, path::Path};

use anyhow::Context;
use auction_config::{AppConfig, Role, load_postgres_maintenance_secrets};
use auction_storage::{
    DatabaseBackend, DatabaseStore, PostgresDatabaseRebuildGuard, PostgresRebuildPreflight,
    PostgresRebuildResult, postgres_expected_migration_version,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use url::Url;

use self::receipt::RebuildReceiptReservation;
use super::{decode_uri_component, require_backup_source, require_pg_value, sha256_text};

const POSTGRES_REBUILD_RECEIPT_FORMAT: &str = "auction-postgres-rebuild.v1";

#[derive(Debug, Clone, Copy)]
pub(super) enum Mode {
    EmptyForRestore,
    FreshSchema,
}

impl Mode {
    const fn name(self) -> &'static str {
        match self {
            Self::EmptyForRestore => "empty_for_restore",
            Self::FreshSchema => "fresh_schema",
        }
    }
}

pub(super) struct Options<'a> {
    pub(super) confirmed_database_name: &'a str,
    pub(super) mode: Mode,
    pub(super) backup_manifest: Option<&'a Path>,
    pub(super) allow_unrecoverable_ephemeral: bool,
    pub(super) confirm_ephemeral: Option<&'a str>,
    pub(super) maintenance_secrets_file: Option<&'a Path>,
    pub(super) execute: bool,
    pub(super) confirm_rebuild: Option<&'a str>,
    pub(super) receipt: Option<&'a Path>,
}

#[derive(Debug, Clone, Serialize)]
struct AuthorizationReceipt {
    kind: &'static str,
    manifest_sha256: Option<String>,
    archive_file_name: Option<String>,
    archive_sha256: Option<String>,
    backup_schema_version: Option<i64>,
    backup_created_at: Option<DateTime<Utc>>,
    unrecoverable_ephemeral: bool,
}

enum Authorization<'a> {
    Backup {
        manifest_path: &'a Path,
        verified: super::postgres_restore::VerifiedBackup,
        receipt: AuthorizationReceipt,
    },
    Ephemeral {
        receipt: AuthorizationReceipt,
    },
}

impl Authorization<'_> {
    fn receipt(&self) -> &AuthorizationReceipt {
        match self {
            Self::Backup { receipt, .. } | Self::Ephemeral { receipt } => receipt,
        }
    }

    fn reverify(&self, confirmed_database_name: &str) -> anyhow::Result<()> {
        let Self::Backup { manifest_path, verified, .. } = self else {
            return Ok(());
        };
        let current =
            super::postgres_restore::verify_backup_pair(manifest_path, confirmed_database_name)?;
        if current.manifest_sha256 != verified.manifest_sha256
            || current.archive_path != verified.archive_path
            || current.manifest.archive_sha256 != verified.manifest.archive_sha256
            || current.manifest.schema_version != verified.manifest.schema_version
            || current.manifest.created_at != verified.manifest.created_at
        {
            anyhow::bail!("backup archive or manifest changed during PostgreSQL rebuild preflight");
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct PostgresRebuildReceipt<'a> {
    format: &'static str,
    recorded_at: DateTime<Utc>,
    status: &'static str,
    failure_stage: Option<&'static str>,
    database_name: &'a str,
    database_name_sha256: String,
    mode: &'static str,
    authorization: &'a AuthorizationReceipt,
    source_schema_version: i64,
    previous_database_oid: i64,
    new_database_oid: Option<i64>,
    database_owner: &'a str,
    locale_provider: &'a str,
    tablespace: &'a str,
    connection_limit: i32,
    template_database: Option<&'static str>,
    forced_drop: bool,
    database_dropped: bool,
    database_recreated: bool,
    postcondition: Option<&'static str>,
    migration_version: Option<i64>,
}

/// 执行只读演练，或在全部显式授权成立后执行破坏性重建。
pub(super) async fn execute(options: Options<'_>) -> anyhow::Result<()> {
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

fn validate_execution_options(options: &Options<'_>) -> anyhow::Result<()> {
    match (options.execute, options.receipt) {
        (false, Some(_)) => {
            anyhow::bail!("postgres-rebuild 只读演练不写 receipt，请移除 --receipt")
        }
        (true, None) => anyhow::bail!("实际 postgres-rebuild 必须提供 --receipt"),
        _ => {}
    }
    if options.execute {
        let expected = format!("REBUILD {}", options.confirmed_database_name);
        if options.confirm_rebuild != Some(expected.as_str()) {
            anyhow::bail!("实际 postgres-rebuild 的 --confirm-rebuild 必须逐字匹配目标数据库");
        }
    }
    if options.backup_manifest.is_some() == options.allow_unrecoverable_ephemeral {
        anyhow::bail!(
            "postgres-rebuild 必须且只能选择 --backup-manifest 或 --allow-unrecoverable-ephemeral"
        );
    }
    if matches!(options.mode, Mode::EmptyForRestore) && options.backup_manifest.is_none() {
        anyhow::bail!("empty-for-restore 模式必须提供 --backup-manifest");
    }
    if options.allow_unrecoverable_ephemeral {
        let expected = format!("EPHEMERAL {}", options.confirmed_database_name);
        if options.confirm_ephemeral != Some(expected.as_str()) {
            anyhow::bail!("一次性环境豁免的 --confirm-ephemeral 必须逐字匹配目标数据库");
        }
    } else if options.confirm_ephemeral.is_some() {
        anyhow::bail!("--confirm-ephemeral 只能与 --allow-unrecoverable-ephemeral 同时使用");
    }
    Ok(())
}

fn load_authorization<'a>(options: &Options<'a>) -> anyhow::Result<Authorization<'a>> {
    if let Some(manifest_path) = options.backup_manifest {
        let verified = super::postgres_restore::verify_backup_pair(
            manifest_path,
            options.confirmed_database_name,
        )?;
        let expected_binary_version = postgres_expected_migration_version();
        if verified.manifest.schema_version > expected_binary_version {
            anyhow::bail!(
                "backup schema version {} is newer than this binary's PostgreSQL migration version {}",
                verified.manifest.schema_version,
                expected_binary_version,
            );
        }
        let receipt = AuthorizationReceipt {
            kind: "verified_backup",
            manifest_sha256: Some(verified.manifest_sha256.clone()),
            archive_file_name: Some(verified.manifest.archive_file_name.clone()),
            archive_sha256: Some(verified.manifest.archive_sha256.clone()),
            backup_schema_version: Some(verified.manifest.schema_version),
            backup_created_at: Some(verified.manifest.created_at),
            unrecoverable_ephemeral: false,
        };
        Ok(Authorization::Backup { manifest_path, verified, receipt })
    } else {
        Ok(Authorization::Ephemeral {
            receipt: AuthorizationReceipt {
                kind: "unrecoverable_ephemeral",
                manifest_sha256: None,
                archive_file_name: None,
                archive_sha256: None,
                backup_schema_version: None,
                backup_created_at: None,
                unrecoverable_ephemeral: true,
            },
        })
    }
}

fn resolve_maintenance_database_url(
    application_database_url: &str,
    configured_maintenance_database_url: Option<&str>,
    confirmed_database_name: &str,
) -> anyhow::Result<String> {
    let application = parse_secure_postgres_url(
        application_database_url,
        confirmed_database_name,
        "application database_url",
    )?;
    let maintenance = match configured_maintenance_database_url {
        Some(database_url) => {
            let maintenance =
                parse_secure_postgres_url(database_url, "postgres", "maintenance database_url")?;
            if application.host_str() != maintenance.host_str()
                || application.port().unwrap_or(5432) != maintenance.port().unwrap_or(5432)
                || postgres_tls_profile(&application) != postgres_tls_profile(&maintenance)
            {
                anyhow::bail!(
                    "maintenance database_url must use the application database endpoint and TLS profile"
                );
            }
            maintenance
        }
        None => {
            let mut maintenance = application;
            maintenance.set_path("/postgres");
            maintenance
        }
    };
    Ok(maintenance.into())
}

fn parse_secure_postgres_url(
    database_url: &str,
    expected_database_name: &str,
    label: &'static str,
) -> anyhow::Result<Url> {
    let url = Url::parse(database_url).with_context(|| format!("parse {label} for rebuild"))?;
    if !matches!(url.scheme(), "postgres" | "postgresql")
        || url.fragment().is_some()
        || url.username().is_empty()
    {
        anyhow::bail!("{label} is outside the secure PostgreSQL rebuild connection profile");
    }
    let raw_database = url.path().strip_prefix('/').unwrap_or(url.path());
    let database = decode_uri_component(raw_database)?;
    if database != expected_database_name || database.is_empty() || database.contains('/') {
        anyhow::bail!("{label} path does not match its required database");
    }
    if url.host_str().is_some_and(|host| host.is_empty() || host.contains(',')) {
        anyhow::bail!("database_url host is empty or uses an unsupported multi-host form");
    }

    let mut seen_query_keys = HashSet::new();
    for (key, value) in url.query_pairs() {
        let key = key.into_owned();
        let value = value.into_owned();
        if !seen_query_keys.insert(key.clone()) {
            anyhow::bail!("database_url contains a duplicate PostgreSQL query parameter");
        }
        if (key == "ssl" && seen_query_keys.contains("sslmode"))
            || (key == "sslmode" && seen_query_keys.contains("ssl"))
        {
            anyhow::bail!("database_url cannot combine ssl and sslmode");
        }
        match key.as_str() {
            "application_name" => require_pg_value(&value, "application_name", 63)?,
            "connect_timeout" => {
                value
                    .parse::<u16>()
                    .ok()
                    .filter(|timeout| (1..=300).contains(timeout))
                    .ok_or_else(|| anyhow::anyhow!("connect_timeout must be 1..=300"))?;
            }
            "sslmode"
                if matches!(
                    value.as_str(),
                    "disable" | "allow" | "prefer" | "require" | "verify-ca" | "verify-full"
                ) => {}
            "ssl" if value == "true" => {}
            _ => anyhow::bail!(
                "database_url query parameter is not supported by the rebuild adapter: {key}"
            ),
        }
    }
    Ok(url)
}

fn postgres_tls_profile(url: &Url) -> &str {
    url.query_pairs()
        .find_map(|(key, value)| match key.as_ref() {
            "sslmode" => Some(match value.as_ref() {
                "disable" => "disable",
                "allow" => "allow",
                "prefer" => "prefer",
                "require" => "require",
                "verify-ca" => "verify-ca",
                "verify-full" => "verify-full",
                _ => "invalid",
            }),
            "ssl" if value == "true" => Some("require"),
            _ => None,
        })
        .unwrap_or("prefer")
}

fn require_rebuild_source(
    preflight: &auction_storage::PostgresRecoveryPreflight,
) -> anyhow::Result<i64> {
    if !preflight.restore_write_possible {
        anyhow::bail!("PostgreSQL rebuild source is read-only or a recovery replica");
    }
    if preflight.active_other_sessions != 0 {
        anyhow::bail!(
            "PostgreSQL rebuild source has {} other session(s); stop all roles and clients first",
            preflight.active_other_sessions,
        );
    }
    require_backup_source(preflight)
}

async fn verify_empty_target(database_url: &str, database_name: &str) -> anyhow::Result<()> {
    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect recreated PostgreSQL database for empty-target verification")?;
    let postgres = match store {
        DatabaseStore::Postgres(postgres) => postgres,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => {
            anyhow::bail!("recreated target no longer resolves to PostgreSQL")
        }
    };
    let preflight = postgres
        .collect_recovery_preflight(database_name)
        .await
        .context("collect recreated PostgreSQL empty-target evidence")?;
    super::postgres_restore::require_empty_restore_target(&preflight)?;
    postgres.close().await;
    Ok(())
}

async fn migrate_and_verify(config: &AppConfig, database_name: &str) -> anyhow::Result<i64> {
    let database_url = config
        .database_url()
        .context("database_url disappeared after PostgreSQL rebuild migration")?;
    let migration_store = DatabaseStore::connect(database_url)
        .await
        .context("connect rebuilt PostgreSQL database for migration")?;
    migration_store.migrate().await.context("run rebuilt PostgreSQL database migrations")?;
    let migration_postgres = match migration_store {
        DatabaseStore::Postgres(postgres) => postgres,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => {
            anyhow::bail!("rebuilt target no longer resolves to PostgreSQL")
        }
    };
    migration_postgres.close().await;
    println!("database migrations applied for postgres");

    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect rebuilt PostgreSQL database for migration verification")?;
    let postgres = match store {
        DatabaseStore::Postgres(postgres) => postgres,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => {
            anyhow::bail!("rebuilt target no longer resolves to PostgreSQL")
        }
    };
    let preflight = postgres
        .collect_recovery_preflight(database_name)
        .await
        .context("collect rebuilt PostgreSQL schema evidence")?;
    let version = require_backup_source(&preflight)?;
    if preflight.active_other_sessions != 0 {
        anyhow::bail!("rebuilt PostgreSQL target gained an unexpected external session");
    }
    postgres.close().await;
    Ok(version)
}

struct ReceiptContext<'a> {
    options: &'a Options<'a>,
    authorization: &'a AuthorizationReceipt,
    source_schema_version: i64,
    preflight: &'a PostgresRebuildPreflight,
    rebuild: Option<&'a PostgresRebuildResult>,
    status: &'static str,
    failure_stage: Option<&'static str>,
    database_dropped: bool,
    database_recreated: bool,
    postcondition: Option<&'static str>,
    migration_version: Option<i64>,
}

fn publish_receipt(
    reservation: &mut Option<RebuildReceiptReservation>,
    context: ReceiptContext<'_>,
) -> anyhow::Result<()> {
    let rebuild = context.rebuild;
    let payload = PostgresRebuildReceipt {
        format: POSTGRES_REBUILD_RECEIPT_FORMAT,
        recorded_at: Utc::now(),
        status: context.status,
        failure_stage: context.failure_stage,
        database_name: context.options.confirmed_database_name,
        database_name_sha256: sha256_text(context.options.confirmed_database_name),
        mode: context.options.mode.name(),
        authorization: context.authorization,
        source_schema_version: context.source_schema_version,
        previous_database_oid: context.preflight.target_database_oid,
        new_database_oid: rebuild.map(|result| result.new_database_oid),
        database_owner: &context.preflight.target_owner,
        locale_provider: rebuild
            .map_or(context.preflight.target_locale_provider.as_str(), |result| {
                result.locale_provider.as_str()
            }),
        tablespace: rebuild.map_or(context.preflight.target_tablespace.as_str(), |result| {
            result.tablespace.as_str()
        }),
        connection_limit: rebuild
            .map_or(context.preflight.target_connection_limit, |result| result.connection_limit),
        template_database: context.database_recreated.then_some("template0"),
        forced_drop: context.database_dropped,
        database_dropped: context.database_dropped,
        database_recreated: context.database_recreated,
        postcondition: context.postcondition,
        migration_version: context.migration_version,
    };
    let bytes = serde_json::to_vec_pretty(&payload).context("encode PostgreSQL rebuild receipt")?;
    reservation
        .take()
        .ok_or_else(|| {
            anyhow::anyhow!("required PostgreSQL rebuild receipt reservation is missing")
        })?
        .publish(&bytes)
}

fn publish_postcondition_failure(
    reservation: &mut Option<RebuildReceiptReservation>,
    options: &Options<'_>,
    authorization: &AuthorizationReceipt,
    source_schema_version: i64,
    preflight: &PostgresRebuildPreflight,
    rebuild: &PostgresRebuildResult,
    failure_stage: &'static str,
) -> anyhow::Result<()> {
    publish_receipt(
        reservation,
        ReceiptContext {
            options,
            authorization,
            source_schema_version,
            preflight,
            rebuild: Some(rebuild),
            status: "postcondition_failed",
            failure_stage: Some(failure_stage),
            database_dropped: true,
            database_recreated: true,
            postcondition: None,
            migration_version: None,
        },
    )
    .context("database changed but publishing PostgreSQL rebuild failure receipt failed")
}
