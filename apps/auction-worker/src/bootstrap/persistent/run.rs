//! 持久化 Worker 的装配主流程与退出收口。

use std::sync::Arc;

use anyhow::Context;
use auction_application::admin::{AdminMailPort, RecoveryBuyerBlacklister};
use auction_application::auction::{
    AuctionLifecycleCommandPort, AuctionLifecyclePolicy, SlowProxyCommandPort, SlowProxyDueReader,
};
use auction_application::mail::MailDeliveryEnqueuer;
use auction_application::player_notifications::{
    PlayerNotificationScanner, PlayerNotificationService,
};
use auction_application::recovery::{RecoveryResolutionStore, RecoveryWorkStore};
use auction_application::scheduling::ScheduledRoundStore;
use auction_application::settlement::{
    SettlementFulfillmentCommandPort, SettlementFulfillmentCustodyReader,
    SettlementFulfillmentEvidenceReader, SettlementFulfillmentRecipientReader,
    SettlementFulfillmentService, SettlementFulfillmentStore, SettlementReturnMailReader,
    SettlementReturnWorkStore,
};
use auction_config::AppConfig;
use auction_jobs::{WorkerControlHandle, consignment::ConsignmentWorker};
use auction_observability::{
    HeartbeatHealthReader, HeartbeatPublisher, HeartbeatPublisherConfig, HeartbeatStore,
    ProcessRole, ServiceName,
};
use auction_process::TaskGuard;
use auction_storage::{
    DatabaseAdminMailCommands, DatabaseAuctionLifecycleCommands, DatabaseCompetitionCommands,
    DatabaseSettlementCommands, DatabaseStore,
};
use auction_time::Timestamp;

use super::PersistentRuntime;
use super::heartbeat;
use super::profile::configure_schedule_runtime;
use super::recovery::{asset_reconciliation_runtime, recovery_health_runtime, recovery_worlds};
use crate::bootstrap::control::WorkerRemoteReads;
use crate::bootstrap::control::{
    WorkerRuntimeTaskExit, abort_worker_task, bind_worker_control_server, join_worker_control_task,
    join_worker_task, spawn_external_heartbeat, spawn_heartbeat, worker_heartbeat_health,
};
use crate::bootstrap::forum::configure_forum;
use crate::bootstrap::fulfillment::{
    MoogleMailReturnMailReader, UnavailableFulfillmentCustodyReader,
    UnavailableFulfillmentEvidenceReader,
};
use crate::bootstrap::hot::spawn_hot_config_watch;
use crate::bootstrap::mail::configure_mail_poll;
use crate::bootstrap::operations_alert::configure_operations_alert;
use crate::bootstrap::{SettlementPorts, SystemClock};

/// 启动已完成寄售预检的持久化任务，并统一监管和排空。
pub(super) async fn run_connected(
    config: &AppConfig,
    store: Arc<DatabaseStore>,
    worker_id: String,
    runtime: PersistentRuntime,
    mut consignment: Option<ConsignmentWorker>,
) -> anyhow::Result<()> {
    let PersistentRuntime {
        consignment: activation,
        tracing_health,
        metrics,
        switches,
        runtime_failure,
        write_lifecycle,
        shutdown,
    } = runtime;
    let _runtime_switch_task = auction_process::TaskGuard::new(
        auction_jobs::start_runtime_switch_refresh(store.clone(), switches.clone()).await,
    );
    // 通知模板保存在数据库：启动时先读一次，之后周期刷新，Admin 修改后无需重启 Worker。
    let _mail_template_task = auction_process::TaskGuard::new(
        auction_jobs::start_mail_template_refresh(store.clone(), store.live_mail_templates()).await,
    );
    // 启动快照来自实际生效的 Worker 配置；周期热读取在各循环的启动运行配置确定后再启动。
    let hot_config =
        auction_jobs::WorkerHotRuntime::new(auction_config::worker_hot_config(&config.worker));
    metrics.register_runtime_switch_metrics(switches.clone()).context("注册运行时开关指标失败")?;
    metrics
        .register_pool_metrics(store.pool_metrics_source())
        .context("注册 Worker 数据库连接池指标失败")?;
    metrics
        .register_audit_metrics(store.audit_metrics_source())
        .context("注册 Worker 审计指标失败")?;
    let _sli_task = if metrics.enabled() {
        let recorder = auction_observability::SliMetrics::register(&metrics, switches.clone())
            .context("注册 Worker SLI 指标失败")?;
        let reader: Arc<dyn auction_application::sli::SliSnapshotReader> = store.clone();
        Some(TaskGuard::new(auction_jobs::start_sli_snapshot(reader, switches.clone(), recorder)))
    } else {
        None
    };
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
        activation,
    )
    .await?;
    if consignment.is_some() {
        let mail_readiness = if let Some(reads) = &mail_control_reads {
            crate::bootstrap::mail::consignment::readiness_runtimes(
                config,
                Arc::clone(&store),
                &reads.gateways,
            )
            .await
        } else {
            Vec::new()
        };
        consignment = super::consignment::attach_loops(
            config,
            &store,
            &switches,
            &worker_id,
            consignment,
            mail_control_reads.as_ref(),
            mail_readiness,
        )
        .await?;
    }
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
        .with_evidence_poll_delay(auction_time::Duration::seconds(
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
    // control `/v1/health/ready` 与 Public/Admin、Worker 心跳共用数据库/schema 与审计模式门禁；
    // service start、deploy 与 auction-server 据此判定 Worker 就绪。
    let control = WorkerControlHandle::new(env!("CARGO_PKG_VERSION"), mail_polls.len())
        .with_mail_progress(mail_progress)
        .with_external_accounts(metrics.external_accounts())
        .with_readiness(Arc::new(
            auction_http::HttpReadinessProbe::new(
                store.clone(),
                config.readiness.cache_enabled,
                metrics.readiness(),
            )
            .with_runtime_switches(switches.clone()),
        ));
    let heartbeat_store: Arc<dyn HeartbeatStore> = store.clone();
    let database_health: Arc<dyn HeartbeatHealthReader> = store.clone();
    let heartbeat_health = worker_heartbeat_health(database_health, control.clone());
    let worker_heartbeat_url = config.worker_heartbeat_url().map(str::to_owned);
    // 结算退件只读重新核验可读的世界（与核验读取器同一 gateway 集合）；未启用 MoogleMail 时为
    // 空，Admin 详情据此说明待重新核验的案件为何不前进。
    let mut settlement_return_recheck_worlds =
        mail_control_reads.as_ref().map_or_else(Vec::new, |reads| {
            reads.gateways.keys().map(|world| world.as_str()).collect::<Vec<_>>()
        });
    settlement_return_recheck_worlds.sort_unstable();
    // 能以持久凭据身份执行结算退件 Take 的世界（与投递领取、Take 激活同一判定）每次发布心跳时
    // 重新计算：只配置 Cookie 或未启用 MoogleMail 时为空，数据库凭据在运行中失效后也不再声明，
    // Admin 详情据此说明待领回的案件为何不前进。
    let heartbeat_transport = delivery_transport.clone();
    let forum_sync_enabled = config.worker.forum.enabled;
    // 持久心跳记录本进程实际装配的启动值（不变量 5）；热重载和能力变化不得改写它。
    let cold_config = config.worker_cold_config_digest()?;
    let heartbeat = Arc::new(heartbeat::with_cold_config(
        HeartbeatPublisher::new(
            heartbeat_store,
            HeartbeatPublisherConfig {
                service_name: ServiceName::Worker,
                process_role: ProcessRole::Worker,
                instance_id: worker_id.clone(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                started_at: Timestamp::now(),
                ttl: auction_time::Duration::seconds(30),
            },
            heartbeat_health,
            tracing_health,
        ),
        control.clone(),
        cold_config,
        move || {
            heartbeat::details(forum_sync_enabled, &settlement_return_recheck_worlds, |world| {
                heartbeat_transport
                    .as_ref()
                    .is_some_and(|transport| transport.frozen_take_identity_available(world))
            })
        },
    ));
    if let Err(error) = heartbeat.publish_starting().await {
        tracing::warn!(error = %error, "发布 Worker starting 心跳失败");
    }
    let control_shutdown = Arc::new(tokio::sync::Notify::new());
    // MoogleMail 或论坛任一可用即装配远端只读能力；缺失的能力单独返回不可用。
    // 论坛读取器与回帖循环共用同一适配器与限速，不建立第二套请求门。
    let forum_reader = forum_reply.as_ref().map(auction_jobs::ForumReplyRuntime::topic_reader);
    // Admin 邮件凭据在线检查与远端只读同在 control socket，由邮件装配提供执行器。
    let mail_credential_check =
        mail_control_reads.as_ref().and_then(|reads| reads.credential_check.clone());
    // 结算退件只读核验复用 MoogleMail 已装配的 gateway；未启用邮件时核验保持待办。
    let settlement_return_reader: Option<Arc<dyn SettlementReturnMailReader>> =
        mail_control_reads.as_ref().map(|reads| {
            Arc::new(MoogleMailReturnMailReader { gateways: reads.gateways.clone() })
                as Arc<dyn SettlementReturnMailReader>
        });
    let remote_reads = (mail_control_reads.is_some() || forum_reader.is_some())
        .then(|| WorkerRemoteReads::build(mail_control_reads, forum_reader, switches.clone()));
    let mut control_task = if config.worker.control.enabled {
        let server =
            bind_worker_control_server(config, control.clone(), Arc::clone(&metrics)).await?;
        let server = match remote_reads {
            Some(remote_reads) => server.with_remote_reads(remote_reads),
            None => server,
        };
        let server = match mail_credential_check {
            Some(checks) => server.with_mail_credential_checks(checks),
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
    let settlement_returns: Option<Arc<dyn SettlementReturnWorkStore>> = Some(store.clone());
    let recovery_resolution: Option<Arc<dyn RecoveryResolutionStore>> = Some(store.clone());
    let recovery_delivery: Option<Arc<dyn MailDeliveryEnqueuer>> =
        if recovery_delivery_enabled { Some(store.clone()) } else { None };
    let recovery_blacklister: Option<Arc<dyn RecoveryBuyerBlacklister>> = Some(store.clone());
    // 玩家通知扫描只读持久事实；发送仍由 delivery 循环负责。
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
    let recovery_worlds = recovery_worlds(config);
    let recovery_health = Some(recovery_health_runtime(
        config,
        &store,
        evidence_reader_for_health,
        &recovery_worlds,
    )?);
    let asset_reconciliation =
        asset_reconciliation_runtime(config, &store, mail_inventory_reader, &recovery_worlds)?;
    let poll_interval_seconds = config.worker_poll_interval_seconds();
    let lease_duration_seconds = poll_interval_seconds
        .checked_mul(3)
        .ok_or_else(|| anyhow::anyhow!("Worker claim lease 派生时长超出可表示范围"))?
        .max(30);
    let auction_poll_interval = std::time::Duration::from_secs(poll_interval_seconds);
    let auction_lease_duration = std::time::Duration::from_secs(lease_duration_seconds);
    // 批量与维护循环共享的连接配额。关键循环（生命周期、慢速代理、独立结算、履约）
    // 从不获取许可，因此始终保留配额之外的连接；配额来源是同一服务池上限，不新增配置项。
    let bulk_quota = auction_jobs::WorkerConnectionQuota::new(
        config.database_pool.worker_bulk_connection_budget(),
    )
    .map(Arc::new)
    .context("Worker 批量连接配额必须大于 0")?;
    // 首次就绪探测仍在 runner 启动前发布；详情在 runner 标记 Serving 前省略冷配置摘要。
    let mut shutdown = shutdown;
    let stopping = tokio::select! {
        biased;
        () = &mut shutdown => true,
        result = heartbeat.publish_current() => {
            if let Err(error) = result {
                tracing::warn!(error = %error, "发布 Worker ready 心跳失败");
            }
            false
        }
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
    let worker = auction_jobs::PersistentWorker {
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
        settlement_returns,
        settlement_return_reader,
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
            retry_delay: std::time::Duration::from_secs(config.worker.recovery_retry_delay_seconds),
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
        hot: hot_config.clone(),
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
    };
    // 热读取的冲突基线取自交给 runner 的同一份装配，只覆盖实际运行的循环；周期读取在各循环的
    // 启动运行配置确定后才开始，且只有配置了 runtime 文件才启动。
    let _hot_config_task = config.runtime_config_path.clone().map(|path| {
        let baseline = auction_jobs::WorkerHotBaseline::for_worker(&worker);
        spawn_hot_config_watch(path, hot_config, baseline, Arc::clone(&store))
    });
    let runner = worker.run_with_consignment_until_shutdown(consignment, shutdown);
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
    // 启用关键段预算时由外层共享截止等待 PM；零预算保留旧的立即中止。
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
        tracing::warn!(error = %error, "发布 Worker stopping 心跳失败");
    }
    // 只有一个接收者；即使控制任务尚未创建等待 future，也必须保留停机通知。
    control_shutdown.notify_one();
    let control_result =
        join_worker_control_task(&mut control_task, "persistent worker control").await;
    result.context("持久 Worker 运行失败")?;
    control_result
}
