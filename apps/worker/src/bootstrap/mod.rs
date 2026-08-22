#[cfg(any(feature = "postgres", feature = "sqlite"))]
use std::sync::Arc;

use anyhow::Context;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_application::{
    AuctionLifecycleCommandPort, Clock, DeliveryTransport, DeliveryWorkStore, MailPollStore,
    ScheduledRoundStore, SettlementCommandPort, SettlementFulfillmentCommandPort,
    SettlementFulfillmentService, SettlementFulfillmentStore, SettlementWorkStore,
};
use auction_cache::CacheStore;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_config::load_mooglemail_keyenv;
use auction_config::{AppConfig, CacheBackend, Role};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_integrations::mooglemail::{
    MoogleMailClient, MoogleMailCredentialSet, MoogleMailGateway, MoogleMailGatewayConfig,
    MoogleMailMessage, MoogleMailWorld,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_jobs::{
    MailPollFetchRequest, MailPollFetchedMessage, MailPollGateway, MailPollGatewayError,
    MailPollPage, MailPollRuntime,
};
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
        if config.worker.mooglemail.enabled {
            anyhow::bail!("启用 MoogleMail Worker 轮询时必须配置 PostgreSQL database_url");
        }
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
    let mail_poll = configure_mail_poll(config, Arc::clone(&store)).await?;
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
            mail_poll,
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

#[cfg(any(feature = "postgres", feature = "sqlite"))]
async fn configure_mail_poll(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
) -> anyhow::Result<Option<MailPollRuntime>> {
    let mail_config = &config.worker.mooglemail;
    if !mail_config.enabled {
        return Ok(None);
    }
    if store.backend() != DatabaseBackend::Postgres {
        anyhow::bail!("MoogleMail Worker 轮询仅支持 PostgreSQL 完整档");
    }
    let credentials = if let Some(path) = &mail_config.keyenv_file {
        let keyenv = load_mooglemail_keyenv(path).context("load MoogleMail keyenv")?;
        MoogleMailCredentialSet::new(keyenv.bot_cookie(), keyenv.test_cookie())
    } else {
        let cookie = config
            .mooglemail_cookie()
            .context("MoogleMail enabled but no keyenv_file or sensitive mooglemail_cookie")?;
        MoogleMailCredentialSet::new(cookie, cookie)
    }
    .map_err(|error| anyhow::Error::msg(error.to_string()))?;
    let client = Arc::new(
        MoogleMailClient::production().map_err(|error| anyhow::Error::msg(error.to_string()))?,
    );
    let gateway = Arc::new(
        MoogleMailGateway::new(
            client,
            credentials,
            MoogleMailGatewayConfig {
                maximum_pages: mail_config.maximum_pages,
                sender_allowlist: mail_config.sender_allowlist.clone(),
                sender_allowlist_disabled: mail_config.sender_allowlist_disabled,
            },
        )
        .map_err(|error| anyhow::Error::msg(error.to_string()))?,
    );
    let world = MoogleMailWorld::parse(&mail_config.world_id)
        .context("MoogleMail world_id must be main or isekai")?;
    let store_port: Arc<dyn MailPollStore> = store;
    store_port
        .ensure_state(mail_config.mailbox_key.clone(), Utc::now())
        .await
        .context("initialize MoogleMail poll state")?;
    let gateway_port: Arc<dyn MailPollGateway> =
        Arc::new(MoogleMailWorkerGateway { gateway, world });
    Ok(Some(MailPollRuntime {
        store: store_port,
        gateway: gateway_port,
        world_id: mail_config.world_id.clone(),
        batch_size: mail_config.batch_size,
    }))
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
struct MoogleMailWorkerGateway {
    gateway: Arc<MoogleMailGateway>,
    world: MoogleMailWorld,
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
impl MailPollGateway for MoogleMailWorkerGateway {
    fn fetch(
        &self,
        request: MailPollFetchRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<MailPollPage, MailPollGatewayError>>
                + Send
                + '_,
        >,
    > {
        let gateway = Arc::clone(&self.gateway);
        let world = self.world;
        Box::pin(async move {
            if request.world_id != world.as_str() {
                return Err(MailPollGatewayError {
                    failure_code: "configuration".to_owned(),
                    safe_detail: "MoogleMail Worker 世界配置与 gateway 不一致".to_owned(),
                    temporary: false,
                });
            }
            let page = gateway
                .fetch(world, request.cursor_value.as_deref(), usize::from(request.limit))
                .await
                .map_err(|error| MailPollGatewayError {
                    failure_code: error.category.as_str().to_owned(),
                    safe_detail: format!("MoogleMail {} 操作失败", error.category),
                    temporary: error.is_temporary(),
                })?;
            let messages = page.messages.into_iter().map(map_mooglemail_message).collect();
            Ok(MailPollPage {
                mailbox_key: request.mailbox_key,
                world_id: world.as_str().to_owned(),
                next_cursor: page.next_cursor,
                messages,
            })
        })
    }
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
fn map_mooglemail_message(mut message: MoogleMailMessage) -> MailPollFetchedMessage {
    let result = MailPollFetchedMessage {
        external_mail_id: message.external_mail_id.clone(),
        world_id: message.world.as_str().to_owned(),
        received_at: message.received_at,
        subject: message.subject.clone(),
        content_digest: message.content_digest(),
        attachment_count: u32::try_from(message.attachments.len()).unwrap_or(u32::MAX),
        credits: message.credits,
        cod: message.cod,
        returnable: message.returnable,
        claimable: message.claimable,
    };
    message.clear_secrets();
    result
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
