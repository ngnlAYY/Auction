//! Admin API 的数据库完整档状态和端口装配。

use std::{collections::HashSet, sync::Arc};

use anyhow::Context;
use auction_application::{
    AdminAuctionCreatePort, AdminAuctionReader, AdminAuctionSchedulePort, AdminAuditPort,
    AdminCredentialCheckPort, AdminCredentialCheckService, AdminCredentialHealthReader,
    AdminCredentialKind, AdminCredentialOpener, AdminCredentialPort, AdminCredentialRuntimeReader,
    AdminCredentialRuntimeWriter, AdminCredentialService, AdminCredentialWorld,
    AdminForumPreviewPort, AdminIntakePort, AdminLotReader, AdminLotUpdatePort, AdminMailPort,
    AdminProxyEventReader, AdminRenameReader, AdminRulesPort, AdminRuntimeConfigPort,
    AdminRuntimeConfigService, AdminSessionConfig, AdminSessionPort, AdminSessionService,
    AdminSessionStore, AdminUserPort, AdminUserService, AdminWorldPort,
    AuctionLifecycleCommandPort, AuctionLifecyclePolicy, DeliveryAdminPort,
    OperationsAlertSourceReader, OperationsAlertStore, RealtimeAdminPort, RecoveryCommandPort,
    RecoveryCommandService, RecoveryReportReader, SettlementCatalogReader, SettlementReportReader,
};
use auction_config::{AppConfig, TomlRuntimeFileStore};
use auction_http::AdminApiState;
use auction_integrations::{
    CredentialSealer, CredentialVerifier, DataKey, EquipmentKeySealer, EquipmentSourceAdapter,
    ForumPreviewAdapter, MoogleMailLoginKeyIssuer, MoogleMailLoginKeyReader,
    build_equipment_http_client,
    mooglemail::{MoogleMailCookie, MoogleMailCredentialSet, MoogleMailWorld},
};
use auction_observability::{
    HealthReader, HealthService, HeartbeatHealthReader, HeartbeatPublisher,
    HeartbeatPublisherConfig, HeartbeatStore, ProcessRole, ServiceName,
};
use auction_storage::{
    DatabaseAdminAuctionCommands, DatabaseAdminIntakeCommands, DatabaseAdminLotCommands,
    DatabaseAdminMailCommands, DatabaseAdminRulesCommands, DatabaseAdminRuntimeCommands,
    DatabaseAdminWorldCommands, DatabaseAuctionLifecycleCommands, DatabaseBackend, DatabaseStore,
};
use chrono::Duration as ChronoDuration;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::super::config;

#[derive(Clone, Copy)]
struct SystemClock;

impl auction_application::Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

async fn load_equipment_source_credentials(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
) -> anyhow::Result<Option<MoogleMailCredentialSet>> {
    let mut cookies = Vec::new();
    if let Some((key_id, key_material)) =
        config.credential_data_key_id().zip(config.credential_data_key())
    {
        let data_key = Arc::new(
            DataKey::from_base64(key_id, key_material)
                .map_err(|error| anyhow::Error::msg(error.to_string()))?,
        );
        let opener = CredentialSealer::new(data_key);
        let reader: Arc<dyn AdminCredentialRuntimeReader> = store.clone();
        let mut records = Vec::new();
        let mut seen_record_ids = HashSet::new();
        for credential_world in [AdminCredentialWorld::Main, AdminCredentialWorld::Isekai] {
            for record in reader
                .list_active(AdminCredentialKind::MailCookie, credential_world)
                .await
                .context("load active equipment source credentials")?
            {
                // shared scope 会同时出现在两个 world 查询中，只向进程内集合加入一次。
                if seen_record_ids.insert(record.id) {
                    records.push((record, credential_world));
                }
            }
        }
        anyhow::ensure!(records.len() <= 1, "装备来源只允许一个 active mail_cookie");
        cookies.reserve(records.len());
        for (record, credential_world) in records {
            let secret = opener
                .open(record.kind, record.id, &record.scope, &record.envelope)
                .map_err(|error| anyhow::Error::msg(error.to_string()))?;
            let value = match String::from_utf8(secret) {
                Ok(value) => value,
                Err(error) => {
                    let mut bytes = error.into_bytes();
                    bytes.fill(0);
                    anyhow::bail!("装备来源凭据不是有效 UTF-8");
                }
            };
            let world = if record.scope
                == AdminCredentialWorld::Shared.scope(AdminCredentialKind::MailCookie)
            {
                None
            } else {
                Some(match credential_world {
                    AdminCredentialWorld::Main => MoogleMailWorld::Main,
                    AdminCredentialWorld::Isekai => MoogleMailWorld::Isekai,
                    AdminCredentialWorld::Shared => anyhow::bail!("装备来源凭据 world scope 无效"),
                })
            };
            cookies.push((
                record.id,
                world,
                MoogleMailCookie::new(value)
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            ));
        }
    }
    if !cookies.is_empty() {
        let invalidator: Arc<dyn AdminCredentialRuntimeWriter> = store;
        return MoogleMailCredentialSet::from_runtime_cookies_with_scopes(cookies, invalidator)
            .map(Some)
            .map_err(|error| anyhow::Error::msg(error.to_string()));
    }
    config
        .mooglemail_cookie()
        .map(|cookie| {
            MoogleMailCredentialSet::from_cookies(vec![
                MoogleMailCookie::new(cookie)
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            ])
            .map_err(|error| anyhow::Error::msg(error.to_string()))
        })
        .transpose()
}

/// 连接数据库并组装 Admin API 的完整端口图。
pub(super) async fn configure_state(
    config: &AppConfig,
    tracing_health: Arc<dyn auction_observability::TracingHealthReader>,
    metrics: Arc<auction_observability::ProcessMetrics>,
) -> anyhow::Result<(AdminApiState, Option<Arc<HeartbeatPublisher>>)> {
    let security = config::configure_security(config)?;
    let worker_control = config::configure_worker_control(config);
    let Some(database_url) = config.database_url() else {
        let forum_preview: Arc<dyn AdminForumPreviewPort> = Arc::new(ForumPreviewAdapter::new());
        return Ok((
            AdminApiState::new(
                None,
                None,
                None,
                None,
                None,
                None,
                Some(forum_preview),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                worker_control,
                security,
            )
            .with_dashboard_worlds(config::configured_dashboard_worlds(config))
            .with_dashboard_mailbox_keys(config::configured_dashboard_mailbox_keys(config)),
            None,
        ));
    };
    let store =
        Arc::new(DatabaseStore::connect(database_url).await.context("connect admin database")?);
    metrics
        .register_pool_metrics(store.pool_metrics_source())
        .context("register admin database pool metrics")?;
    let actor =
        config.admin_actor().context("admin_actor is missing from sensitive configuration")?;
    let secret =
        config.admin_secret().context("admin_secret is missing from sensitive configuration")?;
    let lifecycle_store = Arc::clone(&store);
    let auction_create_store = Arc::clone(&store);
    let admin_rules_store = Arc::clone(&store);
    let admin_runtime: Option<Arc<dyn AdminRuntimeConfigPort>> = if store.backend()
        == DatabaseBackend::Postgres
    {
        if let Some(path) = config.runtime_config_path.clone() {
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
        }
    } else {
        None
    };
    let admin_intake = if store.backend() == DatabaseBackend::Postgres {
        match config.equipment_data_key_id().zip(config.equipment_data_key()) {
            Some((key_id, key_material)) => {
                let data_key = DataKey::from_base64(key_id, key_material)
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?;
                let mut source = EquipmentSourceAdapter::production(
                    build_equipment_http_client()?,
                    config::equipment_source_policy(config)?,
                )
                .map_err(|error| anyhow::Error::msg(error.to_string()))?;
                if let Some(credentials) =
                    load_equipment_source_credentials(config, Arc::clone(&store)).await?
                {
                    source = source.with_mooglemail_credentials(credentials);
                }
                let sealer = EquipmentKeySealer::new(data_key);
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
        }
    } else {
        None
    };
    let forum_preview: Arc<dyn AdminForumPreviewPort> = Arc::new(ForumPreviewAdapter::new());
    let admin_audit: Arc<dyn AdminAuditPort> = store.clone();
    let admin_renames: Arc<dyn AdminRenameReader> = store.clone();
    let admin_users: Option<Arc<dyn AdminUserPort>> =
        match config.user_login_data_key_id().zip(config.user_login_data_key()) {
            Some((key_id, key_material)) => {
                let data_key = Arc::new(
                    DataKey::from_base64(key_id, key_material)
                        .map_err(|error| anyhow::Error::msg(error.to_string()))?,
                );
                let issuer = Arc::new(MoogleMailLoginKeyIssuer::new(Arc::clone(&data_key)));
                let opener = Arc::new(MoogleMailLoginKeyReader::new(Arc::clone(&store), data_key));
                Some(Arc::new(AdminUserService::new(Arc::clone(&store), issuer, opener)))
            }
            None => None,
        };
    let mut credential_check: Option<Arc<dyn AdminCredentialCheckPort>> = None;
    let admin_credentials: Option<Arc<dyn AdminCredentialPort>> = if store.backend()
        == DatabaseBackend::Postgres
    {
        match config.credential_data_key_id().zip(config.credential_data_key()) {
            Some((key_id, key_material)) => {
                let data_key = Arc::new(
                    DataKey::from_base64(key_id, key_material)
                        .map_err(|error| anyhow::Error::msg(error.to_string()))?,
                );
                let sealer = Arc::new(CredentialSealer::new(Arc::clone(&data_key)));
                let acquirer = Arc::new(
                    auction_integrations::CredentialAcquirer::production(data_key)
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
        }
    } else {
        None
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
    let operations_alert: Arc<dyn OperationsAlertStore> = store.clone();
    let operations_alert_source: Arc<dyn OperationsAlertSourceReader> = store.clone();
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
        store.backend() == DatabaseBackend::Postgres
            && config
                .admin_runtime
                .as_ref()
                .is_some_and(|runtime| runtime.allow_manual_early_settlement),
    );
    let lifecycle: Arc<dyn AuctionLifecycleCommandPort> =
        Arc::new(DatabaseAuctionLifecycleCommands::new(
            lifecycle_store,
            Arc::new(SystemClock),
            lifecycle_policy,
            config.auction.policy()?,
        ));
    // 创建和排期共用无可变状态的适配器，避免重复构造同一策略快照与时钟。
    let auction_commands = Arc::new(DatabaseAdminAuctionCommands::new(
        auction_create_store,
        Arc::new(SystemClock),
        config.auction.policy()?,
    ));
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
    .with_forum_reader(store.clone())
    .with_credential_check(credential_check)
    .with_auction_schedule(auction_schedule)
    .with_admin_worlds(admin_worlds)
    .with_settlement_catalog(settlement_catalog)
    .with_dashboard_worlds(config::configured_dashboard_worlds(config))
    .with_dashboard_mailbox_keys(config::configured_dashboard_mailbox_keys(config))
    .with_readiness(store);
    if let Err(error) = heartbeat.publish_current().await {
        tracing::warn!(error = %error, "admin heartbeat ready publish failed");
    }
    Ok((state, Some(heartbeat)))
}
