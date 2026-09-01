//! PostgreSQL/SQLite 持久化 Worker 的装配与退出收口。

#[cfg(any(feature = "postgres", feature = "sqlite"))]
use super::control::{abort_worker_task, join_worker_task, spawn_heartbeat};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use super::control::{join_worker_control_task, worker_control_server_config};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use super::forum::configure_forum;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use super::fulfillment::{
    UnavailableFulfillmentCustodyReader, UnavailableFulfillmentEvidenceReader,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use super::mail::configure_mail_poll;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use super::operations_alert::configure_operations_alert;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use super::skeleton::run_worker_skeleton;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use super::*;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use chrono::Timelike;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(super) async fn run_worker(
    config: &AppConfig,
    tracing_health: Arc<dyn TracingHealthReader>,
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
    let heartbeat_health: Arc<dyn HeartbeatHealthReader> = store.clone();
    let heartbeat = Arc::new(HeartbeatPublisher::new(
        heartbeat_store,
        HeartbeatPublisherConfig {
            service_name: ServiceName::Worker,
            process_role: ProcessRole::Worker,
            instance_id: worker_id.clone(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            started_at: Utc::now(),
            ttl: chrono::Duration::seconds(30),
        },
        heartbeat_health,
        tracing_health,
    ));
    if let Err(error) = heartbeat.publish_starting().await {
        tracing::warn!(error = %error, "worker heartbeat starting publish failed");
    }
    let schedule: Arc<dyn ScheduledRoundStore> = store.clone();
    let lifecycle_store = Arc::clone(&store);
    let lifecycle: Arc<dyn AuctionLifecycleCommandPort> =
        Arc::new(DatabaseAuctionLifecycleCommands::new(
            lifecycle_store,
            Arc::new(SystemClock),
            AuctionLifecyclePolicy::default(),
        ));
    let slow_proxy_due: Arc<dyn SlowProxyDueReader> = store.clone();
    let slow_proxy_commands: Arc<dyn SlowProxyCommandPort> =
        Arc::new(DatabaseCompetitionCommands::new(Arc::clone(&store), Arc::new(SystemClock)));
    let (mail_polls, mail_delivery_transport, mail_evidence_reader, mail_custody_reader) =
        configure_mail_poll(config, Arc::clone(&store)).await?;
    let mail_owner: Option<Arc<dyn AdminMailPort>> = if config.worker.mooglemail.enabled {
        Some(Arc::new(DatabaseAdminMailCommands::new(Arc::clone(&store), Arc::new(SystemClock))))
    } else {
        None
    };
    let schedule_runtime = configure_schedule_runtime(config, mail_owner)?;
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
            let recipient_reader: Arc<dyn SettlementFulfillmentRecipientReader> = store.clone();
            let custody_reader: Arc<dyn SettlementFulfillmentCustodyReader> = mail_custody_reader
                .unwrap_or_else(|| Arc::new(UnavailableFulfillmentCustodyReader));
            let fulfillment_commands: Arc<dyn SettlementFulfillmentCommandPort> =
                Arc::new(SettlementFulfillmentService::new(
                    Arc::clone(&fulfillment_store),
                    recipient_reader,
                    custody_reader,
                    Arc::clone(&evidence_reader),
                    Arc::new(SystemClock),
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
    if let Err(error) = heartbeat.publish_current().await {
        tracing::warn!(error = %error, "worker heartbeat ready publish failed");
    }
    let mut heartbeat_task = Some(TaskGuard::new(spawn_heartbeat(Arc::clone(&heartbeat))));
    let poll_interval_seconds = config.worker_poll_interval_seconds();
    let lease_duration_seconds = poll_interval_seconds
        .checked_mul(3)
        .ok_or_else(|| anyhow::anyhow!("Worker claim lease 派生时长超出可表示范围"))?
        .max(30);
    let auction_poll_interval = std::time::Duration::from_secs(poll_interval_seconds);
    let auction_lease_duration = std::time::Duration::from_secs(lease_duration_seconds);
    let result = auction_jobs::run_persistent_with_settlement_until_shutdown(
        auction_jobs::PersistentWorker {
            schedule,
            lifecycle,
            slow_proxy_due: Some(slow_proxy_due),
            slow_proxy_commands: Some(slow_proxy_commands),
            auction: auction_jobs::AuctionWorkerRuntimeConfig {
                batch_limit: config.worker.batch_limit,
                concurrency: config.worker_concurrency(),
                poll_interval: auction_poll_interval,
                jitter: std::time::Duration::from_secs(config.worker.jitter_seconds),
                lease_duration: auction_lease_duration,
                item_timeout: std::time::Duration::from_secs(config.worker.item_timeout_seconds),
            },
            schedule_runtime,
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
                poll_interval: std::time::Duration::from_secs(
                    config.worker.forum.poll_interval_seconds,
                ),
                jitter: std::time::Duration::from_secs(config.worker.forum.jitter_seconds),
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
fn configure_schedule_runtime(
    config: &AppConfig,
    mail: Option<Arc<dyn AdminMailPort>>,
) -> anyhow::Result<Option<auction_jobs::AuctionScheduleRuntimeConfig>> {
    let schedule = &config.auction.schedule;
    if !schedule.enabled {
        return Ok(None);
    }
    let parse_day = |value: &str| {
        ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]
            .iter()
            .position(|day| *day == value)
            .map(|value| value as u8)
            .ok_or_else(|| anyhow::anyhow!("auction.schedule 星期配置无效"))
    };
    let parse_clock = |value: &str| {
        chrono::NaiveTime::parse_from_str(value, "%H:%M")
            .map(|value| value.hour() as u16 * 60 + value.minute() as u16)
            .map_err(|_| anyhow::anyhow!("auction.schedule 时间配置无效"))
    };
    let mailboxes = config
        .worker
        .mooglemail
        .configured_worlds()
        .into_iter()
        .map(|world| (world.world_id, world.mailbox_key))
        .collect();
    Ok(Some(auction_jobs::AuctionScheduleRuntimeConfig {
        enabled: true,
        timezone: schedule.timezone.clone(),
        preview_weekday: parse_day(&schedule.preview_day)?,
        preview_minute: parse_clock(&schedule.preview_time)?,
        open_weekday: parse_day(&schedule.open_day)?,
        open_minute: parse_clock(&schedule.open_time)?,
        end_weekday: parse_day(&schedule.nominal_end_day)?,
        end_minute: parse_clock(&schedule.nominal_end_time)?,
        worlds: schedule.worlds.clone(),
        policy: config.auction.policy(),
        mail,
        mailboxes,
    }))
}
