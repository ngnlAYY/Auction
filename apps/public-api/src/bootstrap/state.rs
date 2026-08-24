//! Public API 的数据库、端口、heartbeat 和 realtime 状态装配。

use std::sync::Arc;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use std::time::Duration;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
use anyhow::Context;
use auction_application::ReadModelCache;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_application::{
    AuctionReader, CachedPublicRulesReader, Clock, CompetitionCommandPort, PublicAccountReader,
    PublicBidLogReader, PublicBlacklistReader, PublicCompensationReader, PublicForumReader,
    PublicLoginAuthenticator, PublicLotReader, PublicPriceHistoryReader, PublicRealtimeReader,
    PublicRulesReader, PublicSessionConfig, PublicSessionPort, PublicSessionService,
    PublicSessionStore, RealtimeConsumerStore, RealtimeEventSink, RealtimeRelay,
    RealtimeRelayConfig, RealtimeStreamStore, WorldReader,
};
use auction_config::AppConfig;
use auction_http::PublicApiState;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_http::{RealtimeHub, RealtimeHubSink};
use auction_observability::ProcessMetrics;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_observability::{
    HeartbeatHealthReader, HeartbeatPublisher, HeartbeatPublisherConfig, HeartbeatStore,
    ProcessRole, ServiceName,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_storage::{DatabaseCompetitionCommands, DatabaseStore};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use chrono::{DateTime, Utc};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use uuid::Uuid;

use super::config;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
#[derive(Clone, Copy)]
struct SystemClock;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(super) async fn configure_state(
    config: &AppConfig,
    tracing_health: Arc<dyn auction_observability::TracingHealthReader>,
    metrics: Arc<ProcessMetrics>,
    cache: Arc<dyn ReadModelCache>,
) -> anyhow::Result<(PublicApiState, Option<Arc<HeartbeatPublisher>>, Option<Arc<RealtimeRelay>>)> {
    let security = config::configure_security(config)?;
    let Some(database_url) = config.database_url() else {
        return Ok((
            PublicApiState::new(
                None, None, None, None, None, None, None, None, None, None, None, None, None,
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
    let heartbeat_store: Arc<dyn HeartbeatStore> = store.clone();
    let heartbeat_health: Arc<dyn HeartbeatHealthReader> = store.clone();
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
    let lots: Arc<dyn PublicLotReader> = store.clone();
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
    let stream_store: Arc<dyn RealtimeStreamStore> = store.clone();
    let consumer_store: Arc<dyn RealtimeConsumerStore> = store.clone();
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
    let state = PublicApiState::new(
        Some(worlds),
        Some(rules),
        Some(blacklist),
        Some(compensation),
        Some(forum),
        Some(auctions),
        Some(lots),
        Some(bid_log),
        Some(price_history),
        Some(realtime),
        Some(account),
        Some(commands),
        Some(session),
        security,
    )
    .with_realtime_hub(hub)
    .with_readiness(store);
    if let Err(error) = heartbeat.publish_current().await {
        tracing::warn!(error = %error, "public heartbeat ready publish failed");
    }
    Ok((state, Some(heartbeat), Some(realtime_relay)))
}

#[cfg(not(any(feature = "postgres", feature = "sqlite")))]
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
        anyhow::bail!("public-api 配置了数据库，但当前制品未编译 postgres 或 sqlite feature");
    }
    Ok((
        PublicApiState::new(
            None, None, None, None, None, None, None, None, None, None, None, None, None, security,
        ),
        None,
        None,
    ))
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(super) fn spawn_realtime_relay(
    relay: Arc<RealtimeRelay>,
    poll_interval: std::time::Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            match relay.run_once(Utc::now()).await {
                Ok(batch) if batch.delivered > 0 || batch.failures > 0 => {
                    tracing::debug!(
                        streams = batch.streams,
                        delivered = batch.delivered,
                        failures = batch.failures,
                        "public realtime relay batch completed"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(error = %error, "public realtime relay batch failed");
                }
            }
        }
    })
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(super) fn spawn_heartbeat(publisher: Arc<HeartbeatPublisher>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(error) = publisher.publish_current().await {
                tracing::warn!(error = %error, "public heartbeat publish failed");
            }
        }
    })
}
