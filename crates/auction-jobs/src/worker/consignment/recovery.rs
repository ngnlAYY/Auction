//! 恢复处置循环：退回处置（resolve_return）、自动重列（auto_relist）与付款提醒（remind）三类持久工作。
//!
//! 不变量 4、5：待办全部来自数据库（尚无处置的消费、冻结开关为开且挂单已下架的处置、已到期且已证寄出的合同），
//! 内存定时器只负责唤醒；自动重列开关与配置摘要由装配根启动时加载一次、重启生效，决策时落库冻结，
//! 之后改配置不影响已作的决定。确定拒绝只记日志并跳过该项，事实补齐后下一轮重新评估；
//! 提醒的外部发送不在本循环（走通用投递），派前复验在通用投递的 sending 围栏里（与
//! 存储端 `pre_dispatch_in` 是同一份判定）。
//! “没有写任何行”的结果（自动重列未就绪、提醒尚不需要）不算有进展：待办因暂态仍留在数据库里，
//! 若算进展，循环会以数据库查询速率空转；这类结果按空闲等待，由唤醒或有界轮询再评估。
//! 日志只记种类与错误码，不含身份、装备或凭据。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    AutoRelistOutcome, AutoRelistService, AutoRelistSetting, ConsignmentError,
    PaymentReminderService, ReminderOutcome, ReturnDispositionService,
};
use auction_observability::WorkerResult;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

const IDLE_POLL: Duration = Duration::from_secs(30);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAXIMUM: Duration = Duration::from_secs(30);
/// 每类工作单轮最多读取的待办数，之后让出调度。
const PER_ROUND: u32 = 64;

/// 恢复处置循环的运行参数。
pub struct RecoveryRuntime {
    /// 退回处置用例。
    pub resolve: ReturnDispositionService,
    /// 自动重列用例。
    pub relist: AutoRelistService,
    /// 付款提醒用例。
    pub reminders: PaymentReminderService,
    /// 启动时加载的自动重列配置（J12）；缺配置时装配根不得构造本运行体。
    pub auto_relist: AutoRelistSetting,
    /// 额度上限（寄售配置的冻结值）。
    pub max_open_per_user: i64,
    /// 可选唤醒；丢失时由有界轮询恢复。
    pub wakeup: Arc<Notify>,
}

/// 单轮是否产生了新的持久工作或可重试的故障。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Round {
    worked: bool,
    failed: bool,
}

impl Round {
    /// 记一项处理结果：`Ok(true)` 写入了持久事实，算有进展；`Ok(false)` 没有写任何行，不算进展；
    /// 确定拒绝跳过；其余（含结果未知）算故障并退避。
    fn record(&mut self, kind: &'static str, result: Result<bool, ConsignmentError>) {
        match result {
            Ok(progressed) => self.worked |= progressed,
            Err(ConsignmentError::Deposit(rejection)) => {
                tracing::warn!(kind, code = rejection.code(), "恢复处置被确定拒绝，等待事实补齐");
            }
            Err(error) => {
                tracing::warn!(kind, %error, "恢复处置失败，稍后重试");
                self.failed = true;
            }
        }
    }
}

impl RecoveryRuntime {
    /// 依次处理三类待办。
    async fn round(&self) -> Round {
        let mut round = Round::default();
        match self.resolve.pending(PER_ROUND).await {
            Ok(pending) => {
                for item in pending {
                    let result = self.resolve.resolve(item, self.auto_relist).await.map(|_| true);
                    round.record("resolve_return", result);
                }
            }
            Err(error) => round.record("resolve_return", Err(error)),
        }
        match self.relist.pending(PER_ROUND).await {
            Ok(pending) => {
                for item in pending {
                    let result = self
                        .relist
                        .relist(item, self.max_open_per_user)
                        .await
                        .map(|receipt| !matches!(receipt.outcome, AutoRelistOutcome::NotReady));
                    round.record("auto_relist", result);
                }
            }
            Err(error) => round.record("auto_relist", Err(error)),
        }
        match self.reminders.due(PER_ROUND).await {
            Ok(contracts) => {
                for contract_id in contracts {
                    let result = self
                        .reminders
                        .remind(contract_id)
                        .await
                        .map(|receipt| !matches!(receipt.outcome, ReminderOutcome::NotDue));
                    round.record("remind", result);
                }
            }
            Err(error) => round.record("remind", Err(error)),
        }
        round
    }
}

/// 运行恢复处置循环，直到 drain 或 owner 离开。
pub async fn run(
    runtime: Arc<RecoveryRuntime>,
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
        if round.failed {
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use auction_application::consignment::{
        AutoRelist, AutoRelistOutcome, AutoRelistReceipt, AutoRelistStore, ConsignmentFuture,
        PaymentReminderStore, PendingAutoRelist, PendingResolution, RemindContract, ResolveReceipt,
        ResolveReturn, ReturnDispositionStore,
    };
    use uuid::Uuid;

    use super::{
        Arc, AutoRelistService, AutoRelistSetting, ConsignmentError, Duration, Notify,
        PaymentReminderService, RecoveryRuntime, ReturnDispositionService, run, watch,
    };

    /// 没有待处置的消费。
    struct NoResolutions;

    impl ReturnDispositionStore for NoResolutions {
        fn resolve_return(&self, _command: ResolveReturn) -> ConsignmentFuture<'_, ResolveReceipt> {
            Box::pin(async { Err(ConsignmentError::Unavailable) })
        }

        fn pending_resolutions(
            &self,
            _limit: u32,
        ) -> ConsignmentFuture<'_, Vec<PendingResolution>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    /// 没有到期的提醒。
    struct NoReminders;

    impl PaymentReminderStore for NoReminders {
        fn remind(
            &self,
            _command: RemindContract,
        ) -> ConsignmentFuture<'_, auction_application::consignment::ReminderReceipt> {
            Box::pin(async { Err(ConsignmentError::Unavailable) })
        }

        fn due_reminders(&self, _limit: u32) -> ConsignmentFuture<'_, Vec<Uuid>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    /// 永远有一个待办的自动重列，每次评估都回答“尚未就绪”（原件未回库的暂态）。
    struct AlwaysNotReady {
        evaluations: AtomicUsize,
    }

    impl AutoRelistStore for AlwaysNotReady {
        fn auto_relist(&self, command: AutoRelist) -> ConsignmentFuture<'_, AutoRelistReceipt> {
            self.evaluations.fetch_add(1, Ordering::Relaxed);
            Box::pin(async move {
                Ok(AutoRelistReceipt {
                    command_id: command.command_id,
                    outcome: AutoRelistOutcome::NotReady,
                    replayed: false,
                })
            })
        }

        fn pending_auto_relists(
            &self,
            _limit: u32,
        ) -> ConsignmentFuture<'_, Vec<PendingAutoRelist>> {
            Box::pin(async {
                Ok(vec![PendingAutoRelist {
                    disposition_id: Uuid::from_u128(1),
                    listing_id: Uuid::from_u128(2),
                }])
            })
        }
    }

    /// 不变量 5：待办仍在库里但评估没有写任何行时，循环按空闲轮询等待，而不是以查询速率空转。
    #[tokio::test(start_paused = true)]
    async fn not_ready_relist_waits_for_the_idle_poll_instead_of_spinning() {
        let store = Arc::new(AlwaysNotReady { evaluations: AtomicUsize::new(0) });
        let runtime = Arc::new(RecoveryRuntime {
            resolve: ReturnDispositionService::new(Arc::new(NoResolutions)),
            relist: AutoRelistService::new(store.clone()),
            reminders: PaymentReminderService::new(Arc::new(NoReminders)),
            auto_relist: AutoRelistSetting { enabled: true, config_digest: [7; 32] },
            max_open_per_user: 5,
            wakeup: Arc::new(Notify::new()),
        });
        let (drain_tx, drain_rx) = watch::channel(false);
        let task = tokio::spawn(run(runtime, drain_rx, |_, _| {}));
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(store.evaluations.load(Ordering::Relaxed), 1, "首轮评估一次");
        tokio::time::advance(Duration::from_secs(29)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(store.evaluations.load(Ordering::Relaxed), 1, "空闲轮询期内不得再评估");
        tokio::time::advance(Duration::from_secs(2)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(store.evaluations.load(Ordering::Relaxed), 2, "轮询到期才再评估一次");
        drain_tx.send(true).unwrap();
        task.await.unwrap();
    }
}
