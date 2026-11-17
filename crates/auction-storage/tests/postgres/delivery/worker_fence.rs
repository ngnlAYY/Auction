//! 发送围栏的确定结论经真实 Worker 运行时收口的贯通用例（PostgreSQL 18，运行账号）。
//!
//! 每条用例在同一次运行里走完：`auction_jobs::run_persistent_with_settlement_until_shutdown` 启动的
//! 投递循环从真实存储领取投递 → 只读准备（脚本传输）→ 真实发送围栏事务 → 存储层给出确定结论 →
//! Worker 释放 claim、结束本轮并上报轮次结果与未持久化错误：
//! - SQL 失败：收回运行账号对 Send 尝试表的 INSERT（42501），围栏回滚后返回 Faulted，Worker 记错误
//!   轮次与 `repository_failed` 未持久化计数。此前这条链路只有两段边界测试：存储用例证明回滚后
//!   返回 Faulted，Worker 用例由假存储直接返回 Faulted；
//! - 证据异常：现金证据的认证玩家名不是凭据登记的预期名，围栏回滚后返回 Faulted，Worker 记错误
//!   轮次与 `dependency_mismatch`；
//! - 正常失效：运行时 generation 已被新 owner 推进，围栏返回 Deferred，Worker 记成功轮次、不计
//!   未持久化错误。
//!
//! 三例都不 poll 外部发送、不撤销原会话，投递按原 fence 释放回 pending，不冻结尝试、不写 sending
//! 审计。替身边界：MoogleMail 传输是脚本替身（只读准备直接给出冻结 Send 身份与按生产形状合成的
//! 证据，发送 future 被 poll 即记录）；拍卖周计划与生命周期端口是失败关闭的惰性替身。领取、续租、
//! 围栏、释放、Worker 调度、诊断作用域与指标归属都是生产代码。不连接真实账号，不代表远端行为。
//!
//! 仅在 `test-db` feature 下编译，入口见 `mise run test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use auction_application::admin::{
    WorkerDeliveryHotConfig, WorkerForumHotConfig, WorkerHotConfig, WorkerMailHotConfig,
};
use auction_application::auction::{
    AuctionLifecycleCommand, AuctionLifecycleCommandPort, AuctionLifecycleResult,
};
use auction_application::core::diagnostics::SafeErrorKind;
use auction_application::core::{ApplicationError, ApplicationFuture, Clock};
use auction_application::delivery::{
    AuthorizedDeliveryPreparation, AuthorizedDeliverySend, DeliveryPreparation,
    DeliveryPreparedSend, DeliverySendIntervals, DeliverySendResult, DeliveryTransport,
    DeliveryTransportError, DeliveryWorkClaim, DeliveryWorkStore,
};
use auction_application::mail::MailSendClasses;
use auction_application::notification_metrics::noop_notification_recorder;
use auction_application::operations::ExternalWriteLifecycle;
use auction_application::runtime_switches::RuntimeSwitches;
use auction_application::scheduling::{
    ScheduledRoundBootstrap, ScheduledRoundClaim, ScheduledRoundSnapshot, ScheduledRoundStore,
};
use auction_application::settlement::{
    InternalEvidenceFuture, InternalEvidenceLease, PreparedSettlementInternalEvidence,
    internal_evidence_deadline,
};
use auction_jobs::{
    AuctionWorkerRuntimeConfig, DeliveryWorkerRuntimeConfig, ForumWorkerRuntimeConfig,
    FulfillmentWorkerRuntimeConfig, PersistentWorker, WorkerControlHandle, WorkerHotRuntime,
};
use auction_observability::{
    WorkerLoop, WorkerPanicOwner, WorkerRecorder, WorkerResult, WorkerRetryKind,
};
use auction_storage::DatabaseStore;
use auction_time::Timestamp;
use sqlx::PgPool;
use tokio::sync::Notify;
use uuid::Uuid;

use super::evidence_rejections::{
    BOT_NAME, CREDENTIAL_VERSION, CREDITS, EvidenceShape, GENERATION, claim_purpose, evidence,
    fence_footprint, insert_credential, insert_manual_send, send_expectation, set_generation,
};
use crate::support;

/// 被调用即失败关闭；拍卖主循环只会因此记它自己的错误轮次，不触碰投递循环。
fn inert<'a, T: Send + 'a>() -> ApplicationFuture<'a, T> {
    Box::pin(async { Err(ApplicationError::CapabilityUnavailable("测试未装配该端口")) })
}

/// 拍卖周计划与生命周期端口的惰性替身。
struct InertAuctionPorts;

impl ScheduledRoundStore for InertAuctionPorts {
    fn database_now(&self) -> ApplicationFuture<'_, Timestamp> {
        inert()
    }

    fn find_current_round(
        &self,
        _world_id: String,
        _local_week: jiff::civil::Date,
    ) -> ApplicationFuture<'_, Option<ScheduledRoundSnapshot>> {
        inert()
    }

    fn find_slot_round(
        &self,
        _world_id: String,
        _slot_key: String,
    ) -> ApplicationFuture<'_, Option<ScheduledRoundSnapshot>> {
        inert()
    }

    fn ensure_current_round(
        &self,
        _bootstrap: ScheduledRoundBootstrap,
    ) -> ApplicationFuture<'_, ScheduledRoundSnapshot> {
        inert()
    }

    fn claim_due(
        &self,
        _worker_id: String,
        _now: Timestamp,
        _lease_until: Timestamp,
        _limit: u16,
    ) -> ApplicationFuture<'_, Vec<ScheduledRoundClaim>> {
        inert()
    }

    fn next_claimable_at(&self, _now: Timestamp) -> ApplicationFuture<'_, Option<Timestamp>> {
        inert()
    }

    fn complete(
        &self,
        _claim: ScheduledRoundClaim,
        _completed_at: Timestamp,
    ) -> ApplicationFuture<'_, ()> {
        inert()
    }

    fn fail(
        &self,
        _claim: ScheduledRoundClaim,
        _retry_at: Timestamp,
        _failure_code: String,
        _safe_detail: String,
    ) -> ApplicationFuture<'_, ()> {
        inert()
    }
}

impl AuctionLifecycleCommandPort for InertAuctionPorts {
    fn allows_manual_early_open(&self) -> bool {
        false
    }

    fn allows_manual_early_settlement(&self) -> bool {
        false
    }

    fn execute(
        &self,
        _command: AuctionLifecycleCommand,
    ) -> ApplicationFuture<'_, AuctionLifecycleResult> {
        inert()
    }
}

/// 记录每轮结果与未持久化错误；投递循环第一次上报轮次结果时通知停机。
#[derive(Default)]
struct RecordingMetrics {
    iterations: Mutex<Vec<(WorkerLoop, WorkerResult)>>,
    unpersisted: Mutex<Vec<(WorkerLoop, SafeErrorKind)>>,
    delivery_iteration: Notify,
}

impl RecordingMetrics {
    fn delivery_iterations(&self) -> Vec<WorkerResult> {
        let iterations = self.iterations.lock().unwrap();
        iterations
            .iter()
            .filter(|(kind, _)| *kind == WorkerLoop::Delivery)
            .map(|(_, r)| *r)
            .collect()
    }

    fn delivery_unpersisted(&self) -> Vec<SafeErrorKind> {
        let unpersisted = self.unpersisted.lock().unwrap();
        unpersisted
            .iter()
            .filter(|(kind, _)| *kind == WorkerLoop::Delivery)
            .map(|(_, safe_kind)| *safe_kind)
            .collect()
    }
}

impl WorkerRecorder for RecordingMetrics {
    fn observe_task_panic(&self, _loop_kind: WorkerLoop, _owner: WorkerPanicOwner) {}

    fn observe_unpersisted_error(&self, loop_kind: WorkerLoop, kind: SafeErrorKind) {
        self.unpersisted.lock().unwrap().push((loop_kind, kind));
    }

    fn bind_retry_mode(&self, _loop_kind: WorkerLoop, _retry_kind: WorkerRetryKind) {}

    fn observe_iteration(
        &self,
        loop_kind: WorkerLoop,
        result: WorkerResult,
        _duration: StdDuration,
    ) {
        self.iterations.lock().unwrap().push((loop_kind, result));
        if loop_kind == WorkerLoop::Delivery {
            self.delivery_iteration.notify_one();
        }
    }

    fn observe_batch(&self, _loop_kind: WorkerLoop, _items: usize, _capacity: usize) {}

    fn observe_retry(&self, _loop_kind: WorkerLoop, _retry_kind: WorkerRetryKind) {}

    fn observe_schedule_delay(&self, _loop_kind: WorkerLoop, _delay: StdDuration) {}
}

/// 原会话的撤销探针：确定结束的围栏不应撤销原会话。
struct SessionProbe(Arc<AtomicUsize>);

impl InternalEvidenceLease for SessionProbe {
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

/// 不执行外部写入的固定失败；只在替身被意外调用时出现。
fn not_sent() -> DeliveryTransportError {
    DeliveryTransportError {
        failure_code: "test_transport_unused".to_owned(),
        safe_detail: "测试传输不执行外部写入".to_owned(),
        outcome_unknown: false,
        external_reference: None,
        result_metadata: serde_json::json!({"stage": "preflight", "write_started": false}),
    }
}

/// MoogleMail 传输的脚本替身：只读准备直接给出冻结 Send 身份，现金 Send 另附合成证据与原会话
/// 探针；发送 future 只在被 poll 时记录并失败，用来断言围栏没有进入 sending 时绝不外写。
struct ScriptedTransport {
    /// 现金证据的形状；零 Credits 的 Send 为 None。
    evidence: Option<EvidenceShape>,
    prepares: AtomicUsize,
    send_polls: Arc<AtomicUsize>,
    revocations: Arc<AtomicUsize>,
}

impl ScriptedTransport {
    fn new(evidence: Option<EvidenceShape>) -> Arc<Self> {
        Arc::new(Self {
            evidence,
            prepares: AtomicUsize::new(0),
            send_polls: Arc::new(AtomicUsize::new(0)),
            revocations: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// 只读准备次数、发送 future 被 poll 的次数与原会话被撤销的次数。
    fn counters(&self) -> (usize, usize, usize) {
        (
            self.prepares.load(Ordering::SeqCst),
            self.send_polls.load(Ordering::SeqCst),
            self.revocations.load(Ordering::SeqCst),
        )
    }
}

impl DeliveryTransport for ScriptedTransport {
    fn maximum_concurrency(&self) -> u16 {
        1
    }

    fn available(&self, _claim: &DeliveryWorkClaim) -> bool {
        true
    }

    fn frozen_take_identity_available(&self, _world: &str) -> bool {
        true
    }

    fn prepare(&self, _claim: DeliveryWorkClaim) -> DeliveryPreparation<'_> {
        Box::pin(async { Err(not_sent()) })
    }

    fn prepare_authorized(
        &self,
        claim: DeliveryWorkClaim,
        deadline: std::time::Instant,
    ) -> AuthorizedDeliveryPreparation<'_> {
        self.prepares.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let expectation = send_expectation(&claim, self.evidence.map(|shape| shape.binding()));
            let evidence = self.evidence.map(|shape| {
                PreparedSettlementInternalEvidence::new(
                    evidence(claim.command_id, claim_purpose(&claim), &shape),
                    Box::new(SessionProbe(Arc::clone(&self.revocations))),
                    internal_evidence_deadline(deadline),
                )
                .expect("合成证据在准备时有效")
            });
            let polls = Arc::clone(&self.send_polls);
            let send: DeliveryPreparedSend<'_> = Box::pin(async move {
                polls.fetch_add(1, Ordering::SeqCst);
                Err(not_sent())
            });
            Ok(AuthorizedDeliverySend::new(send, evidence)
                .with_fence_lease(Box::new(SessionProbe(Arc::clone(&self.revocations))))
                .with_send_expectation(expectation))
        })
    }

    fn send(
        &self,
        _claim: DeliveryWorkClaim,
    ) -> Pin<Box<dyn Future<Output = Result<DeliverySendResult, DeliveryTransportError>> + Send + '_>>
    {
        Box::pin(async { Err(not_sent()) })
    }
}

/// 墙上时钟；生产装配根同样使用系统时间。
struct WallClock;

impl Clock for WallClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// 通过启动校验的热参数；投递每轮最多 10 条、轮询 60 秒，用例只等第一轮。
fn hot_config() -> WorkerHotConfig {
    WorkerHotConfig {
        mail: WorkerMailHotConfig {
            batch_size: 10,
            jitter_seconds: 0,
            fetch_attempts: 3,
            fetch_timeout_seconds: 30,
            process_attempts: 3,
            item_timeout_seconds: 30,
            batch_timeout_seconds: 600,
            retry_base_seconds: 1,
            retry_maximum_seconds: 30,
            account_reply_window_seconds: 86_400,
            explanation_mail_window_seconds: 86_400,
            explanation_mail_limit: 10,
            send_classes: MailSendClasses::default(),
            send_intervals: DeliverySendIntervals::default(),
        },
        forum: WorkerForumHotConfig {
            batch_size: 10,
            poll_interval_seconds: 300,
            jitter_seconds: 0,
            item_timeout_seconds: 60,
        },
        delivery: WorkerDeliveryHotConfig {
            batch_limit: 10,
            poll_interval_seconds: 60,
            jitter_seconds: 0,
            request_timeout_seconds: 30,
            finalize_timeout_seconds: 5,
        },
    }
}

/// 只装配投递循环的 Worker：投递端口是真实存储与脚本传输，其余可选 owner 都不装配。
fn worker(
    store: Arc<DatabaseStore>,
    transport: Arc<ScriptedTransport>,
    metrics: Arc<RecordingMetrics>,
) -> PersistentWorker<WallClock> {
    let ports = Arc::new(InertAuctionPorts);
    PersistentWorker {
        schedule: ports.clone(),
        worlds: None,
        lifecycle: ports,
        slow_proxy_due: None,
        slow_proxy_commands: None,
        auction: AuctionWorkerRuntimeConfig::default(),
        schedule_runtime: None,
        settlement_store: None,
        settlement_commands: None,
        fulfillment_store: None,
        fulfillment_commands: None,
        fulfillment: FulfillmentWorkerRuntimeConfig::default(),
        recovery_health: None,
        asset_reconciliation: None,
        recovery_store: None,
        settlement_returns: None,
        settlement_return_reader: None,
        recovery_resolution: None,
        recovery_delivery: None,
        recovery_blacklister: None,
        recovery: FulfillmentWorkerRuntimeConfig::default(),
        delivery_store: Some(store as Arc<dyn DeliveryWorkStore>),
        delivery_transport: Some(transport as Arc<dyn DeliveryTransport>),
        delivery: DeliveryWorkerRuntimeConfig::default(),
        hot: WorkerHotRuntime::new(hot_config()),
        mail_polls: Vec::new(),
        forum: ForumWorkerRuntimeConfig {
            poll_interval: StdDuration::from_secs(300),
            jitter: StdDuration::ZERO,
            lease_duration: StdDuration::from_secs(300),
        },
        forum_sync: None,
        forum_reply: None,
        operations_alert: None,
        player_notifications: None,
        lot_watch_reminders: None,
        metrics: metrics as Arc<dyn WorkerRecorder>,
        notifications: noop_notification_recorder(),
        control: WorkerControlHandle::new("test", 0),
        clock: Arc::new(WallClock),
        worker_id: "worker-fence-e2e".to_owned(),
        write_lifecycle: ExternalWriteLifecycle::default(),
        bulk_quota: None,
        switches: RuntimeSwitches::default(),
    }
}

/// 启动 Worker 运行时，等投递循环上报第一轮结果后停机；整体有界，卡住即用例失败。
async fn run_first_delivery_cycle(
    store: &Arc<DatabaseStore>,
    transport: &Arc<ScriptedTransport>,
) -> Arc<RecordingMetrics> {
    let metrics = Arc::new(RecordingMetrics::default());
    let worker = worker(Arc::clone(store), Arc::clone(transport), Arc::clone(&metrics));
    let shutdown = {
        let metrics = Arc::clone(&metrics);
        async move { metrics.delivery_iteration.notified().await }
    };
    tokio::time::timeout(
        StdDuration::from_secs(60),
        auction_jobs::run_persistent_with_settlement_until_shutdown(worker, shutdown),
    )
    .await
    .expect("投递循环必须在预算内完成第一轮")
    .expect("Worker 运行时正常停机");
    metrics
}

/// 投递状态、租约令牌与 Worker 释放写下的 `delivery.release` 审计数。
async fn released(pool: &PgPool, delivery_id: Uuid) -> (String, Option<Uuid>, i64) {
    sqlx::query_as(
        "SELECT delivery.status, delivery.lease_token,
                (SELECT count(*) FROM public.audit_records AS audit
                 WHERE audit.entity_type = 'delivery' AND audit.entity_id = delivery.id::text
                   AND audit.action = 'delivery.release')
         FROM public.deliveries AS delivery WHERE delivery.id = $1",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("读取释放后的投递")
}

/// 三例共同的收口：只做了一次只读准备，发送 future 从未被 poll，原会话没有被撤销；投递按原
/// fence 释放回 pending，没有冻结尝试或 sending 审计。
async fn assert_released_without_external_write(
    pool: &PgPool,
    delivery_id: Uuid,
    transport: &ScriptedTransport,
) {
    assert_eq!(transport.counters(), (1, 0, 0), "一次只读准备、不 poll 发送、不撤销原会话");
    assert_eq!(fence_footprint(pool, delivery_id).await, ("pending".to_owned(), 0, 0));
    assert_eq!(released(pool, delivery_id).await, ("pending".to_owned(), None, 1));
}

/// 同一次运行里：真实 SQL 失败（42501）→ 围栏事务回滚 → 存储层 Faulted → Worker 释放 claim 并
/// 保留错误轮次与按 `repository_failed` 的未持久化计数，不能像业务延期那样记成功（A4 贯通）。
#[tokio::test]
async fn postgres_worker_keeps_error_signal_when_fence_sql_fails_and_rolls_back() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = Arc::new(database.store().await.expect("以运行账号连接"));
    let pool = store.postgres_pool();
    let delivery_id = insert_manual_send(pool, "Worker Fence", 0).await;
    // 故障注入（维护账号，仅本测试库）：围栏先改 sending，再在冻结 Send 身份时以 42501 失败。
    let maintenance = database.maintenance_store().await.expect("以维护账号连接");
    sqlx::query("REVOKE INSERT ON public.delivery_send_attempts FROM auction_runtime")
        .execute(maintenance.postgres_pool())
        .await
        .expect("收回 Send 尝试写入");
    maintenance.postgres_pool().close().await;
    let transport = ScriptedTransport::new(None);

    let metrics = run_first_delivery_cycle(&store, &transport).await;

    assert_eq!(metrics.delivery_iterations(), vec![WorkerResult::Error], "内部故障不得记成成功");
    assert_eq!(metrics.delivery_unpersisted(), vec![SafeErrorKind::REPOSITORY_FAILED]);
    assert_released_without_external_write(pool, delivery_id, &transport).await;

    store.postgres_pool().close().await;
}

/// 现金证据的认证玩家名不是凭据登记的预期名：证据 owner 判为绑定不符，围栏回滚后 Faulted，
/// Worker 记错误轮次与 `dependency_mismatch`；修复前这里冒充业务延期，Worker 记成功。
#[tokio::test]
async fn postgres_worker_keeps_error_signal_when_cash_evidence_binding_mismatches() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = Arc::new(database.store().await.expect("以运行账号连接"));
    let pool = store.postgres_pool();
    let credential =
        insert_credential(pool, "mail_cookie", "active", CREDENTIAL_VERSION, Some(BOT_NAME)).await;
    set_generation(pool, GENERATION).await;
    let delivery_id = insert_manual_send(pool, "Worker Cash", CREDITS).await;
    let transport = ScriptedTransport::new(Some(EvidenceShape {
        player_name: "Impostor Bot",
        ..EvidenceShape::current(credential)
    }));

    let metrics = run_first_delivery_cycle(&store, &transport).await;

    assert_eq!(metrics.delivery_iterations(), vec![WorkerResult::Error], "证据异常不得记成成功");
    assert_eq!(metrics.delivery_unpersisted(), vec![SafeErrorKind::DEPENDENCY_MISMATCH]);
    assert_released_without_external_write(pool, delivery_id, &transport).await;

    store.postgres_pool().close().await;
}

/// 运行时 generation 在证据观察之后被新 owner 推进：观察之后的正常失效，围栏返回 Deferred，
/// Worker 按正常延期记成功轮次，不计未持久化错误。
#[tokio::test]
async fn postgres_worker_treats_superseded_runtime_generation_as_normal_deferral() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = Arc::new(database.store().await.expect("以运行账号连接"));
    let pool = store.postgres_pool();
    let credential =
        insert_credential(pool, "mail_cookie", "active", CREDENTIAL_VERSION, Some(BOT_NAME)).await;
    set_generation(pool, GENERATION + 1).await;
    let delivery_id = insert_manual_send(pool, "Worker Stale", CREDITS).await;
    let transport = ScriptedTransport::new(Some(EvidenceShape::current(credential)));

    let metrics = run_first_delivery_cycle(&store, &transport).await;

    assert_eq!(metrics.delivery_iterations(), vec![WorkerResult::Success]);
    assert!(metrics.delivery_unpersisted().is_empty(), "正常延期没有未落库的结果");
    assert_released_without_external_write(pool, delivery_id, &transport).await;

    store.postgres_pool().close().await;
}
