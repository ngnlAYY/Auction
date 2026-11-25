//! 寄售处罚执行循环：持续接管第 6 期固定的待执行账号工作，经存储端口在买家互斥下落账号效果。
//!
//! 不变量 1、5、6：待执行、执行中与收口状态都在数据库里，内存定时器只负责唤醒；单项失败只记结构化
//! 警告并继续其他工作，重放、重启与并发领取由端口的幂等保证（同一处罚至多一次账号效果）。
//! 持久事实自相矛盾（[`SanctionWorkOutcome::Inconsistent`]）失败关闭：记错误并让本轮按失败退避，
//! 不补写、不重试执行，转人工对账。日志只含固定文案，不含买家身份。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{SanctionWorkOutcome, SanctionWorkStore};
use auction_observability::WorkerResult;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

use super::{error_code, next_scan_cursor, stopped};

const IDLE_POLL: Duration = Duration::from_secs(60);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAXIMUM: Duration = Duration::from_secs(30);
/// 单轮最多处理的工作数，之后让出调度。
/// 每页读取的待办数；一次扫描按页走完全部待办。
const PAGE_SIZE: u32 = 64;

/// 处罚执行循环的运行参数。
pub struct SanctionExecutionRuntime {
    /// 处罚账号工作的持久端口。
    pub store: Arc<dyn SanctionWorkStore>,
    /// 可选唤醒；丢失时由有界轮询恢复。
    pub wakeup: Arc<Notify>,
}

/// 单轮是否产生了新的持久工作或错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Round {
    worked: bool,
    failed: bool,
}

impl SanctionExecutionRuntime {
    /// 处理当前全部待收口工作；单项失败不影响其他项。
    /// 一次扫描：按 `(created_at, id)` 键集分页走完全部未收口工作。矛盾或执行失败的工作不会收口、下一轮仍会
    /// 被选中，分页让它们不会占满整页、饿死排在后面的可执行工作；游标只活在本次扫描里，不跨轮、不作事实来源。
    async fn round(&self) -> Round {
        let mut round = Round { worked: false, failed: false };
        let mut after = None;
        loop {
            let page = match self.store.open_sanction_work(after, PAGE_SIZE).await {
                Ok(page) => page,
                Err(error) => {
                    tracing::warn!(code = error_code(error), "读取待执行寄售处罚工作失败");
                    round.failed = true;
                    return round;
                }
            };
            for work in page.items {
                match self.store.execute_sanction_work(work).await {
                    Ok(SanctionWorkOutcome::Replayed) => {}
                    Ok(SanctionWorkOutcome::Inconsistent) => {
                        tracing::error!(
                            "寄售处罚账号工作的持久事实自相矛盾，已失败关闭，需人工对账"
                        );
                        round.failed = true;
                    }
                    Ok(_) => round.worked = true,
                    Err(error) => {
                        tracing::warn!(
                            code = error_code(error),
                            "寄售处罚账号工作执行失败，保留待重入"
                        );
                        round.failed = true;
                    }
                }
            }
            match next_scan_cursor(after, page.next) {
                Some(next) => after = Some(next),
                None => return round,
            }
        }
    }
}

/// 运行处罚执行循环，直到 drain 或 owner 离开。
pub async fn run(
    runtime: Arc<SanctionExecutionRuntime>,
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
    use std::sync::Mutex;

    use auction_application::consignment::{
        ConsignmentFuture, SanctionWorkOutcome, SanctionWorkRef, SanctionWorkStore, ScanCursor,
        ScanPage,
    };
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::{Arc, Notify, SanctionExecutionRuntime};

    /// 两页未收口工作：第一页两项执行后事实自相矛盾（不会收口），第二页一项可执行。
    #[derive(Default)]
    struct TwoPages {
        executed: Mutex<Vec<Uuid>>,
    }

    fn work(n: u128) -> SanctionWorkRef {
        SanctionWorkRef {
            work_id: Uuid::from_u128(n),
            sanction_id: Uuid::from_u128(n + 100),
            buyer_user_id: Uuid::from_u128(n + 200),
        }
    }

    impl SanctionWorkStore for TwoPages {
        fn open_sanction_work(
            &self,
            after: Option<ScanCursor>,
            _limit: u32,
        ) -> ConsignmentFuture<'_, ScanPage<SanctionWorkRef>> {
            let page = match after {
                None => ScanPage {
                    items: vec![work(1), work(2)],
                    next: Some(ScanCursor {
                        sort_at: Timestamp::from_timestamp(0, 0).unwrap(),
                        tie_id: Uuid::from_u128(2),
                    }),
                },
                Some(_) => ScanPage::last(vec![work(3)]),
            };
            Box::pin(async { Ok(page) })
        }

        fn execute_sanction_work(
            &self,
            work: SanctionWorkRef,
        ) -> ConsignmentFuture<'_, SanctionWorkOutcome> {
            self.executed.lock().unwrap().push(work.work_id);
            let outcome = if work.work_id == Uuid::from_u128(3) {
                SanctionWorkOutcome::Applied { account_command_id: Uuid::from_u128(7) }
            } else {
                SanctionWorkOutcome::Inconsistent
            };
            Box::pin(async move { Ok(outcome) })
        }
    }

    /// 不变量 5：前面的工作因事实矛盾不会收口、仍留在库里时，一次扫描按键集分页走完全部待办，
    /// 排在后面的可执行工作在同一轮落账号效果，不会被前面的工作饿死；矛盾项仍按故障退避。
    #[tokio::test]
    async fn one_round_executes_work_behind_inconsistent_items() {
        let store = Arc::new(TwoPages::default());
        let runtime =
            SanctionExecutionRuntime { store: store.clone(), wakeup: Arc::new(Notify::new()) };
        let round = runtime.round().await;
        assert_eq!(
            *store.executed.lock().unwrap(),
            [Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3)],
            "第二页的工作同一轮被执行"
        );
        assert!(round.worked, "第二页的工作写下了账号效果");
        assert!(round.failed, "矛盾项仍失败关闭、转人工");
    }
}
