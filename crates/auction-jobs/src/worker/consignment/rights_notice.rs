//! 权利通知投递循环：从专用密文 outbox 领取工作，经发信端口投递，并持久化结果。
//!
//! 不变量 4、5：外部写（发信）只在领取提交之后执行；结果未知（`Unknown`）与租约过期后重新领取的工作
//! 一律记 `Unknown` 转人工对账，绝不自动重发；租约、围栏与完成状态都在数据库里，内存定时器只负责唤醒。
//! 发信端口返回“明确未发出且可重试”时才释放租约，下一轮重新领取。日志只记种类与错误码，
//! 不含收件身份、密文或挑战码。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ClaimedRightNotice, RightNoticeDisposition, RightNoticeLease, RightNoticeStoreError,
    RightNoticeWorkStore,
};
use auction_observability::WorkerResult;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

const IDLE_POLL: Duration = Duration::from_secs(30);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAXIMUM: Duration = Duration::from_secs(30);
/// 租约远长于一次发信的核验预算；过期只意味着持有者已失联。
const LEASE: Duration = Duration::from_secs(300);
/// 单轮最多处理的工作数，之后让出调度。
const MAX_PER_ROUND: usize = 64;

/// 发信端口对一次投递的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeDelivery {
    /// 已发送并核实。
    Sent,
    /// 发送是否成功无法证明；转人工对账，不重发。
    Unknown,
    /// 明确没有发出；`retryable` 为真时释放租约稍后重试，否则记 `Suppressed`。
    NotSent {
        /// 是否是临时不可用。
        retryable: bool,
    },
}

/// 发信端口：解封挑战码并向冻结身份发信（装配根实现，测试用回环替身）。
pub trait RightNoticeDeliverer: Send + Sync {
    /// 投递一封已领取的通知；实现不得向 `recipient` 以外的名字发送。
    fn deliver<'a>(
        &'a self,
        notice: &'a ClaimedRightNotice,
    ) -> Pin<Box<dyn Future<Output = NoticeDelivery> + Send + 'a>>;
}

/// 权利通知循环的运行参数。
pub struct RightNoticeRuntime {
    /// 专用 outbox 的工作端口。
    pub store: Arc<dyn RightNoticeWorkStore>,
    /// 发信端口。
    pub deliverer: Arc<dyn RightNoticeDeliverer>,
    /// 可选唤醒；丢失时由有界轮询恢复。
    pub wakeup: Arc<Notify>,
    /// 外部访问暂停时为真：不领取、不发信，已领取的工作照常收口。
    pub paused: Arc<dyn Fn() -> bool + Send + Sync>,
}

/// 单轮是否产生了新的持久工作或错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Round {
    worked: bool,
    failed: bool,
    /// 明确未发出的可重试结果：释放已成功，需要退避，但没有任何持久化失败事实。
    retry: bool,
}

/// 单条工作的处置：继续处理、退避后重试或本轮错误。
enum ProcessOutcome {
    Done,
    Retry,
    Failed,
}

impl RightNoticeRuntime {
    /// 处理到没有可领工作、遇到可重试失败或达到单轮上限。
    async fn round(&self) -> Round {
        let mut round = Round { worked: false, failed: false, retry: false };
        for _ in 0..MAX_PER_ROUND {
            // 暂停在每次领取前复核：领取后才发信，暂停期间不产生新的外部写（不变量 4）。
            if (self.paused)() {
                return round;
            }
            let claimed = match self.store.claim_next(LEASE).await {
                Ok(Some(claimed)) => claimed,
                Ok(None) => return round,
                Err(error) => {
                    tracing::warn!(code = store_code(error), "权利通知领取失败");
                    round.failed = true;
                    return round;
                }
            };
            round.worked = true;
            match self.process(claimed).await {
                ProcessOutcome::Done => {}
                ProcessOutcome::Retry => {
                    round.retry = true;
                    return round;
                }
                ProcessOutcome::Failed => {
                    round.failed = true;
                    return round;
                }
            }
        }
        round
    }

    /// 处理一条已领取的工作；返回需要退避的重试或错误。
    async fn process(&self, claimed: ClaimedRightNotice) -> ProcessOutcome {
        let lease = claimed.lease;
        if claimed.reclaimed {
            // 上一持有者可能已经发出：只记未知，不重发（不变量 4）。
            return self.finish(lease, RightNoticeDisposition::Unknown).await;
        }
        match self.deliverer.deliver(&claimed).await {
            NoticeDelivery::Sent => self.finish(lease, RightNoticeDisposition::Sent).await,
            NoticeDelivery::Unknown => self.finish(lease, RightNoticeDisposition::Unknown).await,
            NoticeDelivery::NotSent { retryable: false } => {
                self.finish(lease, RightNoticeDisposition::Suppressed).await
            }
            NoticeDelivery::NotSent { retryable: true } => match self.store.release(lease).await {
                // 明确未发出且租约已释放：正常退避重试，不是本轮错误。
                Ok(()) => ProcessOutcome::Retry,
                Err(error) => {
                    tracing::warn!(code = store_code(error), "权利通知租约释放失败");
                    ProcessOutcome::Failed
                }
            },
        }
    }

    async fn finish(
        &self,
        lease: RightNoticeLease,
        disposition: RightNoticeDisposition,
    ) -> ProcessOutcome {
        match self.store.complete(lease, disposition).await {
            Ok(()) => ProcessOutcome::Done,
            Err(RightNoticeStoreError::LeaseLost) => {
                // 他人已重新领取或已有结果：本持有者放弃，结果以数据库为准。
                tracing::warn!(code = "lease_lost", "权利通知租约已失效，放弃写入");
                ProcessOutcome::Done
            }
            Err(error) => {
                tracing::warn!(code = store_code(error), "权利通知结果写入失败");
                ProcessOutcome::Failed
            }
        }
    }
}

/// 运行权利通知循环，直到 drain 或 owner 离开。
pub async fn run(
    runtime: Arc<RightNoticeRuntime>,
    mut drain: watch::Receiver<bool>,
    observe: impl Fn(WorkerResult, Duration) + Send + Sync,
) {
    let mut backoff = BACKOFF_INITIAL;
    while !stopped(&drain) {
        let started_at = Instant::now();
        let round = runtime.round().await;
        observe(
            if round.failed { WorkerResult::Error } else { WorkerResult::Success },
            started_at.elapsed(),
        );
        // 可重试的明确未发出与持久化错误共用指数退避，但前者不污染错误指标。
        if round.failed || round.retry {
            if wait_for_backoff(&mut drain, backoff).await {
                return;
            }
            backoff = backoff.saturating_mul(2).min(BACKOFF_MAXIMUM);
        } else if round.worked {
            backoff = BACKOFF_INITIAL;
            tokio::task::yield_now().await;
        } else {
            backoff = BACKOFF_INITIAL;
            if wait_for_wake(&mut drain, runtime.wakeup.as_ref(), IDLE_POLL).await {
                return;
            }
        }
    }
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

fn store_code(error: RightNoticeStoreError) -> &'static str {
    match error {
        RightNoticeStoreError::LeaseLost => "lease_lost",
        RightNoticeStoreError::Unavailable => "unavailable",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use auction_application::consignment::{
        ChallengeAad, ClaimedRightNotice, RightNoticeDisposition, RightNoticeFuture,
        RightNoticeKind, RightNoticeLease, RightNoticeSecret, RightNoticeWorkStore,
        SealedChallengeSecret,
    };
    use auction_domain::SettlementWorld;
    use tokio::sync::Notify;
    use uuid::Uuid;

    use super::{
        Arc, Duration, Future, NoticeDelivery, Pin, RightNoticeDeliverer, RightNoticeRuntime,
    };

    /// 内存工作端口：一条工作，记录结果与释放次数。
    struct FakeStore {
        pending: Mutex<Option<ClaimedRightNotice>>,
        results: Mutex<Vec<RightNoticeDisposition>>,
    }

    fn notice(reclaimed: bool) -> ClaimedRightNotice {
        let sealed =
            SealedChallengeSecret::new(vec![1], [0; 12], "fixture-key".into(), "0".repeat(64))
                .expect("夹具密文形状有效");
        ClaimedRightNotice {
            lease: RightNoticeLease {
                work_id: Uuid::nil(),
                claim_token: Uuid::nil(),
                claim_fence: 1,
            },
            kind: RightNoticeKind::Challenge,
            world: SettlementWorld::Main,
            recipient: "FakeRecipient".to_owned(),
            secret: RightNoticeSecret {
                aad: ChallengeAad {
                    challenge_id: Uuid::nil(),
                    right_id: None,
                    identity_digest: [0; 32],
                    right_version: 1,
                },
                sealed,
            },
            reclaimed,
        }
    }

    impl RightNoticeWorkStore for FakeStore {
        fn claim_next(&self, _: Duration) -> RightNoticeFuture<'_, Option<ClaimedRightNotice>> {
            Box::pin(async move { Ok(self.pending.lock().expect("锁").take()) })
        }

        fn complete(
            &self,
            _: RightNoticeLease,
            disposition: RightNoticeDisposition,
        ) -> RightNoticeFuture<'_, ()> {
            Box::pin(async move {
                self.results.lock().expect("锁").push(disposition);
                Ok(())
            })
        }

        fn release(&self, _: RightNoticeLease) -> RightNoticeFuture<'_, ()> {
            Box::pin(async move { Ok(()) })
        }
    }

    struct CountingDeliverer {
        calls: AtomicUsize,
        outcome: NoticeDelivery,
    }

    impl RightNoticeDeliverer for CountingDeliverer {
        fn deliver<'a>(
            &'a self,
            _: &'a ClaimedRightNotice,
        ) -> Pin<Box<dyn Future<Output = NoticeDelivery> + Send + 'a>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let outcome = self.outcome;
            Box::pin(async move { outcome })
        }
    }

    async fn run_once(
        notice: ClaimedRightNotice,
        outcome: NoticeDelivery,
    ) -> (usize, Vec<RightNoticeDisposition>) {
        let store = Arc::new(FakeStore {
            pending: Mutex::new(Some(notice)),
            results: Mutex::new(Vec::new()),
        });
        let deliverer = Arc::new(CountingDeliverer { calls: AtomicUsize::new(0), outcome });
        let runtime = RightNoticeRuntime {
            store: store.clone(),
            deliverer: deliverer.clone(),
            wakeup: Arc::new(Notify::new()),
            paused: Arc::new(|| false),
        };
        runtime.round().await;
        let results = store.results.lock().expect("锁").clone();
        (deliverer.calls.load(Ordering::SeqCst), results)
    }

    #[tokio::test]
    async fn sent_notice_is_delivered_once_and_recorded_sent() {
        let (calls, results) = run_once(notice(false), NoticeDelivery::Sent).await;
        assert_eq!(calls, 1);
        assert_eq!(results, vec![RightNoticeDisposition::Sent]);
    }

    #[tokio::test]
    async fn unknown_outcome_is_recorded_unknown_without_a_second_send() {
        let (calls, results) = run_once(notice(false), NoticeDelivery::Unknown).await;
        assert_eq!(calls, 1);
        assert_eq!(results, vec![RightNoticeDisposition::Unknown]);
    }

    #[tokio::test]
    async fn reclaimed_notice_is_recorded_unknown_and_never_sent() {
        let (calls, results) = run_once(notice(true), NoticeDelivery::Sent).await;
        assert_eq!(calls, 0);
        assert_eq!(results, vec![RightNoticeDisposition::Unknown]);
    }
}
