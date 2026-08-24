use std::{pin::Pin, sync::Arc};

#[cfg(feature = "postgres")]
use std::collections::{HashMap, HashSet};

use anyhow::Context;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_application::{
    AdminCredentialKind, AdminCredentialOpener, AdminCredentialRuntimeReader,
    AdminCredentialRuntimeWriter, AdminCredentialWorld, ApplicationError, ApplicationFuture,
    AuctionLifecycleCommandPort, Clock, CompetitionCommandPort, DeliveryTransport,
    DeliveryWorkStore, ForumPublisher, ForumReplyStore, ForumSyncStore, ForumTopicReader,
    MailDeliveryEnqueuer, OperationsAlertSourceReader, OperationsAlertStore,
    RecoveryBuyerBlacklister, RecoveryHealthConfig, RecoveryHealthService, RecoveryResolutionStore,
    RecoveryWorkStore, ScheduledRoundStore, SettlementCommandPort,
    SettlementFulfillmentBalanceEvidence, SettlementFulfillmentCommandPort,
    SettlementFulfillmentEvidenceReader, SettlementFulfillmentSentEvidence,
    SettlementFulfillmentService, SettlementFulfillmentStore, SettlementWorkStore,
};
#[cfg(feature = "postgres")]
use auction_application::{
    MailAuctionIntakeService, MailEquipmentFilter, MailLoginKeyReader, MailPollStore,
    MailProcessor, MailProcessorConfig, MailUserService, RecurringSchedule,
};
use auction_cache::CacheStore;
use auction_config::{AppConfig, CacheBackend, Role};
#[cfg(feature = "postgres")]
use auction_config::{StandardCronSchedule, load_mooglemail_keyenv};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_domain::SettlementWorld;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_integrations::ehentai::{
    EhentaiClient, EhentaiClientConfig, EhentaiCookie, EhentaiCredentialSet, EhentaiGateway,
};
#[cfg(feature = "postgres")]
use auction_integrations::mooglemail::{
    MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailCredentialSet,
    MoogleMailDeliveryTransport, MoogleMailError, MoogleMailErrorCategory, MoogleMailGateway,
    MoogleMailGatewayConfig, MoogleMailMessage, MoogleMailNotificationTemplates, MoogleMailWorld,
    into_application_message,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_integrations::{CredentialSealer, DataKey, ForumThreadRenderer};
#[cfg(feature = "postgres")]
use auction_integrations::{
    EquipmentKeySealer, EquipmentSourceAdapter, MoogleMailLoginKeyIssuer, MoogleMailLoginKeyReader,
    build_equipment_http_client,
};
#[cfg(feature = "postgres")]
use auction_jobs::{
    MailPollFetchRequest, MailPollFetchedMessage, MailPollGateway, MailPollGatewayError,
    MailPollPage, MailPollRuntimeConfig,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_jobs::{MailPollRuntime, RecoveryHealthRuntime};
#[cfg(any(feature = "postgres", feature = "sqlite", unix))]
use auction_jobs::{WorkerControlHandle, WorkerControlServer, WorkerControlServerConfig};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_observability::{HeartbeatPublisher, HeartbeatStore, ProcessRole, ServiceName};
use auction_observability::{ProcessMetrics, TracingConfig, TracingRuntime};
use auction_process::{
    DEFAULT_DRAIN_TIMEOUT, DrainOutcome, TaskGuard, TerminationSignals, wait_for_drain,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_storage::{
    DatabaseAuctionLifecycleCommands, DatabaseBackend, DatabaseCompetitionCommands,
    DatabaseSettlementCommands, DatabaseStore,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use chrono::{DateTime, Utc};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use url::Url;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use uuid::Uuid;

type ShutdownFuture = Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>;

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
    Option<Arc<dyn SettlementFulfillmentEvidenceReader>>,
);

pub async fn run() -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let tracing_config = tracing_config(&config.tracing);
    let telemetry = auction_observability::init_with_config(
        "auction_worker",
        "worker",
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
    let mut signals = TerminationSignals::subscribe().context("subscribe worker signals")?;
    let metrics = Arc::new(
        ProcessMetrics::new_with_tracing_health(
            config.metrics.enabled,
            "auction_worker",
            "worker",
            env!("CARGO_PKG_VERSION"),
            telemetry.health_reader(),
        )
        .context("initialize Prometheus metrics")?,
    );
    let cache = configure_cache(config).context("configure worker cache")?;
    tracing::info!(
        database_configured = config.database_url().is_some(),
        cache_backend = cache.backend(),
        worker_concurrency = config.worker_concurrency(),
        "worker configured"
    );

    let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
    let worker = run_worker(
        config,
        Arc::clone(&metrics),
        Box::pin(async move {
            let _ = shutdown_receiver.await;
        }),
    );
    tokio::pin!(worker);
    let result = tokio::select! {
        result = &mut worker => {
            drop(shutdown_sender);
            result
        }
        signal = signals.recv() => {
            let signal = signal.context("receive worker shutdown signal")?;
            tracing::info!(signal = %signal, "worker drain started");
            let _ = shutdown_sender.send(());
            match wait_for_drain(&mut signals, DEFAULT_DRAIN_TIMEOUT, &mut worker).await {
                Ok(DrainOutcome::Completed(result)) => result,
                Ok(DrainOutcome::TimedOut) => {
                    Err(anyhow::anyhow!("worker drain exceeded 10 seconds"))
                }
                Ok(DrainOutcome::Interrupted(signal)) => {
                    Err(anyhow::anyhow!("worker drain interrupted by {signal}"))
                }
                Err(error) => Err(error).context("wait for worker drain"),
            }
        }
    };
    if result.is_ok() {
        tracing::info!("worker stopped");
    }
    result
}

async fn shutdown_tracing(telemetry: TracingRuntime) {
    match tokio::task::spawn_blocking(move || telemetry.shutdown()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(error = %error, "worker tracing shutdown failed"),
        Err(error) => tracing::warn!(error = %error, "worker tracing shutdown task failed"),
    }
}

fn tracing_config(settings: &auction_config::TracingSettings) -> TracingConfig {
    TracingConfig {
        enabled: settings.enabled,
        endpoint: settings.endpoint.clone(),
        batch_timeout: std::time::Duration::from_secs(settings.batch_timeout_seconds),
        export_timeout: std::time::Duration::from_secs(settings.export_timeout_seconds),
        queue_capacity: settings.queue_capacity as usize,
        max_export_batch_size: settings.max_export_batch_size as usize,
        sample_ratio: f64::from(settings.sample_ratio_ppm) / 1_000_000.0,
        shutdown_timeout: std::time::Duration::from_secs(settings.shutdown_timeout_seconds),
    }
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
async fn run_worker(
    config: &AppConfig,
    metrics: Arc<ProcessMetrics>,
    shutdown: ShutdownFuture,
) -> anyhow::Result<()> {
    let Some(database_url) = config.database_url() else {
        if config.worker.mooglemail.enabled
            || config.worker.forum.enabled
            || config.worker.operations_alert.enabled
        {
            anyhow::bail!("启用外部 Worker 功能时必须配置 PostgreSQL database_url");
        }
        run_worker_skeleton(config, metrics, shutdown).await?;
        return Ok(());
    };
    let store =
        Arc::new(DatabaseStore::connect(database_url).await.context("connect worker database")?);
    metrics
        .register_pool_metrics(store.pool_metrics_source())
        .context("register worker database pool metrics")?;
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
    let (mail_polls, mail_delivery_transport, mail_evidence_reader) =
        configure_mail_poll(config, Arc::clone(&store)).await?;
    let operations_alert =
        configure_operations_alert(config, Arc::clone(&store), mail_delivery_transport.is_some())
            .await?;
    let recovery_delivery_enabled = mail_delivery_transport.is_some();
    let (forum_sync, forum_reply) =
        configure_forum(config, Arc::clone(&store), mail_delivery_transport.is_some()).await?;
    let (
        settlement_store,
        settlement_commands,
        fulfillment_store,
        fulfillment_commands,
        delivery_store,
        delivery_transport,
        evidence_reader_for_health,
    ): SettlementPorts = match store.backend() {
        DatabaseBackend::Postgres => {
            let fulfillment_store: Arc<dyn SettlementFulfillmentStore> = store.clone();
            let evidence_reader: Arc<dyn SettlementFulfillmentEvidenceReader> =
                mail_evidence_reader
                    .unwrap_or_else(|| Arc::new(UnavailableFulfillmentEvidenceReader));
            let fulfillment_commands: Arc<dyn SettlementFulfillmentCommandPort> =
                Arc::new(SettlementFulfillmentService::new(
                    Arc::clone(&fulfillment_store),
                    Arc::clone(&evidence_reader),
                ));
            (
                Some(store.clone()),
                Some(Arc::new(DatabaseSettlementCommands::new(Arc::clone(&store)))),
                Some(fulfillment_store),
                Some(fulfillment_commands),
                Some(store.clone()),
                mail_delivery_transport,
                Some(evidence_reader),
            )
        }
        DatabaseBackend::Sqlite => (None, None, None, None, None, None, None),
    };
    let control = WorkerControlHandle::new(env!("CARGO_PKG_VERSION"), mail_polls.len());
    let control_shutdown = Arc::new(tokio::sync::Notify::new());
    let mut control_task = if config.worker.control.enabled {
        let server_config = worker_control_server_config(config)?;
        let server = WorkerControlServer::bind_with_metrics(
            server_config,
            control.clone(),
            Some(Arc::clone(&metrics)),
        )
        .await
        .context("bind Worker control socket")?;
        let shutdown = Arc::clone(&control_shutdown);
        Some(TaskGuard::new(tokio::spawn(async move {
            server.run(async move { shutdown.notified().await }).await
        })))
    } else {
        None
    };
    let recovery_store: Option<Arc<dyn RecoveryWorkStore>> = match store.backend() {
        DatabaseBackend::Postgres => Some(store.clone()),
        DatabaseBackend::Sqlite => None,
    };
    let recovery_resolution: Option<Arc<dyn RecoveryResolutionStore>> = match store.backend() {
        DatabaseBackend::Postgres => Some(store.clone()),
        DatabaseBackend::Sqlite => None,
    };
    let recovery_delivery: Option<Arc<dyn MailDeliveryEnqueuer>> =
        match (store.backend(), recovery_delivery_enabled) {
            (DatabaseBackend::Postgres, true) => Some(store.clone()),
            _ => None,
        };
    let recovery_blacklister: Option<Arc<dyn RecoveryBuyerBlacklister>> = match store.backend() {
        DatabaseBackend::Postgres => Some(store.clone()),
        DatabaseBackend::Sqlite => None,
    };
    let recovery_worlds = if config.worker.mooglemail.enabled {
        config
            .worker
            .mooglemail
            .configured_worlds()
            .into_iter()
            .filter_map(|world| match world.world_id.as_str() {
                "main" => Some(SettlementWorld::Main),
                "isekai" => Some(SettlementWorld::Isekai),
                _ => None,
            })
            .collect()
    } else {
        vec![SettlementWorld::Main, SettlementWorld::Isekai]
    };
    let recovery_health = match (store.backend(), evidence_reader_for_health) {
        (DatabaseBackend::Postgres, Some(balance_reader)) => {
            let health_service = RecoveryHealthService::new(
                RecoveryHealthConfig {
                    worlds: recovery_worlds,
                    low_balance_threshold: config.worker.recovery.low_balance_threshold,
                    evidence_stale_after: std::time::Duration::from_secs(
                        config.worker.recovery.evidence_stale_after_seconds,
                    ),
                },
                store.clone(),
                balance_reader,
            )
            .map_err(|error| anyhow::Error::msg(error.to_owned()))?;
            Some(
                RecoveryHealthRuntime::new(
                    Arc::new(health_service),
                    std::time::Duration::from_secs(
                        config.worker.recovery.health_refresh_interval_seconds,
                    ),
                )
                .map_err(|error| anyhow::Error::msg(error.to_owned()))?,
            )
        }
        (DatabaseBackend::Postgres, None) => {
            anyhow::bail!("PostgreSQL recovery health 缺少 Balance 读取端口")
        }
        (DatabaseBackend::Sqlite, None) => None,
        (DatabaseBackend::Sqlite, Some(_)) => None,
    };
    if let Err(error) = heartbeat.publish_ready().await {
        tracing::warn!(error = %error, "worker heartbeat ready publish failed");
    }
    let mut heartbeat_task = Some(TaskGuard::new(spawn_heartbeat(Arc::clone(&heartbeat))));
    let auction_poll_interval =
        std::time::Duration::from_secs(config.worker_poll_interval_seconds());
    let auction_lease_duration = std::time::Duration::from_secs(
        config.worker_poll_interval_seconds().saturating_mul(3).max(30),
    );
    let result = auction_jobs::run_persistent_with_settlement_until_shutdown(
        auction_jobs::PersistentWorker {
            schedule,
            lifecycle,
            auction: auction_jobs::AuctionWorkerRuntimeConfig {
                batch_limit: config.worker.batch_limit,
                concurrency: config.worker_concurrency(),
                poll_interval: auction_poll_interval,
                jitter: std::time::Duration::from_secs(config.worker.jitter_seconds),
                lease_duration: auction_lease_duration,
                item_timeout: std::time::Duration::from_secs(config.worker.item_timeout_seconds),
            },
            settlement_store,
            settlement_commands,
            fulfillment_store,
            fulfillment_commands,
            fulfillment: auction_jobs::FulfillmentWorkerRuntimeConfig {
                batch_limit: config.worker.fulfillment.batch_limit,
                concurrency: config.worker.fulfillment.concurrency,
                poll_interval: std::time::Duration::from_secs(
                    config.worker.fulfillment.poll_interval_seconds,
                ),
                jitter: std::time::Duration::from_secs(config.worker.fulfillment.jitter_seconds),
                lease_duration: std::time::Duration::from_secs(
                    config.worker.fulfillment.lease_duration_seconds,
                ),
                item_timeout: std::time::Duration::from_secs(
                    config.worker.fulfillment.item_timeout_seconds,
                ),
                retry_delay: std::time::Duration::from_secs(
                    config.worker.fulfillment.retry_delay_seconds,
                ),
            },
            recovery_health,
            recovery_store,
            recovery_resolution,
            recovery_delivery,
            recovery_blacklister,
            recovery: auction_jobs::RecoveryWorkerRuntimeConfig {
                batch_limit: config.worker.recovery.batch_limit,
                concurrency: config.worker.recovery.concurrency,
                poll_interval: std::time::Duration::from_secs(
                    config.worker.recovery.poll_interval_seconds,
                ),
                jitter: std::time::Duration::from_secs(config.worker.recovery.jitter_seconds),
                lease_duration: std::time::Duration::from_secs(
                    config.worker.recovery.lease_duration_seconds,
                ),
                item_timeout: std::time::Duration::from_secs(
                    config.worker.recovery.item_timeout_seconds,
                ),
                retry_delay: std::time::Duration::from_secs(
                    config.worker.recovery_retry_delay_seconds,
                ),
            },
            delivery_store,
            delivery_transport,
            delivery: auction_jobs::DeliveryWorkerRuntimeConfig {
                batch_limit: config.worker.delivery.batch_limit,
                concurrency: config.worker.delivery.concurrency,
                poll_interval: std::time::Duration::from_secs(
                    config.worker.delivery.poll_interval_seconds,
                ),
                jitter: std::time::Duration::from_secs(config.worker.delivery.jitter_seconds),
                lease_duration: std::time::Duration::from_secs(
                    config.worker.delivery.lease_duration_seconds,
                ),
                renew_interval: std::time::Duration::from_secs(
                    config.worker.delivery.renew_interval_seconds,
                ),
                sending_grace: std::time::Duration::from_secs(
                    config.worker.delivery.sending_grace_seconds,
                ),
                request_timeout: std::time::Duration::from_secs(
                    config.worker.delivery.request_timeout_seconds,
                ),
                finalize_timeout: std::time::Duration::from_secs(
                    config.worker.delivery.finalize_timeout_seconds,
                ),
            },
            mail_polls,
            forum: auction_jobs::ForumWorkerRuntimeConfig {
                poll_interval: auction_poll_interval,
                jitter: std::time::Duration::from_secs(config.worker.jitter_seconds),
                lease_duration: std::time::Duration::from_secs(
                    config.worker.forum.lease_duration_seconds,
                ),
            },
            forum_sync,
            forum_reply,
            operations_alert,
            // 外部广播协议尚未冻结；没有具体 sink 时不启动实时消费，避免伪造转发成功。
            realtime_consumer: None,
            metrics: metrics.worker(),
            control: control.clone(),
            clock: Arc::new(SystemClock),
            worker_id,
        },
        shutdown,
    )
    .await;
    control.mark_draining();
    abort_worker_task(&heartbeat_task);
    join_worker_task(&mut heartbeat_task, "worker heartbeat").await;
    if let Err(error) = heartbeat.publish_stopping().await {
        tracing::warn!(error = %error, "worker heartbeat stopping publish failed");
    }
    control_shutdown.notify_waiters();
    if let Err(error) =
        join_worker_control_task(&mut control_task, "persistent worker control").await
    {
        tracing::warn!(error = %error, "Worker control server stopped with an error");
    }
    result.context("run persistent worker")?;
    Ok(())
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
async fn configure_forum(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
    delivery_enabled: bool,
) -> anyhow::Result<(Option<auction_jobs::ForumSyncRuntime>, Option<auction_jobs::ForumReplyRuntime>)>
{
    let forum_config = &config.worker.forum;
    if !forum_config.enabled {
        return Ok((None, None));
    }
    if store.backend() != DatabaseBackend::Postgres {
        anyhow::bail!("E-Hentai 论坛同步仅支持 PostgreSQL 完整档");
    }

    let credential_world = AdminCredentialWorld::parse(&forum_config.credential_world)
        .context("forum credential_world must be main, isekai, or shared")?;
    let credentials = if let Some((key_id, key_material)) =
        config.credential_data_key_id().zip(config.credential_data_key())
    {
        let data_key = Arc::new(
            DataKey::from_base64(key_id, key_material)
                .map_err(|error| anyhow::Error::msg(error.to_string()))?,
        );
        let opener = CredentialSealer::new(Arc::clone(&data_key));
        let reader: Arc<dyn AdminCredentialRuntimeReader> = store.clone();
        let records = reader
            .list_active(AdminCredentialKind::ForumCookie, credential_world)
            .await
            .context("load active E-Hentai forum credentials")?;
        let mut cookies = Vec::with_capacity(records.len());
        for record in records {
            let secret = opener
                .open(record.kind, record.id, &record.scope, &record.envelope)
                .map_err(|error| anyhow::Error::msg(error.to_string()))?;
            cookies.push((
                record.id,
                EhentaiCookie::from_bytes(&secret)
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            ));
        }
        let invalidator: Arc<dyn AdminCredentialRuntimeWriter> = store.clone();
        EhentaiCredentialSet::from_runtime_cookies(cookies, invalidator)
    } else {
        let cookie = config
            .ehentai_cookie()
            .context("forum enabled but no ehentai_cookie or credential data key")?;
        EhentaiCredentialSet::from_cookies(vec![
            EhentaiCookie::from_bytes(cookie.as_bytes())
                .map_err(|error| anyhow::Error::msg(error.to_string()))?,
        ])
    }
    .map_err(|error| anyhow::Error::msg(error.to_string()))?;

    let endpoint = forum_config
        .endpoint
        .as_deref()
        .map(Url::parse)
        .transpose()
        .context("parse E-Hentai forum endpoint")?
        .unwrap_or(Url::parse(auction_integrations::ehentai::PRODUCTION_ENDPOINT)?);
    let client = Arc::new(
        EhentaiClient::with_config(EhentaiClientConfig {
            endpoint,
            maximum_response_bytes: forum_config.maximum_response_bytes,
            minimum_request_interval: std::time::Duration::from_millis(
                forum_config.minimum_request_interval_millis,
            ),
            request_timeout: std::time::Duration::from_secs(forum_config.request_timeout_seconds),
            test_board_mode: forum_config.test_board_mode,
        })
        .map_err(|error| anyhow::Error::msg(error.to_string()))?,
    );
    let gateway = Arc::new(
        EhentaiGateway::new(Arc::clone(&client), credentials)
            .map_err(|error| anyhow::Error::msg(error.to_string()))?,
    );
    let store_port: Arc<dyn ForumSyncStore> = store.clone();
    let publisher: Arc<dyn ForumPublisher> = gateway.clone();
    let renderer = Arc::new(ForumThreadRenderer);
    let sync = Some(auction_jobs::ForumSyncRuntime::new(
        store_port,
        publisher,
        renderer,
        forum_config.batch_size,
        std::time::Duration::from_secs(forum_config.item_timeout_seconds),
        forum_config.public_base_url.clone(),
    )?);
    let reply_store: Arc<dyn ForumReplyStore> = store.clone();
    let reader: Arc<dyn ForumTopicReader> = gateway.clone();
    let commands: Arc<dyn CompetitionCommandPort> =
        Arc::new(DatabaseCompetitionCommands::new(Arc::clone(&store), Arc::new(SystemClock)));
    let notifier: Option<Arc<dyn MailDeliveryEnqueuer>> =
        if delivery_enabled { Some(store.clone()) } else { None };
    let reply = Some(auction_jobs::ForumReplyRuntime::new(
        reply_store,
        reader,
        commands,
        notifier,
        forum_config.batch_size,
        std::time::Duration::from_secs(forum_config.item_timeout_seconds),
    )?);
    Ok((sync, reply))
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
async fn configure_mail_poll(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
) -> anyhow::Result<(
    Vec<MailPollRuntime>,
    Option<Arc<dyn DeliveryTransport>>,
    Option<Arc<dyn SettlementFulfillmentEvidenceReader>>,
)> {
    let mail_config = &config.worker.mooglemail;
    if !mail_config.enabled {
        return Ok((Vec::new(), None, None));
    }

    #[cfg(not(feature = "postgres"))]
    {
        let _ = store;
        anyhow::bail!("MoogleMail Worker 轮询仅支持 PostgreSQL 完整档");
    }

    #[cfg(feature = "postgres")]
    {
        if store.backend() != DatabaseBackend::Postgres {
            anyhow::bail!("MoogleMail Worker 轮询仅支持 PostgreSQL 完整档");
        }
        let world_configs = mail_config.configured_worlds();
        let schedule: Arc<dyn RecurringSchedule> = Arc::new(
            StandardCronSchedule::parse(&mail_config.schedule)
                .context("parse MoogleMail standard cron schedule")?,
        );
        let credentials = if let Some((key_id, key_material)) =
            config.credential_data_key_id().zip(config.credential_data_key())
        {
            let data_key = Arc::new(
                DataKey::from_base64(key_id, key_material)
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            );
            let opener = CredentialSealer::new(Arc::clone(&data_key));
            let reader: Arc<dyn AdminCredentialRuntimeReader> = store.clone();
            let mut records = Vec::new();
            let mut seen_record_ids = HashSet::new();
            for world_config in &world_configs {
                let credential_world = AdminCredentialWorld::parse(&world_config.world_id)
                    .context("MoogleMail world_id must be main or isekai")?;
                for record in reader
                    .list_active(AdminCredentialKind::MailCookie, credential_world)
                    .await
                    .context("load active MoogleMail credentials")?
                {
                    // shared scope 会在 main/isekai 两次查询中出现，只把同一凭据放入池一次。
                    if seen_record_ids.insert(record.id) {
                        records.push((record, credential_world));
                    }
                }
            }
            let mut cookies = Vec::with_capacity(records.len());
            for (record, credential_world) in records {
                let secret = opener
                    .open(
                        record.kind,
                        record.id,
                        &record.scope,
                        &record.envelope,
                    )
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?;
                let value = match String::from_utf8(secret) {
                    Ok(value) => value,
                    Err(error) => {
                        let mut bytes = error.into_bytes();
                        bytes.fill(0);
                        anyhow::bail!("MoogleMail 凭据不是有效 UTF-8");
                    }
                };
                cookies.push((
                    record.id,
                    if record.scope
                        == AdminCredentialWorld::Shared.scope(AdminCredentialKind::MailCookie)
                    {
                        None
                    } else {
                        Some(match credential_world {
                            AdminCredentialWorld::Main => MoogleMailWorld::Main,
                            AdminCredentialWorld::Isekai => MoogleMailWorld::Isekai,
                            AdminCredentialWorld::Shared => {
                                anyhow::bail!("MoogleMail 凭据 world scope 无效")
                            }
                        })
                    },
                    MoogleMailCookie::new(value)
                        .map_err(|error| anyhow::Error::msg(error.to_string()))?,
                ));
            }
            let invalidator: Arc<dyn AdminCredentialRuntimeWriter> = store.clone();
            MoogleMailCredentialSet::from_runtime_cookies_with_scopes(cookies, invalidator)
        } else if let Some(path) = &mail_config.keyenv_file {
            let keyenv = load_mooglemail_keyenv(path).context("load MoogleMail keyenv")?;
            MoogleMailCredentialSet::new(keyenv.bot_cookie(), keyenv.test_cookie())
        } else {
            let cookie = config.mooglemail_cookie().context(
                "MoogleMail enabled but no keyenv_file, credential pool, or sensitive mooglemail_cookie",
            )?;
            MoogleMailCredentialSet::new(cookie, cookie)
        }
        .map_err(|error| anyhow::Error::msg(error.to_string()))?;
        let endpoint = Url::parse(&mail_config.endpoint).context("parse MoogleMail endpoint")?;
        let client = Arc::new(
            MoogleMailClient::with_config(MoogleMailClientConfig {
                endpoint,
                maximum_response_bytes: mail_config.maximum_response_bytes,
                minimum_request_interval: std::time::Duration::from_millis(
                    mail_config.minimum_request_interval_millis,
                ),
                request_timeout: std::time::Duration::from_secs(
                    mail_config.request_timeout_seconds,
                ),
            })
            .map_err(|error| anyhow::Error::msg(error.to_string()))?,
        );
        let (processor, delivery_transport): (Arc<dyn MailProcessor>, Arc<dyn DeliveryTransport>) =
            match store.as_ref() {
                DatabaseStore::Postgres(postgres_store) => {
                    let data_key_id = config
                        .equipment_data_key_id()
                        .context("MoogleMail business processing requires equipment_data_key_id")?;
                    let data_key_material = config
                        .equipment_data_key()
                        .context("MoogleMail business processing requires equipment_data_key")?;
                    let data_key = DataKey::from_base64(data_key_id, data_key_material)
                        .map_err(|error| anyhow::Error::msg(error.to_string()))?;
                    let sealer = Arc::new(EquipmentKeySealer::new(data_key));
                    let equipment_client = build_equipment_http_client()
                        .map_err(|error| anyhow::Error::msg(error.to_string()))?;
                    let equipment = Arc::new(
                        EquipmentSourceAdapter::production(equipment_client, 0)
                            .map_err(|error| anyhow::Error::msg(error.to_string()))?,
                    );
                    let auctions = Arc::new(MailAuctionIntakeService::new(
                        Arc::new(postgres_store.clone()),
                        sealer,
                        Arc::new(SystemClock),
                    ));
                    let deliveries = Arc::clone(&store);
                    let user_data_key_id = config.user_login_data_key_id().context(
                        "MoogleMail business processing requires user_login_data_key_id",
                    )?;
                    let user_data_key_material = config
                        .user_login_data_key()
                        .context("MoogleMail business processing requires user_login_data_key")?;
                    let user_data_key = Arc::new(
                        DataKey::from_base64(user_data_key_id, user_data_key_material)
                            .map_err(|error| anyhow::Error::msg(error.to_string()))?,
                    );
                    let key_issuer =
                        Arc::new(MoogleMailLoginKeyIssuer::new(Arc::clone(&user_data_key)));
                    let users = Arc::new(MailUserService::new(Arc::clone(&store), key_issuer));
                    let login_keys: Arc<dyn MailLoginKeyReader> =
                        Arc::new(MoogleMailLoginKeyReader::new(Arc::clone(&store), user_data_key));
                    let notification_templates = MoogleMailNotificationTemplates::from_values(
                        mail_config.mail_templates.values(),
                    )
                    .map_err(|error| anyhow::Error::msg(error.to_string()))?;
                    let delivery_transport: Arc<dyn DeliveryTransport> =
                        Arc::new(MoogleMailDeliveryTransport::with_templates(
                            (*client).clone(),
                            credentials.clone(),
                            login_keys,
                            notification_templates,
                        ));
                    let processor: Arc<dyn MailProcessor> = Arc::new(
                        auction_application::MailProcessorService::new_for_worlds(
                            world_configs.iter().map(|world| world.world_id.clone()).collect(),
                            MailProcessorConfig {
                                start_price: mail_config
                                    .start_price
                                    .unwrap_or(config.auction.economics.start_price),
                                equipment_filter: MailEquipmentFilter {
                                    enabled: mail_config.equipment_filter.enabled,
                                    qualities: mail_config.equipment_filter.qualities.clone(),
                                    prefixes: mail_config.equipment_filter.prefixes.clone(),
                                    suffixes: mail_config.equipment_filter.suffixes.clone(),
                                    categories: mail_config.equipment_filter.categories.clone(),
                                    equipment_types: mail_config
                                        .equipment_filter
                                        .equipment_types
                                        .clone(),
                                    level_min: mail_config.equipment_filter.level_min,
                                    level_max: mail_config.equipment_filter.level_max,
                                },
                            },
                            equipment,
                            auctions,
                            deliveries,
                            users,
                            Arc::new(SystemClock),
                        )
                        .map_err(|error| anyhow::Error::msg(error.to_string()))?,
                    );
                    (processor, delivery_transport)
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    anyhow::bail!("MoogleMail 业务处理仅支持 PostgreSQL 完整档");
                }
            };
        let store_port: Arc<dyn MailPollStore> = store.clone();
        let mut gateways = HashMap::with_capacity(world_configs.len());
        let mut runtimes = Vec::with_capacity(world_configs.len());
        for world_config in world_configs {
            let world = MoogleMailWorld::parse(&world_config.world_id)
                .context("MoogleMail world_id must be main or isekai")?;
            let gateway = Arc::new(
                MoogleMailGateway::new(
                    Arc::clone(&client),
                    credentials.clone(),
                    MoogleMailGatewayConfig {
                        maximum_pages: mail_config.maximum_pages,
                        sender_allowlist: world_config.sender_allowlist,
                        sender_allowlist_disabled: world_config.sender_allowlist_disabled,
                    },
                )
                .map_err(|error| anyhow::Error::msg(error.to_string()))?,
            );
            store_port
                .ensure_state(world_config.mailbox_key.clone(), Utc::now())
                .await
                .context("initialize MoogleMail poll state")?;
            gateways.insert(world, Arc::clone(&gateway));
            let gateway_port: Arc<dyn MailPollGateway> =
                Arc::new(MoogleMailWorkerGateway { gateway, world });
            runtimes.push(MailPollRuntime::new(
                Arc::clone(&store_port),
                gateway_port,
                Some(Arc::clone(&processor)),
                world_config.world_id,
                world_config.mailbox_key,
                Arc::clone(&schedule),
                MailPollRuntimeConfig {
                    batch_size: mail_config.batch_size,
                    jitter: std::time::Duration::from_secs(mail_config.jitter_seconds),
                    lease_duration: std::time::Duration::from_secs(
                        mail_config.lease_duration_seconds,
                    ),
                    renew_interval: std::time::Duration::from_secs(
                        mail_config.renew_interval_seconds,
                    ),
                    fetch_attempts: mail_config.fetch_attempts,
                    fetch_timeout: std::time::Duration::from_secs(
                        mail_config.fetch_timeout_seconds,
                    ),
                    process_attempts: mail_config.process_attempts,
                    item_timeout: std::time::Duration::from_secs(mail_config.item_timeout_seconds),
                    retry_base: std::time::Duration::from_secs(mail_config.retry_base_seconds),
                    retry_maximum: std::time::Duration::from_secs(
                        mail_config.retry_maximum_seconds,
                    ),
                },
            )?);
        }
        let evidence_reader: Arc<dyn SettlementFulfillmentEvidenceReader> =
            Arc::new(MoogleMailFulfillmentEvidenceReader { gateways });
        Ok((runtimes, Some(delivery_transport), Some(evidence_reader)))
    }
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
async fn configure_operations_alert(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
    delivery_enabled: bool,
) -> anyhow::Result<Option<Arc<auction_jobs::OperationsAlertRuntime>>> {
    let alert_config = &config.worker.operations_alert;
    if !alert_config.enabled {
        return Ok(None);
    }
    if store.backend() != DatabaseBackend::Postgres {
        anyhow::bail!("operations alert 仅支持 PostgreSQL 完整档");
    }
    if !delivery_enabled {
        anyhow::bail!("启用 operations alert 时必须同时启用 MoogleMail delivery Worker");
    }
    let source: Arc<dyn OperationsAlertSourceReader> = store.clone();
    let alert_store: Arc<dyn OperationsAlertStore> = store.clone();
    let delivery: Arc<dyn MailDeliveryEnqueuer> = store;
    let claim_lease = std::time::Duration::from_secs(alert_config.claim_lease_seconds);
    let source_timeout = std::time::Duration::from_secs(alert_config.source_timeout_seconds);
    let dispatch_timeout = claim_lease
        .checked_sub(source_timeout)
        .context("operations alert dispatch timeout must remain inside claim lease")?;
    auction_jobs::OperationsAlertRuntime::new(
        source,
        alert_store,
        delivery,
        alert_config.policy(),
        alert_config.target(),
        auction_jobs::OperationsAlertRuntimeConfig {
            poll_interval: std::time::Duration::from_secs(alert_config.scan_interval_seconds),
            scan_timeout: claim_lease,
            dispatch_timeout,
            reconcile_timeout: claim_lease,
            reconcile_limit: u16::try_from(alert_config.max_batch_items)
                .context("operations alert reconcile limit exceeds u16")?,
        },
    )
    .map(Arc::new)
    .map(Some)
    .map_err(|error| anyhow::Error::msg(error.to_string()))
}

#[cfg(feature = "postgres")]
struct MoogleMailWorkerGateway {
    gateway: Arc<MoogleMailGateway>,
    world: MoogleMailWorld,
}

#[cfg(feature = "postgres")]
struct MoogleMailFulfillmentEvidenceReader {
    gateways: HashMap<MoogleMailWorld, Arc<MoogleMailGateway>>,
}

#[cfg(feature = "postgres")]
fn fulfillment_gateway(
    gateways: &HashMap<MoogleMailWorld, Arc<MoogleMailGateway>>,
    world: SettlementWorld,
    unavailable_detail: &'static str,
) -> Result<(MoogleMailWorld, Arc<MoogleMailGateway>), ApplicationError> {
    let world = mooglemail_world(world)?;
    let gateway = gateways
        .get(&world)
        .cloned()
        .ok_or(ApplicationError::CapabilityUnavailable(unavailable_detail))?;
    Ok((world, gateway))
}

#[cfg(feature = "postgres")]
impl SettlementFulfillmentEvidenceReader for MoogleMailFulfillmentEvidenceReader {
    fn read_sent(
        &self,
        world: SettlementWorld,
        external_mail_id: String,
    ) -> ApplicationFuture<'_, SettlementFulfillmentSentEvidence> {
        let (world, gateway) = match fulfillment_gateway(
            &self.gateways,
            world,
            "MoogleMail Sent evidence world unavailable",
        ) {
            Ok(value) => value,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        Box::pin(async move {
            let snapshot = gateway
                .read_sent(world, &external_mail_id, Utc::now())
                .await
                .map_err(mooglemail_evidence_error)?;
            Ok(SettlementFulfillmentSentEvidence {
                external_mail_id: snapshot.external_mail_id,
                world: settlement_world(snapshot.world),
                recipient: snapshot.recipient,
                subject: snapshot.subject,
                remaining_credits: snapshot.remaining_credits,
                cod_pending: snapshot.cod_pending,
                remaining_equipment_ids: snapshot.remaining_equipment_ids,
                collected: snapshot.collected,
                observed_at: snapshot.observed_at,
            })
        })
    }

    fn read_balance(
        &self,
        world: SettlementWorld,
    ) -> ApplicationFuture<'_, SettlementFulfillmentBalanceEvidence> {
        let (world, gateway) = match fulfillment_gateway(
            &self.gateways,
            world,
            "MoogleMail Balance evidence world unavailable",
        ) {
            Ok(value) => value,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        Box::pin(async move {
            let snapshot =
                gateway.read_balance(world, Utc::now()).await.map_err(mooglemail_evidence_error)?;
            Ok(SettlementFulfillmentBalanceEvidence {
                world: settlement_world(snapshot.world),
                credits: snapshot.credits,
                observed_at: snapshot.observed_at,
            })
        })
    }
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
struct UnavailableFulfillmentEvidenceReader;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
impl SettlementFulfillmentEvidenceReader for UnavailableFulfillmentEvidenceReader {
    fn read_sent(
        &self,
        _world: SettlementWorld,
        _external_mail_id: String,
    ) -> ApplicationFuture<'_, SettlementFulfillmentSentEvidence> {
        Box::pin(async {
            Err(ApplicationError::CapabilityUnavailable(
                "MoogleMail Sent evidence reader unavailable",
            ))
        })
    }

    fn read_balance(
        &self,
        _world: SettlementWorld,
    ) -> ApplicationFuture<'_, SettlementFulfillmentBalanceEvidence> {
        Box::pin(async {
            Err(ApplicationError::CapabilityUnavailable(
                "MoogleMail Balance evidence reader unavailable",
            ))
        })
    }
}

#[cfg(feature = "postgres")]
fn mooglemail_world(world: SettlementWorld) -> Result<MoogleMailWorld, ApplicationError> {
    Ok(match world {
        SettlementWorld::Main => MoogleMailWorld::Main,
        SettlementWorld::Isekai => MoogleMailWorld::Isekai,
    })
}

#[cfg(feature = "postgres")]
fn settlement_world(world: MoogleMailWorld) -> SettlementWorld {
    match world {
        MoogleMailWorld::Main => SettlementWorld::Main,
        MoogleMailWorld::Isekai => SettlementWorld::Isekai,
    }
}

#[cfg(feature = "postgres")]
fn mooglemail_evidence_error(error: MoogleMailError) -> ApplicationError {
    if error.category == MoogleMailErrorCategory::NotObserved {
        return ApplicationError::FulfillmentEvidenceNotObserved;
    }
    auction_application::RepositoryError::Failed(format!(
        "MoogleMail {} 证据读取失败",
        error.category
    ))
    .into()
}

#[cfg(feature = "postgres")]
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
            let messages = page
                .messages
                .into_iter()
                .map(|message| map_mooglemail_message(message, request.mailbox_key.clone()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| MailPollGatewayError {
                    failure_code: "protocol_drift".to_owned(),
                    safe_detail: "MoogleMail 业务 envelope 转换失败".to_owned(),
                    temporary: false,
                })?;
            Ok(MailPollPage {
                mailbox_key: request.mailbox_key,
                world_id: world.as_str().to_owned(),
                next_cursor: page.next_cursor,
                messages,
            })
        })
    }
}

#[cfg(feature = "postgres")]
fn map_mooglemail_message(
    message: MoogleMailMessage,
    mailbox_key: String,
) -> Result<MailPollFetchedMessage, auction_integrations::mooglemail::MoogleMailError> {
    let external_mail_id = message.external_mail_id.clone();
    let world_id = message.world.as_str().to_owned();
    let received_at = message.received_at;
    let subject = message.subject.clone();
    let content_digest = message.content_digest();
    let attachment_count = u32::try_from(message.attachments.len()).unwrap_or(u32::MAX);
    let credits = message.credits;
    let cod = message.cod;
    let returnable = message.returnable;
    let claimable = message.claimable;
    let business = into_application_message(message, mailbox_key)?;
    Ok(MailPollFetchedMessage {
        external_mail_id,
        world_id,
        received_at,
        subject,
        content_digest,
        attachment_count,
        credits,
        cod,
        returnable,
        claimable,
        business: Some(business),
    })
}

#[cfg(not(any(feature = "postgres", feature = "sqlite")))]
async fn run_worker(
    config: &AppConfig,
    metrics: Arc<ProcessMetrics>,
    shutdown: ShutdownFuture,
) -> anyhow::Result<()> {
    if config.database_url().is_some() {
        anyhow::bail!("worker 配置了数据库，但当前制品未编译 postgres 或 sqlite feature");
    }
    run_worker_skeleton(config, metrics, shutdown).await?;
    Ok(())
}

/// 运行没有数据库的启动骨架，并在启用时提供同一套本地 Worker control 观察面。
///
/// 该路径仍然不领取任务、不伪造数据库就绪；control 只报告进程存活和空的 poller 能力，
/// 让 Admin 在演示/部署检查时可以区分“进程已启动”和“持久化 Worker 已可用”。
#[cfg(unix)]
async fn run_worker_skeleton(
    config: &AppConfig,
    metrics: Arc<ProcessMetrics>,
    shutdown: ShutdownFuture,
) -> anyhow::Result<()> {
    if !config.worker.control.enabled {
        auction_jobs::run_until_shutdown(config.worker_concurrency(), shutdown)
            .await
            .context("run worker skeleton without database")?;
        return Ok(());
    }

    let control = WorkerControlHandle::new(env!("CARGO_PKG_VERSION"), 0);
    control.mark_serving();
    let server_config = worker_control_server_config(config)?;
    let server =
        WorkerControlServer::bind_with_metrics(server_config, control.clone(), Some(metrics))
            .await
            .context("bind Worker control socket for worker skeleton")?;
    let control_shutdown = Arc::new(tokio::sync::Notify::new());
    let control_stop = Arc::clone(&control_shutdown);
    let mut control_task = Some(TaskGuard::new(tokio::spawn(async move {
        server.run(async move { control_stop.notified().await }).await
    })));

    let worker_result = auction_jobs::run_until_shutdown(config.worker_concurrency(), shutdown)
        .await
        .context("run worker skeleton without database");
    control.mark_draining();
    control_shutdown.notify_waiters();
    let control_result =
        join_worker_control_task(&mut control_task, "skeleton worker control").await;
    worker_result?;
    control_result
}

#[cfg(unix)]
fn worker_control_server_config(config: &AppConfig) -> anyhow::Result<WorkerControlServerConfig> {
    let control = &config.worker.control;
    WorkerControlServerConfig::new(
        control.socket_path.clone(),
        control.runtime_directory.clone(),
        control.group.clone(),
        control.allowed_peer_uids.clone(),
        control.allowed_peer_gids.clone(),
    )
    .map_err(|error| anyhow::Error::msg(error.to_string()))
}

/// 非 Unix 平台没有本地 UDS control 实现；启用该配置时显式失败，而不是假报可用。
#[cfg(not(unix))]
async fn run_worker_skeleton(
    config: &AppConfig,
    _metrics: Arc<ProcessMetrics>,
    shutdown: ShutdownFuture,
) -> anyhow::Result<()> {
    if config.worker.control.enabled {
        anyhow::bail!("Worker control 需要 Unix socket，当前平台不支持");
    }
    auction_jobs::run_until_shutdown(config.worker_concurrency(), shutdown)
        .await
        .context("run worker skeleton without database")?;
    Ok(())
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
fn abort_worker_task(task: &Option<TaskGuard<()>>) {
    if let Some(task) = task {
        task.abort();
    }
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
async fn join_worker_task(task: &mut Option<TaskGuard<()>>, name: &'static str) {
    let Some(task) = task else {
        return;
    };
    if let Some(Err(error)) = task.join().await
        && !error.is_cancelled()
    {
        tracing::warn!(task = name, error = %error, "worker background task join failed");
    }
}

async fn join_worker_control_task<E>(
    task: &mut Option<TaskGuard<Result<(), E>>>,
    name: &'static str,
) -> anyhow::Result<()>
where
    E: std::fmt::Display,
{
    let Some(task) = task else {
        return Ok(());
    };
    match task.join().await {
        Some(Ok(Ok(()))) => Ok(()),
        Some(Ok(Err(error))) => {
            Err(anyhow::Error::msg(error.to_string())).with_context(|| format!("run {name}"))
        }
        Some(Err(error)) => Err(error).with_context(|| format!("join {name}")),
        None => Ok(()),
    }
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
