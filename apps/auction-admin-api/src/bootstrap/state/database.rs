//! Admin API 配置数据库后的状态与端口装配。

use std::sync::Arc;

use anyhow::Context;
use auction_application::admin::{
    AdminAuctionCreatePort, AdminAuctionCreateService, AdminAuctionReader,
    AdminAuctionSchedulePort, AdminAuctionScheduleService, AdminAuditPort,
    AdminCredentialCheckPort, AdminCredentialCheckService, AdminCredentialHealthReader,
    AdminCredentialOpener, AdminCredentialPort, AdminCredentialService, AdminIntakePort,
    AdminIntakeService, AdminLotReader, AdminLotUpdatePort, AdminLotUpdateService, AdminMailPort,
    AdminProxyEventReader, AdminRenameReader, AdminRulesPort, AdminRuntimeConfigPort,
    AdminRuntimeConfigService, AdminUserPort, AdminUserService, AdminWorldPort, AdminWorldService,
    DeliverySentProofService, SettlementCatalogReader,
};
use auction_application::auction::{AuctionLifecycleCommandPort, AuctionLifecyclePolicy};
use auction_application::consignment::{
    AdminListingRead, ListingCommandStore, ManageListingService,
};
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
use auction_http::{AdminApiState, AdminConsignmentCapabilities};
use auction_integrations::{
    CredentialSealer, CredentialVerifier, EquipmentKeySealer, MoogleMailLoginKeyIssuer,
    MoogleMailLoginKeyReader, build_keyring,
};
use auction_observability::{
    HealthReader, HealthService, HeartbeatHealthReader, HeartbeatPublisher,
    HeartbeatPublisherConfig, HeartbeatStore, ProcessRole, ServiceName,
};
use auction_storage::{
    DatabaseAdminMailCommands, DatabaseAdminRulesCommands, DatabaseAdminRuntimeCommands,
    DatabaseAuctionLifecycleCommands, DatabaseStore,
};
use auction_time::TimeDelta;
use auction_time::Timestamp;
use uuid::Uuid;

use super::super::config;

#[derive(Clone, Copy)]
struct SystemClock;

impl auction_application::core::Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
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
    Option<[auction_process::TaskGuard<()>; 2]>,
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
    // 审计失败按类别向共享影子框架记样本；进程级安装一次。
    auction_storage::install_audit_shadow_recorder(metrics.shadow());
    let store = Arc::new(
        DatabaseStore::connect(database_url, database_profile(config)?)
            .await
            .context("连接 Admin 数据库失败")?
            .with_database_recorder(metrics.database())
            .with_runtime_switches(switches.clone())
            .with_shadow_recorder(metrics.shadow())
            .with_migration_compatibility_mode(config.readiness.migration_compatibility_mode)
            .with_migration_readiness_recorder(metrics.migration_readiness()),
    );
    let runtime_switch_task = auction_process::TaskGuard::new(
        auction_jobs::start_runtime_switch_refresh(store.clone(), switches.clone()).await,
    );
    // 生命周期迁移创建被超提醒时用的通知模板保存在数据库：启动时先读一次，之后周期刷新。
    let mail_template_task = auction_process::TaskGuard::new(
        auction_jobs::start_mail_template_refresh(store.clone(), store.live_mail_templates()).await,
    );
    metrics.register_runtime_switch_metrics(switches.clone()).context("注册运行时开关指标失败")?;
    metrics
        .register_pool_metrics(store.pool_metrics_source())
        .context("注册 Admin 数据库连接池指标失败")?;
    metrics
        .register_audit_metrics(store.audit_metrics_source())
        .context("注册 Admin 审计指标失败")?;
    let actor = config.admin_actor().context("敏感配置缺少 admin_actor")?;
    let secret = config.admin_secret().context("敏感配置缺少 admin_secret")?;
    let lifecycle_store = Arc::clone(&store);
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
        let service = Arc::new(
            AdminRuntimeConfigService::new(commands, files, Arc::new(SystemClock))
                .with_worker_cold_configs(store.clone()),
        );
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
            // URL 来源经 Worker control 执行；Admin 不持有 Cookie、HTTP 客户端或第二套
            // 请求限速器。socket 路径无效时装配明确不可用的端口，不回退到本地直连。
            let remote: Arc<dyn auction_application::operations::WorkerRemoteReadPort> =
                if let Ok(client) = auction_integrations::WorkerRemoteReadClient::new(
                    config.worker_control_socket_path(),
                    std::time::Duration::from_secs(
                        auction_application::operations::WORKER_REMOTE_READ_TIMEOUT_SECONDS,
                    ),
                ) {
                    Arc::new(client)
                } else {
                    Arc::new(auction_application::operations::UnavailableWorkerRemoteRead)
                };
            let source = auction_integrations::WorkerEquipmentSourceReader::new(remote);
            let sealer = EquipmentKeySealer::new(Arc::clone(keyring.current()));
            let intake: Arc<dyn AdminIntakePort> = Arc::new(AdminIntakeService::new(
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
                    .context("初始化凭据获取客户端失败")?,
            );
            let opener: Arc<dyn AdminCredentialOpener> = sealer.clone();
            // 邮件 Cookie 的在线检查经 Worker control 交给 Worker 执行，与邮件循环共用同账号在途锁和
            // 账号忙熔断；socket 路径无效时装配明确不可用的端口，不回退为 Admin 直连。
            let mail_check: Arc<
                dyn auction_application::operations::WorkerMailCredentialCheckPort,
            > = if let Ok(client) = auction_integrations::WorkerRemoteReadClient::new(
                config.worker_control_socket_path(),
                std::time::Duration::from_secs(
                    auction_application::operations::WORKER_MAIL_CREDENTIAL_CHECK_TIMEOUT_SECONDS,
                ),
            ) {
                Arc::new(client)
            } else {
                Arc::new(auction_application::operations::UnavailableWorkerMailCredentialCheck)
            };
            let verifier = Arc::new(
                CredentialVerifier::production(opener, mail_check)
                    .context("初始化凭据检查客户端失败")?,
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
            started_at: Timestamp::now(),
            ttl: TimeDelta::seconds(30),
        },
        heartbeat_health,
        tracing_health,
    ));
    if let Err(error) = heartbeat.publish_starting().await {
        tracing::warn!(error = %error, "发布 Admin starting 心跳失败");
    }
    let session_store: Arc<dyn AdminSessionStore> = store.clone();
    let settlement_report: Arc<dyn SettlementReportReader> = store.clone();
    let settlement_catalog: Arc<dyn SettlementCatalogReader> = store.clone();
    let recovery_report: Arc<dyn RecoveryReportReader> = store.clone();
    let admin_auctions: Arc<dyn AdminAuctionReader> = store.clone();
    let admin_worlds: Arc<dyn AdminWorldPort> =
        Arc::new(AdminWorldService::new(Arc::clone(&store)));
    let admin_lots: Arc<dyn AdminLotReader> = store.clone();
    let admin_lot_update: Arc<dyn AdminLotUpdatePort> =
        Arc::new(AdminLotUpdateService::new(Arc::clone(&store), Arc::new(SystemClock)));
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
        anyhow::bail!("Admin 会话配置不可用");
    };
    let session_config = AdminSessionConfig {
        idle_ttl: TimeDelta::seconds(session_settings.idle_ttl_seconds as i64),
        absolute_ttl: TimeDelta::seconds(session_settings.absolute_ttl_seconds as i64),
    };
    let session: Arc<dyn AdminSessionPort> = Arc::new(
        AdminSessionService::new(
            session_store,
            actor.to_owned(),
            secret.as_bytes().to_vec(),
            session_config,
            Timestamp::now(),
        )
        .await
        .context("初始化 Admin 会话授权失败")?,
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
    let auction_create: Arc<dyn AdminAuctionCreatePort> =
        Arc::new(AdminAuctionCreateService::new(Arc::clone(&store), config.auction.policy()?));
    let auction_schedule: Arc<dyn AdminAuctionSchedulePort> =
        Arc::new(AdminAuctionScheduleService::new(Arc::clone(&store)));
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
    .with_settlement_returns(store.clone())
    .with_dashboard_worlds(config::configured_dashboard_worlds(config))
    .with_dashboard_mailbox_keys(config::configured_dashboard_mailbox_keys(config))
    .with_runtime_switches(store.clone(), switches.clone())
    .with_mail_templates(store.clone())
    .with_runtime_switch_snapshot(switches.clone())
    .with_readiness(Arc::new(
        auction_http::HttpReadinessProbe::new(
            store.clone(),
            config.readiness.cache_enabled,
            metrics.readiness(),
        )
        .with_runtime_switches(switches),
    ));
    let state = configure_consignment(state, config, store);
    if let Err(error) = heartbeat.publish_current().await {
        tracing::warn!(error = %error, "发布 Admin ready 心跳失败");
    }
    Ok((state, Some(heartbeat), Some([runtime_switch_task, mail_template_task])))
}

fn configure_consignment<S: AdminListingRead + ListingCommandStore + 'static>(
    state: AdminApiState,
    config: &AppConfig,
    store: Arc<S>,
) -> AdminApiState {
    // 不变量 1、6：停用仍需读取历史与仲裁原下架命令，首次操作由持久有效态约束。
    state.with_consignment(AdminConsignmentCapabilities {
        read: Some(store.clone()),
        manage: Some(Arc::new(ManageListingService::new(store, config.consignment.enabled()))),
    })
}

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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::SocketAddr;
    use std::sync::Arc;

    use auction_application::admin::EquipmentKeySealerPort;
    use auction_application::consignment::{
        AdminDelistListingCommand, AdminListingDetail, AdminListingQuery, AdminListingRead,
        AdminListingRecord, ApplyListingLookupCommand, AutoCloseListingCommand, ConsignmentFuture,
        ListingAutoCloseEffect, ListingClosed, ListingCommandOutcome, ListingCommandStore,
        ListingLookupEffect, ListingPriceReceipt, ListingReadPage, ListingRegistered,
        RegisterListingCommand, RegistrationReplayCrypto, RepriceListingCommand,
        StoredListingDecision, WithdrawListingCommand,
    };
    use auction_application::core::ApplicationFuture;
    use auction_application::public::{AdminLoginResult, AdminSessionIdentity, AdminSessionPort};
    use auction_config::{AppConfig, Role};
    use auction_domain::consignment::ListingVersion;
    use auction_http::AdminApiState;
    use auction_time::Timestamp;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::configure_consignment;
    use crate::bootstrap::config::configure_security;
    use crate::bootstrap::service::configure_router;

    const LIST: &str = "/api/v1/admin/consignment/listings";
    const LISTING: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0001);
    const COMMAND: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0002);
    const ORIGIN: &str = "https://admin.example.invalid";
    const ACTOR: &str = "fixture-admin";
    const REASON: &str = "运营夹具下架";

    #[tokio::test]
    async fn consignment_disabled_preserves_admin_list_and_delist_replay() {
        let config = disabled_config();
        let state = AdminApiState::new(
            Some(Arc::new(Session)),
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
            None,
            None,
            None,
            None,
            None,
            None,
            configure_security(&config).unwrap(),
        );
        let app =
            configure_router(configure_consignment(state, &config, Arc::new(ConsignmentStore)));
        let page = send(&app, "GET", &format!("{LIST}?state=closed&page_size=37"), None).await;
        assert_eq!(page["data"]["items"], json!([]));
        assert_eq!(page["data"]["page_size"], 37);
        let receipt = send(
            &app,
            "POST",
            &format!("{LIST}/{LISTING}/delist"),
            Some(json!({"command_id": COMMAND, "expected_version": 7, "reason": REASON})),
        )
        .await;
        assert_eq!(receipt["data"]["command_id"], COMMAND.to_string());
        assert_eq!(receipt["data"]["listing_id"], LISTING.to_string());
        assert_eq!(receipt["data"]["operation"], "admin_delist");
        assert_eq!(receipt["data"]["listing_version"], 8);
        assert_eq!(receipt["data"]["state"], "closed");
    }

    fn disabled_config() -> AppConfig {
        let directory =
            std::env::temp_dir().join(format!("auction-admin-consignment-{}", Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let role_path = directory.join("admin.toml");
        fs::write(&role_path, "common_file = 'common.toml'\nsecrets_file = 'secrets.toml'\n")
            .unwrap();
        fs::write(directory.join("common.toml"), "[consignment]\nenabled = false\n").unwrap();
        fs::write(directory.join("secrets.toml"), "").unwrap();
        let config = AppConfig::load_from_path(Role::Admin, &role_path);
        fs::remove_dir_all(directory).unwrap();
        let mut config = config.unwrap();
        config.http_security.allowed_origins = vec![ORIGIN.to_owned()];
        config
    }

    async fn send(app: &Router, method: &str, uri: &str, input: Option<Value>) -> Value {
        let mut request = Request::builder().method(method).uri(uri).header(
            "cookie",
            "auction_admin_session=fixture-session; auction_admin_csrf=fixture-csrf",
        );
        let body = match input {
            Some(input) => {
                request = request
                    .header("origin", ORIGIN)
                    .header("x-csrf-token", "fixture-csrf")
                    .header("content-type", "application/json");
                Body::from(input.to_string())
            }
            None => Body::empty(),
        };
        let mut request = request.body(body).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 22], 3123))));
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 8192).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{method} {uri}: {}", String::from_utf8_lossy(&body));
        serde_json::from_slice(&body).unwrap()
    }

    struct Session;

    impl AdminSessionPort for Session {
        fn login(
            &self,
            _actor: String,
            _secret: Vec<u8>,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, AdminLoginResult> {
            panic!("测试不登录")
        }

        fn current(
            &self,
            token: String,
            csrf: String,
            now: Timestamp,
        ) -> ApplicationFuture<'_, AdminSessionIdentity> {
            assert_eq!(token, "fixture-session");
            assert_eq!(csrf, "fixture-csrf");
            Box::pin(async move {
                Ok(AdminSessionIdentity {
                    actor: ACTOR.to_owned(),
                    expires_at: now,
                    absolute_expires_at: now,
                })
            })
        }

        fn touch(
            &self,
            token: String,
            csrf: String,
            now: Timestamp,
        ) -> ApplicationFuture<'_, AdminSessionIdentity> {
            self.current(token, csrf, now)
        }

        fn logout(&self, _token: String, _now: Timestamp) -> ApplicationFuture<'_, ()> {
            panic!("测试不退出")
        }
    }

    struct ConsignmentStore;

    impl AdminListingRead for ConsignmentStore {
        fn list_admin(
            &self,
            query: AdminListingQuery,
        ) -> ConsignmentFuture<'_, ListingReadPage<AdminListingRecord>> {
            assert_eq!(query.listings.window.page_size(), 37);
            Box::pin(async move {
                Ok(ListingReadPage { items: vec![], total: 0, window: query.listings.window })
            })
        }

        fn admin_detail(
            &self,
            _listing_id: Uuid,
        ) -> ConsignmentFuture<'_, Option<AdminListingDetail>> {
            panic!("测试不读详情")
        }
    }

    impl ListingCommandStore for ConsignmentStore {
        fn register<'a>(
            &'a self,
            _command: RegisterListingCommand<'a>,
            _sealer: Option<&'a dyn EquipmentKeySealerPort>,
            _replay_crypto: Option<&'a dyn RegistrationReplayCrypto>,
        ) -> ConsignmentFuture<'a, ListingCommandOutcome<ListingRegistered>> {
            panic!("Admin 不登记")
        }

        fn reprice(
            &self,
            _command: RepriceListingCommand,
        ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingPriceReceipt>> {
            panic!("Admin 不改价")
        }

        fn withdraw(
            &self,
            _command: WithdrawListingCommand,
        ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
            panic!("Admin 不取消")
        }

        fn apply_lookup(
            &self,
            _command: ApplyListingLookupCommand,
        ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingLookupEffect>> {
            panic!("Admin 不解析")
        }

        fn auto_close(
            &self,
            _command: AutoCloseListingCommand,
        ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingAutoCloseEffect>> {
            panic!("Admin 不清退")
        }

        fn admin_delist(
            &self,
            command: AdminDelistListingCommand,
        ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
            assert_eq!(command.admin_actor, ACTOR);
            assert_eq!(command.command_id.as_uuid(), COMMAND);
            assert_eq!(command.listing_id, LISTING);
            assert_eq!(command.expected_version.value(), 7);
            assert_eq!(command.reason.as_str(), REASON);
            Box::pin(async {
                let receipt = ListingClosed::new(LISTING, ListingVersion::new(8).unwrap(), None)?;
                Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(receipt)))
            })
        }
    }
}
