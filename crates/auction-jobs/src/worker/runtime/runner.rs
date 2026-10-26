//! Worker 持久化运行时的启动、任务装配与优雅排空。

use super::*;

/// 无数据库时仅用于保持进程存活的启动骨架。
/// 真实 Worker 必须使用下面的 `run_persistent_until_shutdown`；该函数不执行任何任务，避免
/// 在没有持久化端口时误把内存等待器当成生产调度器。
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

/// 使用持久化轮次表运行 Worker。
/// interval 只是唤醒信号；每一轮都会重新从数据库领取 due 且租约可用的记录。生命周期
/// 状态迁移统一调用 `auction-application`，本 crate 不复制领域状态机。
pub async fn run_persistent_until_shutdown<C, S>(
    schedule: Arc<dyn ScheduledRoundStore>,
    lifecycle: Arc<dyn AuctionLifecycleCommandPort>,
    clock: Arc<C>,
    worker_id: String,
    concurrency: u16,
    poll_interval: std::time::Duration,
    shutdown: S,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
    S: std::future::Future<Output = ()>,
{
    let lease_duration_seconds = poll_interval
        .as_secs()
        .checked_mul(3)
        .ok_or(WorkerError::InvalidAuctionConfiguration)?
        .max(30);
    let auction_lease = std::time::Duration::from_secs(lease_duration_seconds);
    let auction_item_timeout = std::time::Duration::from_secs(30)
        .min(auction_lease.saturating_sub(std::time::Duration::from_millis(1)));
    run_persistent_with_settlement_until_shutdown(
        PersistentWorker {
            schedule,
            worlds: None,
            lifecycle,
            slow_proxy_due: None,
            slow_proxy_commands: None,
            auction: AuctionWorkerRuntimeConfig {
                batch_limit: concurrency,
                concurrency,
                poll_interval,
                jitter: std::time::Duration::ZERO,
                lease_duration: auction_lease,
                item_timeout: auction_item_timeout,
            },
            schedule_runtime: None,
            settlement_store: None,
            settlement_commands: None,
            fulfillment_store: None,
            fulfillment_commands: None,
            fulfillment: FulfillmentWorkerRuntimeConfig::default(),
            recovery_health: None,
            recovery_store: None,
            recovery_resolution: None,
            recovery_delivery: None,
            recovery_blacklister: None,
            recovery: RecoveryWorkerRuntimeConfig::default(),
            delivery_store: None,
            delivery_transport: None,
            delivery: DeliveryWorkerRuntimeConfig::default(),
            mail_polls: Vec::new(),
            forum: ForumWorkerRuntimeConfig::default(),
            forum_sync: None,
            forum_reply: None,
            operations_alert: None,
            realtime_consumer: None,
            metrics: auction_observability::noop_worker_recorder(),
            control: WorkerControlHandle::new("unknown", 0),
            clock,
            worker_id,
        },
        shutdown,
    )
    .await
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
            delivery_wakeup_tasks.spawn(delivery::run_delivery_wakeup(
                Arc::clone(store),
                Arc::clone(&delivery_wakeup),
                drain_sender.subscribe(),
            ));
        }
        let delivery_worker = Arc::clone(&worker);
        delivery_cycles.spawn(async move { run_delivery_cycle(delivery_worker.as_ref()).await });
    }
    let mut delivery_scheduled_wake = tokio::time::Instant::now();
    let mut delivery_sleep = Box::pin(tokio::time::sleep(std::time::Duration::ZERO));
    let mut operations_alert_tasks = JoinSet::new();
    if let Some(runtime) = &worker.operations_alert {
        for action in [
            OperationsAlertLoopAction::Scan,
            OperationsAlertLoopAction::Dispatch,
            OperationsAlertLoopAction::Reconcile,
        ] {
            operations_alert_tasks.spawn(run_operations_alert_loop(
                Arc::clone(runtime),
                Arc::clone(&worker.clock),
                Arc::clone(&worker.metrics),
                action,
                worker.control.clone(),
                drain_sender.subscribe(),
            ));
        }
    }
    let mut auction_tasks = JoinSet::new();
    auction_tasks.spawn(auction::run_loop(Arc::clone(&worker), drain_sender.subscribe()));
    let mut fulfillment_tasks = JoinSet::new();
    if worker.fulfillment_store.is_some() && worker.fulfillment_commands.is_some() {
        fulfillment_tasks
            .spawn(fulfillment::run_loop(Arc::clone(&worker), drain_sender.subscribe()));
    }
    let mut recovery_tasks = JoinSet::new();
    if worker.recovery_health.is_some()
        || worker.recovery_store.is_some()
        || worker.recovery_resolution.is_some()
    {
        recovery_tasks.spawn(recovery::run_loop(Arc::clone(&worker), drain_sender.subscribe()));
    }
    let mut mail_poll_tasks = JoinSet::new();
    for runtime in worker.mail_polls.iter().cloned() {
        mail_poll_tasks.spawn(mail_poll::run_loop(
            Arc::clone(&worker),
            runtime,
            drain_sender.subscribe(),
        ));
    }
    let mut forum_tasks = JoinSet::new();
    if worker.forum_sync.is_some() || worker.forum_reply.is_some() {
        forum_tasks.spawn(forum::run_loop(Arc::clone(&worker), drain_sender.subscribe()));
    }
    let mut realtime_tasks = JoinSet::new();
    if worker.realtime_consumer.is_some() {
        realtime_tasks
            .spawn(realtime_consumer::run_loop(Arc::clone(&worker), drain_sender.subscribe()));
    }

    loop {
        tokio::select! {
            biased;
            () = &mut shutdown => {
                worker.control.mark_draining();
                let _ = drain_sender.send(true);
                // 单组错误不能提前 drop 其他在途 owner；共享装配根的总排空截止时间，
                // 按原顺序等待所有组之后再返回首个错误，不为每组重置预算。
                let results = [
                    drain_delivery_tasks(&mut delivery_cycles).await,
                    drain_worker_tasks(&mut delivery_wakeup_tasks, "delivery wakeup").await,
                    drain_worker_tasks(&mut operations_alert_tasks, "operations alert").await,
                    drain_worker_tasks(&mut auction_tasks, "auction").await,
                    drain_worker_tasks(&mut fulfillment_tasks, "fulfillment").await,
                    drain_worker_tasks(&mut recovery_tasks, "recovery").await,
                    drain_worker_tasks(&mut mail_poll_tasks, "MoogleMail").await,
                    drain_worker_tasks(&mut forum_tasks, "forum").await,
                    drain_worker_tasks(&mut realtime_tasks, "realtime").await,
                ];
                tracing::info!(worker_id = %worker.worker_id, "auction worker stopping");
                return results.into_iter().find_map(Result::err).map_or(Ok(()), Err);
            }
            _ = &mut delivery_sleep, if delivery_enabled && delivery_cycles.is_empty() =>
            {
                worker.metrics.observe_schedule_delay(
                    WorkerLoop::Delivery,
                    tokio::time::Instant::now().saturating_duration_since(delivery_scheduled_wake),
                );
                let delivery_worker = Arc::clone(&worker);
                delivery_cycles.spawn(async move {
                    run_delivery_cycle(delivery_worker.as_ref()).await
                });
            }
            result = delivery_cycles.join_next(), if !delivery_cycles.is_empty() => {
                match result {
                    Some(Ok(Ok(()))) => {}
                    Some(Ok(Err(error))) => {
                        tracing::warn!(error = %error, "delivery worker cycle failed; retrying after poll delay");
                    }
                    Some(Err(error)) => {
                        worker.control.mark_degraded();
                        return Err(WorkerError::DeliveryTaskJoin(error));
                    }
                    None => {}
                }
                let wait = if std::mem::take(&mut delivery_wake_pending) {
                    std::time::Duration::ZERO
                } else {
                    poll_delay(worker.delivery.poll_interval, worker.delivery.jitter)
                };
                delivery_scheduled_wake = tokio::time::Instant::now() + wait;
                delivery_sleep.as_mut().reset(delivery_scheduled_wake);
            }
            result = delivery_wakeup_tasks.join_next(), if !delivery_wakeup_tasks.is_empty() => {
                if let Some(Err(error)) = result {
                    worker.control.mark_degraded();
                    return Err(WorkerError::DeliveryTaskJoin(error));
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
                worker.control.mark_degraded();
                match result {
                    Some(Err(error)) => return Err(WorkerError::OperationsAlertTaskJoin(error)),
                    Some(Ok(())) | None => return Err(WorkerError::OperationsAlertTaskExited),
                }
            }
            result = auction_tasks.join_next(), if !auction_tasks.is_empty() => {
                worker.control.mark_degraded();
                match result {
                    Some(Err(error)) => return Err(WorkerError::AuctionTaskJoin(error)),
                    Some(Ok(())) | None => return Err(WorkerError::AuctionTaskExited),
                }
            }
            result = fulfillment_tasks.join_next(), if !fulfillment_tasks.is_empty() => {
                worker.control.mark_degraded();
                match result {
                    Some(Err(error)) => return Err(WorkerError::FulfillmentTaskJoin(error)),
                    Some(Ok(())) | None => return Err(WorkerError::FulfillmentTaskExited),
                }
            }
            result = recovery_tasks.join_next(), if !recovery_tasks.is_empty() => {
                worker.control.mark_degraded();
                match result {
                    Some(Err(error)) => return Err(WorkerError::RecoveryTaskJoin(error)),
                    Some(Ok(())) | None => return Err(WorkerError::RecoveryTaskExited),
                }
            }
            result = mail_poll_tasks.join_next(), if !mail_poll_tasks.is_empty() => {
                worker.control.mark_degraded();
                match result {
                    Some(Err(error)) => return Err(WorkerError::MailPollTaskJoin(error)),
                    Some(Ok(())) | None => return Err(WorkerError::MailPollTaskExited),
                }
            }
            result = forum_tasks.join_next(), if !forum_tasks.is_empty() => {
                worker.control.mark_degraded();
                match result {
                    Some(Err(error)) => return Err(WorkerError::ForumTaskJoin(error)),
                    Some(Ok(())) | None => return Err(WorkerError::ForumTaskExited),
                }
            }
            result = realtime_tasks.join_next(), if !realtime_tasks.is_empty() => {
                worker.control.mark_degraded();
                match result {
                    Some(Err(error)) => return Err(WorkerError::RealtimeTaskJoin(error)),
                    Some(Ok(())) | None => return Err(WorkerError::RealtimeTaskExited),
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
    }
}
