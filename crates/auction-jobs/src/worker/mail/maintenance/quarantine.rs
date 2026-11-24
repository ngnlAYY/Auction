//! 隔离重检循环：持久工作项与租约决定 due，内存通知只负责唤醒。
//!
//! 循环没有第二套状态机；每次唤醒只调用一次应用用例，空闲、错误与租约丢失都按持久工作
//! 仓储的下一轮结果继续。日志只记录安全计数与稳定原因码。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{ConsignmentError, ConsignmentFuture};
use auction_application::core::Clock;
use auction_application::mail::{
    DispositionRoundRequest, QuarantineRecheckRound, QuarantineRecheckService,
};
use auction_observability::WorkerResult;
use auction_time::TimeDelta;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

const IDLE_POLL: Duration = Duration::from_secs(5);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAXIMUM: Duration = Duration::from_secs(30);

/// 应用隔离重检用例的单轮入口；测试以同一接口替换外部读取。
pub trait QuarantineRecheckRoundRunner: Send + Sync {
    fn run_once<'a>(
        &'a self,
        request: DispositionRoundRequest,
    ) -> ConsignmentFuture<'a, QuarantineRecheckRound>;
}

impl QuarantineRecheckRoundRunner for QuarantineRecheckService {
    fn run_once<'a>(
        &'a self,
        request: DispositionRoundRequest,
    ) -> ConsignmentFuture<'a, QuarantineRecheckRound> {
        Box::pin(QuarantineRecheckService::run_once(self, request))
    }
}

/// 隔离重检运行参数。启动条件由装配根决定；暂停只阻止开始新一轮读取。
pub struct QuarantineRecheckRuntime {
    /// 应用用例。
    pub service: Arc<dyn QuarantineRecheckRoundRunner>,
    /// 本地唤醒时钟；持久 due 仍由工作仓储给出。
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

/// 运行隔离重检循环，直到 drain 或 owner 离开。
pub async fn run(
    runtime: Arc<QuarantineRecheckRuntime>,
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
            tracing::error!("寄售隔离重检租约时长无效");
            return;
        };
        let now = runtime.clock.now();
        let Some(lease_until) = now.checked_add_signed(lease) else {
            tracing::error!("寄售隔离重检租约截止时间溢出");
            return;
        };
        let Some(deadline) = Instant::now().into_std().checked_add(runtime.lease_duration) else {
            tracing::error!("寄售隔离重检外部截止时间溢出");
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
            Ok(QuarantineRecheckRound::Appended { quarantine_version, observation_id }) => {
                tracing::info!(
                    quarantine_version,
                    observation_id = %observation_id,
                    "寄售隔离观察已追加"
                );
                (WorkerResult::Success, Wait::Immediate)
            }
            Ok(QuarantineRecheckRound::Idle) => (WorkerResult::Success, Wait::Idle),
            Ok(QuarantineRecheckRound::Retried | QuarantineRecheckRound::Reconciled) => {
                // 不变量 5：`Retried` 的到期时间已在释放时持久化；本轮立即继续只为处理其他
                // 到期项，同一项在 retry_at 前不会被存储再次返回。
                (WorkerResult::Success, Wait::Immediate)
            }
            Ok(QuarantineRecheckRound::LeaseLost) | Err(ConsignmentError::LeaseLost) => {
                tracing::debug!(code = "lease_lost", "寄售隔离重检围栏已失效");
                (WorkerResult::Canceled, Wait::Immediate)
            }
            Err(error) => {
                tracing::warn!(code = error_code(error), "寄售隔离重检一轮未确认完成");
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
    use auction_application::mail::{DispositionRoundRequest, QuarantineRecheckRound};
    use auction_observability::WorkerResult;
    use tokio::sync::{Notify, watch};
    use tokio::time::{Duration, advance};
    use uuid::Uuid;

    use super::{QuarantineRecheckRoundRunner, QuarantineRecheckRuntime, run};
    use crate::worker::runtime::test_fixture::PausedClock;

    struct FakeRound {
        calls: AtomicUsize,
        available: AtomicBool,
        remote_reads: AtomicUsize,
        outcomes: Mutex<VecDeque<Result<QuarantineRecheckRound, ConsignmentError>>>,
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
    }

    impl QuarantineRecheckRoundRunner for FakeRound {
        fn run_once<'a>(
            &'a self,
            _request: DispositionRoundRequest,
        ) -> ConsignmentFuture<'a, QuarantineRecheckRound> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if let Some(outcome) = self.outcomes.lock().expect("结果队列").pop_front() {
                if matches!(
                    outcome,
                    Ok(QuarantineRecheckRound::Appended { .. }
                        | QuarantineRecheckRound::Retried
                        | QuarantineRecheckRound::Reconciled)
                ) {
                    self.remote_reads.fetch_add(1, Ordering::Relaxed);
                }
                return Box::pin(async move { outcome });
            }
            let outcome = if self.available.load(Ordering::Relaxed) {
                self.available.store(false, Ordering::Relaxed);
                self.remote_reads.fetch_add(1, Ordering::Relaxed);
                Ok(QuarantineRecheckRound::Appended {
                    quarantine_version: 2,
                    observation_id: Uuid::from_u128(1),
                })
            } else {
                Ok(QuarantineRecheckRound::Idle)
            };
            Box::pin(async move { outcome })
        }
    }

    fn runtime(service: Arc<FakeRound>, paused: Arc<AtomicBool>) -> Arc<QuarantineRecheckRuntime> {
        Arc::new(QuarantineRecheckRuntime {
            service,
            clock: Arc::new(PausedClock::new(auction_time::Timestamp::UNIX_EPOCH)),
            worker_id: "test-worker".to_owned(),
            lease_duration: Duration::from_secs(30),
            wakeup: Arc::new(Notify::default()),
            paused: { Arc::new(move || paused.load(Ordering::Relaxed)) },
        })
    }

    async fn settle() {
        for _ in 0..12 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn retried_immediately_rechecks_storage_without_second_remote_read() {
        let service = Arc::new(FakeRound::new());
        service.outcomes.lock().expect("结果队列").push_back(Ok(QuarantineRecheckRound::Retried));
        service.outcomes.lock().expect("结果队列").push_back(Ok(QuarantineRecheckRound::Idle));
        let runtime = runtime(service.clone(), Arc::new(AtomicBool::default()));
        let (drain, receiver) = watch::channel(false);
        let task = tokio::spawn(run(runtime, receiver, |_, _| {}));
        settle().await;
        assert_eq!(service.calls.load(Ordering::Relaxed), 2);
        assert_eq!(service.remote_reads.load(Ordering::Relaxed), 1);
        drain.send_replace(true);
        task.await.expect("循环任务");
    }

    #[tokio::test(start_paused = true)]
    async fn lost_wakeup_still_retries_from_persistent_work() {
        let service = Arc::new(FakeRound::new());
        let runtime = runtime(service.clone(), Arc::new(AtomicBool::default()));
        let wakeup = runtime.wakeup.clone();
        let (drain, receiver) = watch::channel(false);
        let observed = Arc::new(Mutex::new(Vec::new()));
        let recorded = observed.clone();
        let task = tokio::spawn(run(runtime, receiver, move |result, _duration| {
            recorded.lock().expect("观察锁").push(result);
        }));
        settle().await;
        assert_eq!(service.calls.load(Ordering::Relaxed), 1);

        service.available.store(true, Ordering::Relaxed);
        // 故意不 notify；有界轮询必须在下一轮读取持久工作。
        advance(Duration::from_secs(5)).await;
        settle().await;
        assert!(service.calls.load(Ordering::Relaxed) >= 2);
        assert!(
            observed.lock().expect("观察锁").iter().all(|result| *result == WorkerResult::Success)
        );

        drain.send_replace(true);
        wakeup.notify_one();
        task.await.expect("循环任务");
    }

    #[tokio::test(start_paused = true)]
    async fn notification_wakes_before_the_idle_poll() {
        let service = Arc::new(FakeRound::new());
        let runtime = runtime(service.clone(), Arc::new(AtomicBool::default()));
        let wakeup = runtime.wakeup.clone();
        let (drain, receiver) = watch::channel(false);
        let task = tokio::spawn(run(runtime, receiver, |_, _| {}));
        settle().await;
        assert_eq!(service.calls.load(Ordering::Relaxed), 1);
        service.available.store(true, Ordering::Relaxed);
        wakeup.notify_one();
        settle().await;
        assert!(service.calls.load(Ordering::Relaxed) >= 2);
        drain.send_replace(true);
        wakeup.notify_one();
        task.await.expect("循环任务");
    }

    #[tokio::test(start_paused = true)]
    async fn pause_never_claims_until_resumed() {
        let service = Arc::new(FakeRound::new());
        let paused = Arc::new(AtomicBool::new(true));
        let runtime = runtime(service.clone(), paused.clone());
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
}
