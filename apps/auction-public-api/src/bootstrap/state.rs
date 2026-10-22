//! Public API 的数据库、端口、heartbeat 和 realtime 状态装配。

use std::sync::Arc;
#[cfg(feature = "postgres")]
use std::sync::{
    RwLock,
    atomic::{AtomicBool, Ordering},
};
#[cfg(feature = "postgres")]
use std::time::Duration;

#[cfg(feature = "postgres")]
use anyhow::Context;
use auction_application::ReadModelCache;
#[cfg(feature = "postgres")]
use auction_application::{
    ApplicationError, ApplicationFuture, AuctionReader, CachedPublicRulesReader, Clock,
    CompetitionCommandPort, ProcessReadinessPort, PublicAccountReader, PublicBidLogReader,
    PublicBlacklistReader, PublicCompensationReader, PublicEquipmentLinkResolver,
    PublicEquipmentLinkSecretReader, PublicForumReader, PublicLoginAuthenticator, PublicLotReader,
    PublicPriceHistoryReader, PublicRealtimeReader, PublicRulesReader, PublicSessionConfig,
    PublicSessionPort, PublicSessionService, PublicSessionStore, RealtimeConsumerStore,
    RealtimeEventSink, RealtimeRelay, RealtimeRelayConfig, RealtimeStreamStore, RepositoryError,
    WorldReader,
};
use auction_config::AppConfig;
use auction_http::PublicApiState;
#[cfg(feature = "postgres")]
use auction_http::{RealtimeHub, RealtimeHubSink};
#[cfg(feature = "postgres")]
use auction_integrations::{DataKey, EquipmentGameLinkResolver, EquipmentKeyOpener};
use auction_observability::ProcessMetrics;
#[cfg(feature = "postgres")]
use auction_observability::{
    HeartbeatHealthReader, HeartbeatPublisher, HeartbeatPublisherConfig, HeartbeatReadinessFuture,
    HeartbeatStore, ProcessRole, ServiceName,
};
#[cfg(feature = "postgres")]
use auction_storage::{DatabaseCompetitionCommands, DatabaseStore};
#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use uuid::Uuid;

use super::config;

#[cfg(feature = "postgres")]
#[derive(Clone, Copy)]
struct SystemClock;

#[cfg(feature = "postgres")]
struct PublicRealtimeRelayHealth {
    healthy: AtomicBool,
    last_success: RwLock<Option<std::time::Instant>>,
    stale_after: Duration,
}

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
struct PublicProcessHealth {
    database: Arc<DatabaseStore>,
    relay: Arc<PublicRealtimeRelayHealth>,
}

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
impl HeartbeatHealthReader for PublicProcessHealth {
    fn readiness(&self) -> HeartbeatReadinessFuture<'_> {
        Box::pin(async move { self.database.readiness().await && self.relay.is_healthy() })
    }

    fn audit_failure_count(&self) -> i64 {
        self.database.audit_failure_count()
    }
}

#[cfg(feature = "postgres")]
pub(super) struct ConfiguredRealtimeRelay {
    relay: Arc<RealtimeRelay>,
    health: Arc<PublicRealtimeRelayHealth>,
}

#[cfg(feature = "postgres")]
struct RealtimeRelayTaskHealthGuard {
    health: Arc<PublicRealtimeRelayHealth>,
}

#[cfg(feature = "postgres")]
impl Drop for RealtimeRelayTaskHealthGuard {
    fn drop(&mut self) {
        // panic、意外返回或 shutdown abort 都立即撤销任务健康证明，不等待新鲜度窗口自然过期。
        self.health.mark_failure();
    }
}

#[cfg(feature = "postgres")]
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[cfg(feature = "postgres")]
fn configure_equipment_links(
    config: &AppConfig,
    store: &Arc<DatabaseStore>,
) -> anyhow::Result<Option<Arc<dyn PublicEquipmentLinkResolver>>> {
    let (key_id, encoded_key) = match (config.equipment_data_key_id(), config.equipment_data_key())
    {
        (Some(key_id), Some(encoded_key)) => (key_id, encoded_key),
        (None, None) => return Ok(None),
        _ => anyhow::bail!("Public 真实装备 URL 的数据密钥配置不完整"),
    };
    let data_key = DataKey::from_base64(key_id, encoded_key)
        .map_err(|_| anyhow::anyhow!("Public 真实装备 URL 的数据密钥配置无效"))?;
    let secrets: Arc<dyn PublicEquipmentLinkSecretReader> = store.clone();
    Ok(Some(Arc::new(EquipmentGameLinkResolver::new(secrets, EquipmentKeyOpener::new(data_key)))))
}

#[cfg(feature = "postgres")]
pub(super) async fn configure_state(
    config: &AppConfig,
    tracing_health: Arc<dyn auction_observability::TracingHealthReader>,
    metrics: Arc<ProcessMetrics>,
    cache: Arc<dyn ReadModelCache>,
) -> anyhow::Result<(
    PublicApiState,
    Option<Arc<HeartbeatPublisher>>,
    Option<Arc<ConfiguredRealtimeRelay>>,
)> {
    let security = config::configure_security(config)?;
    let Some(database_url) = config.database_url() else {
        return Ok((
            PublicApiState::new(
                None, None, None, None, None, None, None, None, None, None, None, None, None, None,
                security,
            ),
            None,
            None,
        ));
    };

    let store =
        Arc::new(DatabaseStore::connect(database_url).await.context("connect public database")?);
    metrics
        .register_pool_metrics(store.pool_metrics_source())
        .context("register public database pool metrics")?;
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
    let rules_authority: Arc<dyn PublicRulesReader> = store.clone();
    let rules: Arc<dyn PublicRulesReader> = Arc::new(CachedPublicRulesReader::new(
        rules_authority,
        cache,
        Duration::from_secs(config.cache.public_rules_ttl_seconds),
    ));
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
        Arc::new(DatabaseCompetitionCommands::new(store.clone(), Arc::new(SystemClock)));
    let authenticator: Arc<dyn PublicLoginAuthenticator> = store.clone();
    let session_store: Arc<dyn PublicSessionStore> = store.clone();
    let session_config = PublicSessionConfig {
        idle_ttl: chrono::Duration::seconds(config.session.idle_ttl_seconds as i64),
        absolute_ttl: chrono::Duration::seconds(config.session.absolute_ttl_seconds as i64),
        max_public_sessions: config.session.max_public_sessions,
    };
    let session: Arc<dyn PublicSessionPort> =
        Arc::new(PublicSessionService::new(authenticator, session_store, session_config)?);
    let hub =
        Arc::new(RealtimeHub::new(config.realtime.client_buffer).map_err(anyhow::Error::msg)?);
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
    let realtime_relay = Arc::new(
        RealtimeRelay::new(
            stream_store,
            consumer_store,
            sink,
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
    let readiness: Arc<dyn ProcessReadinessPort> = process_health;
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
    .with_readiness(readiness);
    if let Err(error) = heartbeat.publish_current().await {
        tracing::warn!(error = %error, "public heartbeat ready publish failed");
    }
    Ok((state, Some(heartbeat), Some(realtime_relay)))
}

#[cfg(not(feature = "postgres"))]
pub(super) async fn configure_state(
    config: &AppConfig,
    _tracing_health: Arc<dyn auction_observability::TracingHealthReader>,
    _metrics: Arc<ProcessMetrics>,
    _cache: Arc<dyn ReadModelCache>,
) -> anyhow::Result<(
    PublicApiState,
    Option<Arc<auction_observability::HeartbeatPublisher>>,
    Option<Arc<auction_application::RealtimeRelay>>,
)> {
    let security = config::configure_security(config)?;
    if config.database_url().is_some() {
        anyhow::bail!("public-api 配置了数据库，但当前制品未编译 postgres feature");
    }
    Ok((
        PublicApiState::new(
            None, None, None, None, None, None, None, None, None, None, None, None, None, None,
            security,
        ),
        None,
        None,
    ))
}

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
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
