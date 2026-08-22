use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Context;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_application::{
    AuctionReader, Clock, CompetitionCommandPort, PublicAccountReader, PublicBidLogReader,
    PublicBlacklistReader, PublicCompensationReader, PublicForumReader, PublicLoginAuthenticator,
    PublicLotReader, PublicPriceHistoryReader, PublicRealtimeReader, PublicRulesReader,
    PublicSessionConfig, PublicSessionPort, PublicSessionService, PublicSessionStore, WorldReader,
};
use auction_cache::CacheStore;
use auction_config::{AppConfig, CacheBackend, Role};
use auction_http::{PublicApiState, PublicHttpSecurity};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_observability::{HeartbeatPublisher, HeartbeatStore, ProcessRole, ServiceName};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_storage::{DatabaseCompetitionCommands, DatabaseStore};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use chrono::{DateTime, Utc};
use tokio::net::TcpListener;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use uuid::Uuid;

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
    auction_observability::init("auction_public_api", config.log_filter.as_deref())
        .context("initialize tracing")?;
    let cache = configure_cache(&config).context("configure public-api cache")?;
    let address = config.listen_addr.context("public-api must have a listen address")?;
    let listener = TcpListener::bind(address).await?;
    let (state, heartbeat) =
        configure_state(&config).await.context("configure public-api state")?;
    #[cfg(any(feature = "postgres", feature = "sqlite"))]
    let heartbeat_task = heartbeat.as_ref().map(Arc::clone).map(spawn_heartbeat);
    #[cfg(not(any(feature = "postgres", feature = "sqlite")))]
    let heartbeat_task: Option<tokio::task::JoinHandle<()>> = None;
    // 传入本二进制的版本，健康检查才能报告运行进程的版本而非契约 crate 的版本。
    let app = auction_http::public_router(env!("CARGO_PKG_VERSION")).with_state(state);

    tracing::info!(%address, cache_backend = cache.backend(), "public api listening");
    let result =
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await;
    if let Some(publisher) = heartbeat.as_ref()
        && let Err(error) = publisher.publish_stopping().await
    {
        tracing::warn!(error = %error, "public heartbeat stopping publish failed");
    }
    if let Some(task) = heartbeat_task {
        task.abort();
    }
    result?;
    Ok(())
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
async fn configure_state(
    config: &AppConfig,
) -> anyhow::Result<(PublicApiState, Option<Arc<HeartbeatPublisher>>)> {
    let security = configure_security(config)?;
    let Some(database_url) = config.database_url() else {
        return Ok((
            PublicApiState::new(
                None, None, None, None, None, None, None, None, None, None, None, None, None,
                security,
            ),
            None,
        ));
    };

    let store =
        Arc::new(DatabaseStore::connect(database_url).await.context("connect public database")?);
    let heartbeat_store: Arc<dyn HeartbeatStore> = store.clone();
    let heartbeat = Arc::new(HeartbeatPublisher::new(
        heartbeat_store,
        ServiceName::Public,
        ProcessRole::Public,
        Uuid::new_v4().to_string(),
        env!("CARGO_PKG_VERSION").to_owned(),
        Utc::now(),
        chrono::Duration::seconds(30),
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
    let lots: Arc<dyn PublicLotReader> = store.clone();
    let bid_log: Arc<dyn PublicBidLogReader> = store.clone();
    let price_history: Arc<dyn PublicPriceHistoryReader> = store.clone();
    let realtime: Arc<dyn PublicRealtimeReader> = store.clone();
    let account: Arc<dyn PublicAccountReader> = store.clone();
    let commands: Arc<dyn CompetitionCommandPort> =
        Arc::new(DatabaseCompetitionCommands::new(store.clone(), Arc::new(SystemClock)));
    let authenticator: Arc<dyn PublicLoginAuthenticator> = store.clone();
    let session_store: Arc<dyn PublicSessionStore> = store;
    let session_config = PublicSessionConfig {
        idle_ttl: chrono::Duration::seconds(config.session.idle_ttl_seconds as i64),
        absolute_ttl: chrono::Duration::seconds(config.session.absolute_ttl_seconds as i64),
        max_public_sessions: config.session.max_public_sessions,
    };
    let session: Arc<dyn PublicSessionPort> =
        Arc::new(PublicSessionService::new(authenticator, session_store, session_config)?);
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
    );
    if let Err(error) = heartbeat.publish_ready().await {
        tracing::warn!(error = %error, "public heartbeat ready publish failed");
    }
    Ok((state, Some(heartbeat)))
}

#[cfg(not(any(feature = "postgres", feature = "sqlite")))]
async fn configure_state(
    config: &AppConfig,
) -> anyhow::Result<(PublicApiState, Option<Arc<auction_observability::HeartbeatPublisher>>)> {
    let security = configure_security(config)?;
    if config.database_url().is_some() {
        anyhow::bail!("public-api 配置了数据库，但当前制品未编译 postgres 或 sqlite feature");
    }
    Ok((
        PublicApiState::new(
            None, None, None, None, None, None, None, None, None, None, None, None, None, security,
        ),
        None,
    ))
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
fn spawn_heartbeat(publisher: Arc<HeartbeatPublisher>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(error) = publisher.publish_ready().await {
                tracing::warn!(error = %error, "public heartbeat publish failed");
            }
        }
    })
}

fn configure_security(config: &AppConfig) -> anyhow::Result<PublicHttpSecurity> {
    PublicHttpSecurity::new(
        config.http_security.cookie_secure,
        config.http_security.allowed_origins_enabled,
        &config.http_security.allowed_origins,
        config.http_security.login_rate_limit,
        config.http_security.mutation_rate_limit,
        Duration::from_secs(config.http_security.rate_limit_window_seconds),
    )
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
