use std::{sync::Arc, time::Duration};

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
use auction_cache::CacheStore;
use auction_config::{AppConfig, CacheBackend, Role};
use auction_http::{HttpSecurityOptions, PublicApiState, PublicHttpSecurity};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_http::{RealtimeHub, RealtimeHubSink};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_observability::{
    HeartbeatHealthReader, HeartbeatPublisher, HeartbeatPublisherConfig, HeartbeatStore,
    ProcessRole, ServiceName,
};
use auction_observability::{ProcessMetrics, TracingConfig, TracingHealthReader, TracingRuntime};
use auction_process::{
    DEFAULT_DRAIN_TIMEOUT, DrainOutcome, TaskGuard, TerminationSignals, wait_for_drain,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_storage::{DatabaseCompetitionCommands, DatabaseStore};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use chrono::{DateTime, Utc};
use tokio::sync::oneshot;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use uuid::Uuid;

mod listener;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
#[derive(Clone, Copy)]
struct SystemClock;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

pub async fn run() -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Public).context("load public-api configuration")?;
    let tracing_config = tracing_config(&config.tracing);
    let telemetry = auction_observability::init_with_config(
        "auction_public_api",
        "public",
        env!("CARGO_PKG_VERSION"),
        config.log_filter.as_deref(),
        Some(&tracing_config),
        config.tracing_headers(),
    )
    .context("initialize tracing")?;

    let result = run_service(&config, &telemetry).await;
    shutdown_tracing(telemetry).await;
    result
}

async fn run_service(config: &AppConfig, telemetry: &TracingRuntime) -> anyhow::Result<()> {
    let mut signals = TerminationSignals::subscribe().context("subscribe public-api signals")?;
    let cache = Arc::new(configure_cache(config).context("configure public-api cache")?);
    let metrics = Arc::new(
        ProcessMetrics::new_with_tracing_health(
            config.metrics.enabled,
            "auction_public_api",
            "public",
            env!("CARGO_PKG_VERSION"),
            telemetry.health_reader(),
        )
        .context("initialize Prometheus metrics")?,
    );
    let listener = listener::bind(config).await?;
    let listener_description = listener.description();
    let (state, heartbeat, realtime_relay) =
        configure_state(config, telemetry.health_reader(), Arc::clone(&metrics), cache.clone())
            .await
            .context("configure public-api state")?;
    #[cfg(not(any(feature = "postgres", feature = "sqlite")))]
    let _ = realtime_relay;
    let state = state.with_metrics(metrics);
    #[cfg(any(feature = "postgres", feature = "sqlite"))]
    let mut heartbeat_task =
        heartbeat.as_ref().map(Arc::clone).map(spawn_heartbeat).map(TaskGuard::new);
    #[cfg(not(any(feature = "postgres", feature = "sqlite")))]
    let mut heartbeat_task: Option<TaskGuard<()>> = None;
    #[cfg(any(feature = "postgres", feature = "sqlite"))]
    let mut realtime_relay_task = realtime_relay
        .map(|relay| {
            spawn_realtime_relay(
                relay,
                std::time::Duration::from_millis(config.realtime.poll_interval_millis),
            )
        })
        .map(TaskGuard::new);
    #[cfg(not(any(feature = "postgres", feature = "sqlite")))]
    let mut realtime_relay_task: Option<TaskGuard<()>> = None;
    // 传入本二进制的版本，健康检查才能报告运行进程的版本而非契约 crate 的版本。
    let boundary = state.http_boundary();
    let app = auction_http::public_router::<crate::web_assets::PublicWebAssets>(
        env!("CARGO_PKG_VERSION"),
        boundary,
    )
    .with_state(state);

    tracing::info!(listener = %listener_description, cache_backend = cache.backend(), "public api listening");
    let (shutdown_sender, shutdown_receiver) = oneshot::channel::<()>();
    let mut server_task = TaskGuard::new(tokio::spawn(async move {
        listener::serve(listener, app, async move {
            let _ = shutdown_receiver.await;
        })
        .await
    }));
    let mut immediate_server_result = None;
    let signal = tokio::select! {
        result = server_task.join() => {
            immediate_server_result = result;
            None
        }
        signal = signals.recv() => Some(signal.context("receive public-api shutdown signal")?),
    };

    if let Some(signal) = signal {
        tracing::info!(signal = %signal, "public api drain started");
        let _ = shutdown_sender.send(());
    } else {
        drop(shutdown_sender);
    }
    abort_background_task(&heartbeat_task);
    abort_background_task(&realtime_relay_task);

    let drain = async {
        join_background_task(&mut heartbeat_task, "public heartbeat").await;
        join_background_task(&mut realtime_relay_task, "public realtime relay").await;
        if let Some(publisher) = heartbeat.as_ref()
            && let Err(error) = publisher.publish_stopping().await
        {
            tracing::warn!(error = %error, "public heartbeat stopping publish failed");
        }
        match immediate_server_result.take() {
            Some(result) => Some(result),
            None => server_task.join().await,
        }
    };
    let result = match wait_for_drain(&mut signals, DEFAULT_DRAIN_TIMEOUT, drain).await {
        Ok(DrainOutcome::Completed(result)) => public_server_result(result),
        Ok(DrainOutcome::TimedOut) => {
            force_public_tasks(&mut heartbeat_task, &mut realtime_relay_task, &mut server_task)
                .await;
            Err(anyhow::anyhow!("public-api drain exceeded 10 seconds"))
        }
        Ok(DrainOutcome::Interrupted(signal)) => {
            force_public_tasks(&mut heartbeat_task, &mut realtime_relay_task, &mut server_task)
                .await;
            Err(anyhow::anyhow!("public-api drain interrupted by {signal}"))
        }
        Err(error) => {
            force_public_tasks(&mut heartbeat_task, &mut realtime_relay_task, &mut server_task)
                .await;
            Err(error).context("wait for public-api drain")
        }
    };
    if result.is_ok() {
        tracing::info!("public api stopped");
    }
    result
}

fn abort_background_task(task: &Option<TaskGuard<()>>) {
    if let Some(task) = task {
        task.abort();
    }
}

async fn join_background_task(task: &mut Option<TaskGuard<()>>, name: &'static str) {
    let Some(task) = task else {
        return;
    };
    if let Some(Err(error)) = task.join().await
        && !error.is_cancelled()
    {
        tracing::warn!(task = name, error = %error, "public background task join failed");
    }
}

async fn force_public_tasks(
    heartbeat_task: &mut Option<TaskGuard<()>>,
    realtime_relay_task: &mut Option<TaskGuard<()>>,
    server_task: &mut TaskGuard<std::io::Result<()>>,
) {
    abort_background_task(heartbeat_task);
    abort_background_task(realtime_relay_task);
    server_task.abort();
    join_background_task(heartbeat_task, "public heartbeat").await;
    join_background_task(realtime_relay_task, "public realtime relay").await;
    let _ = server_task.join().await;
}

fn public_server_result(
    result: Option<Result<std::io::Result<()>, tokio::task::JoinError>>,
) -> anyhow::Result<()> {
    match result {
        Some(Ok(Ok(()))) => Ok(()),
        Some(Ok(Err(error))) => Err(error).context("serve public-api HTTP"),
        Some(Err(error)) => Err(error).context("join public-api HTTP server"),
        None => Err(anyhow::anyhow!("public-api HTTP server result is missing")),
    }
}

async fn shutdown_tracing(telemetry: TracingRuntime) {
    match tokio::task::spawn_blocking(move || telemetry.shutdown()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(error = %error, "public tracing shutdown failed"),
        Err(error) => tracing::warn!(error = %error, "public tracing shutdown task failed"),
    }
}

fn tracing_config(settings: &auction_config::TracingSettings) -> TracingConfig {
    TracingConfig {
        enabled: settings.enabled,
        endpoint: settings.endpoint.clone(),
        batch_timeout: Duration::from_secs(settings.batch_timeout_seconds),
        export_timeout: Duration::from_secs(settings.export_timeout_seconds),
        queue_capacity: settings.queue_capacity as usize,
        max_export_batch_size: settings.max_export_batch_size as usize,
        sample_ratio: f64::from(settings.sample_ratio_ppm) / 1_000_000.0,
        shutdown_timeout: Duration::from_secs(settings.shutdown_timeout_seconds),
    }
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
async fn configure_state(
    config: &AppConfig,
    tracing_health: Arc<dyn TracingHealthReader>,
    metrics: Arc<ProcessMetrics>,
    cache: Arc<dyn ReadModelCache>,
) -> anyhow::Result<(PublicApiState, Option<Arc<HeartbeatPublisher>>, Option<Arc<RealtimeRelay>>)> {
    let security = configure_security(config)?;
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
async fn configure_state(
    config: &AppConfig,
    _tracing_health: Arc<dyn TracingHealthReader>,
    _metrics: Arc<ProcessMetrics>,
    _cache: Arc<dyn ReadModelCache>,
) -> anyhow::Result<(
    PublicApiState,
    Option<Arc<auction_observability::HeartbeatPublisher>>,
    Option<Arc<auction_application::RealtimeRelay>>,
)> {
    let security = configure_security(config)?;
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
fn spawn_realtime_relay(
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
fn spawn_heartbeat(publisher: Arc<HeartbeatPublisher>) -> tokio::task::JoinHandle<()> {
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

fn configure_security(config: &AppConfig) -> anyhow::Result<PublicHttpSecurity> {
    PublicHttpSecurity::new(HttpSecurityOptions {
        cookie_secure: config.http_security.cookie_secure,
        origins_enabled: config.http_security.allowed_origins_enabled,
        allowed_origins: &config.http_security.allowed_origins,
        login_limit: config.http_security.login_rate_limit,
        mutation_limit: config.http_security.mutation_rate_limit,
        rate_limit_window: Duration::from_secs(config.http_security.rate_limit_window_seconds),
        request_timeout: Duration::from_secs(config.http_security.request_timeout_seconds),
        trusted_proxy_cidrs: &config.http_security.trusted_proxy_cidrs,
        trusted_unix_proxy: config.http_security.trusted_unix_proxy,
    })
    .map_err(anyhow::Error::msg)
}

fn configure_cache(config: &AppConfig) -> anyhow::Result<CacheStore> {
    match config.cache.backend {
        CacheBackend::Disabled => Ok(CacheStore::disabled()),
        CacheBackend::Redis => {
            let redis_url = config.redis_url().context("redis cache URL is missing")?;
            CacheStore::redis(redis_url).map_err(Into::into)
        }
    }
}
