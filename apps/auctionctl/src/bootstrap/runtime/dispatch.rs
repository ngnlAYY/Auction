//! auctionctl 命令分派、配置诊断、退出码和迁移入口。

use super::*;

const EXIT_USAGE_OR_CONFIG: u8 = 2;
const EXIT_DEPENDENCY: u8 = 3;
const EXIT_DEPLOYMENT: u8 = 4;
const EXIT_OPERATION: u8 = 5;

/// auctionctl 对外稳定的失败结果；错误正文沿用已脱敏的业务上下文，退出码由命令类别决定。
#[derive(Debug)]
pub struct AuctionCtlError {
    code: u8,
    source: anyhow::Error,
}

impl AuctionCtlError {
    fn new(code: u8, source: anyhow::Error) -> Self {
        Self { code, source }
    }

    /// 返回供 shell/systemd 使用的稳定退出码。
    pub const fn exit_code(&self) -> u8 {
        self.code
    }
}

impl std::fmt::Display for AuctionCtlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:#}", self.source)
    }
}

impl std::error::Error for AuctionCtlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.source()
    }
}

impl Command {
    fn failure_code(&self) -> u8 {
        match self {
            Self::CheckConfig { .. } | Self::DeploymentCheck { .. } => EXIT_USAGE_OR_CONFIG,
            Self::Deployment { .. } => EXIT_DEPLOYMENT,
            Self::Migrate { .. }
            | Self::MigrateManaged { .. }
            | Self::PostgresBootstrap { .. }
            | Self::RetentionInventory { .. }
            | Self::RecoveryPreflight { .. }
            | Self::PostgresBackup { .. }
            | Self::PostgresBackupVerify { .. }
            | Self::PostgresLegacyV28Backup { .. }
            | Self::PostgresLegacyV28BackupVerify { .. }
            | Self::PostgresAdoptLegacyV28 { .. }
            | Self::PostgresRestore { .. }
            | Self::PostgresRebuild { .. } => EXIT_DEPENDENCY,
            Self::BootstrapMailCredentials { .. }
            | Self::ImportRulesDirectory { .. }
            | Self::ForumWriteRecoveryList { .. }
            | Self::AuthorizeForumWriteRetry { .. }
            | Self::VerifyLegacySettlement { .. }
            | Self::MailEvidence { .. } => EXIT_OPERATION,
        }
    }
}

pub async fn run() -> Result<(), AuctionCtlError> {
    let command = Cli::try_parse()
        .map_err(|error| {
            AuctionCtlError::new(EXIT_USAGE_OR_CONFIG, anyhow::anyhow!(error.to_string()))
        })?
        .command;
    let failure_code = command.failure_code();
    run_command(command).await.map_err(|error| AuctionCtlError::new(failure_code, error))
}

async fn run_command(command: Command) -> anyhow::Result<()> {
    match command {
        Command::CheckConfig { role } => check_config(role.into())?,
        Command::MigrateManaged { config_dir } => {
            #[cfg(unix)]
            managed::migrate(&config_dir).await?;
            #[cfg(not(unix))]
            {
                let _ = config_dir;
                anyhow::bail!("managed migration requires Unix");
            }
        }
        Command::MailEvidence { action } => match action {
            MailEvidenceAction::Export { evidence_id, output, key_source } => {
                mail_diagnostics::export(evidence_id, &output, key_source)?;
            }
            MailEvidenceAction::InspectSent { obligation_id, output } => {
                mail_diagnostics::inspect_sent(obligation_id, output).await?;
            }
            MailEvidenceAction::BackfillDisplay { evidence_id, key_source } => {
                mail_diagnostics::backfill_display(evidence_id, key_source).await?;
            }
        },
        Command::Migrate { controller } => {
            migrate(controller).await?;
        }
        Command::PostgresBootstrap { controller, confirm_database, maintenance_secrets_file } => {
            postgres_bootstrap(controller, &confirm_database, maintenance_secrets_file.as_deref())
                .await?;
        }
        Command::DeploymentCheck { config_dir, strict_permissions } => {
            deployment_check(&config_dir, strict_permissions)?;
        }
        Command::Deployment { controller, action } => {
            let controller = deployment_control::ControllerOptions {
                systemctl: controller.systemctl,
                state_dir: controller.state_dir,
                worker_unit: controller.worker_unit,
                admin_unit: controller.admin_unit,
                public_unit: controller.public_unit,
            };
            match action {
                DeploymentAction::Start { config_dir, startup_timeout_seconds } => {
                    deployment_control::start(
                        controller,
                        config_dir,
                        std::time::Duration::from_secs(startup_timeout_seconds),
                    )
                    .await?;
                }
                DeploymentAction::Stop => deployment_control::stop(controller)?,
                DeploymentAction::Restart { config_dir, startup_timeout_seconds } => {
                    deployment_control::restart(
                        controller,
                        config_dir,
                        std::time::Duration::from_secs(startup_timeout_seconds),
                    )
                    .await?;
                }
                DeploymentAction::Status => deployment_control::status(controller)?,
            }
        }
        Command::BootstrapMailCredentials { world, label } => {
            bootstrap_mail_credentials(&world, &label).await?;
        }
        Command::ImportRulesDirectory { directory, command_id, actor } => {
            import_rules_directory(&directory, command_id, &actor).await?;
        }
        Command::ForumWriteRecoveryList { limit } => {
            forum_write_recovery_list(limit).await?;
        }
        Command::AuthorizeForumWriteRetry { auction_id, command_id, actor, confirm_write } => {
            authorize_forum_write_retry(auction_id, command_id, &actor, &confirm_write).await?;
        }
        Command::VerifyLegacySettlement { auction_id } => {
            verify_legacy_settlement(auction_id).await?;
        }
        Command::RetentionInventory { output, confirm_database } => {
            retention_inventory(&output, &confirm_database).await?;
        }
        Command::RecoveryPreflight { output, confirm_database } => {
            recovery_preflight(&output, &confirm_database).await?;
        }
        Command::PostgresBackup { output, confirm_database } => {
            postgres_backup(&output, &confirm_database).await?;
        }
        Command::PostgresBackupVerify { manifest, confirm_database } => {
            postgres_backup_verify(&manifest, &confirm_database).await?;
        }
        Command::PostgresLegacyV28Backup { output, confirm_database } => {
            execute_postgres_legacy_v28_backup(&output, &confirm_database).await?;
        }
        Command::PostgresLegacyV28BackupVerify { manifest, confirm_database } => {
            verify_postgres_legacy_v28_backup(&manifest, &confirm_database)?;
        }
        Command::PostgresAdoptLegacyV28 {
            confirm_database,
            backup_manifest,
            execute,
            confirm_adoption,
            receipt,
        } => {
            execute_postgres_legacy_v28_adoption(
                &confirm_database,
                backup_manifest.as_deref(),
                execute,
                confirm_adoption.as_deref(),
                receipt.as_deref(),
            )
            .await?;
        }
        Command::PostgresRestore { manifest, confirm_database, receipt, dry_run } => {
            execute_postgres_restore(&manifest, receipt.as_deref(), &confirm_database, dry_run)
                .await?;
        }
        Command::PostgresRebuild {
            confirm_database,
            mode,
            backup_manifest,
            allow_unrecoverable_ephemeral,
            confirm_ephemeral,
            maintenance_secrets_file,
            execute,
            confirm_rebuild,
            receipt,
        } => {
            execute_postgres_rebuild(
                &confirm_database,
                mode,
                backup_manifest.as_deref(),
                allow_unrecoverable_ephemeral,
                confirm_ephemeral.as_deref(),
                maintenance_secrets_file.as_deref(),
                execute,
                confirm_rebuild.as_deref(),
                receipt.as_deref(),
            )
            .await?;
        }
    }

    Ok(())
}

fn check_config(role: Role) -> anyhow::Result<()> {
    let config = AppConfig::load(role).context("load configuration")?;
    let database_backend = config
        .database_url()
        .map(DatabaseBackend::from_url)
        .transpose()
        .context("validate database_url")?;
    let listener = config.listener.as_ref();
    let listener_mode = listener.map(|listener| listener.mode.name());
    let listener_socket_path = listener
        .filter(|listener| listener.mode == HttpListenMode::Unix)
        .map(|listener| listener.socket_path.as_str());
    let listener_runtime_directory = listener
        .filter(|listener| listener.mode == HttpListenMode::Unix)
        .map(|listener| listener.runtime_directory.as_str());
    let listener_group = listener
        .filter(|listener| listener.mode == HttpListenMode::Unix)
        .map(|listener| listener.group.as_str());
    let mooglemail_worlds = config.worker.mooglemail.configured_worlds();
    let mooglemail_world_ids =
        mooglemail_worlds.iter().map(|world| world.world_id.as_str()).collect::<Vec<_>>();
    let mooglemail_sender_allowlist_entry_count =
        mooglemail_worlds.iter().map(|world| world.sender_allowlist.len()).sum::<usize>();
    let summary = serde_json::json!({
        "role": config.role.name(),
        "listener": {
            "mode": listener_mode,
            "address": config.listen_addr.map(|address| address.to_string()),
            "runtime_directory": listener_runtime_directory,
            "socket_path": listener_socket_path,
            "group": listener_group,
        },
        "database": {
            "configured": config.database_url().is_some(),
            "backend": database_backend.map(DatabaseBackend::name),
        },
        "cache": {
            "backend": config.cache.backend.name(),
            "public_rules_ttl_seconds": config.cache.public_rules_ttl_seconds,
        },
        "metrics": {
            "enabled": config.metrics.enabled,
        },
        "tracing": {
            "enabled": config.tracing.enabled,
            "endpoint_configured": !config.tracing.endpoint.is_empty(),
            "headers_configured": config.tracing_headers().is_some(),
            "queue_capacity": config.tracing.queue_capacity,
        },
        "http_security": {
            "allowed_origin_count": config.http_security.allowed_origins.len(),
            "trusted_proxy_cidr_count": config.http_security.trusted_proxy_cidrs.len(),
            "trusted_unix_proxy": config.http_security.trusted_unix_proxy,
            "cookie_secure": config.http_security.cookie_secure,
        },
        "session": {
            "idle_ttl_seconds": config.session.idle_ttl_seconds,
            "absolute_ttl_seconds": config.session.absolute_ttl_seconds,
            "max_public_sessions": config.session.max_public_sessions,
        },
        "worker": {
            "concurrency": config.worker_concurrency(),
            "poll_interval_seconds": config.worker_poll_interval_seconds(),
            "control_enabled": config.worker.control.enabled,
            "control_socket_path": config.worker.control.socket_path,
            "control_allowed_peer_uid_count": config.worker.control.allowed_peer_uids.len(),
            "control_allowed_peer_gid_count": config.worker.control.allowed_peer_gids.len(),
            "mooglemail_enabled": config.worker.mooglemail.enabled,
            "mooglemail_worlds": mooglemail_world_ids,
            "mooglemail_sender_allowlist_entry_count": mooglemail_sender_allowlist_entry_count,
            "mooglemail_equipment_filter_enabled": config.worker.mooglemail.equipment_filter.enabled,
            "mooglemail_mail_template_count": config.worker.mooglemail.mail_templates.values().len(),
        },
        "realtime": {
            "consumer_key": config.realtime.consumer_key,
            "poll_interval_millis": config.realtime.poll_interval_millis,
            "client_buffer": config.realtime.client_buffer,
        },
        "admin_runtime": config.admin_runtime.as_ref().map(|runtime| serde_json::json!({
            "equipment_source_minimum_request_interval_millis":
                runtime.equipment_source.minimum_request_interval_millis,
        })),
        "admin_runtime_configured": config.admin_runtime.is_some(),
        "admin_credentials_configured": config.admin_credentials_configured(),
        "config_file": config.config_path.as_ref().map(|path| path.display().to_string()),
        "secrets_file_configured": config.secrets_path.is_some(),
        "runtime_config_file": config.runtime_config_path.as_ref().map(|path| path.display().to_string()),
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

async fn migrate(controller: SystemdControllerArgs) -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Public).context("load configuration for migration")?;

    #[cfg(unix)]
    let _migration_guard =
        deployment_control::acquire_migration_guard(deployment_control::ControllerOptions {
            systemctl: controller.systemctl,
            state_dir: controller.state_dir,
            worker_unit: controller.worker_unit,
            admin_unit: controller.admin_unit,
            public_unit: controller.public_unit,
        })?;

    #[cfg(not(unix))]
    {
        let _ = controller;
        anyhow::bail!("独立 migrate 需要 Unix/systemd 角色运行状态检查")
    }

    migrate_config(&config).await
}

pub(super) async fn migrate_config(config: &AppConfig) -> anyhow::Result<()> {
    #[cfg(unix)]
    let _managed_guard = managed::guard(config)?;
    let database_url = config.database_url().context("database_url is required for migration")?;

    #[cfg(feature = "postgres")]
    {
        let store =
            DatabaseStore::connect(database_url).await.context("connect database for migration")?;
        store.migrate().await.context("run database migrations")?;
        println!("database migrations applied for {}", store.backend());
        Ok(())
    }

    #[cfg(not(feature = "postgres"))]
    {
        let _ = database_url;
        anyhow::bail!("auctionctl was built without a database feature; enable postgres")
    }
}
