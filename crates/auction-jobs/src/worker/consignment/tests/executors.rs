//! 派发、补寄与退件 Take 三个 executor 与订单复核的登记：关闭寄售也续办旧权利，四个循环都领取（D9 §355）；
//! 开放寄售同样四个循环都领取。来源只计领取次数，存储与传输被触碰即失败。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use auction_application::consignment::{
    AcquireReceipt, AcquireReturn, AttemptReceipt, AuthorizeDispatch, ConsignmentFuture,
    ConsumeReceipt, ConsumeReturn, DispatchService, DispatchStore, EvidenceReceipt,
    MarkWriteStarted, OrderRecheckClaim, OrderRecheckLease, OrderRecheckOutcome,
    OrderRecheckWorkStore, PostPermit, RecheckDispatchAttempt, RecordEvidence, ReturnIntakeStore,
    ReturnService, RoundReceipt, StartAttempt, StartReturnTake, WriteStartOutcome,
};
use auction_application::core::Clock;
use auction_domain::consignment::dispatch::AttemptKey;
use auction_time::Timestamp;
use tokio::sync::Notify;
use uuid::Uuid;

use super::fixture::{Fixture, Running, settle};
use crate::worker::consignment::{
    ClaimedTake, ConsignmentDispatchRuntime, DispatchExecution, DispatchLease, DispatchSentReader,
    DispatchTransport, DispatchWork, DispatchWorkSource, OrderRecheckRuntime, PendingRedispatch,
    PreparedDispatch, PreparedTake, RedispatchRuntime, RedispatchWorkSource, ReturnTakeRuntime,
    ReturnTakeTransport, ReturnTakeWorkSource, SentObservation, TakeExecution,
};

/// 三个来源都没有工作；只计领取 / 列出次数。
#[derive(Default)]
struct Sources {
    dispatch: AtomicUsize,
    redispatch: AtomicUsize,
    takes: AtomicUsize,
    rechecks: AtomicUsize,
}

impl DispatchWorkSource for Sources {
    fn claim_next<'a>(
        &'a self,
        _worker_id: &'a str,
        _lease: Duration,
    ) -> ConsignmentFuture<'a, Option<DispatchWork>> {
        self.dispatch.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Ok(None) })
    }

    fn lease_round<'a>(&'a self, _work: &'a DispatchWork) -> ConsignmentFuture<'a, DispatchLease> {
        panic!("没有领取就不应衔接")
    }
}

impl RedispatchWorkSource for Sources {
    fn pending_redispatches(&self, _limit: u32) -> ConsignmentFuture<'_, Vec<PendingRedispatch>> {
        self.redispatch.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Ok(Vec::new()) })
    }
}

impl ReturnTakeWorkSource for Sources {
    fn claim_next<'a>(
        &'a self,
        _worker_id: &'a str,
        _lease: Duration,
    ) -> ConsignmentFuture<'a, Option<ClaimedTake>> {
        self.takes.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Ok(None) })
    }
}

impl OrderRecheckWorkStore for Sources {
    fn claim_due_order_recheck(
        &self,
        _worker_id: String,
        _now: Timestamp,
        _lease_until: Timestamp,
    ) -> ConsignmentFuture<'_, Option<OrderRecheckClaim>> {
        self.rechecks.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Ok(None) })
    }

    fn complete_order_recheck(
        &self,
        _lease: OrderRecheckLease,
    ) -> ConsignmentFuture<'_, OrderRecheckOutcome> {
        panic!("没有领取就不应完成")
    }
}

struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        Timestamp::parse_from_rfc3339("2026-12-01T00:00:00Z").unwrap()
    }
}

/// 存储与传输：没有工作时任何调用都是接线错误。
struct Untouched;

impl DispatchStore for Untouched {
    fn authorize_dispatch(&self, _: AuthorizeDispatch) -> ConsignmentFuture<'_, RoundReceipt> {
        panic!("没有待办就不应授权")
    }

    fn start_attempt(&self, _: StartAttempt) -> ConsignmentFuture<'_, AttemptReceipt> {
        panic!("没有领取就不应登记尝试")
    }

    fn mark_write_started(&self, _: MarkWriteStarted) -> ConsignmentFuture<'_, WriteStartOutcome> {
        panic!("没有领取就不应起写")
    }

    fn record_evidence(&self, _: RecordEvidence) -> ConsignmentFuture<'_, EvidenceReceipt> {
        panic!("没有外写就不应记证据")
    }
}

impl ReturnIntakeStore for Untouched {
    fn acquire_return(&self, _: AcquireReturn) -> ConsignmentFuture<'_, AcquireReceipt> {
        panic!("Take executor 不接管")
    }

    fn start_return_take(&self, _: StartReturnTake) -> ConsignmentFuture<'_, WriteStartOutcome> {
        panic!("没有领取就不应起写")
    }

    fn consume_return(&self, _: ConsumeReturn) -> ConsignmentFuture<'_, ConsumeReceipt> {
        panic!("没有 Take 就不应消费")
    }
}

impl DispatchTransport for Untouched {
    fn prepare<'a>(
        &'a self,
        _work: &'a DispatchWork,
        _attempt: AttemptKey,
        _claim_token: Uuid,
    ) -> ConsignmentFuture<'a, Option<PreparedDispatch>> {
        panic!("没有领取就不应准备外写")
    }

    fn execute(
        &self,
        _prepared: PreparedDispatch,
        _permit: PostPermit,
    ) -> ConsignmentFuture<'_, DispatchExecution> {
        panic!("没有许可就不应外写")
    }
}

impl DispatchSentReader for Untouched {
    fn find_sent<'a>(
        &'a self,
        _attempt: &'a RecheckDispatchAttempt,
    ) -> ConsignmentFuture<'a, Option<SentObservation>> {
        panic!("没有领取就不应读 Sent")
    }
}

impl ReturnTakeTransport for Untouched {
    fn prepare<'a>(
        &'a self,
        _take: &'a ClaimedTake,
    ) -> ConsignmentFuture<'a, Option<PreparedTake>> {
        panic!("没有领取就不应准备 Take")
    }

    fn execute(
        &self,
        _prepared: PreparedTake,
        _permit: PostPermit,
    ) -> ConsignmentFuture<'_, TakeExecution> {
        panic!("没有许可就不应 Take")
    }
}

async fn run(fixture: &Fixture, enabled: bool) -> Arc<Sources> {
    let sources = Arc::new(Sources::default());
    let worker = fixture.prepare(enabled, async { Ok(true) }).await.unwrap().unwrap();
    let service = Arc::new(DispatchService::new(Arc::new(Untouched)));
    let worker = worker
        .with_dispatch(Arc::new(ConsignmentDispatchRuntime {
            work: sources.clone(),
            service: service.clone(),
            transport: Arc::new(Untouched),
            worker_id: "fake-worker".to_owned(),
            lease_duration: Duration::from_secs(60),
            wakeup: Arc::new(Notify::new()),
            paused: Arc::new(|| false),
        }))
        .with_order_rechecks(Arc::new(OrderRecheckRuntime {
            store: sources.clone(),
            reader: Arc::new(Untouched),
            service: service.clone(),
            clock: Arc::new(FixedClock),
            worker_id: "fake-worker".to_owned(),
            lease_duration: Duration::from_secs(60),
            wakeup: Arc::new(Notify::new()),
            paused: Arc::new(|| false),
        }))
        .with_redispatch(Arc::new(RedispatchRuntime {
            source: sources.clone(),
            service,
            wakeup: Arc::new(Notify::new()),
        }))
        .with_return_takes(Arc::new(ReturnTakeRuntime {
            work: sources.clone(),
            service: Arc::new(ReturnService::new(Arc::new(Untouched))),
            transport: Arc::new(Untouched),
            worker_id: "fake-worker".to_owned(),
            lease_duration: Duration::from_secs(60),
            wakeup: Arc::new(Notify::new()),
            paused: Arc::new(|| false),
        }));
    let running = Running::spawn(worker);
    settle().await;
    running.finish().await;
    sources
}

#[tokio::test(start_paused = true)]
async fn closed_consignment_still_runs_dispatch_takes_redispatch_and_rechecks() {
    let sources = run(&Fixture::new(), false).await;
    assert!(sources.dispatch.load(Ordering::Relaxed) >= 1, "已成交订单的派发是续办旧权利");
    assert!(sources.takes.load(Ordering::Relaxed) >= 1, "已接管的退件必须取回");
    assert!(sources.redispatch.load(Ordering::Relaxed) >= 1, "已排队的补寄必须收口");
    assert!(sources.rechecks.load(Ordering::Relaxed) >= 1, "已排队的订单复核必须收口");
}

#[tokio::test(start_paused = true)]
async fn open_consignment_runs_all_executors_and_rechecks() {
    let sources = run(&Fixture::new(), true).await;
    assert!(sources.dispatch.load(Ordering::Relaxed) >= 1);
    assert!(sources.takes.load(Ordering::Relaxed) >= 1);
    assert!(sources.redispatch.load(Ordering::Relaxed) >= 1);
    assert!(sources.rechecks.load(Ordering::Relaxed) >= 1);
}
