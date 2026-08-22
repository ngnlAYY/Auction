#[cfg(any(feature = "postgres", feature = "sqlite"))]
use std::sync::Arc;

use anyhow::Context;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_application::{
    AuctionLifecycleCommandPort, Clock, DeliveryTransport, DeliveryWorkStore, ScheduledRoundStore,
    SettlementCommandPort, SettlementFulfillmentCommandPort, SettlementFulfillmentService,
    SettlementFulfillmentStore, SettlementWorkStore,
};
use auction_cache::CacheStore;
use auction_config::{AppConfig, CacheBackend, Role};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_observability::{HeartbeatPublisher, HeartbeatStore, ProcessRole, ServiceName};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_storage::{
    DatabaseAuctionLifecycleCommands, DatabaseBackend, DatabaseSettlementCommands, DatabaseStore,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use chrono::{DateTime, Utc};
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

#[cfg(any(feature = "postgres", feature = "sqlite"))]
type SettlementPorts = (
    Option<Arc<dyn SettlementWorkStore>>,
    Option<Arc<dyn SettlementCommandPort>>,
    Option<Arc<dyn SettlementFulfillmentStore>>,
    Option<Arc<dyn SettlementFulfillmentCommandPort>>,
    Option<Arc<dyn DeliveryWorkStore>>,
    Option<Arc<dyn DeliveryTransport>>,
);

pub async fn run() -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    auction_observability::init("auction_worker", config.log_filter.as_deref())
        .context("initialize tracing")?;
    let cache = configure_cache(&config).context("configure worker cache")?;
    tracing::info!(
        database_configured = config.database_url().is_some(),
        cache_backend = cache.backend(),
        worker_concurrency = config.worker.concurrency,
        "worker configured"
    );

    run_worker(&config).await?;
    Ok(())
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
async fn run_worker(config: &AppConfig) -> anyhow::Result<()> {
    let Some(database_url) = config.database_url() else {
        auction_jobs::run_until_shutdown(config.worker.concurrency)
            .await
            .context("run worker skeleton without database")?;
        return Ok(());
    };
    let store =
        Arc::new(DatabaseStore::connect(database_url).await.context("connect worker database")?);
    let worker_id = format!("auction-worker-{}", Uuid::new_v4());
    let heartbeat_store: Arc<dyn HeartbeatStore> = store.clone();
    let heartbeat = Arc::new(HeartbeatPublisher::new(
        heartbeat_store,
        ServiceName::Worker,
        ProcessRole::Worker,
        worker_id.clone(),
        env!("CARGO_PKG_VERSION").to_owned(),
        Utc::now(),
        chrono::Duration::seconds(30),
    ));
    if let Err(error) = heartbeat.publish_starting().await {
        tracing::warn!(error = %error, "worker heartbeat starting publish failed");
    }
    let schedule: Arc<dyn ScheduledRoundStore> = store.clone();
    let lifecycle_store = Arc::clone(&store);
    let lifecycle: Arc<dyn AuctionLifecycleCommandPort> =
        Arc::new(DatabaseAuctionLifecycleCommands::new(lifecycle_store, Arc::new(SystemClock)));
    let (
        settlement_store,
        settlement_commands,
        fulfillment_store,
        fulfillment_commands,
        delivery_store,
        delivery_transport,
    ): SettlementPorts = match store.backend() {
        DatabaseBackend::Postgres => {
            let fulfillment_store: Arc<dyn SettlementFulfillmentStore> = store.clone();
            let fulfillment_commands: Arc<dyn SettlementFulfillmentCommandPort> =
                Arc::new(SettlementFulfillmentService::new(Arc::clone(&fulfillment_store)));
            (
                Some(store.clone()),
                Some(Arc::new(DatabaseSettlementCommands::new(Arc::clone(&store)))),
                Some(fulfillment_store),
                Some(fulfillment_commands),
                Some(store.clone()),
                None,
            )
        }
        DatabaseBackend::Sqlite => (None, None, None, None, None, None),
    };
    if let Err(error) = heartbeat.publish_ready().await {
        tracing::warn!(error = %error, "worker heartbeat ready publish failed");
    }
    let heartbeat_task = spawn_heartbeat(Arc::clone(&heartbeat));
    let result = auction_jobs::run_persistent_with_settlement_until_shutdown(
        auction_jobs::PersistentWorker {
            schedule,
            lifecycle,
            settlement_store,
            settlement_commands,
            fulfillment_store,
            fulfillment_commands,
            delivery_store,
            delivery_transport,
            // 外部广播协议尚未冻结；没有具体 sink 时不启动实时消费，避免伪造转发成功。
            realtime_consumer: None,
            clock: Arc::new(SystemClock),
            worker_id,
            concurrency: config.worker.concurrency,
            poll_interval: std::time::Duration::from_secs(config.worker.poll_interval_seconds),
        },
    )
    .await;
    if let Err(error) = heartbeat.publish_stopping().await {
        tracing::warn!(error = %error, "worker heartbeat stopping publish failed");
    }
    heartbeat_task.abort();
    result.context("run persistent worker")?;
    Ok(())
}

#[cfg(not(any(feature = "postgres", feature = "sqlite")))]
async fn run_worker(config: &AppConfig) -> anyhow::Result<()> {
    if config.database_url().is_some() {
        anyhow::bail!("worker 配置了数据库，但当前制品未编译 postgres 或 sqlite feature");
    }
    auction_jobs::run_until_shutdown(config.worker.concurrency)
        .await
        .context("run worker skeleton without database")?;
    Ok(())
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

#[cfg(any(feature = "postgres", feature = "sqlite"))]
fn spawn_heartbeat(publisher: Arc<HeartbeatPublisher>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(10));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(error) = publisher.publish_ready().await {
                tracing::warn!(error = %error, "worker heartbeat publish failed");
            }
        }
    })
}
