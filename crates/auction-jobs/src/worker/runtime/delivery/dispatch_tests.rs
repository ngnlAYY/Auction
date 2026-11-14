//! 回归：投递工作项从领取、只读准备、sending 围栏到发送与终态收口的状态机。
//!
//! 守护外部写入的安全边界：发送期间失去租约或请求超时一律按远端结果未知落库、绝不重发；
//! 尚未 poll 外部写入的失败（准备未完成、围栏 Deferred 或提交未知、租约已不可用）只释放
//! claim，不伪造发送失败；排空时已领取但未开始的工作项按生命周期协议释放或保留原租约，
//! 已越过围栏的发送继续完成并落库，终态提交确认后才释放排空活动。
//!
//! 内存替身只实现应用端口：存储按发生顺序记录续租、围栏、释放与终态，排空替身把活动的登记
//! 与释放记入同一条时间线，传输记录准备次数与发送 future 的 poll。业务时钟跟随 Tokio 暂停
//! 时间，调用时刻以相对用例起点的秒数断言。

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::auction::ApplicationFuture;
use auction_application::core::diagnostics::SafeErrorKind;
use auction_application::core::{ApplicationError, Clock, RepositoryError, RequestHash};
use auction_application::delivery::{
    AuthorizedDeliveryPreparation, AuthorizedDeliverySend, DeliveryClaimExclusions,
    DeliveryFenceOutcome, DeliveryPreparedExpectation, DeliverySendResult, DeliveryTransport,
    DeliveryTransportError, DeliveryWorkClaim, DeliveryWorkStore,
};
use auction_application::notification_metrics::noop_notification_recorder;
use auction_application::operations::{
    CriticalWriteKind, ExternalWriteActivity, ExternalWriteLifecycle,
};
use auction_application::settlement::{
    InternalEvidenceFuture, InternalEvidenceLease, SettlementInternalEvidenceSnapshot,
};
use auction_observability::{WorkerLoop, WorkerResult};
use chrono::{DateTime, Utc};
use tokio::time::Instant;
use uuid::Uuid;

use super::cycle::run_delivery_cycle;
use super::dispatch::process_delivery_claim;
use crate::WorkerControlHandle;
use crate::worker::PersistentWorker;
use crate::worker::diagnostics;
use crate::worker::runtime::DeliveryWorkerRuntimeConfig;
use crate::worker::runtime::test_fixture::{
    DrainEvent, PausedClock, RecordingMetrics, TestDrain, base_time, unavailable, worker,
};

/// 时间线上的一次事件：持久化边界上的调用，或排空活动的登记与释放。续租、围栏与释放在
/// 调用时记录，永不回应的调用同样可见；终态在提交确认时记录，排空活动的释放必须排在其后。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Renew,
    Fence,
    Release,
    Succeed,
    /// 终态失败的安全投影：失败码、是否转人工对账与失败阶段。
    Fail {
        failure_code: String,
        outcome_unknown: bool,
        stage: Option<String>,
    },
    Drain(DrainEvent),
}

/// 排空活动登记交接；必须先于围栏调用，关闭入口之后的围栏往返才仍受排空等待保护。
const ENTERED: Call = Call::Drain(DrainEvent::Entered);
/// 结果端口确认提交后释放排空活动。
const COMPLETED: Call = Call::Drain(DrainEvent::Released { persisted: true });

/// 发送已被 poll 后在 `stage` 失去确定性：远端结果未知，只能转人工对账。
fn outcome_unknown(stage: &str) -> Call {
    Call::Fail {
        failure_code: "gateway_outcome_unknown".to_owned(),
        outcome_unknown: true,
        stage: Some(stage.to_owned()),
    }
}

/// 续租脚本：前若干次成功，之后被拒绝或一直挂起。
#[derive(Clone, Copy)]
enum Renewal {
    Healthy,
    RejectedAfter(usize),
    HangsAfter(usize),
}

/// sending 围栏的应答。
#[derive(Clone, Copy)]
enum Fence {
    Sending,
    Deferred,
    /// 提交结果未知，例如 COMMIT 回执前连接断开。
    Unknown,
    Hangs,
}

/// 按存储语义领取并记录调用的 outbox 替身；未覆盖的端口失败关闭。
struct OutboxStore {
    origin: Instant,
    renewal: Renewal,
    fence: Fence,
    /// 待领取工作项的世界（均为 Send）；领取跳过本轮已排除的世界，领到即出队。
    pending: Mutex<VecDeque<&'static str>>,
    exclusions: Mutex<Vec<DeliveryClaimExclusions>>,
    calls: Mutex<Vec<(u64, Call)>>,
    renewals: AtomicUsize,
    /// 领取已提交、Worker 检查结果之前执行一次；模拟领取等待跨过排空。
    after_claim: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl OutboxStore {
    fn new(worlds: &[&'static str], renewal: Renewal, fence: Fence) -> Arc<Self> {
        Arc::new(Self {
            origin: Instant::now(),
            renewal,
            fence,
            pending: Mutex::new(worlds.iter().copied().collect()),
            exclusions: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
            renewals: AtomicUsize::new(0),
            after_claim: Mutex::new(None),
        })
    }

    fn after_claim(&self, hook: impl FnOnce() + Send + 'static) {
        *self.after_claim.lock().unwrap() = Some(Box::new(hook));
    }

    fn record(&self, call: Call) {
        let offset = Instant::now().duration_since(self.origin).as_secs();
        self.calls.lock().unwrap().push((offset, call));
    }

    fn calls(&self) -> Vec<(u64, Call)> {
        self.calls.lock().unwrap().clone()
    }

    /// 每次领取请求携带的本轮已暂缓 Send 的世界。
    fn deferred_send_worlds(&self) -> Vec<Vec<String>> {
        let exclusions = self.exclusions.lock().unwrap();
        exclusions.iter().map(|exclusion| exclusion.unavailable_send_worlds.clone()).collect()
    }
}

fn repository_failure(detail: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(detail.to_owned()))
}

impl DeliveryWorkStore for OutboxStore {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        _sending_grace: Duration,
        exclusions: DeliveryClaimExclusions,
        _limit: u16,
    ) -> ApplicationFuture<'_, Vec<DeliveryWorkClaim>> {
        Box::pin(async move {
            let world = {
                let mut pending = self.pending.lock().unwrap();
                let eligible = pending.iter().position(|world| {
                    !exclusions
                        .unavailable_worlds
                        .iter()
                        .chain(&exclusions.unavailable_send_worlds)
                        .any(|excluded| excluded.as_str() == *world)
                });
                eligible.and_then(|index| pending.remove(index))
            };
            self.exclusions.lock().unwrap().push(exclusions);
            let Some(world) = world else {
                return Ok(Vec::new());
            };
            let hook = self.after_claim.lock().unwrap().take();
            if let Some(hook) = hook {
                hook();
            }
            Ok(vec![claim(world, worker_id, now, lease_until)])
        })
    }

    fn start_sending(
        &self,
        _claim: DeliveryWorkClaim,
        _started_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        unavailable()
    }

    fn start_sending_authorized<'a>(
        &'a self,
        _claim: DeliveryWorkClaim,
        _started_at: DateTime<Utc>,
        _evidence: Option<&'a SettlementInternalEvidenceSnapshot>,
        _expectation: Option<&'a DeliveryPreparedExpectation>,
    ) -> ApplicationFuture<'a, DeliveryFenceOutcome> {
        self.record(Call::Fence);
        let fence = self.fence;
        Box::pin(async move {
            match fence {
                Fence::Sending => Ok(DeliveryFenceOutcome::Sending),
                Fence::Deferred => Ok(DeliveryFenceOutcome::Deferred),
                Fence::Unknown => Err(repository_failure("围栏提交回执丢失")),
                Fence::Hangs => std::future::pending().await,
            }
        })
    }

    fn release(
        &self,
        _claim: DeliveryWorkClaim,
        _released_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        self.record(Call::Release);
        Box::pin(async { Ok(()) })
    }

    fn renew(
        &self,
        _claim: DeliveryWorkClaim,
        _renewed_at: DateTime<Utc>,
        _lease_until: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        self.record(Call::Renew);
        let attempt = self.renewals.fetch_add(1, Ordering::SeqCst);
        let renewal = self.renewal;
        Box::pin(async move {
            match renewal {
                Renewal::RejectedAfter(healthy) if attempt >= healthy => {
                    Err(repository_failure("租约已被接管"))
                }
                Renewal::HangsAfter(healthy) if attempt >= healthy => std::future::pending().await,
                _ => Ok(()),
            }
        })
    }

    fn succeed(
        &self,
        _claim: DeliveryWorkClaim,
        _result: DeliverySendResult,
        _completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        // 终态在 future 就绪即提交确认时记录；只创建未等待的调用不算落库。
        Box::pin(async move {
            self.record(Call::Succeed);
            Ok(())
        })
    }

    fn fail(
        &self,
        _claim: DeliveryWorkClaim,
        error: DeliveryTransportError,
        _completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        let stage = error.result_metadata.get("stage").and_then(serde_json::Value::as_str);
        let call = Call::Fail {
            failure_code: error.failure_code.clone(),
            outcome_unknown: error.outcome_unknown,
            stage: stage.map(str::to_owned),
        };
        Box::pin(async move {
            self.record(call);
            Ok(())
        })
    }
}

/// 只读准备的应答。
#[derive(Clone, Copy)]
enum Preparation {
    Ready,
    /// 只读检查确定失败，尚未开始远端写入。
    Rejected,
    Hangs,
}

/// 按脚本准备与发送的传输替身；记录发送 future 的 poll 与围栏后的会话撤销。
struct ScriptedTransport {
    preparation: Preparation,
    /// 发送 future 首次 poll 后保持的时长；`None` 表示远端一直不回应。
    send_hold: Option<Duration>,
    prepares: AtomicUsize,
    send_polls: AtomicUsize,
    sends_completed: AtomicUsize,
    revocations: Arc<AtomicUsize>,
}

impl ScriptedTransport {
    fn new(preparation: Preparation, send_hold: Option<Duration>) -> Arc<Self> {
        Arc::new(Self {
            preparation,
            send_hold,
            prepares: AtomicUsize::new(0),
            send_polls: AtomicUsize::new(0),
            sends_completed: AtomicUsize::new(0),
            revocations: Arc::new(AtomicUsize::new(0)),
        })
    }

    fn prepares(&self) -> usize {
        self.prepares.load(Ordering::SeqCst)
    }

    fn send_polls(&self) -> usize {
        self.send_polls.load(Ordering::SeqCst)
    }

    fn sends_completed(&self) -> usize {
        self.sends_completed.load(Ordering::SeqCst)
    }

    fn revocations(&self) -> usize {
        self.revocations.load(Ordering::SeqCst)
    }
}

/// 围栏撤销探针：提交结果未知时，丢弃句柄必须同步撤销原独占会话。
struct FenceProbe(Arc<AtomicUsize>);

impl InternalEvidenceLease for FenceProbe {
    fn is_current(&self) -> bool {
        true
    }

    fn check_current(&self, _deadline: std::time::Instant) -> InternalEvidenceFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn revoke(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl DeliveryTransport for ScriptedTransport {
    fn maximum_concurrency(&self) -> u16 {
        1
    }

    fn available(&self, _claim: &DeliveryWorkClaim) -> bool {
        true
    }

    fn prepare_authorized(
        &self,
        claim: DeliveryWorkClaim,
        _deadline: std::time::Instant,
    ) -> AuthorizedDeliveryPreparation<'_> {
        self.prepares.fetch_add(1, Ordering::SeqCst);
        let preparation = self.preparation;
        Box::pin(async move {
            match preparation {
                Preparation::Ready => {}
                Preparation::Rejected => {
                    return Err(DeliveryTransportError {
                        failure_code: "compose_unavailable".to_owned(),
                        safe_detail: "测试 compose 只读检查未通过".to_owned(),
                        outcome_unknown: false,
                        external_reference: None,
                        result_metadata: serde_json::json!({"stage": "preflight"}),
                    });
                }
                Preparation::Hangs => std::future::pending::<()>().await,
            }
            let probe = FenceProbe(Arc::clone(&self.revocations));
            Ok(AuthorizedDeliverySend::new(self.send(claim), None)
                .with_fence_lease(Box::new(probe)))
        })
    }

    fn send(
        &self,
        _claim: DeliveryWorkClaim,
    ) -> Pin<Box<dyn Future<Output = Result<DeliverySendResult, DeliveryTransportError>> + Send + '_>>
    {
        let hold = self.send_hold;
        Box::pin(async move {
            self.send_polls.fetch_add(1, Ordering::SeqCst);
            match hold {
                Some(hold) => tokio::time::sleep(hold).await,
                None => std::future::pending::<()>().await,
            }
            self.sends_completed.fetch_add(1, Ordering::SeqCst);
            Ok(DeliverySendResult {
                external_reference: "9000001".to_owned(),
                result_metadata: serde_json::json!({}),
            })
        })
    }
}

fn claim(
    world: &str,
    worker_id: String,
    claimed_at: DateTime<Utc>,
    lease_until: DateTime<Utc>,
) -> DeliveryWorkClaim {
    DeliveryWorkClaim {
        delivery_id: Uuid::new_v4(),
        command_id: Uuid::new_v4(),
        request_hash: RequestHash::new([1; 32]),
        origin_kind: "settlement".to_owned(),
        origin_id: "origin".to_owned(),
        recipient: "FixtureRecipient".to_owned(),
        payload: serde_json::json!({
            "world": world,
            "operation": "send",
            "template": "settlement_seller_payout",
        }),
        lease_token: Uuid::new_v4(),
        lease_until,
        fencing_version: 1,
        attempt_count: 1,
        worker_id,
        claimed_at,
        created_at: claimed_at,
    }
}

/// 只装配投递 owner 的 Worker；租约 60 秒、续租 10 秒、请求 30 秒、终态与围栏 5 秒。
fn delivery_worker(
    store: &Arc<OutboxStore>,
    transport: &Arc<ScriptedTransport>,
    write_lifecycle: ExternalWriteLifecycle,
) -> (PersistentWorker<PausedClock>, Arc<RecordingMetrics>) {
    let metrics = Arc::new(RecordingMetrics::default());
    let worker = PersistentWorker {
        delivery_store: Some(store.clone()),
        delivery_transport: Some(transport.clone()),
        write_lifecycle,
        metrics: metrics.clone(),
        ..worker(Arc::new(PausedClock::new(base_time())))
    };
    (worker, metrics)
}

/// 排空替身把活动的登记与释放记入 outbox 时间线，与续租、围栏和终态统一排序。
fn observed_drain(store: &Arc<OutboxStore>) -> TestDrain {
    let store = Arc::clone(store);
    TestDrain::observed(move |event| store.record(Call::Drain(event)))
}

/// 准备、围栏前、围栏后三次续租与围栏本身都发生在领取时刻。
fn handoff_calls() -> Vec<(u64, Call)> {
    vec![(0, Call::Renew), (0, Call::Renew), (0, Call::Fence), (0, Call::Renew)]
}

#[tokio::test(start_paused = true)]
async fn request_timeout_persists_outcome_unknown_and_never_resends() {
    let store = OutboxStore::new(&["main"], Renewal::Healthy, Fence::Sending);
    let transport = ScriptedTransport::new(Preparation::Ready, None);
    let (worker, metrics) = delivery_worker(&store, &transport, ExternalWriteLifecycle::default());

    run_delivery_cycle(&worker).await.unwrap();

    // 发送期间按续租间隔维持租约；30 秒请求预算耗尽后不推断远端未写入，转人工对账。
    let mut expected = handoff_calls();
    expected.extend([
        (10, Call::Renew),
        (20, Call::Renew),
        (30, outcome_unknown("request_timeout")),
    ]);
    assert_eq!(store.calls(), expected);
    assert_eq!(transport.send_polls(), 1, "结果未知的发送绝不自动重发");
    assert_eq!(transport.sends_completed(), 0);
    assert_eq!(metrics.iterations(), vec![(WorkerLoop::Delivery, WorkerResult::Success)]);
}

#[tokio::test(start_paused = true)]
async fn losing_lease_during_send_persists_outcome_unknown() {
    // 前三次续租属于准备与围栏交接；第四次是发送开始 10 秒后的首次周期续租。
    for (renewal, failed_at, stage) in [
        (Renewal::RejectedAfter(3), 10, "lease_renewal"),
        // 续租回执挂起时只等待终态预算（5 秒），不凭可能提交的续租继续驱动外部写入。
        (Renewal::HangsAfter(3), 15, "lease_renewal_timeout"),
    ] {
        let store = OutboxStore::new(&["main"], renewal, Fence::Sending);
        let transport = ScriptedTransport::new(Preparation::Ready, None);
        let (worker, metrics) =
            delivery_worker(&store, &transport, ExternalWriteLifecycle::default());

        run_delivery_cycle(&worker).await.unwrap();

        let mut expected = handoff_calls();
        expected.extend([(10, Call::Renew), (failed_at, outcome_unknown(stage))]);
        assert_eq!(store.calls(), expected, "{stage}");
        assert_eq!(transport.send_polls(), 1, "{stage}");
        assert_eq!(transport.sends_completed(), 0, "{stage}: 失去租约后立即停止驱动发送");
        assert_eq!(
            metrics.iterations(),
            vec![(WorkerLoop::Delivery, WorkerResult::Error)],
            "{stage}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn unfinished_preparation_only_releases_claim_and_defers_world() {
    for (preparation, expected) in [
        (Preparation::Rejected, vec![(0, Call::Renew), (0, Call::Release)]),
        // 准备期照常续租；30 秒准备预算耗尽后释放。
        (
            Preparation::Hangs,
            vec![(0, Call::Renew), (10, Call::Renew), (20, Call::Renew), (30, Call::Release)],
        ),
    ] {
        let store = OutboxStore::new(&["main"], Renewal::Healthy, Fence::Sending);
        let transport = ScriptedTransport::new(preparation, Some(Duration::ZERO));
        let (worker, metrics) =
            delivery_worker(&store, &transport, ExternalWriteLifecycle::default());

        run_delivery_cycle(&worker).await.unwrap();

        // 准备期没有外部写入：只释放 claim，不落发送失败也不进入人工对账。
        assert_eq!(store.calls(), expected);
        assert_eq!(transport.send_polls(), 0);
        // 本轮暂缓该世界的 Send，下一次领取跳过它，避免同一 claim 紧密重领。
        assert_eq!(store.deferred_send_worlds(), vec![Vec::new(), vec!["main".to_owned()]]);
        assert_eq!(metrics.iterations(), vec![(WorkerLoop::Delivery, WorkerResult::Success)]);
        assert!(
            metrics.unpersisted().iter().any(|(loop_kind, _)| *loop_kind == WorkerLoop::Delivery),
            "延期原因未落库，须按本循环计入未持久化处置"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn unconfirmed_fence_never_polls_send_and_revokes_owner_when_commit_unknown() {
    for (fence, released_at, revocations) in [
        // Deferred 已确定提交冻结或回滚：确认围栏结束，不撤销原会话。
        (Fence::Deferred, 0, 0),
        // 提交结果未知：丢弃句柄同步撤销原会话，不能当作已回滚继续使用。
        (Fence::Unknown, 0, 1),
        // 围栏预算是终态超时 5 秒；超时同样按提交未知处理。
        (Fence::Hangs, 5, 1),
    ] {
        let store = OutboxStore::new(&["main"], Renewal::Healthy, fence);
        let transport = ScriptedTransport::new(Preparation::Ready, Some(Duration::ZERO));
        let (worker, metrics) =
            delivery_worker(&store, &transport, ExternalWriteLifecycle::default());

        run_delivery_cycle(&worker).await.unwrap();

        assert_eq!(
            store.calls(),
            vec![
                (0, Call::Renew),
                (0, Call::Renew),
                (0, Call::Fence),
                (released_at, Call::Release)
            ]
        );
        assert_eq!(transport.send_polls(), 0, "围栏未确认 Sending 前绝不 poll 外部写入");
        assert_eq!(transport.revocations(), revocations);
        assert_eq!(store.deferred_send_worlds(), vec![Vec::new(), vec!["main".to_owned()]]);
        assert_eq!(metrics.iterations(), vec![(WorkerLoop::Delivery, WorkerResult::Error)]);
    }
}

#[tokio::test(start_paused = true)]
async fn claim_without_usable_lease_is_released_before_preparation() {
    let metrics = Arc::new(RecordingMetrics::default());
    let clock = Arc::new(PausedClock::new(base_time()));
    let store = OutboxStore::new(&[], Renewal::Healthy, Fence::Sending);
    let transport = ScriptedTransport::new(Preparation::Ready, Some(Duration::ZERO));
    // 领取回执到达时租约已过期：不准备、不续租，按原 fence 尽力释放。
    let now = clock.now();
    let expired = claim(
        "main",
        "worker-test".to_owned(),
        now - chrono::Duration::seconds(61),
        now - chrono::Duration::seconds(1),
    );

    let result = diagnostics::scope(
        metrics.clone(),
        WorkerLoop::Delivery,
        process_delivery_claim(
            store.clone(),
            transport.clone(),
            clock,
            DeliveryWorkerRuntimeConfig::default(),
            WorkerControlHandle::new("test", 0),
            noop_notification_recorder(),
            expired,
            &ExternalWriteActivity::default(),
        ),
    )
    .await;

    assert_eq!(result.worker_result, WorkerResult::Error);
    assert_eq!(store.calls(), vec![(0, Call::Release)]);
    assert_eq!(transport.prepares(), 0);
    assert!(metrics.unpersisted().contains(&(WorkerLoop::Delivery, SafeErrorKind::LEASE_EXPIRED)));
}

#[tokio::test(start_paused = true)]
async fn drain_after_claim_releases_on_legacy_path_and_keeps_lease_under_drain_protocol() {
    // 旧路径：领取后才发现排空，尚未准备的 claim 按原 fence 释放回 pending。
    let store = OutboxStore::new(&["main"], Renewal::Healthy, Fence::Sending);
    let transport = ScriptedTransport::new(Preparation::Ready, Some(Duration::ZERO));
    let (worker, _metrics) = delivery_worker(&store, &transport, ExternalWriteLifecycle::default());
    let control = worker.control.clone();
    store.after_claim(move || control.mark_draining());

    run_delivery_cycle(&worker).await.unwrap();

    assert_eq!(store.calls(), vec![(0, Call::Release)]);
    assert_eq!(transport.prepares(), 0);

    // 排空协议：入口关闭后不再释放，claim 保留原租约，到期后由下一个 owner 恢复。
    let drain = TestDrain::default();
    let store = OutboxStore::new(&["main"], Renewal::Healthy, Fence::Sending);
    let transport = ScriptedTransport::new(Preparation::Ready, Some(Duration::ZERO));
    let (worker, _metrics) = delivery_worker(&store, &transport, drain.lifecycle());
    let control = worker.control.clone();
    let closing = drain.clone();
    store.after_claim(move || {
        closing.close();
        control.mark_draining();
    });

    run_delivery_cycle(&worker).await.unwrap();

    assert!(store.calls().is_empty(), "排空协议下不得释放或续租已领取的工作项");
    assert_eq!(transport.prepares(), 0);
    assert_eq!(store.deferred_send_worlds().len(), 1, "排空后不再领取");
}

#[tokio::test(start_paused = true)]
async fn task_seeing_drain_before_first_poll_releases_only_on_legacy_path() {
    let clock = Arc::new(PausedClock::new(base_time()));
    let control = WorkerControlHandle::new("test", 0);
    control.mark_draining();
    let drain = TestDrain::default();
    drain.close();

    for (activity, expected, worker_result) in [
        (ExternalWriteActivity::default(), vec![(0, Call::Release)], WorkerResult::Success),
        // 排空协议下派生后尚未执行的任务直接取消，claim 保留原租约。
        (
            drain.lifecycle().activity(CriticalWriteKind::DeliverySend),
            Vec::new(),
            WorkerResult::Canceled,
        ),
    ] {
        let store = OutboxStore::new(&[], Renewal::Healthy, Fence::Sending);
        let transport = ScriptedTransport::new(Preparation::Ready, Some(Duration::ZERO));
        let now = clock.now();
        let claimed =
            claim("main", "worker-test".to_owned(), now, now + chrono::Duration::seconds(60));

        let result = process_delivery_claim(
            store.clone(),
            transport.clone(),
            clock.clone(),
            DeliveryWorkerRuntimeConfig::default(),
            control.clone(),
            noop_notification_recorder(),
            claimed,
            &activity,
        )
        .await;

        assert_eq!(result.worker_result, worker_result);
        assert_eq!(store.calls(), expected);
        assert_eq!(transport.prepares(), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn drain_during_preparation_cancels_without_release_and_keeps_lease() {
    let store = OutboxStore::new(&["main"], Renewal::Healthy, Fence::Sending);
    let drain = observed_drain(&store);
    let transport = ScriptedTransport::new(Preparation::Hangs, Some(Duration::ZERO));
    let (worker, metrics) = delivery_worker(&store, &transport, drain.lifecycle());
    let stop = async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        drain.close();
        worker.control.mark_draining();
    };

    let (cycle, ()) = tokio::join!(run_delivery_cycle(&worker), stop);
    cycle.unwrap();

    // 准备期尚未登记交接（时间线上没有登记事件）：只取消本地 future，不释放、不落失败，
    // 原租约到期后恢复。
    assert_eq!(store.calls(), vec![(0, Call::Renew)]);
    assert_eq!(transport.send_polls(), 0);
    assert_eq!(metrics.iterations(), vec![(WorkerLoop::Delivery, WorkerResult::Canceled)]);
}

#[tokio::test(start_paused = true)]
async fn drain_after_sending_fence_persists_outcome_before_releasing_activity() {
    for (case, send_hold, settled) in [
        (
            "远端 15 秒后回应，落库成功",
            Some(Duration::from_secs(15)),
            vec![(10, Call::Renew), (15, Call::Succeed), (15, COMPLETED)],
        ),
        (
            "远端一直不回应，请求预算耗尽后落库结果未知",
            None,
            vec![
                (10, Call::Renew),
                (20, Call::Renew),
                (30, outcome_unknown("request_timeout")),
                (30, COMPLETED),
            ],
        ),
    ] {
        let store = OutboxStore::new(&["main"], Renewal::Healthy, Fence::Sending);
        let drain = observed_drain(&store);
        let transport = ScriptedTransport::new(Preparation::Ready, send_hold);
        let (worker, metrics) = delivery_worker(&store, &transport, drain.lifecycle());
        let stop = async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            drain.close();
            worker.control.mark_draining();
        };

        let (cycle, ()) = tokio::join!(run_delivery_cycle(&worker), stop);
        cycle.unwrap();

        // 活动在围栏调用前登记，5 秒时关闭入口不影响它：继续续租、驱动原发送，终态提交确认
        // 之后才释放。提前释放会让排空不再等待，关闭期间仍挂起的落库也会被取消、停在 sending。
        let mut expected = vec![
            (0, Call::Renew),
            (0, Call::Renew),
            (0, ENTERED),
            (0, Call::Fence),
            (0, Call::Renew),
        ];
        expected.extend(settled);
        assert_eq!(store.calls(), expected, "{case}");
        assert_eq!(transport.send_polls(), 1, "{case}: 排空期间同样只发送一次、绝不重发");
        assert_eq!(store.deferred_send_worlds().len(), 1, "{case}: 排空后不再领取新工作");
        assert_eq!(
            metrics.iterations(),
            vec![(WorkerLoop::Delivery, WorkerResult::Success)],
            "{case}"
        );
    }
}
