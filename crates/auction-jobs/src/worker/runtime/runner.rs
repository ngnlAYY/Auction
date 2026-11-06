//! Worker 持久化运行时的启动、任务装配与优雅排空。

use auction_application::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_observability::RateLimitedLog;

use super::*;
use crate::worker::diagnostics;

/// 无数据库时仅用于保持进程存活的启动骨架。
/// 真实 Worker 必须使用下面的 `run_persistent_with_settlement_until_shutdown`；
/// 该函数不执行任何任务，避免在没有持久化端口时误把内存等待器当成生产调度器。
pub async fn run_until_shutdown(
    concurrency: u16,
    shutdown: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    if concurrency == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "worker concurrency must be greater than zero",
        ));
    }
    tracing::info!(worker_concurrency = concurrency, "auction worker started");
    shutdown.await;
    tracing::info!("auction worker stopping");
    Ok(())
}

/// 使用持久化轮次表和结算 workflow 运行 Worker。
/// 结算通过 `SettlementCommandPort` 进入应用层，Worker 不复制结算计算。
pub async fn run_persistent_with_settlement_until_shutdown<C, S>(
    worker: PersistentWorker<C>,
    shutdown: S,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
    S: std::future::Future<Output = ()>,
{
    if !worker.auction.validate() {
        return Err(WorkerError::InvalidAuctionConfiguration);
    }
    if !worker.delivery.validate() {
        return Err(WorkerError::InvalidDeliveryConfiguration);
    }
    if !worker.fulfillment.validate() {
        return Err(WorkerError::InvalidFulfillmentConfiguration);
    }
    if !worker.recovery.validate() {
        return Err(WorkerError::InvalidRecoveryConfiguration);
    }
    if worker.mail_polls.iter().any(|runtime| !runtime.validate()) {
        return Err(WorkerError::InvalidMailPollConfiguration);
    }
    if !worker.forum.validate()
        || worker
            .forum_sync
            .as_ref()
            .is_some_and(|runtime| runtime.item_timeout >= worker.forum.lease_duration)
        || worker
            .forum_reply
            .as_ref()
            .is_some_and(|runtime| runtime.item_timeout >= worker.forum.lease_duration)
    {
        return Err(WorkerError::InvalidForumConfiguration);
    }
    if worker.fulfillment_store.is_some() != worker.fulfillment_commands.is_some() {
        return Err(WorkerError::InvalidFulfillmentConfiguration);
    }
    if worker.slow_proxy_due.is_some() != worker.slow_proxy_commands.is_some() {
        return Err(WorkerError::InvalidAuctionConfiguration);
    }
    if worker.schedule_runtime.is_some() && worker.worlds.is_none() {
        return Err(WorkerError::InvalidAuctionConfiguration);
    }
    if worker.worker_id.is_empty() || worker.worker_id != worker.worker_id.trim() {
        return Err(WorkerError::InvalidWorkerIdentity);
    }

    let worker = Arc::new(worker);
    let diagnostic_logs = diagnostics::install();
    let _diagnostic_guard = diagnostics::DiagnosticLogGuard::new(diagnostic_logs.clone());

    bind_worker_metric_modes(worker.metrics.as_ref());

    tokio::pin!(shutdown);
    // spawn 可立即在其他线程领取工作；先消费启动等待期间已到达的停止请求。
    tokio::select! {
        biased;
        () = &mut shutdown => {
            worker.control.mark_draining();
            return Ok(());
        }
        () = std::future::ready(()) => {}
    }
    tracing::info!(
        worker_id = %worker.worker_id,
        worker_concurrency = worker.auction.concurrency,
        "persistent auction worker started"
    );
    worker.control.mark_serving();
    let (drain_sender, _) = tokio::sync::watch::channel(false);
    let poll_wakeup = worker.control.poll_wakeup();
    let mut poll_wakeup_wait = Box::pin(poll_wakeup.notified());
    let mut delivery_cycles = JoinSet::new();
    let delivery_wakeup = Arc::new(tokio::sync::Notify::new());
    let mut delivery_wakeup_tasks = JoinSet::new();
    let mut delivery_wake_pending = false;
    let delivery_enabled = worker.delivery_store.is_some() && worker.delivery_transport.is_some();
    if delivery_enabled {
        if let Some(store) = &worker.delivery_store {
            delivery_wakeup_tasks.spawn(run_background(
                worker.write_lifecycle.clone(),
                delivery::run_delivery_wakeup(
                    Arc::clone(store),
                    Arc::clone(&delivery_wakeup),
                    drain_sender.subscribe(),
                ),
            ));
        }
        let delivery_worker = Arc::clone(&worker);
        delivery_cycles.spawn(diagnostics::inherit(async move {
            run_delivery_cycle(delivery_worker.as_ref()).await
        }));
    }
    let mut delivery_scheduled_wake = tokio::time::Instant::now();
    let mut delivery_sleep = Box::pin(tokio::time::sleep(std::time::Duration::ZERO));
    // 仅给启动时有限个常驻任务保留指标 owner；不参与调度或恢复。
    let mut diagnostic_task_loops = std::collections::HashMap::new();
    let mut operations_alert_tasks = JoinSet::new();
    if let Some(runtime) = &worker.operations_alert {
        for action in [
            OperationsAlertLoopAction::Scan,
            OperationsAlertLoopAction::Dispatch,
            OperationsAlertLoopAction::Reconcile,
        ] {
            let diagnostic_loop = action.loop_kind();
            let diagnostic_task = operations_alert_tasks.spawn(run_background(
                worker.write_lifecycle.clone(),
                run_operations_alert_loop(
                    Arc::clone(runtime),
                    Arc::clone(&worker.clock),
                    Arc::clone(&worker.metrics),
                    action,
                    worker.control.clone(),
                    worker.bulk_quota.clone(),
                    drain_sender.subscribe(),
                ),
            ));
            diagnostic_task_loops.insert(diagnostic_task.id(), diagnostic_loop);
        }
    }
    let mut auction_tasks = JoinSet::new();
    auction_tasks.spawn(run_background(
        worker.write_lifecycle.clone(),
        auction::run_loop(Arc::clone(&worker), drain_sender.subscribe()),
    ));
    // W12.5：结算独立成任务，结算单项的耗时不再推迟生命周期与慢速代理的推进。
    let mut settlement_tasks = JoinSet::new();
    if worker.settlement_store.is_some() && worker.settlement_commands.is_some() {
        settlement_tasks.spawn(run_background(
            worker.write_lifecycle.clone(),
            auction::run_settlement_loop(Arc::clone(&worker), drain_sender.subscribe()),
        ));
    }
    let mut fulfillment_tasks = JoinSet::new();
    if worker.fulfillment_store.is_some() && worker.fulfillment_commands.is_some() {
        fulfillment_tasks.spawn(run_background(
            worker.write_lifecycle.clone(),
            fulfillment::run_loop(Arc::clone(&worker), drain_sender.subscribe()),
        ));
    }
    let mut recovery_tasks = JoinSet::new();
    if worker.recovery_health.is_some()
        || worker.asset_reconciliation.is_some()
        || worker.recovery_store.is_some()
        || worker.recovery_resolution.is_some()
    {
        recovery_tasks.spawn(run_background(
            worker.write_lifecycle.clone(),
            recovery::run_loop(Arc::clone(&worker), drain_sender.subscribe()),
        ));
    }
    let mut player_notification_tasks = JoinSet::new();
    if worker.player_notifications.is_some() {
        player_notification_tasks.spawn(run_background(
            worker.write_lifecycle.clone(),
            player_notifications::run_loop(Arc::clone(&worker), drain_sender.subscribe()),
        ));
    }
    let mut mail_poll_tasks = JoinSet::new();
    let mut mail_maintenance_tasks = JoinSet::new();
    for runtime in worker.mail_polls.iter().cloned() {
        if runtime.has_independent_maintenance() {
            let diagnostic_loop = if runtime.world_id == "isekai" {
                WorkerLoop::MailMaintenanceIsekai
            } else {
                WorkerLoop::MailMaintenanceMain
            };
            let diagnostic_task = mail_maintenance_tasks.spawn(mail_poll::run_maintenance_loop(
                Arc::clone(&worker),
                runtime.clone(),
                drain_sender.subscribe(),
            ));
            diagnostic_task_loops.insert(diagnostic_task.id(), diagnostic_loop);
        }
        let diagnostic_loop = runtime.loop_kind();
        let diagnostic_task = mail_poll_tasks.spawn(mail_poll::run_loop(
            Arc::clone(&worker),
            runtime,
            drain_sender.subscribe(),
        ));
        diagnostic_task_loops.insert(diagnostic_task.id(), diagnostic_loop);
    }
    let mut forum_tasks = JoinSet::new();
    if worker.forum_sync.is_some() || worker.forum_reply.is_some() {
        forum_tasks.spawn(forum::run_loop(Arc::clone(&worker), drain_sender.subscribe()));
    }
    let mut realtime_tasks = JoinSet::new();
    if worker.realtime_consumer.is_some() {
        realtime_tasks.spawn(run_background(
            worker.write_lifecycle.clone(),
            realtime_consumer::run_loop(Arc::clone(&worker), drain_sender.subscribe()),
        ));
    }

    let mut delivery_failure_log = RateLimitedLog::new(module_path!(), "delivery_cycle_failed");
    let fatal_error = loop {
        tokio::select! {
            biased;
            () = &mut shutdown => break None,
            () = delivery_failure_log.flush_when_due() => {}
            () = diagnostic_logs.flush_when_due() => {}
            _ = &mut delivery_sleep, if delivery_enabled && delivery_cycles.is_empty() =>
            {
                worker.metrics.observe_schedule_delay(
                    WorkerLoop::Delivery,
                    tokio::time::Instant::now().saturating_duration_since(delivery_scheduled_wake),
                );
                let delivery_worker = Arc::clone(&worker);
                delivery_cycles.spawn(diagnostics::inherit(async move {
                    run_delivery_cycle(delivery_worker.as_ref()).await
                }));
            }
            result = delivery_cycles.join_next(), if !delivery_cycles.is_empty() => {
                match result {
                    Some(Ok(Ok(()))) => {}
                    Some(Ok(Err(error))) => {
                        // 只去重附加日志；轮询节奏、指标、持久化与未知外写处置沿用原路径。
                        if tracing::enabled!(tracing::Level::WARN)
                            && delivery_failure_log.should_log()
                        {
                            diagnostics::warn(
                                "worker_runtime",
                                "poll_delivery",
                                error.diagnostic_kind(),
                                DiagnosticContext::default(),
                            );
                        }
                    }
                    Some(Err(error)) => {
                        observe_task_join(
                            worker.metrics.as_ref(),
                            Some(WorkerLoop::Delivery),
                            &error,
                        );
                        break Some(WorkerError::DeliveryTaskJoin(error));
                    }
                    None => {}
                }
                let wait = if std::mem::take(&mut delivery_wake_pending) {
                    std::time::Duration::ZERO
                } else {
                    let delivery = worker.delivery_runtime();
                    poll_delay(delivery.poll_interval, delivery.jitter)
                };
                delivery_scheduled_wake = tokio::time::Instant::now() + wait;
                delivery_sleep.as_mut().reset(delivery_scheduled_wake);
            }
            result = delivery_wakeup_tasks.join_next(), if !delivery_wakeup_tasks.is_empty() => {
                if let Some(Err(error)) = result {
                    if !error.is_cancelled() {
                        worker.metrics.observe_unpersisted_error(
                            WorkerLoop::Delivery,
                            diagnostics::join_kind(&error),
                        );
                    }
                    break Some(WorkerError::DeliveryTaskJoin(error));
                }
            }
            _ = delivery_wakeup.notified(), if delivery_enabled => {
                if delivery_cycles.is_empty() {
                    delivery_scheduled_wake = tokio::time::Instant::now();
                    delivery_sleep.as_mut().reset(delivery_scheduled_wake);
                } else {
                    // 在途轮次结束后立刻再领取，不取消已经取得的安全发送句柄。
                    delivery_wake_pending = true;
                }
            }
            result = operations_alert_tasks.join_next(), if !operations_alert_tasks.is_empty() => {
                match result {
                    Some(Err(error)) => {
                        observe_task_join(
                            worker.metrics.as_ref(),
                            diagnostic_task_loops.get(&error.id()).copied(),
                            &error,
                        );
                        break Some(WorkerError::OperationsAlertTaskJoin(error));
                    }
                    Some(Ok(())) if worker.control.is_draining() => {}
                    Some(Ok(())) | None => break Some(WorkerError::OperationsAlertTaskExited),
                }
            }
            result = auction_tasks.join_next(), if !auction_tasks.is_empty() => {
                match result {
                    Some(Err(error)) => {
                        observe_task_join(
                            worker.metrics.as_ref(),
                            Some(WorkerLoop::Auction),
                            &error,
                        );
                        break Some(WorkerError::AuctionTaskJoin(error));
                    }
                    Some(Ok(())) if worker.control.is_draining() => {}
                    Some(Ok(())) | None => break Some(WorkerError::AuctionTaskExited),
                }
            }
            result = settlement_tasks.join_next(), if !settlement_tasks.is_empty() => {
                match result {
                    Some(Err(error)) => {
                        observe_task_join(
                            worker.metrics.as_ref(),
                            Some(WorkerLoop::Settlement),
                            &error,
                        );
                        break Some(WorkerError::SettlementTaskJoin(error));
                    }
                    Some(Ok(())) if worker.control.is_draining() => {}
                    Some(Ok(())) | None => break Some(WorkerError::SettlementTaskExited),
                }
            }
            result = fulfillment_tasks.join_next(), if !fulfillment_tasks.is_empty() => {
                match result {
                    Some(Err(error)) => {
                        observe_task_join(
                            worker.metrics.as_ref(),
                            Some(WorkerLoop::AuctionFulfillment),
                            &error,
                        );
                        break Some(WorkerError::FulfillmentTaskJoin(error));
                    }
                    Some(Ok(())) if worker.control.is_draining() => {}
                    Some(Ok(())) | None => break Some(WorkerError::FulfillmentTaskExited),
                }
            }
            result = recovery_tasks.join_next(), if !recovery_tasks.is_empty() => {
                match result {
                    Some(Err(error)) => {
                        observe_task_join(
                            worker.metrics.as_ref(),
                            Some(WorkerLoop::AuctionRecovery),
                            &error,
                        );
                        break Some(WorkerError::RecoveryTaskJoin(error));
                    }
                    Some(Ok(())) if worker.control.is_draining() => {}
                    Some(Ok(())) | None => break Some(WorkerError::RecoveryTaskExited),
                }
            }
            result = player_notification_tasks.join_next(), if !player_notification_tasks.is_empty() => {
                match result {
                    Some(Err(error)) => {
                        observe_task_join(
                            worker.metrics.as_ref(),
                            Some(WorkerLoop::PlayerNotifications),
                            &error,
                        );
                        break Some(WorkerError::PlayerNotificationTaskJoin(error));
                    }
                    Some(Ok(())) if worker.control.is_draining() => {}
                    Some(Ok(())) | None => break Some(WorkerError::PlayerNotificationTaskExited),
                }
            }
            result = mail_poll_tasks.join_next(), if !mail_poll_tasks.is_empty() => {
                match result {
                    Some(Err(error)) => {
                        observe_task_join(
                            worker.metrics.as_ref(),
                            diagnostic_task_loops.get(&error.id()).copied(),
                            &error,
                        );
                        break Some(WorkerError::MailPollTaskJoin(error));
                    }
                    Some(Ok(())) if worker.control.is_draining() => {}
                    Some(Ok(())) | None => break Some(WorkerError::MailPollTaskExited),
                }
            }
            result = mail_maintenance_tasks.join_next(), if !mail_maintenance_tasks.is_empty() => {
                match result {
                    Some(Err(error)) => {
                        observe_task_join(
                            worker.metrics.as_ref(),
                            diagnostic_task_loops.get(&error.id()).copied(),
                            &error,
                        );
                        break Some(WorkerError::MailMaintenanceTaskJoin(error));
                    }
                    Some(Ok(())) if worker.control.is_draining() => {}
                    Some(Ok(())) | None => break Some(WorkerError::MailMaintenanceTaskExited),
                }
            }
            result = forum_tasks.join_next(), if !forum_tasks.is_empty() => {
                match result {
                    Some(Err(error)) => {
                        observe_task_join(worker.metrics.as_ref(), Some(WorkerLoop::Forum), &error);
                        break Some(WorkerError::ForumTaskJoin(error));
                    }
                    Some(Ok(())) if worker.control.is_draining() => {}
                    Some(Ok(())) | None => break Some(WorkerError::ForumTaskExited),
                }
            }
            result = realtime_tasks.join_next(), if !realtime_tasks.is_empty() => {
                match result {
                    Some(Err(error)) => {
                        observe_task_join(
                            worker.metrics.as_ref(),
                            Some(WorkerLoop::Realtime),
                            &error,
                        );
                        break Some(WorkerError::RealtimeTaskJoin(error));
                    }
                    Some(Ok(())) if worker.control.is_draining() => {}
                    Some(Ok(())) | None => break Some(WorkerError::RealtimeTaskExited),
                }
            }
            _ = &mut poll_wakeup_wait => {
                if worker.control.take_mail_poll_request() {
                    for runtime in &worker.mail_polls {
                        runtime.trigger();
                    }
                }
                poll_wakeup_wait = Box::pin(poll_wakeup.notified());
            }
        }
    };

    if fatal_error.is_some() {
        // 先通知装配根启动共享截止与 PM drain，再等待任何在途组。
        worker.control.report_fatal();
    } else {
        worker.control.mark_draining();
    }
    let _ = drain_sender.send(true);
    // 先沿用原停止通知，再补齐日志计数，避免排空等待延后窗口汇总。
    drop(delivery_failure_log);
    let drain = async {
        // 单组错误不能提前 drop 其他在途 owner；所有组共享同一总排空截止。
        let results = [
            drain_delivery_tasks(&mut delivery_cycles).await,
            drain_worker_tasks(&mut delivery_wakeup_tasks, "delivery wakeup").await,
            drain_worker_tasks(&mut operations_alert_tasks, "operations alert").await,
            drain_worker_tasks(&mut auction_tasks, "auction").await,
            drain_worker_tasks(&mut settlement_tasks, "settlement").await,
            drain_worker_tasks(&mut fulfillment_tasks, "fulfillment").await,
            drain_worker_tasks(&mut recovery_tasks, "recovery").await,
            drain_worker_tasks(&mut player_notification_tasks, "player notifications").await,
            drain_worker_tasks(&mut mail_poll_tasks, "MoogleMail").await,
            drain_worker_tasks(&mut mail_maintenance_tasks, "MoogleMail maintenance").await,
            drain_worker_tasks(&mut forum_tasks, "forum").await,
            drain_worker_tasks(&mut realtime_tasks, "realtime").await,
        ];
        results.into_iter().find_map(Result::err).map_or(Ok(()), Err)
    };
    // 装配根统一拥有关键段、普通收口与二次信号截止，致命分支不得另建短预算。
    tokio::pin!(drain);
    let drained = loop {
        tokio::select! {
            biased;
            result = &mut drain => break result,
            () = diagnostic_logs.flush_when_due() => {}
        }
    };
    let result = match fatal_error {
        Some(error) => {
            if let Err(drain_error) = drained {
                diagnostics::warn(
                    "worker_runtime",
                    "drain_shutdown",
                    drain_error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
            }
            Err(error)
        }
        None => drained,
    };
    diagnostic_logs.close();
    tracing::info!(worker_id = %worker.worker_id, "auction worker stopping");
    result
}

/// 非外写 owner 在停机时立即释放；取消不产生任何持久失败或释放命令。
async fn run_background<F: std::future::Future>(
    lifecycle: auction_application::ExternalWriteLifecycle,
    future: F,
) {
    let _completed = lifecycle.background(future).await;
}

/// 常驻任务的 panic/join 不经过业务失败持久化；正常取消不增加计数。
fn observe_task_join(
    metrics: &dyn WorkerRecorder,
    loop_kind: Option<WorkerLoop>,
    error: &tokio::task::JoinError,
) {
    if let Some(loop_kind) = loop_kind
        && !error.is_cancelled()
    {
        if error.is_panic() {
            metrics.observe_task_panic(
                loop_kind,
                auction_observability::WorkerPanicOwner::CriticalLoop,
            );
        }
        metrics.observe_unpersisted_error(loop_kind, diagnostics::join_kind(error));
    }
}
