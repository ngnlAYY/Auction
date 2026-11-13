//! Public API 的数据库、端口、heartbeat 和 realtime 状态装配。

use std::sync::Arc;
use std::sync::{
    RwLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use anyhow::Context;
use auction_application::auction::{
    ApplicationFuture, AuctionReader, CompetitionCommandPort, PublicAccountReader,
    PublicBidLogReader, PublicLotReader, PublicPriceHistoryReader, WorldReader,
};
use auction_application::core::{ApplicationError, RepositoryError};
use auction_application::forum::PublicForumReader;
use auction_application::operations::ProcessReadinessPort;
use auction_application::public::{
    PublicBlacklistReader, PublicCompensationReader, PublicEquipmentLinkResolver,
    PublicEquipmentLinkSecretReader, PublicLoginAuthenticator, PublicRulesReader,
    PublicSessionConfig, PublicSessionPort, PublicSessionStore,
};
use auction_application::realtime::{
    PublicRealtimeReader, RealtimeConsumerStore, RealtimeEventSink, RealtimeRelay,
    RealtimeRelayConfig, RealtimeSequenceAllocator, RealtimeStreamStore,
};
use auction_application::sessions::PublicSessionService;
use auction_config::AppConfig;
use auction_http::PublicApiState;
use auction_http::{RealtimeHub, RealtimeHubSink};
use auction_integrations::{EquipmentGameLinkResolver, EquipmentKeyOpener, build_keyring};
use auction_observability::ProcessMetrics;
use auction_observability::{
    HeartbeatHealthReader, HeartbeatPublisher, HeartbeatPublisherConfig, HeartbeatReadinessFuture,
    HeartbeatStore, ProcessRole, ServiceName,
};
use auction_storage::{DatabaseCompetitionCommands, DatabaseStore};
use chrono::Utc;
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

fn configure_equipment_links(
    config: &AppConfig,
    store: &Arc<DatabaseStore>,
) -> anyhow::Result<Option<Arc<dyn PublicEquipmentLinkResolver>>> {
    let keyring = build_keyring(
        auction_config::DataKeyDomain::Equipment,
        config.data_key_current(auction_config::DataKeyDomain::Equipment),
        config
            .data_key_previous(auction_config::DataKeyDomain::Equipment)
            .iter()
            .map(|key| (key.id(), key.material())),
    )
    .map_err(|_| anyhow::anyhow!("Public 真实装备 URL 的数据密钥配置无效"))?;
    let Some(keyring) = keyring else {
        return Ok(None);
    };
    let secrets: Arc<dyn PublicEquipmentLinkSecretReader> = store.clone();
    Ok(Some(Arc::new(EquipmentGameLinkResolver::new(secrets, EquipmentKeyOpener::new(keyring)))))
}

pub(super) async fn configure_state(
    config: &AppConfig,
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
        return Ok((
            PublicApiState::new(
                None, None, None, None, None, None, None, None, None, None, None, None, None, None,
                security,
            )
            .with_runtime_switch_snapshot(switches),
            None,
            None,
            None,
        ));
    };

    // 审计失败按类别向共享影子框架记样本；进程级安装一次。
    auction_storage::install_audit_shadow_recorder(metrics.shadow());
    let store = Arc::new(
        DatabaseStore::connect(database_url, database_profile(config)?)
            .await
            .context("connect public database")?
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
    metrics
        .register_runtime_switch_metrics(switches.clone())
        .context("register runtime switch metrics")?;
    metrics
        .register_pool_metrics(store.pool_metrics_source())
        .context("register public database pool metrics")?;
    metrics
        .register_audit_metrics(store.audit_metrics_source())
        .context("register public audit metrics")?;
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
            started_at: Utc::now(),
            ttl: chrono::Duration::seconds(30),
        },
        heartbeat_health,
        tracing_health,
    ));
    if let Err(error) = heartbeat.publish_starting().await {
        tracing::warn!(error = %error, "public heartbeat starting publish failed");
    }
    let worlds: Arc<dyn WorldReader> = store.clone();
    let rules: Arc<dyn PublicRulesReader> = store.clone();
    let blacklist: Arc<dyn PublicBlacklistReader> = store.clone();
    let compensation: Arc<dyn PublicCompensationReader> = store.clone();
    let forum: Arc<dyn PublicForumReader> = store.clone();
    let auctions: Arc<dyn AuctionReader> = store.clone();
    let lots: Arc<dyn PublicLotReader> =
        Arc::new(store.public_lot_reader(config.equipment_colors.clone()));
    let equipment_links = configure_equipment_links(config, &store)?;
    let bid_log: Arc<dyn PublicBidLogReader> = store.clone();
    let price_history: Arc<dyn PublicPriceHistoryReader> = store.clone();
    let realtime: Arc<dyn PublicRealtimeReader> = store.clone();
    let account: Arc<dyn PublicAccountReader> = store.clone();
    let commands: Arc<dyn CompetitionCommandPort> =
        Arc::new(DatabaseCompetitionCommands::new(store.clone()));
    let authenticator: Arc<dyn PublicLoginAuthenticator> = store.clone();
    let session_store: Arc<dyn PublicSessionStore> = store.clone();
    let auction_config::RoleSessionConfig::Public(session_settings) = config.session else {
        anyhow::bail!("Public session configuration is unavailable");
    };
    let session_config =
        PublicSessionConfig { max_public_sessions: session_settings.max_public_sessions };
    let session: Arc<dyn PublicSessionPort> =
        Arc::new(PublicSessionService::new(authenticator, session_store, session_config)?);
    let realtime_metrics = auction_observability::RealtimeRecorder::register(&metrics)
        .context("register public realtime latency metrics")?;
    let hub = RealtimeHub::new(config.realtime.client_buffer)
        .map_err(anyhow::Error::msg)?
        .with_metrics(realtime_metrics);
    let hub = Arc::new(hub);
    let sink: Arc<dyn RealtimeEventSink> = Arc::new(RealtimeHubSink::new(Arc::clone(&hub)));
    let ownership = Arc::new(
        store
            .acquire_realtime_consumer_ownership(&config.realtime.consumer_key)
            .await
            .context("acquire public realtime consumer ownership")?,
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
    .with_lot_watches(store.clone())
    .with_lot_watch_preference(store.clone())
    .with_realtime_hub(hub)
    .with_runtime_switch_snapshot(switches)
    .with_readiness(readiness);
    if let Err(error) = heartbeat.publish_current().await {
        tracing::warn!(error = %error, "public heartbeat ready publish failed");
    }
    Ok((
        state,
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
            match configured.relay.run_once(Utc::now()).await {
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
                        "public realtime relay batch completed"
                    );
                }
                Ok(_) => configured.health.mark_success(),
                Err(error) => {
                    configured.health.mark_failure();
                    tracing::warn!(error = %error, "public realtime relay batch failed");
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
                tracing::warn!(error = %error, "public heartbeat publish failed");
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
