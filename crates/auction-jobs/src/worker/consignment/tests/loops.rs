//! 处罚、恢复处置与权利通知循环的登记：关闭寄售仍收口存量处罚与恢复处置，权利通知只在准入开放且
//! 未暂停时领取。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use auction_application::consignment::{
    AutoRelist, AutoRelistReceipt, AutoRelistService, AutoRelistSetting, AutoRelistStore,
    ClaimedRightNotice, ConsignmentFuture, PaymentReminderService, PaymentReminderStore,
    PendingAutoRelist, PendingResolution, RemindContract, ReminderReceipt, ResolveReceipt,
    ResolveReturn, ReturnDispositionService, ReturnDispositionStore, RightNoticeDisposition,
    RightNoticeFuture, RightNoticeLease, RightNoticeWorkStore, SanctionWorkOutcome,
    SanctionWorkRef, SanctionWorkStore, ScanCursor, ScanPage,
};
use tokio::sync::Notify;
use uuid::Uuid;

use super::fixture::{Fixture, Running, settle};
use crate::worker::consignment::{
    NoticeDelivery, RecoveryRuntime, RightNoticeDeliverer, RightNoticeRuntime,
    SanctionExecutionRuntime,
};

/// 没有待执行处罚；只计列出次数。
#[derive(Default)]
struct Sanctions {
    opens: AtomicUsize,
}

impl SanctionWorkStore for Sanctions {
    fn open_sanction_work(
        &self,
        _after: Option<ScanCursor>,
        _limit: u32,
    ) -> ConsignmentFuture<'_, ScanPage<SanctionWorkRef>> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Ok(ScanPage::last(Vec::new())) })
    }

    fn execute_sanction_work(
        &self,
        _work: SanctionWorkRef,
    ) -> ConsignmentFuture<'_, SanctionWorkOutcome> {
        panic!("没有工作可执行")
    }
}

/// 三类恢复待办都为空；只计退回处置的列出次数。
#[derive(Default)]
struct Recovery {
    scans: AtomicUsize,
}

impl ReturnDispositionStore for Recovery {
    fn resolve_return(&self, _command: ResolveReturn) -> ConsignmentFuture<'_, ResolveReceipt> {
        panic!("没有待处置的消费")
    }

    fn pending_resolutions(
        &self,
        _after: Option<ScanCursor>,
        _limit: u32,
    ) -> ConsignmentFuture<'_, ScanPage<PendingResolution>> {
        self.scans.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Ok(ScanPage::last(Vec::new())) })
    }
}

impl AutoRelistStore for Recovery {
    fn auto_relist(&self, _command: AutoRelist) -> ConsignmentFuture<'_, AutoRelistReceipt> {
        panic!("没有待重列的处置")
    }

    fn pending_auto_relists(
        &self,
        _after: Option<ScanCursor>,
        _limit: u32,
    ) -> ConsignmentFuture<'_, ScanPage<PendingAutoRelist>> {
        Box::pin(async { Ok(ScanPage::last(Vec::new())) })
    }
}

impl PaymentReminderStore for Recovery {
    fn remind(&self, _command: RemindContract) -> ConsignmentFuture<'_, ReminderReceipt> {
        panic!("没有到期的提醒")
    }

    fn due_reminders(
        &self,
        _after: Option<ScanCursor>,
        _limit: u32,
    ) -> ConsignmentFuture<'_, ScanPage<Uuid>> {
        Box::pin(async { Ok(ScanPage::last(Vec::new())) })
    }
}

/// 没有待发通知；只计领取次数。
#[derive(Default)]
struct Notices {
    claims: AtomicUsize,
}

impl RightNoticeWorkStore for Notices {
    fn claim_next(&self, _lease: Duration) -> RightNoticeFuture<'_, Option<ClaimedRightNotice>> {
        self.claims.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Ok(None) })
    }

    fn complete(
        &self,
        _lease: RightNoticeLease,
        _disposition: RightNoticeDisposition,
    ) -> RightNoticeFuture<'_, ()> {
        panic!("没有领取就不应写结果")
    }

    fn release(&self, _lease: RightNoticeLease) -> RightNoticeFuture<'_, ()> {
        panic!("没有领取就不应释放")
    }
}

struct NoDeliverer;

impl RightNoticeDeliverer for NoDeliverer {
    fn deliver<'a>(
        &'a self,
        _notice: &'a ClaimedRightNotice,
    ) -> std::pin::Pin<Box<dyn Future<Output = NoticeDelivery> + Send + 'a>> {
        panic!("没有领取就不应发信")
    }
}

struct Loops {
    sanctions: Arc<Sanctions>,
    recovery: Arc<Recovery>,
    notices: Arc<Notices>,
    paused: Arc<AtomicBool>,
}

impl Loops {
    fn new() -> Self {
        Self {
            sanctions: Arc::default(),
            recovery: Arc::default(),
            notices: Arc::default(),
            paused: Arc::default(),
        }
    }

    async fn run(&self, fixture: &Fixture, enabled: bool) {
        let worker = fixture.prepare(enabled, async { Ok(true) }).await.unwrap().unwrap();
        let paused = self.paused.clone();
        let worker = worker
            .with_sanctions(Arc::new(SanctionExecutionRuntime {
                store: self.sanctions.clone(),
                wakeup: Arc::new(Notify::new()),
            }))
            .with_recovery(Arc::new(RecoveryRuntime {
                resolve: ReturnDispositionService::new(self.recovery.clone()),
                relist: AutoRelistService::new(self.recovery.clone()),
                reminders: PaymentReminderService::new(self.recovery.clone()),
                auto_relist: AutoRelistSetting { enabled: false, config_digest: [7; 32] },
                max_open_per_user: 5,
                wakeup: Arc::new(Notify::new()),
            }))
            .with_right_notices(Arc::new(RightNoticeRuntime {
                store: self.notices.clone(),
                deliverer: Arc::new(NoDeliverer),
                wakeup: Arc::new(Notify::new()),
                paused: Arc::new(move || paused.load(Ordering::Relaxed)),
            }));
        let running = Running::spawn(worker);
        settle().await;
        running.finish().await;
    }
}

#[tokio::test(start_paused = true)]
async fn closed_consignment_still_runs_sanctions_and_recovery_but_never_claims_notices() {
    let loops = Loops::new();
    loops.run(&Fixture::new(), false).await;
    assert!(loops.sanctions.opens.load(Ordering::Relaxed) >= 1, "存量处罚必须收口");
    assert!(loops.recovery.scans.load(Ordering::Relaxed) >= 1, "已接受订单的恢复处置必须收口");
    assert_eq!(loops.notices.claims.load(Ordering::Relaxed), 0, "关闭寄售不再发码");
}

#[tokio::test(start_paused = true)]
async fn open_consignment_runs_both_loops() {
    let loops = Loops::new();
    loops.run(&Fixture::new(), true).await;
    assert!(loops.sanctions.opens.load(Ordering::Relaxed) >= 1);
    assert!(loops.notices.claims.load(Ordering::Relaxed) >= 1);
}

#[tokio::test(start_paused = true)]
async fn paused_external_access_never_claims_notices() {
    let loops = Loops::new();
    loops.paused.store(true, Ordering::Relaxed);
    loops.run(&Fixture::new(), true).await;
    assert_eq!(loops.notices.claims.load(Ordering::Relaxed), 0, "暂停期间不领取、不发信");
}
