//! Public API 的数据库、端口、heartbeat 和 realtime 状态装配。

use std::sync::Arc;
use std::sync::{
    RwLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use anyhow::Context;
use auction_application::admin::EquipmentKeySealerPort;
use auction_application::auction::{
    AuctionReader, CompetitionCommandPort, PublicAccountReader, PublicBidLogReader,
    PublicLotReader, PublicPriceHistoryReader, WorldReader,
};
use auction_application::consignment::{
    DepositCommandService, DepositRead, ListingCommandStore, ListingRegistrationPolicy,
    ListingWorlds, ManageListingService, OwnedListingRead, PublicListingRead,
    RegisterListingService, RegistrationReplayCrypto,
};
use auction_application::core::ApplicationFuture;
use auction_application::core::{ApplicationError, RepositoryError};
use auction_application::forum::PublicForumReader;
use auction_application::operations::ProcessReadinessPort;
use auction_application::public::{
    PublicBlacklistReader, PublicCompensationReader, PublicEquipmentLinkResolver,
    PublicEquipmentLinkSecretReader, PublicFulfillmentReader, PublicLoginAuthenticator,
    PublicRulesReader, PublicSessionConfig, PublicSessionPort, PublicSessionStore,
};
use auction_application::realtime::{
    PublicRealtimeReader, RealtimeConsumerStore, RealtimeEventSink, RealtimeRelay,
    RealtimeRelayConfig, RealtimeSequenceAllocator, RealtimeStreamStore,
};
use auction_application::sessions::PublicSessionService;
use auction_config::{AppConfig, ConsignmentActivation, ConsignmentConfig, DataKeyDomain};
use auction_http::{ConsignmentCapabilities, PublicApiState, RealtimeHub, RealtimeHubSink};
use auction_integrations::{
    ConsignmentReservationCrypto, EquipmentGameLinkResolver, EquipmentKeyOpener,
    EquipmentKeySealer, HentaiVerseListingUrlValidator, Keyring, build_keyring,
    consignment_reservation_crypto,
};
use auction_observability::ProcessMetrics;
use auction_observability::{
    HeartbeatHealthReader, HeartbeatPublisher, HeartbeatPublisherConfig, HeartbeatReadinessFuture,
    HeartbeatStore, ProcessRole, ServiceName,
};
use auction_storage::{
    DatabaseCompetitionCommands, DatabaseStore, DepositRepository, DepositSettings,
};
use auction_time::Timestamp;
use uuid::Uuid;

use super::config;

struct PublicRealtimeRelayHealth {
    healthy: AtomicBool,
    last_success: RwLock<Option<std::time::Instant>>,
    stale_after: Duration,
}

impl PublicRealtimeRelayHealth {
    fn new(poll_interval: Duration) -> Self {
        let stale_after = poll_interval.checked_mul(4).unwrap_or(Duration::from_secs(30));
        Self {
            healthy: AtomicBool::new(false),
            last_success: RwLock::new(None),
            stale_after: stale_after.max(Duration::from_secs(5)),
        }
    }

    fn mark_success(&self) {
        if let Ok(mut last_success) = self.last_success.write() {
            *last_success = Some(std::time::Instant::now());
            self.healthy.store(true, Ordering::Release);
        } else {
            self.healthy.store(false, Ordering::Release);
        }
    }

    fn mark_failure(&self) {
        self.healthy.store(false, Ordering::Release);
    }

    fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
            && self
                .last_success
                .read()
                .ok()
                .and_then(|last_success| *last_success)
                .is_some_and(|last_success| last_success.elapsed() <= self.stale_after)
    }
}

struct PublicProcessHealth {
    database: Arc<DatabaseStore>,
    relay: Arc<PublicRealtimeRelayHealth>,
}

impl ProcessReadinessPort for PublicProcessHealth {
    fn check(&self) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            self.database.check().await?;
            if !self.relay.is_healthy() {
                return Err(ApplicationError::Repository(RepositoryError::Failed(
                    "Public realtime relay 尚未取得新鲜成功证明".to_owned(),
                )));
            }
            Ok(())
        })
    }
}

impl HeartbeatHealthReader for PublicProcessHealth {
    fn readiness(&self) -> HeartbeatReadinessFuture<'_> {
        Box::pin(async move { self.database.readiness().await && self.relay.is_healthy() })
    }

    fn audit_failure_count(&self) -> i64 {
        self.database.audit_failure_count()
    }
}

pub(super) struct ConfiguredRealtimeRelay {
    relay: Arc<RealtimeRelay>,
    health: Arc<PublicRealtimeRelayHealth>,
}

struct RealtimeRelayTaskHealthGuard {
    health: Arc<PublicRealtimeRelayHealth>,
}

impl Drop for RealtimeRelayTaskHealthGuard {
    fn drop(&mut self) {
        // panic、意外返回或 shutdown abort 都立即撤销任务健康证明，不等待新鲜度窗口自然过期。
        self.health.mark_failure();
    }
}

fn equipment_keyring(config: &AppConfig) -> anyhow::Result<Option<Arc<Keyring>>> {
    build_keyring(
        DataKeyDomain::Equipment,
        config.data_key_current(DataKeyDomain::Equipment),
        config
            .data_key_previous(DataKeyDomain::Equipment)
            .iter()
            .map(|key| (key.id(), key.material())),
    )
    .context("Public 装备数据密钥配置无效")
}

fn credential_keyring(config: &AppConfig) -> anyhow::Result<Option<Arc<Keyring>>> {
    build_keyring(
        DataKeyDomain::Credential,
        config.data_key_current(DataKeyDomain::Credential),
        config
            .data_key_previous(DataKeyDomain::Credential)
            .iter()
            .map(|key| (key.id(), key.material())),
    )
    .context("Public 凭据数据密钥配置无效")
}

/// 寄售激活时加载预约码所需的凭据域密钥环；未激活时不读取密钥，激活后缺失即拒绝启动。
fn reservation_crypto(
    config: &AppConfig,
    activation: ConsignmentActivation,
) -> anyhow::Result<Option<Arc<ConsignmentReservationCrypto>>> {
    let enabled = activation == ConsignmentActivation::Enabled;
    let keyring = if enabled { credential_keyring(config)? } else { None };
    consignment_reservation_crypto(enabled, keyring).map_err(anyhow::Error::from)
}

struct DepositAssembly {
    enabled: bool,
    read: Option<Arc<dyn DepositRead>>,
    commands: Option<Arc<DepositCommandService>>,
}

/// 构造 v2 收件端口；存储不回退硬编码费率、价格边界、额度或预约有效期。
fn configure_deposit(
    config: &ConsignmentConfig,
    activation: ConsignmentActivation,
    store: Option<Arc<DatabaseStore>>,
    crypto: Option<Arc<ConsignmentReservationCrypto>>,
) -> anyhow::Result<DepositAssembly> {
    let enabled = activation == ConsignmentActivation::Enabled;
    let Some(store) = store.filter(|_store| enabled) else {
        return Ok(DepositAssembly { enabled, read: None, commands: None });
    };
    let crypto = crypto.ok_or_else(|| anyhow::anyhow!("寄售已启用，但预约码加密能力不可用"))?;
    let settings = DepositSettings::with_standard_price_bounds(
        config.fee_bps(),
        config.reservation_ttl_hours(),
        config.max_open_per_user(),
    )
    .context("寄售收件设置无效")?;
    let repository = Arc::new(DepositRepository::new((*store).clone(), crypto, settings));
    let commands = Arc::new(DepositCommandService::new(
        repository.clone(),
        repository.clone(),
        repository.clone(),
        repository.clone(),
        repository.clone(),
    ));
    Ok(DepositAssembly { enabled, read: Some(repository), commands: Some(commands) })
}

struct ConsignmentCredentials {
    sealer: Arc<dyn EquipmentKeySealerPort>,
    replay_crypto: Arc<dyn RegistrationReplayCrypto>,
}

impl ConsignmentCredentials {
    fn from_keyring(keyring: Option<Arc<Keyring>>) -> Option<Self> {
        keyring.map(|keyring| Self {
            sealer: Arc::new(EquipmentKeySealer::new(Arc::clone(keyring.current()))),
            replay_crypto: Arc::new(EquipmentKeyOpener::new(keyring)),
        })
    }
}

fn configure_consignment<
    S: PublicListingRead + OwnedListingRead + ListingCommandStore + 'static,
>(
    config: &ConsignmentConfig,
    activation: ConsignmentActivation,
    store: Option<&Arc<S>>,
    credentials: Option<ConsignmentCredentials>,
) -> anyhow::Result<ConsignmentCapabilities> {
    let enabled = activation == ConsignmentActivation::Enabled;
    let fee_bps = config.fee_bps();
    let Some(store) = store else {
        return Ok(ConsignmentCapabilities::without_database(enabled, fee_bps));
    };
    let sealer = credentials.as_ref().map(|credentials| Arc::clone(&credentials.sealer));
    let replay_crypto = credentials.map(|credentials| credentials.replay_crypto);
    let policy = ListingRegistrationPolicy {
        enabled: config.enabled(),
        // 决定 4 对两世界开放，寄售节没有独立世界配置。实际准入由数据库的世界可用性再收紧。
        worlds: ListingWorlds { main: true, isekai: true },
        consigned_fee_bps: fee_bps,
        max_open_per_user: config.max_open_per_user().try_into().context("寄售活动挂单额度无效")?,
        registrations_per_hour: config
            .registrations_per_hour()
            .try_into()
            .context("寄售每小时登记额度无效")?,
    };
    let registration_sealer_available = sealer.is_some();
    // 不变量 1、6：停用仍交给原命令账本仲裁，首次登记与改价受有效开关约束。
    Ok(ConsignmentCapabilities {
        enabled,
        fee_bps,
        public_read: Some(store.clone()),
        owned_read: Some(store.clone()),
        register: Some(Arc::new(RegisterListingService::new(
            store.clone(),
            Arc::new(HentaiVerseListingUrlValidator),
            sealer,
            replay_crypto.clone(),
            policy,
            enabled,
            Uuid::new_v4,
        ))),
        manage: Some(Arc::new(ManageListingService::new(store.clone(), enabled))),
        registration_sealer_available,
        replay_crypto,
    })
}

pub(super) async fn configure_state(
    config: &AppConfig,
    activation: ConsignmentActivation,
    tracing_health: Arc<dyn auction_observability::TracingHealthReader>,
    metrics: Arc<ProcessMetrics>,
    switches: auction_application::runtime_switches::RuntimeSwitches,
) -> anyhow::Result<(
    PublicApiState,
    Option<Arc<HeartbeatPublisher>>,
    Option<Arc<ConfiguredRealtimeRelay>>,
    Option<[auction_process::TaskGuard<()>; 2]>,
)> {
    let security = config::configure_security(config)?;
    let Some(database_url) = config.database_url() else {
        // 骨架模式（未配置数据库）不构造收件端口，也就不要求凭据域密钥：v2 能力返回不可用。
        let state = PublicApiState::new(
            None, None, None, None, None, None, None, None, None, None, None, None, None, None,
            security,
        )
        .with_runtime_switch_snapshot(switches);
        let consignment =
            configure_consignment::<DatabaseStore>(&config.consignment, activation, None, None)?;
        let deposit = configure_deposit(&config.consignment, activation, None, None)?;
        return Ok((
            state.with_consignment(consignment).with_deposit(
                deposit.enabled,
                deposit.read,
                deposit.commands,
            ),
            None,
            None,
            None,
        ));
    };
    // 不变量 7：凭据域密钥只用于预约码信封；配置了数据库且寄售激活时缺失或无效即启动失败，
    // 不回显密钥材料。
    let deposit_crypto = reservation_crypto(config, activation)?;

    // 审计失败按类别向共享影子框架记样本；进程级安装一次。
    auction_storage::install_audit_shadow_recorder(metrics.shadow());
    let store = Arc::new(
        DatabaseStore::connect(database_url, database_profile(config)?)
            .await
            .context("连接 Public 数据库失败")?
            .with_database_recorder(metrics.database())
            .with_runtime_switches(switches.clone())
            .with_migration_compatibility_mode(config.readiness.migration_compatibility_mode)
            .with_migration_readiness_recorder(metrics.migration_readiness()),
    );
    let runtime_switch_task = auction_process::TaskGuard::new(
        auction_jobs::start_runtime_switch_refresh(store.clone(), switches.clone()).await,
    );
    // 出价事务创建被超提醒时用的通知模板保存在数据库：启动时先读一次，之后周期刷新。
    let mail_template_task = auction_process::TaskGuard::new(
        auction_jobs::start_mail_template_refresh(store.clone(), store.live_mail_templates()).await,
    );
    metrics.register_runtime_switch_metrics(switches.clone()).context("注册运行时开关指标失败")?;
    metrics
        .register_pool_metrics(store.pool_metrics_source())
        .context("注册 Public 数据库连接池指标失败")?;
    metrics
        .register_audit_metrics(store.audit_metrics_source())
        .context("注册 Public 审计指标失败")?;
    let poll_interval = Duration::from_millis(config.realtime.poll_interval_millis);
    let relay_health = Arc::new(PublicRealtimeRelayHealth::new(poll_interval));
    let process_health = Arc::new(PublicProcessHealth {
        database: Arc::clone(&store),
        relay: Arc::clone(&relay_health),
    });
    let heartbeat_store: Arc<dyn HeartbeatStore> = store.clone();
    let heartbeat_health: Arc<dyn HeartbeatHealthReader> = process_health.clone();
    let heartbeat = Arc::new(HeartbeatPublisher::new(
        heartbeat_store,
        HeartbeatPublisherConfig {
            service_name: ServiceName::Public,
            process_role: ProcessRole::Public,
            instance_id: Uuid::new_v4().to_string(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            started_at: Timestamp::now(),
            ttl: auction_time::Duration::seconds(30),
        },
        heartbeat_health,
        tracing_health,
    ));
    if let Err(error) = heartbeat.publish_starting().await {
        tracing::warn!(error = %error, "发布 Public starting 心跳失败");
    }
    let worlds: Arc<dyn WorldReader> = store.clone();
    let rules: Arc<dyn PublicRulesReader> = store.clone();
    let blacklist: Arc<dyn PublicBlacklistReader> = store.clone();
    let compensation: Arc<dyn PublicCompensationReader> = store.clone();
    let fulfillment: Arc<dyn PublicFulfillmentReader> = store.clone();
    let forum: Arc<dyn PublicForumReader> = store.clone();
    let auctions: Arc<dyn AuctionReader> = store.clone();
    let lots: Arc<dyn PublicLotReader> =
        Arc::new(store.public_lot_reader(config.equipment_colors.clone()));
    let keyring = equipment_keyring(config)?;
    let equipment_links: Option<Arc<dyn PublicEquipmentLinkResolver>> =
        keyring.as_ref().map(|keyring| {
            let secrets: Arc<dyn PublicEquipmentLinkSecretReader> = store.clone();
            let resolver: Arc<dyn PublicEquipmentLinkResolver> =
                Arc::new(EquipmentGameLinkResolver::new(
                    secrets,
                    EquipmentKeyOpener::new(Arc::clone(keyring)),
                ));
            resolver
        });
    let credentials = ConsignmentCredentials::from_keyring(keyring);
    // 不变量 1、2、7：初始化由仓储锁定持久准入并仲裁，故障只记录安全分类。
    // Worker 尚未同步或初始化失败时保留历史能力，登记就绪和首次写入各自重新核验依赖。
    if activation == ConsignmentActivation::Enabled
        && let Some(credentials) = &credentials
        && let Err(error) = store
            .initialize_listing_replay_key(Uuid::new_v4(), credentials.replay_crypto.as_ref())
            .await
    {
        tracing::warn!(error = %error, "Public 启动时未能确认寄售登记比较密钥");
    }
    let bid_log: Arc<dyn PublicBidLogReader> = store.clone();
    let price_history: Arc<dyn PublicPriceHistoryReader> = store.clone();
    let realtime: Arc<dyn PublicRealtimeReader> = store.clone();
    let account: Arc<dyn PublicAccountReader> = store.clone();
    let commands: Arc<dyn CompetitionCommandPort> =
        Arc::new(DatabaseCompetitionCommands::new(store.clone()));
    let authenticator: Arc<dyn PublicLoginAuthenticator> = store.clone();
    let session_store: Arc<dyn PublicSessionStore> = store.clone();
    let auction_config::RoleSessionConfig::Public(session_settings) = config.session else {
        anyhow::bail!("Public 会话配置不可用");
    };
    let session_config =
        PublicSessionConfig { max_public_sessions: session_settings.max_public_sessions };
    let session: Arc<dyn PublicSessionPort> =
        Arc::new(PublicSessionService::new(authenticator, session_store, session_config)?);
    let realtime_metrics = auction_observability::RealtimeRecorder::register(&metrics)
        .context("注册 Public 实时延迟指标失败")?;
    let hub = RealtimeHub::new(config.realtime.client_buffer)
        .map_err(anyhow::Error::msg)?
        .with_metrics(realtime_metrics);
    let hub = Arc::new(hub);
    let sink: Arc<dyn RealtimeEventSink> = Arc::new(RealtimeHubSink::new(Arc::clone(&hub)));
    let ownership = Arc::new(
        store
            .acquire_realtime_consumer_ownership(&config.realtime.consumer_key)
            .await
            .context("取得 Public 实时消费者所有权失败")?,
    );
    // 目录扫描和游标写入共用持锁 session；断线后不能回退到普通连接池继续消费。
    let stream_store: Arc<dyn RealtimeStreamStore> = ownership.clone();
    let consumer_store: Arc<dyn RealtimeConsumerStore> = ownership;
    // 竞价事件的序号在提交后分配；relay 每轮用普通池连接兜底补齐，不占用持锁 session。
    let sequence_allocator: Arc<dyn RealtimeSequenceAllocator> = store.clone();
    let realtime_relay = Arc::new(
        RealtimeRelay::new(
            stream_store,
            consumer_store,
            sink,
            sequence_allocator,
            RealtimeRelayConfig {
                consumer_key: config.realtime.consumer_key.clone(),
                stream_prefix: "auction:".to_owned(),
                stream_batch_size: config.realtime.stream_batch_size,
                event_batch_size: config.realtime.event_batch_size,
            },
        )
        .map_err(anyhow::Error::msg)?,
    );
    let realtime_relay =
        Arc::new(ConfiguredRealtimeRelay { relay: realtime_relay, health: relay_health });
    let readiness: Arc<dyn ProcessReadinessPort> = Arc::new(
        auction_http::HttpReadinessProbe::new(
            process_health,
            config.readiness.cache_enabled,
            metrics.readiness(),
        )
        .with_runtime_switches(switches.clone()),
    );
    let state = PublicApiState::new(
        Some(worlds),
        Some(rules),
        Some(blacklist),
        Some(compensation),
        Some(forum),
        Some(auctions),
        Some(lots),
        equipment_links,
        Some(bid_log),
        Some(price_history),
        Some(realtime),
        Some(account),
        Some(commands),
        Some(session),
        security,
    )
    .with_fulfillment(fulfillment)
    .with_auction_summary(store.clone())
    .with_lot_watches(store.clone())
    .with_lot_watch_preference(store.clone())
    .with_realtime_hub(hub)
    .with_runtime_switch_snapshot(switches)
    .with_readiness(readiness);
    let consignment =
        configure_consignment(&config.consignment, activation, Some(&store), credentials)?;
    let deposit = configure_deposit(
        &config.consignment,
        activation,
        Some(Arc::clone(&store)),
        deposit_crypto,
    )?;
    if let Err(error) = heartbeat.publish_current().await {
        tracing::warn!(error = %error, "发布 Public ready 心跳失败");
    }
    Ok((
        state.with_consignment(consignment).with_deposit(
            deposit.enabled,
            deposit.read,
            deposit.commands,
        ),
        Some(heartbeat),
        Some(realtime_relay),
        Some([runtime_switch_task, mail_template_task]),
    ))
}

pub(super) fn spawn_realtime_relay(
    configured: Arc<ConfiguredRealtimeRelay>,
    poll_interval: std::time::Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let _task_health_guard =
            RealtimeRelayTaskHealthGuard { health: Arc::clone(&configured.health) };
        let mut interval = tokio::time::interval(poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            match configured.relay.run_once(Timestamp::now()).await {
                Ok(batch) if batch.delivered > 0 || batch.failures > 0 => {
                    if batch.failures == 0 {
                        configured.health.mark_success();
                    } else {
                        configured.health.mark_failure();
                    }
                    tracing::debug!(
                        streams = batch.streams,
                        delivered = batch.delivered,
                        failures = batch.failures,
                        "Public 实时中继批次已完成"
                    );
                }
                Ok(_) => configured.health.mark_success(),
                Err(error) => {
                    configured.health.mark_failure();
                    tracing::warn!(error = %error, "Public 实时中继批次失败");
                }
            }
        }
    })
}

pub(super) fn spawn_heartbeat(publisher: Arc<HeartbeatPublisher>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        // 心跳只报告当前状态；数据库恢复后不补发过期 tick，避免集中查询和写入。
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(error) = publisher.publish_current().await {
                tracing::warn!(error = %error, "发布 Public 心跳失败");
            }
        }
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
    use std::sync::{Arc, Mutex};

    use auction_application::admin::EquipmentKeySealerPort;
    use auction_application::consignment::{
        AdminDelistListingCommand, ApplyListingLookupCommand, AutoCloseListingCommand,
        ConsignmentError, ConsignmentFuture, ListingAutoCloseEffect, ListingClosed,
        ListingCommandOutcome, ListingCommandStore, ListingEquipmentUrlValidator,
        ListingLookupEffect, ListingPriceDisposition, ListingPriceReceipt, ListingReadPage,
        ListingRegistered, ListingRegistrationAvailability, ListingRegistrationPolicy,
        ListingWorlds, OwnedListingCommandReceipt, OwnedListingDecision, OwnedListingRead,
        OwnedListingRecord, PrivateListingQuery, PublicListingDetail, PublicListingQuery,
        PublicListingRead, PublicListingSummary, RegisterListingCommand, RegistrationReplayCrypto,
        RepriceListingCommand, StoredListingDecision, WithdrawListingCommand,
    };
    use auction_application::core::ApplicationFuture;
    use auction_application::public::{
        PublicLoginResult, PublicSessionIdentity, PublicSessionPort, PublicSessionPreferenceResult,
        SetPublicSessionPreferenceCommand,
    };
    use auction_application::runtime_switches::{RuntimeSwitchRole, RuntimeSwitches};
    use auction_config::{AppConfig, ConsignmentActivation, DataKeyDomain, Role};
    use auction_domain::consignment::{ListingPrice, ListingState};
    use auction_domain::{CommandId, UserId};
    use auction_http::PublicApiState;
    use auction_integrations::{
        EquipmentKeyOpener, HentaiVerseListingUrlValidator, Keyring, build_keyring,
    };
    use auction_observability::{ProcessMetrics, TracingHealth, TracingHealthReader};
    use auction_storage::DatabaseStore;
    use auction_time::Timestamp;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::{
        ConsignmentCredentials, configure_consignment, configure_state, equipment_keyring,
    };
    use crate::bootstrap::config::configure_security;
    use crate::bootstrap::service::configure_router;

    const STATUS: &str = "/api/v1/consignment/status";
    const REGISTER: &str = "/api/v1/consignment/listings";
    const OWNED: &str = "/api/v1/me/consignment/listings";
    const SELLER: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0001);
    const LISTING: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0002);
    const REGISTER_COMMAND: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0003);
    const REPRICE_COMMAND: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0004);
    const ORIGIN: &str = "https://public.example.invalid";

    fn load_config(enabled: bool) -> AppConfig {
        let directory =
            std::env::temp_dir().join(format!("auction-public-consignment-{}", Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let role_path = directory.join("public.toml");
        fs::write(&role_path, "common_file = 'common.toml'\nsecrets_file = 'secrets.toml'\n")
            .unwrap();
        fs::write(
            directory.join("common.toml"),
            format!(
                "[consignment]\nenabled = {enabled}\nfee_bps = 175\nmax_open_per_user = 13\nregistrations_per_hour = 7\n"
            ),
        )
        .unwrap();
        fs::write(directory.join("secrets.toml"), "").unwrap();
        let config = AppConfig::load_from_path(Role::Public, &role_path);
        fs::remove_dir_all(directory).unwrap();
        let mut config = config.unwrap();
        config.http_security.allowed_origins = vec![ORIGIN.to_owned()];
        config
    }

    struct UnusedTracingHealth;

    impl TracingHealthReader for UnusedTracingHealth {
        fn snapshot(&self) -> TracingHealth {
            panic!("无数据库骨架不读取 tracing 健康快照")
        }
    }

    #[tokio::test]
    async fn consignment_without_database_reports_configured_status() {
        for (enabled, activation, expected_status, expected_code) in [
            (
                true,
                ConsignmentActivation::Enabled,
                StatusCode::SERVICE_UNAVAILABLE,
                "consignment_unavailable",
            ),
            (false, ConsignmentActivation::Disabled, StatusCode::NOT_FOUND, "consignment_disabled"),
        ] {
            let config = load_config(enabled);
            let metrics = Arc::new(
                ProcessMetrics::new(false, "auction_public_api", "public", "test").unwrap(),
            );
            let switches = RuntimeSwitches::new(
                RuntimeSwitchRole::Public,
                config.readiness.audit_failure_mode,
                config.readiness.cache_enabled,
            );
            let (state, _, _, _) = configure_state(
                &config,
                activation,
                Arc::new(UnusedTracingHealth),
                metrics,
                switches,
            )
            .await
            .unwrap();
            let app = configure_router(state);
            let status = send(&app, "GET", STATUS, None, StatusCode::OK).await;
            assert_eq!(status["data"]["enabled"], enabled);
            assert_eq!(status["data"]["fee_bps"], 175);
            assert_eq!(status["data"]["registration_ready"], false);

            let catalog = send(&app, "GET", REGISTER, None, expected_status).await;
            assert_eq!(catalog["code"], expected_code);

            let capabilities =
                configure_consignment::<DatabaseStore>(&config.consignment, activation, None, None)
                    .unwrap();
            assert!(capabilities.public_read.is_none());
            assert!(capabilities.owned_read.is_none());
        }
    }

    #[tokio::test]
    async fn consignment_without_equipment_key_reports_unready_and_rejects_first_registration() {
        let config = load_config(true);
        let store = Arc::new(ConsignmentStore::default());
        let app = router_with_store(
            &config,
            ConsignmentActivation::Enabled,
            &store,
            equipment_keyring(&config).unwrap(),
        );
        let status = send(&app, "GET", STATUS, None, StatusCode::OK).await;
        assert_eq!(status["data"]["enabled"], true);
        assert_eq!(status["data"]["registration_ready"], false);
        let failure =
            send(&app, "POST", REGISTER, Some(registration()), StatusCode::SERVICE_UNAVAILABLE)
                .await;
        assert_eq!(failure["code"], "consignment_unavailable");
        let calls = store.registrations.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(!calls[0].sealer && !calls[0].replay_crypto);
    }

    #[tokio::test]
    async fn consignment_registration_passes_configured_policy_and_credentials() {
        let config = load_config(true);
        let store = Arc::new(ConsignmentStore::default());
        let app = router_with_store(
            &config,
            ConsignmentActivation::Enabled,
            &store,
            Some(fixture_equipment_keyring()),
        );
        let status = send(&app, "GET", STATUS, None, StatusCode::OK).await;
        assert_eq!(status["data"]["registration_ready"], true);
        assert_eq!(status["data"]["fee_bps"], 175);
        send(&app, "POST", REGISTER, Some(registration()), StatusCode::ACCEPTED).await;
        let calls = store.registrations.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert!(call.sealer && call.replay_crypto);
        assert_eq!(call.world, "isekai");
        assert!(call.policy.enabled);
        assert_eq!(call.policy.worlds, ListingWorlds { main: true, isekai: true });
        assert_eq!(call.policy.consigned_fee_bps.value(), 175);
        assert_eq!(call.policy.max_open_per_user, 13);
        assert_eq!(call.policy.registrations_per_hour, 7);
    }

    #[tokio::test]
    async fn consignment_disabled_preserves_owned_history_and_command_replay() {
        let config = load_config(false);
        let store = Arc::new(ConsignmentStore { replay: true, ..Default::default() });
        let app = router_with_store(
            &config,
            ConsignmentActivation::Disabled,
            &store,
            Some(fixture_equipment_keyring()),
        );
        let history = send(&app, "GET", OWNED, None, StatusCode::OK).await;
        assert_eq!(history["data"]["items"], json!([]));

        let receipt = send(
            &app,
            "GET",
            &format!("/api/v1/me/consignment/commands/{REGISTER_COMMAND}"),
            None,
            StatusCode::OK,
        )
        .await;
        assert_eq!(receipt["data"]["http_status"], 202);
        assert_eq!(receipt["data"]["receipt"]["listing_id"], LISTING.to_string());

        let replay = send(&app, "POST", REGISTER, Some(registration()), StatusCode::ACCEPTED).await;
        assert_eq!(replay["data"]["listing_id"], LISTING.to_string());
        let repriced = send(
            &app,
            "PATCH",
            &format!("{REGISTER}/{LISTING}/price"),
            Some(json!({"command_id": REPRICE_COMMAND, "expected_version": 1, "price": 102})),
            StatusCode::OK,
        )
        .await;
        assert_eq!(repriced["data"]["listing_id"], LISTING.to_string());

        let without_comparator =
            router_with_store(&config, ConsignmentActivation::Disabled, &store, None);
        let failure = send(
            &without_comparator,
            "POST",
            REGISTER,
            Some(registration()),
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .await;
        assert_eq!(failure["code"], "consignment_unavailable");
    }

    fn fixture_equipment_keyring() -> Arc<Keyring> {
        build_keyring(
            DataKeyDomain::Equipment,
            Some(("fixture-equipment", "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=")),
            [],
        )
        .unwrap()
        .unwrap()
    }

    fn router_with_store(
        config: &AppConfig,
        activation: ConsignmentActivation,
        store: &Arc<ConsignmentStore>,
        keyring: Option<Arc<Keyring>>,
    ) -> Router {
        let state = PublicApiState::new(
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
            Some(Arc::new(Session)),
            configure_security(config).unwrap(),
        );
        let credentials = ConsignmentCredentials::from_keyring(keyring);
        let capabilities =
            configure_consignment(&config.consignment, activation, Some(store), credentials)
                .unwrap();
        configure_router(state.with_consignment(capabilities))
    }

    fn registration() -> Value {
        json!({
            "command_id": REGISTER_COMMAND,
            "mode": "consigned",
            "equipment_url": "https://hentaiverse.org/isekai/equip/42/0000000000",
            "price": 101,
            "expected_fee_bps": 175,
        })
    }

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        input: Option<Value>,
        expected_status: StatusCode,
    ) -> Value {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("cookie", "auction_public_session=fixture-session")
            .header("x-auction-expected-user-id", SELLER.to_string());
        let body = match input {
            Some(input) => {
                request =
                    request.header("origin", ORIGIN).header("content-type", "application/json");
                Body::from(input.to_string())
            }
            None => Body::empty(),
        };
        let mut request = request.body(body).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 22], 3123))));
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 8192).await.unwrap();
        assert_eq!(status, expected_status, "{method} {uri}: {}", String::from_utf8_lossy(&body));
        serde_json::from_slice(&body).unwrap()
    }

    struct Session;

    impl PublicSessionPort for Session {
        fn current(
            &self,
            token: String,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, PublicSessionIdentity> {
            assert_eq!(token, "fixture-session");
            Box::pin(async {
                Ok(PublicSessionIdentity {
                    user_id: SELLER,
                    username: "fixture-seller".to_owned(),
                    expires_at: None,
                    absolute_expires_at: None,
                })
            })
        }

        fn inspect(
            &self,
            _token: String,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, PublicSessionIdentity> {
            panic!("测试不读取只读会话快照")
        }

        fn login(
            &self,
            _key: Vec<u8>,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, PublicLoginResult> {
            panic!("测试不登录")
        }

        fn logout(&self, _token: String, _now: Timestamp) -> ApplicationFuture<'_, ()> {
            panic!("测试不退出")
        }

        fn get_preference(
            &self,
            _token: String,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
            panic!("测试不读会话偏好")
        }

        fn set_preference(
            &self,
            _token: String,
            _command: SetPublicSessionPreferenceCommand,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
            panic!("测试不写会话偏好")
        }
    }

    struct RegistrationCall {
        policy: ListingRegistrationPolicy,
        world: &'static str,
        sealer: bool,
        replay_crypto: bool,
    }

    #[derive(Default)]
    struct ConsignmentStore {
        registrations: Mutex<Vec<RegistrationCall>>,
        replay: bool,
    }

    impl PublicListingRead for ConsignmentStore {
        fn list_public(
            &self,
            _query: PublicListingQuery,
        ) -> ConsignmentFuture<'_, ListingReadPage<PublicListingSummary>> {
            panic!("测试不读公开列表")
        }

        fn public_detail(
            &self,
            _listing_id: Uuid,
        ) -> ConsignmentFuture<'_, Option<PublicListingDetail>> {
            panic!("测试不读公开详情")
        }

        fn registration_availability<'a>(
            &'a self,
            _crypto: &'a dyn RegistrationReplayCrypto,
        ) -> ConsignmentFuture<'a, ListingRegistrationAvailability> {
            Box::pin(async {
                Ok(ListingRegistrationAvailability::Ready(ListingWorlds {
                    main: true,
                    isekai: true,
                }))
            })
        }
    }

    impl OwnedListingRead for ConsignmentStore {
        fn list_owned(
            &self,
            seller: UserId,
            query: PrivateListingQuery,
        ) -> ConsignmentFuture<'_, ListingReadPage<OwnedListingRecord>> {
            assert_eq!(seller.as_uuid(), SELLER);
            Box::pin(async move {
                Ok(ListingReadPage { items: vec![], total: 0, window: query.window })
            })
        }

        fn owned_detail(
            &self,
            _seller: UserId,
            _listing_id: Uuid,
        ) -> ConsignmentFuture<'_, Option<OwnedListingRecord>> {
            panic!("测试不读本人详情")
        }

        fn owned_command(
            &self,
            seller: UserId,
            command_id: CommandId,
        ) -> ConsignmentFuture<'_, Option<OwnedListingCommandReceipt>> {
            assert_eq!(seller.as_uuid(), SELLER);
            assert_eq!(command_id.as_uuid(), REGISTER_COMMAND);
            Box::pin(async move {
                let registered = ListingRegistered::new(LISTING, ListingPrice::new(101).unwrap())?;
                Ok(Some(OwnedListingCommandReceipt {
                    command_id,
                    decision: OwnedListingDecision::Register(StoredListingDecision::Accepted(
                        registered,
                    )),
                }))
            })
        }
    }

    impl ListingCommandStore for ConsignmentStore {
        fn register<'a>(
            &'a self,
            command: RegisterListingCommand<'a>,
            sealer: Option<&'a dyn EquipmentKeySealerPort>,
            replay_crypto: Option<&'a dyn RegistrationReplayCrypto>,
        ) -> ConsignmentFuture<'a, ListingCommandOutcome<ListingRegistered>> {
            assert_eq!(command.seller_user_id.as_uuid(), SELLER);
            assert_eq!(command.command_id.as_uuid(), REGISTER_COMMAND);
            let sealer = sealer.is_some();
            let replay_crypto = replay_crypto.is_some();
            self.registrations.lock().unwrap().push(RegistrationCall {
                policy: command.policy,
                world: command.input.identity.world_id(),
                sealer,
                replay_crypto,
            });
            let price = command.input.price;
            Box::pin(async move {
                if !replay_crypto {
                    return Err(ConsignmentError::Unavailable);
                }
                if self.replay {
                    assert!(!command.policy.enabled);
                    let registered = ListingRegistered::new(LISTING, price)?;
                    return Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(
                        registered,
                    )));
                }
                if !sealer {
                    return Err(ConsignmentError::Unavailable);
                }
                Ok(ListingCommandOutcome::Accepted(ListingRegistered::new(
                    command.listing_id,
                    price,
                )?))
            })
        }

        fn reprice(
            &self,
            command: RepriceListingCommand,
        ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingPriceReceipt>> {
            assert!(self.replay && !command.enabled);
            assert_eq!(command.target.command_id.as_uuid(), REPRICE_COMMAND);
            assert_eq!(command.target.listing_id, LISTING);
            Box::pin(async move {
                let receipt = ListingPriceReceipt::new(
                    LISTING,
                    command.target.expected_version,
                    ListingState::Displayed,
                    command.price,
                    ListingPriceDisposition::Unchanged,
                )?;
                Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(receipt)))
            })
        }

        fn withdraw(
            &self,
            _command: WithdrawListingCommand,
        ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
            panic!("测试不取消")
        }

        fn apply_lookup(
            &self,
            _command: ApplyListingLookupCommand,
        ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingLookupEffect>> {
            panic!("测试不解析")
        }

        fn auto_close(
            &self,
            _command: AutoCloseListingCommand,
        ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingAutoCloseEffect>> {
            panic!("测试不清退")
        }

        fn admin_delist(
            &self,
            _command: AdminDelistListingCommand,
        ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
            panic!("Public 不执行下架")
        }
    }

    #[test]
    fn consignment_without_equipment_key_has_no_credentials() {
        assert!(ConsignmentCredentials::from_keyring(None).is_none());
    }

    #[test]
    fn consignment_seals_with_current_key_and_checks_previous_replay_key() {
        let old = ("fixture-previous", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
        let new = ("fixture-current", "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=");
        let old_ring = build_keyring(DataKeyDomain::Equipment, Some(old), []).unwrap().unwrap();
        let new_ring = build_keyring(DataKeyDomain::Equipment, Some(new), [old]).unwrap();
        let credentials = ConsignmentCredentials::from_keyring(new_ring).unwrap();
        let reference = HentaiVerseListingUrlValidator
            .validate("https://hentaiverse.org/equip/42/0000000000")
            .expect("夹具链接应通过本地校验");
        let envelope = credentials.sealer.seal(Uuid::new_v4().into(), &reference.key).unwrap();
        assert_eq!(envelope.encryption_key_id(), "fixture-current");

        let replay_key_id = Uuid::new_v4();
        let wrapped = EquipmentKeyOpener::new(old_ring).generate_wrapped(replay_key_id).unwrap();
        assert!(credentials.replay_crypto.check_wrapped(replay_key_id, &wrapped).is_ok());
        assert!(credentials.replay_crypto.check_wrapped(Uuid::new_v4(), &wrapped).is_err());
    }
}
