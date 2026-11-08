//! Admin API 的数据库完整档状态和端口装配。

use std::sync::Arc;

use anyhow::Context;
use auction_application::admin::{
    AdminAuctionCreatePort, AdminAuctionReader, AdminAuctionSchedulePort, AdminAuditPort,
    AdminCredentialCheckPort, AdminCredentialCheckService, AdminCredentialHealthReader,
    AdminCredentialOpener, AdminCredentialPort, AdminCredentialService, AdminForumPreviewPort,
    AdminIntakePort, AdminLotReader, AdminLotUpdatePort, AdminMailPort, AdminProxyEventReader,
    AdminRenameReader, AdminRulesPort, AdminRuntimeConfigPort, AdminRuntimeConfigService,
    AdminUserPort, AdminUserService, AdminWorldPort, DeliverySentProofService,
    SettlementCatalogReader,
};
use auction_application::auction::{AuctionLifecycleCommandPort, AuctionLifecyclePolicy};
use auction_application::delivery::{DeliveryAdminPort, DeliverySentConfirmationPort};
use auction_application::operations::{
    OperationsAlertConsole, OperationsAlertSourceReader, OperationsAlertStore,
};
use auction_application::public::{AdminSessionConfig, AdminSessionPort, AdminSessionStore};
use auction_application::realtime::RealtimeAdminPort;
use auction_application::recovery::{
    RecoveryCommandPort, RecoveryCommandService, RecoveryReportReader,
};
use auction_application::sessions::AdminSessionService;
use auction_application::settlement::SettlementReportReader;
use auction_config::{AppConfig, TomlRuntimeFileStore};
use auction_http::AdminApiState;
use auction_integrations::{
    CredentialSealer, CredentialVerifier, EquipmentKeySealer, ForumPreviewAdapter,
    MoogleMailLoginKeyIssuer, MoogleMailLoginKeyReader, build_keyring,
};
use auction_observability::{
    HealthReader, HealthService, HeartbeatHealthReader, HeartbeatPublisher,
    HeartbeatPublisherConfig, HeartbeatStore, ProcessRole, ServiceName,
};
use auction_storage::{
    DatabaseAdminAuctionCommands, DatabaseAdminIntakeCommands, DatabaseAdminLotCommands,
    DatabaseAdminMailCommands, DatabaseAdminRulesCommands, DatabaseAdminRuntimeCommands,
    DatabaseAdminWorldCommands, DatabaseAuctionLifecycleCommands, DatabaseStore,
};
use chrono::Duration as ChronoDuration;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::super::config;

#[derive(Clone, Copy)]
struct SystemClock;

impl auction_application::core::Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// 连接数据库并组装 Admin API 的完整端口图。
pub(super) async fn configure_state(
    config: &AppConfig,
    tracing_health: Arc<dyn auction_observability::TracingHealthReader>,
    metrics: Arc<auction_observability::ProcessMetrics>,
    switches: auction_application::runtime_switches::RuntimeSwitches,
) -> anyhow::Result<(
    AdminApiState,
    Option<Arc<HeartbeatPublisher>>,
    Option<auction_process::TaskGuard<()>>,
)> {
    let security = config::configure_security(config)?;
    let worker_control = config::configure_worker_control(config);
    let Some(database_url) = config.database_url() else {
        return Ok((
            super::fallback::empty_state(config, security, worker_control, switches),
            None,
            None,
        ));
    };
    let store = Arc::new(
        DatabaseStore::connect(database_url, database_profile(config)?)
            .await
            .context("connect admin database")?
            .with_database_recorder(metrics.database())
            .with_runtime_switches(switches.clone())
            .with_shadow_recorder(metrics.shadow())
            .with_mail_templates(config.worker.mooglemail.mail_templates.clone())
            .with_migration_compatibility_mode(config.readiness.migration_compatibility_mode)
            .with_migration_readiness_recorder(metrics.migration_readiness()),
    );
    let runtime_switch_task = auction_process::TaskGuard::new(
        auction_jobs::start_runtime_switch_refresh(store.clone(), switches.clone()).await,
    );
    metrics
        .register_runtime_switch_metrics(switches.clone())
        .context("register runtime switch metrics")?;
    metrics
        .register_pool_metrics(store.pool_metrics_source())
        .context("register admin database pool metrics")?;
    metrics
        .register_audit_metrics(store.audit_metrics_source())
        .context("register admin audit metrics")?;
    let actor =
        config.admin_actor().context("admin_actor is missing from sensitive configuration")?;
    let secret =
        config.admin_secret().context("admin_secret is missing from sensitive configuration")?;
    let lifecycle_store = Arc::clone(&store);
    let auction_create_store = Arc::clone(&store);
    let admin_rules_store = Arc::clone(&store);
    let admin_runtime: Option<Arc<dyn AdminRuntimeConfigPort>> = if let Some(path) =
        config.runtime_config_path.clone()
    {
        let files = Arc::new(
            TomlRuntimeFileStore::new(
                path,
                config.tracing_headers().is_some(),
                config.runtime_worker_budget,
            )
            .map_err(|error| anyhow::Error::msg(error.to_string()))?,
        );
        let commands = Arc::new(DatabaseAdminRuntimeCommands::new(Arc::clone(&store)));
        let service =
            Arc::new(AdminRuntimeConfigService::new(commands, files, Arc::new(SystemClock)));
        // 恢复失败可能发生在候选文件已经替换之后；结果未知时不得带旧安全配置监听。
        service
            .recover()
            .await
            .context("恢复 runtime 配置失败；停止启动，请按恢复手册核对文件与命令账本")?;
        let restored = service.current().await.context("读取恢复后的 runtime 配置")?;
        if config
            .admin_runtime
            .as_ref()
            .is_none_or(|loaded| loaded.fingerprint() != restored.fingerprint)
        {
            anyhow::bail!(
                "runtime 文件与已加载的启动配置不一致；请重新执行部署重启，加载最新的 Admin/Worker 配置"
            );
        }
        Some(service)
    } else {
        None
    };
    let equipment_keyring = build_keyring(
        auction_config::DataKeyDomain::Equipment,
        config.data_key_current(auction_config::DataKeyDomain::Equipment),
        config
            .data_key_previous(auction_config::DataKeyDomain::Equipment)
            .iter()
            .map(|key| (key.id(), key.material())),
    )
    .map_err(|error| anyhow::Error::msg(error.to_string()))?;
    let admin_intake = match equipment_keyring {
        Some(keyring) => {
            // W1.10：URL 来源改经 Worker control 执行；Admin 不再持有 Cookie、HTTP 客户端或
            // 第二套请求限速器。socket 路径无效时装配明确不可用的端口，不回退到本地直连。
            let remote: Arc<dyn auction_application::operations::WorkerRemoteReadPort> =
                match auction_integrations::WorkerRemoteReadClient::new(
                    config.worker_control_socket_path(),
                    std::time::Duration::from_secs(
                        auction_application::operations::WORKER_REMOTE_READ_TIMEOUT_SECONDS,
                    ),
                ) {
                    Ok(client) => Arc::new(client),
                    Err(error) => {
                        let _ = error;
                        Arc::new(auction_application::operations::UnavailableWorkerRemoteRead)
                    }
                };
            let source = auction_integrations::WorkerEquipmentSourceReader::new(remote);
            let sealer = EquipmentKeySealer::new(Arc::clone(keyring.current()));
            let intake: Arc<dyn AdminIntakePort> = Arc::new(DatabaseAdminIntakeCommands::new(
                Arc::clone(&store),
                Arc::new(source),
                Arc::new(sealer),
                Arc::new(SystemClock),
                config::admin_intake_policy(config),
            ));
            Some(intake)
        }
        None => None,
    };
    let forum_preview: Arc<dyn AdminForumPreviewPort> = Arc::new(ForumPreviewAdapter::new());
    let admin_audit: Arc<dyn AdminAuditPort> = store.clone();
    let admin_renames: Arc<dyn AdminRenameReader> = store.clone();
    let user_login_keyring = build_keyring(
        auction_config::DataKeyDomain::UserLogin,
        config.data_key_current(auction_config::DataKeyDomain::UserLogin),
        config
            .data_key_previous(auction_config::DataKeyDomain::UserLogin)
            .iter()
            .map(|key| (key.id(), key.material())),
    )
    .map_err(|error| anyhow::Error::msg(error.to_string()))?;
    let admin_users: Option<Arc<dyn AdminUserPort>> = match user_login_keyring {
        Some(keyring) => {
            let issuer = Arc::new(MoogleMailLoginKeyIssuer::new(Arc::clone(keyring.current())));
            let opener =
                Arc::new(MoogleMailLoginKeyReader::new(Arc::clone(&store), Arc::clone(&keyring)));
            Some(Arc::new(AdminUserService::new(Arc::clone(&store), issuer, opener)))
        }
        None => None,
    };
    let mut credential_check: Option<Arc<dyn AdminCredentialCheckPort>> = None;
    let credential_keyring = build_keyring(
        auction_config::DataKeyDomain::Credential,
        config.data_key_current(auction_config::DataKeyDomain::Credential),
        config
            .data_key_previous(auction_config::DataKeyDomain::Credential)
            .iter()
            .map(|key| (key.id(), key.material())),
    )
    .map_err(|error| anyhow::Error::msg(error.to_string()))?;
    let admin_credentials: Option<Arc<dyn AdminCredentialPort>> = match credential_keyring {
        Some(keyring) => {
            let sealer = Arc::new(CredentialSealer::new(Arc::clone(&keyring)));
            let acquirer = Arc::new(
                auction_integrations::CredentialAcquirer::production(Arc::clone(keyring.current()))
                    .context("initialize credential acquisition client")?,
            );
            let opener: Arc<dyn AdminCredentialOpener> = sealer.clone();
            let verifier = Arc::new(
                CredentialVerifier::production(opener)
                    .context("initialize credential check clients")?,
            );
            credential_check = Some(Arc::new(AdminCredentialCheckService::new(
                Arc::clone(&store),
                verifier,
                Arc::new(SystemClock),
            )));
            Some(Arc::new(AdminCredentialService::new(Arc::clone(&store), sealer, acquirer)))
        }
        None => None,
    };
    let heartbeat_store: Arc<dyn HeartbeatStore> = store.clone();
    let health: Arc<dyn HealthReader> = Arc::new(HealthService::new(heartbeat_store));
    let heartbeat_store: Arc<dyn HeartbeatStore> = store.clone();
    let heartbeat_health: Arc<dyn HeartbeatHealthReader> = store.clone();
    let heartbeat = Arc::new(HeartbeatPublisher::new(
        heartbeat_store,
        HeartbeatPublisherConfig {
            service_name: ServiceName::Admin,
            process_role: ProcessRole::Admin,
            instance_id: Uuid::new_v4().to_string(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            started_at: Utc::now(),
            ttl: ChronoDuration::seconds(30),
        },
        heartbeat_health,
        tracing_health,
    ));
    if let Err(error) = heartbeat.publish_starting().await {
        tracing::warn!(error = %error, "admin heartbeat starting publish failed");
    }
    let session_store: Arc<dyn AdminSessionStore> = store.clone();
    let settlement_report: Arc<dyn SettlementReportReader> = store.clone();
    let settlement_catalog: Arc<dyn SettlementCatalogReader> = store.clone();
    let recovery_report: Arc<dyn RecoveryReportReader> = store.clone();
    let admin_auctions: Arc<dyn AdminAuctionReader> = store.clone();
    let admin_worlds: Arc<dyn AdminWorldPort> =
        Arc::new(DatabaseAdminWorldCommands::new(Arc::clone(&store)));
    let admin_lots: Arc<dyn AdminLotReader> = store.clone();
    let admin_lot_update: Arc<dyn AdminLotUpdatePort> =
        Arc::new(DatabaseAdminLotCommands::new(Arc::clone(&store), Arc::new(SystemClock)));
    let admin_mail: Arc<dyn AdminMailPort> =
        Arc::new(DatabaseAdminMailCommands::new(Arc::clone(&store), Arc::new(SystemClock)));
    let admin_proxy_events: Arc<dyn AdminProxyEventReader> = store.clone();
    let delivery_admin: Arc<dyn DeliveryAdminPort> = store.clone();
    let delivery_disposition: Arc<dyn auction_application::delivery::DeliveryDispositionPort> =
        store.clone();
    let delivery_sent_confirmation: Arc<dyn DeliverySentConfirmationPort> = store.clone();
    let delivery_sent_proof = Arc::new(DeliverySentProofService::new(
        Arc::clone(&delivery_admin),
        delivery_sent_confirmation,
    ));
    let operations_alert: Arc<dyn OperationsAlertStore> = store.clone();
    let operations_alert_source: Arc<dyn OperationsAlertSourceReader> = store.clone();
    let operations_alert_console: Arc<dyn OperationsAlertConsole> = store.clone();
    let credential_health: Arc<dyn AdminCredentialHealthReader> = store.clone();
    let realtime_admin: Arc<dyn RealtimeAdminPort> = store.clone();
    let recovery_command_store = Arc::clone(&store);
    let auction_config::RoleSessionConfig::Timed(session_settings) = config.session else {
        anyhow::bail!("Admin session configuration is unavailable");
    };
    let session_config = AdminSessionConfig {
        idle_ttl: ChronoDuration::seconds(session_settings.idle_ttl_seconds as i64),
        absolute_ttl: ChronoDuration::seconds(session_settings.absolute_ttl_seconds as i64),
    };
    let session: Arc<dyn AdminSessionPort> = Arc::new(
        AdminSessionService::new(
            session_store,
            actor.to_owned(),
            secret.as_bytes().to_vec(),
            session_config,
            Utc::now(),
        )
        .await
        .context("initialize Admin session authority")?,
    );
    let lifecycle_policy = AuctionLifecyclePolicy::new(
        config.admin_runtime.as_ref().is_some_and(|runtime| runtime.allow_manual_early_open),
        config.admin_runtime.as_ref().is_some_and(|runtime| runtime.allow_manual_early_settlement),
    );
    // 人工开拍与 Worker 开拍必须用同一排期日历计算后继，避免早鸟场次身份分叉。
    let lifecycle: Arc<dyn AuctionLifecycleCommandPort> = Arc::new(
        DatabaseAuctionLifecycleCommands::new(
            lifecycle_store,
            lifecycle_policy,
            config.auction.policy()?,
        )
        .with_schedule_calendar(config.auction.schedule_calendar()?),
    );
    // 创建和排期共用无可变状态的适配器，避免重复构造同一策略快照。
    let auction_commands =
        Arc::new(DatabaseAdminAuctionCommands::new(auction_create_store, config.auction.policy()?));
    let auction_create: Arc<dyn AdminAuctionCreatePort> = auction_commands.clone();
    let auction_schedule: Arc<dyn AdminAuctionSchedulePort> = auction_commands;
    let admin_rules: Arc<dyn AdminRulesPort> =
        Arc::new(DatabaseAdminRulesCommands::new(admin_rules_store, Arc::new(SystemClock)));
    let recovery_commands: Arc<dyn RecoveryCommandPort> =
        Arc::new(RecoveryCommandService::new(recovery_command_store));
    let state = AdminApiState::new(
        Some(session),
        Some(lifecycle),
        Some(auction_create),
        Some(admin_audit),
        Some(health),
        admin_intake,
        Some(forum_preview),
        Some(admin_mail),
        Some(admin_rules),
        admin_runtime,
        admin_credentials,
        Some(admin_renames),
        admin_users,
        Some(recovery_commands),
        Some(settlement_report),
        Some(recovery_report),
        Some(admin_auctions),
        Some(admin_lots),
        Some(admin_lot_update),
        Some(admin_proxy_events),
        Some(delivery_admin),
        Some(operations_alert),
        Some(operations_alert_source),
        Some(credential_health),
        Some(realtime_admin),
        worker_control,
        security,
    )
    .with_operations_alert_console(Some(operations_alert_console))
    .with_operations_alert_policy(config.worker.operations_alert.policy())
    .with_forum_reader(store.clone())
    .with_delivery_disposition(Some(delivery_disposition))
    .with_delivery_sent_proof(delivery_sent_proof)
    .with_credential_check(credential_check)
    .with_auction_schedule(auction_schedule)
    .with_admin_worlds(admin_worlds)
    .with_settlement_catalog(settlement_catalog)
    .with_dashboard_worlds(config::configured_dashboard_worlds(config))
    .with_dashboard_mailbox_keys(config::configured_dashboard_mailbox_keys(config))
    .with_runtime_switches(store.clone(), switches.clone())
    .with_runtime_switch_snapshot(switches.clone())
    .with_readiness(Arc::new(
        auction_http::HttpReadinessProbe::new(
            store,
            config.readiness.cache_enabled,
            metrics.readiness(),
        )
        .with_runtime_switches(switches),
    ));
    if let Err(error) = heartbeat.publish_current().await {
        tracing::warn!(error = %error, "admin heartbeat ready publish failed");
    }
    Ok((state, Some(heartbeat), Some(runtime_switch_task)))
}

#[cfg(feature = "postgres")]
fn database_profile(
    config: &auction_config::AppConfig,
) -> Result<auction_storage::DatabaseConnectionProfile, auction_config::ConfigError> {
    use auction_config::{DatabaseProfileMode, Role};
    use auction_storage::{DatabaseConnectionProfile, DatabaseConnectionPurpose as Purpose};
    if config.database_profiles.mode == DatabaseProfileMode::Legacy {
        return Ok(DatabaseConnectionProfile::legacy());
    }
    let purpose = match config.role {
        Role::Public => Purpose::Public,
        Role::Admin => Purpose::Admin,
        Role::Worker => Purpose::Worker,
    };
    let timeouts =
        config.database_pool.timeouts()?.map(|value| auction_storage::DatabaseConnectionTimeouts {
            acquire_millis: value.acquire_timeout_millis,
            statement_millis: value.statement_timeout_millis,
            lock_millis: value.lock_timeout_millis,
            idle_transaction_millis: value.idle_in_transaction_session_timeout_millis,
        });
    Ok(DatabaseConnectionProfile::service(purpose, config.database_pool.max_connections, timeouts))
}
