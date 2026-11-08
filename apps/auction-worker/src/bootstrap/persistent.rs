//! PostgreSQL 持久化 Worker 的装配与退出收口。

#[cfg(feature = "postgres")]
use super::control::{
    WorkerRemoteReads, WorkerRuntimeTaskExit, join_worker_control_task,
    worker_control_server_config,
};
#[cfg(feature = "postgres")]
use super::control::{
    abort_worker_task, join_worker_task, spawn_external_heartbeat, spawn_heartbeat,
    worker_heartbeat_health,
};
#[cfg(feature = "postgres")]
use super::forum::configure_forum;
#[cfg(feature = "postgres")]
use super::fulfillment::{
    UnavailableFulfillmentCustodyReader, UnavailableFulfillmentEvidenceReader,
};
#[cfg(feature = "postgres")]
use super::hot::spawn_hot_config_watch;
#[cfg(feature = "postgres")]
use super::mail::configure_mail_poll;
#[cfg(feature = "postgres")]
use super::operations_alert::configure_operations_alert;
#[cfg(feature = "postgres")]
use super::skeleton::run_worker_skeleton;
#[cfg(feature = "postgres")]
use super::*;
#[cfg(feature = "postgres")]
use auction_application::player_notifications::{
    PlayerNotificationScanner, PlayerNotificationService,
};
#[cfg(feature = "postgres")]
use chrono::Timelike;

#[cfg(feature = "postgres")]
pub(super) async fn run_worker(
    config: &AppConfig,
    tracing_health: Arc<dyn TracingHealthReader>,
    metrics: Arc<ProcessMetrics>,
    switches: auction_application::runtime_switches::RuntimeSwitches,
    runtime_failure: tokio::sync::mpsc::Sender<&'static str>,
    write_lifecycle: auction_application::operations::ExternalWriteLifecycle,
    shutdown: ShutdownFuture,
) -> anyhow::Result<()> {
    let Some(database_url) = config.database_url() else {
        if config.worker.mooglemail.enabled
            || config.worker.forum.enabled
            || config.worker.operations_alert.notify_enabled()
            || config.worker.operations_alert.webhook_enabled()
        {
            anyhow::bail!("启用外部 Worker 功能时必须配置 PostgreSQL database_url");
        }
        run_worker_skeleton(config, metrics, runtime_failure, shutdown).await?;
        return Ok(());
    };
    let store = Arc::new(
        DatabaseStore::connect(database_url, database_profile(config)?)
            .await
            .context("connect worker database")?
            .with_database_recorder(metrics.database())
            .with_runtime_switches(switches.clone())
            .with_notification_recorder(metrics.notifications())
            .with_shadow_recorder(metrics.shadow())
            .with_public_base_url(config.worker.forum.public_base_url.clone())
            .with_mail_templates(config.worker.mooglemail.mail_templates.clone())
            .with_migration_compatibility_mode(config.readiness.migration_compatibility_mode)
            .with_migration_readiness_recorder(metrics.migration_readiness())
            .with_delivery_send_intervals(config.worker.mooglemail.send_intervals)
            .context("configure delivery send intervals")?,
    );
    let _runtime_switch_task = auction_process::TaskGuard::new(
        auction_jobs::start_runtime_switch_refresh(store.clone(), switches.clone()).await,
    );
    // 启动快照来自实际生效的 Worker 配置；只有配置了 runtime 文件才启动周期热读取。
    let hot_config =
        auction_jobs::WorkerHotRuntime::new(auction_config::worker_hot_config(&config.worker));
    let _hot_config_task = config
        .runtime_config_path
        .clone()
        .map(|path| spawn_hot_config_watch(path, hot_config.clone(), Arc::clone(&store)));
    metrics
        .register_runtime_switch_metrics(switches.clone())
        .context("register runtime switch metrics")?;
    metrics
        .register_pool_metrics(store.pool_metrics_source())
        .context("register worker database pool metrics")?;
    metrics
        .register_audit_metrics(store.audit_metrics_source())
        .context("register worker audit metrics")?;
    let _sli_task = if metrics.enabled() {
        let recorder = auction_observability::SliMetrics::register(&metrics, switches.clone())
            .context("register worker SLI metrics")?;
        let reader: Arc<dyn auction_application::sli::SliSnapshotReader> = store.clone();
        Some(TaskGuard::new(auction_jobs::start_sli_snapshot(reader, switches.clone(), recorder)))
    } else {
        None
    };
    let worker_id = format!("auction-worker-{}", Uuid::new_v4());
    let schedule: Arc<dyn ScheduledRoundStore> = store.clone();
    let lifecycle_store = Arc::clone(&store);
    let lifecycle: Arc<dyn AuctionLifecycleCommandPort> = Arc::new(
        DatabaseAuctionLifecycleCommands::new(
            lifecycle_store,
            AuctionLifecyclePolicy::default(),
            config.auction.policy()?,
        )
        .with_schedule_calendar(config.auction.schedule_calendar()?),
    );
    let slow_proxy_due: Arc<dyn SlowProxyDueReader> = store.clone();
    let slow_proxy_commands: Arc<dyn SlowProxyCommandPort> =
        Arc::new(DatabaseCompetitionCommands::new(Arc::clone(&store)));
    let (forum_sync, forum_reply, forum_pm, forum_profile_reader, forum_runtime_ownership) =
        configure_forum(config, Arc::clone(&store), metrics.external_metrics()).await?;
    let mail_progress = auction_jobs::MailPollProgressTracker::default();
    let (
        mail_polls,
        mail_delivery_transport,
        mail_evidence_reader,
        mail_custody_reader,
        internal_evidence_reader,
        mail_runtime_ownership,
        mail_control_reads,
        mail_inventory_reader,
    ) = configure_mail_poll(
        config,
        Arc::clone(&store),
        forum_profile_reader,
        Arc::new(mail_progress.clone()),
        metrics.sent_baseline(),
        metrics.mail_scan(),
        Arc::new(hot_config.clone()),
        switches.clone(),
        metrics.external_metrics(),
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
        .with_bot_players(config.bot_players.clone())
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
        .with_mail_progress(mail_progress)
        .with_external_accounts(metrics.external_accounts());
    let heartbeat_store: Arc<dyn HeartbeatStore> = store.clone();
    let database_health: Arc<dyn HeartbeatHealthReader> = store.clone();
    let heartbeat_health = worker_heartbeat_health(database_health, control.clone());
    let worker_heartbeat_url = config.worker_heartbeat_url().map(str::to_owned);
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
            Arc::clone(&heartbeat_health),
            tracing_health,
        )
        .with_details(serde_json::json!({ "forum_sync_enabled": config.worker.forum.enabled }))
        .context("configure worker heartbeat details")?,
    );
    if let Err(error) = heartbeat.publish_starting().await {
        tracing::warn!(error = %error, "worker heartbeat starting publish failed");
    }
    let control_shutdown = Arc::new(tokio::sync::Notify::new());
    // W1.10/W10.9：MoogleMail 或论坛任一可用即装配远端只读能力；缺失的能力单独返回不可用。
    // 论坛读取器与回帖循环共用同一适配器与限速，不建立第二套请求门。
    let forum_reader = forum_reply.as_ref().map(auction_jobs::ForumReplyRuntime::topic_reader);
    let remote_reads = (mail_control_reads.is_some() || forum_reader.is_some())
        .then(|| WorkerRemoteReads::build(mail_control_reads, forum_reader, switches.clone()));
    let mut control_task = if config.worker.control.enabled {
        let server_config = worker_control_server_config(config)?;
        let server = WorkerControlServer::bind_with_metrics(
            server_config,
            control.clone(),
            Some(Arc::clone(&metrics)),
        )
        .await
        .context("bind Worker control socket")?;
        let server = match remote_reads {
            Some(remote_reads) => server.with_remote_reads(remote_reads),
            None => server,
        };
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
    // W14.3：玩家通知扫描只读持久事实；发送仍由 delivery 循环负责。
    let player_notifications = {
        let scanner: Arc<dyn PlayerNotificationScanner> = store.clone();
        let delivery: Arc<dyn MailDeliveryEnqueuer> = store.clone();
        Some(Arc::new(
            auction_jobs::PlayerNotificationRuntime::new(
                Arc::new(PlayerNotificationService::new(scanner, delivery, Arc::new(SystemClock))),
                auction_jobs::PlayerNotificationRuntimeConfig {
                    poll_interval: std::time::Duration::from_secs(300),
                    jitter: std::time::Duration::from_secs(30),
                },
            )
            .map_err(|error| anyhow::Error::msg(error.to_string()))?,
        ))
    };
    let recovery_worlds = if config.worker.mooglemail.enabled {
        config
            .worker
            .mooglemail
            .configured_mailboxes()
            .filter_map(|(world_id, _)| SettlementWorld::from_name(world_id))
            .collect()
    } else {
        vec![SettlementWorld::Main, SettlementWorld::Isekai]
    };
    let recovery_health = match evidence_reader_for_health {
        Some(balance_reader) => {
            let health_service = RecoveryHealthService::new(
                RecoveryHealthConfig {
                    worlds: recovery_worlds.clone(),
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
    // W11.1–W11.3：只读 Armory 观察 + 数据库负债投影；未配置邮箱时不产生报告。
    let asset_reconciliation = match mail_inventory_reader {
        Some(inventory_reader) => {
            let mut worlds = recovery_worlds.clone();
            worlds.sort_by_key(|world| world.as_str());
            worlds.dedup();
            let service = AssetReconciliationService::new(
                auction_application::reconciliation::AssetReconciliationConfig {
                    worlds,
                    balance_evidence_max_age_seconds: config
                        .worker
                        .recovery
                        .evidence_stale_after_seconds,
                },
                store.clone(),
                inventory_reader,
            )
            .map_err(|error| anyhow::Error::msg(error.to_owned()))?;
            Some(
                AssetReconciliationRuntime::new(
                    Arc::new(service),
                    auction_jobs::ASSET_RECONCILIATION_INTERVAL,
                    auction_jobs::ASSET_RECONCILIATION_TIMEOUT,
                )
                .map_err(|error| anyhow::Error::msg(error.to_owned()))?,
            )
        }
        None => None,
    };
    if let Err(error) = heartbeat.publish_current().await {
        tracing::warn!(error = %error, "worker heartbeat ready publish failed");
    }
    // 装配等待可能跨过外层停止信号；不能先启动 PM 再让主循环补发 drain。
    let mut shutdown = shutdown;
    let stopping = tokio::select! {
        biased;
        () = &mut shutdown => true,
        () = std::future::ready(()) => false,
    };
    if stopping {
        control.mark_draining();
    }
    let mut heartbeat_task =
        (!stopping).then(|| TaskGuard::new(spawn_heartbeat(Arc::clone(&heartbeat))));
    let mut external_heartbeat_task = (!stopping)
        .then(|| {
            worker_heartbeat_url
                .map(|url| TaskGuard::new(spawn_external_heartbeat(Arc::clone(&heartbeat), url)))
        })
        .flatten();
    let poll_interval_seconds = config.worker_poll_interval_seconds();
    let lease_duration_seconds = poll_interval_seconds
        .checked_mul(3)
        .ok_or_else(|| anyhow::anyhow!("Worker claim lease 派生时长超出可表示范围"))?
        .max(30);
    let auction_poll_interval = std::time::Duration::from_secs(poll_interval_seconds);
    let auction_lease_duration = std::time::Duration::from_secs(lease_duration_seconds);
    let (pm_drain, pm_receiver) = tokio::sync::watch::channel(stopping);
    let write_lifecycle_for_pm = write_lifecycle.clone();
    let metrics_for_pm = metrics.worker();
    let notifications_for_pm = metrics.notifications();
    let switches_for_pm = switches.clone();
    let mut pm_task = forum_pm.filter(|_| !stopping).map(|runtime| {
        let worker_id = worker_id.clone();
        let task_exit = WorkerRuntimeTaskExit::new(
            control.clone(),
            runtime_failure.clone(),
            "lot_watch_pm_stopped",
        );
        TaskGuard::new(tokio::spawn(async move {
            let _task_exit = task_exit;
            runtime
                .with_metrics(metrics_for_pm)
                .with_notifications(notifications_for_pm)
                .with_write_lifecycle(write_lifecycle_for_pm)
                .with_runtime_switches(switches_for_pm)
                .run(worker_id, pm_receiver)
                .await;
        }))
    });
    // 主 shutdown 一经触发即停止 PM 新领取，不等其他循环排空完成。
    let shutdown = async {
        if !stopping {
            shutdown.await;
        }
        pm_drain.send_replace(true);
    };
    let fatal_wakeup = control.fatal_wakeup();
    // W12.5：批量与维护循环共享的连接配额。关键循环（生命周期、慢速代理、独立结算、履约）
    // 从不获取许可，因此始终保留配额之外的连接；配额来源是同一服务池上限，不新增配置项。
    let bulk_quota = auction_jobs::WorkerConnectionQuota::new(
        config.database_pool.worker_bulk_connection_budget(),
    )
    .map(Arc::new)
    .context("worker bulk connection quota must be non-zero")?;
    let runner = auction_jobs::run_persistent_with_settlement_until_shutdown(
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
            asset_reconciliation,
            recovery_store,
            recovery_resolution,
            recovery_delivery,
            recovery_blacklister,
            player_notifications,
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
            hot: hot_config,
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
            // 出价提交后的标记/被超提醒 outbox；与出价共用同一 Store，处理使用独立事务。
            lot_watch_reminders: Some(store.clone()),
            metrics: metrics.worker(),
            notifications: metrics.notifications(),
            control: control.clone(),
            clock: Arc::new(SystemClock),
            worker_id,
            write_lifecycle: write_lifecycle.clone(),
            bulk_quota: Some(bulk_quota),
            switches: switches.clone(),
        },
        shutdown,
    );
    tokio::pin!(runner);
    let result = tokio::select! {
        result = &mut runner => result,
        () = fatal_wakeup.notified() => {
            // 运行器正在收口时不会再轮询 shutdown；先阻止 PM 继续领取，再启动外层统一限时排空。
            pm_drain.send_replace(true);
            let _ = runtime_failure.try_send("persistent_worker_stopped");
            runner.await
        }
    };
    control.mark_draining();
    pm_drain.send_replace(true);
    // 新路径由外层共享关键段截止等待 PM；零预算保留旧的立即中止。
    // 两种路径均只按持久租约恢复，绝不因关闭把未知写入改回 pending。
    if !write_lifecycle.enabled() {
        abort_worker_task(&pm_task);
    }
    join_worker_task(&mut pm_task, "lot watch PM").await;
    abort_worker_task(&credential_owner_task);
    join_worker_task(&mut credential_owner_task, "credential ownership monitor").await;
    abort_worker_task(&external_heartbeat_task);
    join_worker_task(&mut external_heartbeat_task, "worker external heartbeat").await;
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
    let runtime_mailboxes = config.worker.mooglemail.configured_mailboxes();
    let worlds = schedule
        .worlds
        .iter()
        .filter(|world_id| {
            runtime_mailboxes
                .clone()
                .any(|(configured_world, _)| configured_world == world_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    anyhow::ensure!(!worlds.is_empty(), "auction.schedule.worlds 与 runtime 启用世界没有交集");
    let mailboxes = runtime_mailboxes
        .map(|(world_id, mailbox_key)| (world_id.to_owned(), mailbox_key.to_owned()))
        .collect();
    // 与生命周期后继共用同一配置投影，保证 bootstrap 与开拍事务计算出同一早鸟时段。
    let early_bird =
        config.auction.schedule_calendar()?.and_then(|calendar| calendar.early_bird().cloned());
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
        early_bird,
        mail,
        mailboxes,
    }))
}

#[cfg(feature = "postgres")]
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
