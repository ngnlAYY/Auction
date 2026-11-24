//! 寄售处置执行循环：持久工作、租约与 fence 决定何时执行，通知只负责唤醒。
//!
//! 应用用例已经保证只有首次起写提交会拿到一次性 POST 许可；本循环只负责节奏、暂停与
//! 有界退避，不缓存任何许可或完成状态。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{ConsignmentError, ConsignmentFuture};
use auction_application::core::Clock;
use auction_application::mail::{
    DispositionMaintenanceService, DispositionRound, DispositionRoundRequest,
};
use auction_observability::WorkerResult;
use auction_time::TimeDelta;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

const IDLE_POLL: Duration = Duration::from_secs(5);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAXIMUM: Duration = Duration::from_secs(30);

/// 应用处置执行用例的单轮入口；测试以同一接口替换持久工作与 4D1 适配器。
pub trait DispositionRoundRunner: Send + Sync {
    fn run_once<'a>(
        &'a self,
        request: DispositionRoundRequest,
    ) -> ConsignmentFuture<'a, DispositionRound>;
}

impl DispositionRoundRunner for DispositionMaintenanceService {
    fn run_once<'a>(
        &'a self,
        request: DispositionRoundRequest,
    ) -> ConsignmentFuture<'a, DispositionRound> {
        Box::pin(DispositionMaintenanceService::run_once(self, request))
    }
}

/// 处置执行运行参数。
pub struct DispositionMaintenanceRuntime {
    /// 应用用例。
    pub service: Arc<dyn DispositionRoundRunner>,
    /// 本地唤醒时钟。
    pub clock: Arc<dyn Clock>,
    /// 当前 Worker 实例名。
    pub worker_id: String,
    /// 每次领取的持久租约时长。
    pub lease_duration: Duration,
    /// 可选通知生产者；丢失通知时由有界轮询恢复。
    pub wakeup: Arc<Notify>,
    /// 外部访问暂停快照。
    pub paused: Arc<dyn Fn() -> bool + Send + Sync>,
}

/// 运行处置执行循环，直到 drain 或 owner 离开。
pub async fn run(
    runtime: Arc<DispositionMaintenanceRuntime>,
    mut drain: watch::Receiver<bool>,
    observe: impl Fn(WorkerResult, Duration) + Send + Sync,
) {
    let mut backoff = BACKOFF_INITIAL;
    while !stopped(&drain) {
        if (runtime.paused)() {
            if wait_for_wake(&mut drain, runtime.wakeup.as_ref(), IDLE_POLL).await {
                return;
            }
            continue;
        }
        let Some(lease) = TimeDelta::from_std(runtime.lease_duration).ok() else {
            tracing::error!("寄售处置租约时长无效");
            return;
        };
        let now = runtime.clock.now();
        let Some(lease_until) = now.checked_add_signed(lease) else {
            tracing::error!("寄售处置租约截止时间溢出");
            return;
        };
        let Some(deadline) = Instant::now().into_std().checked_add(runtime.lease_duration) else {
            tracing::error!("寄售处置外部截止时间溢出");
            return;
        };
        let started_at = Instant::now();
        let result = runtime
            .service
            .run_once(DispositionRoundRequest {
                worker_id: runtime.worker_id.clone(),
                now,
                lease_until,
                deadline,
            })
            .await;
        let (worker_result, wait) = match result {
            Ok(DispositionRound::Applied) => {
                tracing::info!("寄售处置证明已应用");
                (WorkerResult::Success, Wait::Immediate)
            }
            Ok(DispositionRound::Reconciled | DispositionRound::Retried) => {
                // 不变量 5：`Retried` 的到期时间已在释放时持久化；本轮立即继续只为处理其他
                // 到期项，同一项在 retry_at 前不会被存储再次返回。
                (WorkerResult::Success, Wait::Immediate)
            }
            Ok(DispositionRound::Idle) => (WorkerResult::Success, Wait::Idle),
            Ok(DispositionRound::LeaseLost) | Err(ConsignmentError::LeaseLost) => {
                tracing::debug!(code = "lease_lost", "寄售处置围栏已失效");
                (WorkerResult::Canceled, Wait::Immediate)
            }
            Err(error) => {
                tracing::warn!(code = error_code(error), "寄售处置一轮未确认完成");
                (WorkerResult::Error, Wait::Backoff)
            }
        };
        observe(worker_result, started_at.elapsed());
        if stopped(&drain) {
            return;
        }
        match wait {
            Wait::Immediate => {
                backoff = BACKOFF_INITIAL;
                tokio::task::yield_now().await;
            }
            Wait::Idle => {
                backoff = BACKOFF_INITIAL;
                if wait_for_wake(&mut drain, runtime.wakeup.as_ref(), IDLE_POLL).await {
                    return;
                }
            }
            Wait::Backoff => {
                if wait_for_backoff(&mut drain, backoff).await {
                    return;
                }
                backoff = backoff.saturating_mul(2).min(BACKOFF_MAXIMUM);
            }
        }
    }
}

enum Wait {
    Immediate,
    Idle,
    Backoff,
}

fn stopped(drain: &watch::Receiver<bool>) -> bool {
    *drain.borrow() || drain.has_changed().is_err()
}

async fn wait_for_wake(
    drain: &mut watch::Receiver<bool>,
    wakeup: &Notify,
    delay: Duration,
) -> bool {
    tokio::select! {
        biased;
        changed = drain.changed() => changed.is_err() || *drain.borrow(),
        () = wakeup.notified() => false,
        () = tokio::time::sleep(delay) => false,
    }
}

async fn wait_for_backoff(drain: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    tokio::select! {
        biased;
        changed = drain.changed() => changed.is_err() || *drain.borrow(),
        () = tokio::time::sleep(delay) => false,
    }
}

fn error_code(error: ConsignmentError) -> &'static str {
    error.api_error().map_or("consignment_unavailable", |(_, code)| code)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use auction_application::consignment::{ConsignmentError, ConsignmentFuture};
    use auction_application::mail::{DispositionRound, DispositionRoundRequest};
    use auction_observability::WorkerResult;
    use tokio::sync::{Notify, watch};
    use tokio::time::{Duration, advance};

    use super::{DispositionMaintenanceRuntime, DispositionRoundRunner, run};
    use crate::worker::runtime::test_fixture::PausedClock;

    struct FakeRound {
        calls: AtomicUsize,
        available: AtomicBool,
        remote_reads: AtomicUsize,
        outcomes: Mutex<VecDeque<Result<DispositionRound, ConsignmentError>>>,
    }

    impl FakeRound {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::default(),
                available: AtomicBool::default(),
                remote_reads: AtomicUsize::default(),
                outcomes: Mutex::default(),
            }
        }

        fn push(&self, outcome: Result<DispositionRound, ConsignmentError>) {
            self.outcomes.lock().expect("结果队列").push_back(outcome);
        }
    }

    impl DispositionRoundRunner for FakeRound {
        fn run_once<'a>(
            &'a self,
            _request: DispositionRoundRequest,
        ) -> ConsignmentFuture<'a, DispositionRound> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let outcome = match self.outcomes.lock().expect("结果队列").pop_front() {
                Some(outcome) => outcome,
                None if self.available.swap(false, Ordering::Relaxed) => {
                    Ok(DispositionRound::Applied)
                }
                None => Ok(DispositionRound::Idle),
            };
            if matches!(
                outcome,
                Ok(DispositionRound::Applied
                    | DispositionRound::Reconciled
                    | DispositionRound::Retried)
            ) {
                self.remote_reads.fetch_add(1, Ordering::Relaxed);
            }
            Box::pin(async move { outcome })
        }
    }

    fn runtime(service: Arc<FakeRound>) -> Arc<DispositionMaintenanceRuntime> {
        runtime_with_pause(service, Arc::new(|| false))
    }

    fn runtime_with_pause(
        service: Arc<FakeRound>,
        paused: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Arc<DispositionMaintenanceRuntime> {
        Arc::new(DispositionMaintenanceRuntime {
            service,
            clock: Arc::new(PausedClock::new(auction_time::Timestamp::UNIX_EPOCH)),
            worker_id: "test-worker".to_owned(),
            lease_duration: Duration::from_secs(30),
            wakeup: Arc::new(Notify::default()),
            paused,
        })
    }

    async fn settle() {
        for _ in 0..12 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn processed_round_immediately_continues_then_idles() {
        let service = Arc::new(FakeRound::new());
        service.push(Ok(DispositionRound::Applied));
        service.push(Ok(DispositionRound::Idle));
        let runtime = runtime(service.clone());
        let (drain, receiver) = watch::channel(false);
        let observed = Arc::new(Mutex::new(Vec::new()));
        let recorded = observed.clone();
        let task = tokio::spawn(run(runtime, receiver, move |result, _| {
            recorded.lock().expect("观察锁").push(result);
        }));
        settle().await;
        assert_eq!(service.calls.load(Ordering::Relaxed), 2);
        assert_eq!(
            *observed.lock().expect("观察锁"),
            [WorkerResult::Success, WorkerResult::Success]
        );
        advance(Duration::from_secs(5)).await;
        settle().await;
        assert!(service.calls.load(Ordering::Relaxed) >= 3);
        drain.send_replace(true);
        task.await.expect("循环任务");
    }

    #[tokio::test(start_paused = true)]
    async fn lease_lost_is_canceled_without_backoff() {
        let service = Arc::new(FakeRound::new());
        service.push(Ok(DispositionRound::LeaseLost));
        service.push(Ok(DispositionRound::Idle));
        let runtime = runtime(service.clone());
        let (drain, receiver) = watch::channel(false);
        let observed = Arc::new(Mutex::new(Vec::new()));
        let recorded = observed.clone();
        let task = tokio::spawn(run(runtime, receiver, move |result, _| {
            recorded.lock().expect("观察锁").push(result);
        }));
        settle().await;
        assert_eq!(service.calls.load(Ordering::Relaxed), 2);
        assert_eq!(
            *observed.lock().expect("观察锁"),
            [WorkerResult::Canceled, WorkerResult::Success]
        );
        drain.send_replace(true);
        task.await.expect("循环任务");
    }

    #[tokio::test(start_paused = true)]
    async fn errors_back_off_before_the_next_round() {
        let service = Arc::new(FakeRound::new());
        service.push(Err(ConsignmentError::Unavailable));
        service.push(Ok(DispositionRound::Idle));
        let runtime = runtime(service.clone());
        let (drain, receiver) = watch::channel(false);
        let observed = Arc::new(Mutex::new(Vec::new()));
        let recorded = observed.clone();
        let task = tokio::spawn(run(runtime, receiver, move |result, _| {
            recorded.lock().expect("观察锁").push(result);
        }));
        settle().await;
        assert_eq!(service.calls.load(Ordering::Relaxed), 1);
        advance(Duration::from_millis(999)).await;
        settle().await;
        assert_eq!(service.calls.load(Ordering::Relaxed), 1);
        advance(Duration::from_millis(1)).await;
        settle().await;
        assert!(service.calls.load(Ordering::Relaxed) >= 2);
        assert_eq!(
            *observed.lock().expect("观察锁").first().expect("首个观察"),
            WorkerResult::Error
        );
        drain.send_replace(true);
        task.await.expect("循环任务");
    }

    #[tokio::test(start_paused = true)]
    async fn already_draining_does_not_claim() {
        let service = Arc::new(FakeRound::new());
        let runtime = runtime(service.clone());
        let (_drain, receiver) = watch::channel(true);
        run(runtime, receiver, |_, _| {}).await;
        assert_eq!(service.calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn retried_immediately_rechecks_storage_without_second_remote_read() {
        let service = Arc::new(FakeRound::new());
        service.push(Ok(DispositionRound::Retried));
        service.push(Ok(DispositionRound::Idle));
        let runtime = runtime(service.clone());
        let (drain, receiver) = watch::channel(false);
        let task = tokio::spawn(run(runtime, receiver, |_, _| {}));
        settle().await;
        assert_eq!(service.calls.load(Ordering::Relaxed), 2);
        assert_eq!(service.remote_reads.load(Ordering::Relaxed), 1);
        drain.send_replace(true);
        task.await.expect("循环任务");
    }

    #[tokio::test(start_paused = true)]
    async fn pause_never_claims_until_resumed() {
        let service = Arc::new(FakeRound::new());
        let paused = Arc::new(AtomicBool::new(true));
        let paused_flag = paused.clone();
        let runtime = runtime_with_pause(
            service.clone(),
            Arc::new(move || paused_flag.load(Ordering::Relaxed)),
        );
        let wakeup = runtime.wakeup.clone();
        let (drain, receiver) = watch::channel(false);
        let task = tokio::spawn(run(runtime, receiver, |_, _| {}));
        advance(Duration::from_secs(10)).await;
        settle().await;
        assert_eq!(service.calls.load(Ordering::Relaxed), 0);
        paused.store(false, Ordering::Relaxed);
        advance(Duration::from_secs(5)).await;
        settle().await;
        assert_eq!(service.calls.load(Ordering::Relaxed), 1);
        drain.send_replace(true);
        wakeup.notify_one();
        task.await.expect("循环任务");
    }

    #[tokio::test(start_paused = true)]
    async fn lost_wakeup_still_retries_from_persistent_work() {
        let service = Arc::new(FakeRound::new());
        let runtime = runtime(service.clone());
        let wakeup = runtime.wakeup.clone();
        let (drain, receiver) = watch::channel(false);
        let task = tokio::spawn(run(runtime, receiver, |_, _| {}));
        settle().await;
        assert_eq!(service.calls.load(Ordering::Relaxed), 1);
        service.available.store(true, Ordering::Relaxed);
        // 故意不 notify；有界轮询必须在下一轮重新领取持久工作。
        advance(Duration::from_secs(5)).await;
        settle().await;
        assert!(service.calls.load(Ordering::Relaxed) >= 2);
        assert!(service.remote_reads.load(Ordering::Relaxed) >= 1);
        drain.send_replace(true);
        wakeup.notify_one();
        task.await.expect("循环任务");
    }
}
