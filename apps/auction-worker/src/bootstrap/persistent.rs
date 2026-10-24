//! PostgreSQL 持久化 Worker 的装配与退出收口。

#[cfg(feature = "postgres")]
use super::control::{
    WorkerRuntimeTaskExit, join_worker_control_task, worker_control_server_config,
};
#[cfg(feature = "postgres")]
use super::control::{
    abort_worker_task, join_worker_task, spawn_heartbeat, worker_heartbeat_health,
};
#[cfg(feature = "postgres")]
use super::forum::configure_forum;
#[cfg(feature = "postgres")]
use super::fulfillment::{
    UnavailableFulfillmentCustodyReader, UnavailableFulfillmentEvidenceReader,
};
#[cfg(feature = "postgres")]
use super::mail::configure_mail_poll;
#[cfg(feature = "postgres")]
use super::operations_alert::configure_operations_alert;
#[cfg(feature = "postgres")]
use super::skeleton::run_worker_skeleton;
#[cfg(feature = "postgres")]
use super::*;
#[cfg(feature = "postgres")]
use chrono::Timelike;

#[cfg(feature = "postgres")]
pub(super) async fn run_worker(
    config: &AppConfig,
    tracing_health: Arc<dyn TracingHealthReader>,
    metrics: Arc<ProcessMetrics>,
    runtime_failure: tokio::sync::mpsc::Sender<&'static str>,
    shutdown: ShutdownFuture,
) -> anyhow::Result<()> {
    let Some(database_url) = config.database_url() else {
        if config.worker.mooglemail.enabled
            || config.worker.forum.enabled
            || config.worker.operations_alert.enabled
        {
            anyhow::bail!("启用外部 Worker 功能时必须配置 PostgreSQL database_url");
        }
        run_worker_skeleton(config, metrics, runtime_failure, shutdown).await?;
        return Ok(());
    };
    let store = Arc::new(
        DatabaseStore::connect(database_url)
            .await
            .context("connect worker database")?
            .with_delivery_send_intervals(config.worker.mooglemail.send_intervals)
            .context("configure delivery send intervals")?,
    );
    metrics
        .register_pool_metrics(store.pool_metrics_source())
        .context("register worker database pool metrics")?;
    let worker_id = format!("auction-worker-{}", Uuid::new_v4());
    let schedule: Arc<dyn ScheduledRoundStore> = store.clone();
    let lifecycle_store = Arc::clone(&store);
    let lifecycle: Arc<dyn AuctionLifecycleCommandPort> =
        Arc::new(DatabaseAuctionLifecycleCommands::new(
            lifecycle_store,
            Arc::new(SystemClock),
            AuctionLifecyclePolicy::default(),
            config.auction.policy()?,
        ));
    let slow_proxy_due: Arc<dyn SlowProxyDueReader> = store.clone();
    let slow_proxy_commands: Arc<dyn SlowProxyCommandPort> =
        Arc::new(DatabaseCompetitionCommands::new(Arc::clone(&store), Arc::new(SystemClock)));
    let (forum_sync, forum_reply, forum_pm, forum_profile_reader, forum_runtime_ownership) =
        configure_forum(config, Arc::clone(&store), config.worker.mooglemail.enabled).await?;
    let mail_progress = auction_jobs::MailPollProgressTracker::default();
    let (
        mail_polls,
        mail_delivery_transport,
        mail_evidence_reader,
        mail_custody_reader,
        internal_evidence_reader,
        mail_runtime_ownership,
    ) = configure_mail_poll(
        config,
        Arc::clone(&store),
        forum_profile_reader,
        Arc::new(mail_progress.clone()),
    )
    .await?;
    let runtime_owners = [
        ("mail_owner_invalid", mail_runtime_ownership),
        ("forum_owner_invalid", forum_runtime_ownership),
    ];
    let mut credential_owner_task =
        runtime_owners.iter().any(|(_, owner)| owner.is_some()).then(|| {
            let runtime_failure = runtime_failure.clone();
            TaskGuard::new(tokio::spawn(async move {
                // 两类已取得的 owner 都只观察 sticky 状态；失效后有界排空，不原地抢锁。
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    interval.tick().await;
                    for (reason, ownership) in &runtime_owners {
                        if ownership.as_ref().is_some_and(|owner| !owner.is_valid()) {
                            let _ = runtime_failure.try_send(*reason);
                            return;
                        }
                    }
                }
            }))
        });
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
    let (
        settlement_store,
        settlement_commands,
        fulfillment_store,
        fulfillment_commands,
        delivery_store,
        delivery_transport,
        evidence_reader_for_health,
    ): SettlementPorts = {
        let fulfillment_store: Arc<dyn SettlementFulfillmentStore> = store.clone();
        let evidence_reader: Arc<dyn SettlementFulfillmentEvidenceReader> =
            mail_evidence_reader.unwrap_or_else(|| Arc::new(UnavailableFulfillmentEvidenceReader));
        let recipient_reader: Arc<dyn SettlementFulfillmentRecipientReader> = store.clone();
        let custody_reader: Arc<dyn SettlementFulfillmentCustodyReader> =
            mail_custody_reader.unwrap_or_else(|| Arc::new(UnavailableFulfillmentCustodyReader));
        let mut fulfillment_service = SettlementFulfillmentService::new(
            Arc::clone(&fulfillment_store),
            recipient_reader,
            custody_reader,
            Arc::clone(&evidence_reader),
            Arc::new(SystemClock),
        )
        .with_funding_policy_reader(store.clone())
        .with_bot_players(config.worker_bot_players.clone())
        .with_evidence_poll_delay(chrono::Duration::seconds(
            i64::try_from(config.worker.fulfillment.evidence_poll_seconds).unwrap_or(300),
        ));
        if let Some(internal_evidence_reader) = internal_evidence_reader {
            fulfillment_service = fulfillment_service
                .with_internal_fulfillment(store.clone(), internal_evidence_reader);
        }
        let fulfillment_commands: Arc<dyn SettlementFulfillmentCommandPort> =
            Arc::new(fulfillment_service);
        (
            Some(store.clone()),
            Some(Arc::new(DatabaseSettlementCommands::new(Arc::clone(&store)))),
            Some(fulfillment_store),
            Some(fulfillment_commands),
            Some(store.clone()),
            mail_delivery_transport,
            Some(evidence_reader),
        )
    };
    let control = WorkerControlHandle::new(env!("CARGO_PKG_VERSION"), mail_polls.len())
        .with_mail_progress(mail_progress);
    let heartbeat_store: Arc<dyn HeartbeatStore> = store.clone();
    let database_health: Arc<dyn HeartbeatHealthReader> = store.clone();
    let heartbeat = Arc::new(
        HeartbeatPublisher::new(
            heartbeat_store,
            HeartbeatPublisherConfig {
                service_name: ServiceName::Worker,
                process_role: ProcessRole::Worker,
                instance_id: worker_id.clone(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                started_at: Utc::now(),
                ttl: chrono::Duration::seconds(30),
            },
            worker_heartbeat_health(database_health, control.clone()),
            tracing_health,
        )
        .with_details(serde_json::json!({ "forum_sync_enabled": config.worker.forum.enabled }))
        .context("configure worker heartbeat details")?,
    );
    if let Err(error) = heartbeat.publish_starting().await {
        tracing::warn!(error = %error, "worker heartbeat starting publish failed");
    }
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
        let task_exit = WorkerRuntimeTaskExit::new(
            control.clone(),
            runtime_failure.clone(),
            "control_server_stopped",
        );
        Some(TaskGuard::new(tokio::spawn(async move {
            let _task_exit = task_exit;
            server.run(async move { shutdown.notified().await }).await
        })))
    } else {
        None
    };
    let recovery_store: Option<Arc<dyn RecoveryWorkStore>> = Some(store.clone());
    let recovery_resolution: Option<Arc<dyn RecoveryResolutionStore>> = Some(store.clone());
    let recovery_delivery: Option<Arc<dyn MailDeliveryEnqueuer>> =
        if recovery_delivery_enabled { Some(store.clone()) } else { None };
    let recovery_blacklister: Option<Arc<dyn RecoveryBuyerBlacklister>> = Some(store.clone());
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
    let recovery_health = match evidence_reader_for_health {
        Some(balance_reader) => {
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
            .map_err(|error| anyhow::Error::msg(error.to_owned()))?
            .with_funding_policy_reader(store.clone());
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
        None => {
            anyhow::bail!("PostgreSQL recovery health 缺少 Balance 读取端口")
        }
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
    let (pm_drain, pm_receiver) = tokio::sync::watch::channel(false);
    let mut pm_task = forum_pm.map(|runtime| {
        let worker_id = worker_id.clone();
        let task_exit =
            WorkerRuntimeTaskExit::new(control.clone(), runtime_failure, "lot_watch_pm_stopped");
        TaskGuard::new(tokio::spawn(async move {
            let _task_exit = task_exit;
            runtime.run(worker_id, pm_receiver).await;
        }))
    });
    // 主 shutdown 一经触发即停止 PM 新领取，不等其他循环排空完成。
    let shutdown = async {
        shutdown.await;
        pm_drain.send_replace(true);
    };
    let result = auction_jobs::run_persistent_with_settlement_until_shutdown(
        auction_jobs::PersistentWorker {
            schedule,
            worlds: Some(store.clone()),
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
    pm_drain.send_replace(true);
    abort_worker_task(&credential_owner_task);
    join_worker_task(&mut credential_owner_task, "credential ownership monitor").await;
    // 主循环已有有界 drain；不再追加论坛长预算。剩余 PM 中止后仅按持久租约恢复，
    // claimed 可重领，任何 sending 只读对账，绝不因关闭把未知写入回 pending。
    abort_worker_task(&pm_task);
    join_worker_task(&mut pm_task, "lot watch PM").await;
    abort_worker_task(&heartbeat_task);
    join_worker_task(&mut heartbeat_task, "worker heartbeat").await;
    if let Err(error) = heartbeat.publish_stopping().await {
        tracing::warn!(error = %error, "worker heartbeat stopping publish failed");
    }
    // 只有一个接收者；即使控制任务尚未创建等待 future，也必须保留停机通知。
    control_shutdown.notify_one();
    let control_result =
        join_worker_control_task(&mut control_task, "persistent worker control").await;
    result.context("run persistent worker")?;
    control_result
}

#[cfg(feature = "postgres")]
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
    let runtime_worlds =
        config.worker.mooglemail.configured_worlds().into_iter().collect::<Vec<_>>();
    let worlds = schedule
        .worlds
        .iter()
        .filter(|world_id| {
            runtime_worlds.iter().any(|world| world.world_id.as_str() == world_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    anyhow::ensure!(!worlds.is_empty(), "auction.schedule.worlds 与 runtime 启用世界没有交集");
    let mailboxes =
        runtime_worlds.into_iter().map(|world| (world.world_id, world.mailbox_key)).collect();
    Ok(Some(auction_jobs::AuctionScheduleRuntimeConfig {
        enabled: true,
        timezone: schedule.timezone.clone(),
        preview_weekday: parse_day(&schedule.preview_day)?,
        preview_minute: parse_clock(&schedule.preview_time)?,
        open_weekday: parse_day(&schedule.open_day)?,
        open_minute: parse_clock(&schedule.open_time)?,
        end_weekday: parse_day(&schedule.nominal_end_day)?,
        end_minute: parse_clock(&schedule.nominal_end_time)?,
        worlds,
        policy: config.auction.policy()?,
        mail,
        mailboxes,
    }))
}
