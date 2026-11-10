//! 停机排空验收：在邮件替身上于发送关键段中途停止 Worker。
//!
//! 走真实装配路径：`WorkerShutdown` 的两段排空、`DrainCoordinator` 活动计数与真实
//! MoogleMail 客户端（compose GET → 慢速 POST → 发送成功证明）。投递 owner 按
//! `delivery/handoff.rs` 的协议编排：准备期可取消；围栏前登记关键段；sending 落库后才 POST；
//! 结果落库后才 `complete`。持久化由内存账本代替，只记录 sending 围栏与结果落库时刻。
//!
//! 保护的不变量：已越过 sending 围栏的外部写入在关键段预算内完成并落库后 Worker 才退出；
//! 未越过围栏的工作在关闭时立即放弃且不得再登记；预算耗尽只留下 sending 围栏（结果未知，
//! 人工对账），不补写结果、不重发。

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use auction_application::operations::{CriticalWriteKind, ExternalWriteActivity};
use auction_config::ShutdownSettings;
use auction_integrations::mooglemail::{
    MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailForm,
    MoogleMailMessageFields, MoogleMailParser, MoogleMailWorld,
};
use auction_observability::{
    ShutdownCriticalKind, ShutdownOutcome, ShutdownPhase, ShutdownReason, ShutdownRecorder,
    ShutdownRole,
};
use auction_process::TerminationSignals;
use auction_test_support::mooglemail::{Fault, MailDouble};

use super::WorkerShutdown;

const WORLD: MoogleMailWorld = MoogleMailWorld::Main;

/// 只记录闭集 label 的排空指标替身。
#[derive(Default)]
struct RecordingShutdown(Mutex<Vec<String>>);

impl RecordingShutdown {
    fn push(&self, event: String) {
        self.0.lock().expect("metrics lock").push(event);
    }
    fn events(&self) -> Vec<String> {
        self.0.lock().expect("metrics lock").clone()
    }
    fn count(&self, event: &str) -> usize {
        self.events().iter().filter(|recorded| recorded.as_str() == event).count()
    }
}

impl ShutdownRecorder for RecordingShutdown {
    fn critical_entered(&self, kind: ShutdownCriticalKind) {
        self.push(format!("entered:{}", kind.label()));
    }
    fn critical_released(&self, kind: ShutdownCriticalKind) {
        self.push(format!("released:{}", kind.label()));
    }
    fn finished(&self, role: ShutdownRole, reason: ShutdownReason, outcome: ShutdownOutcome) {
        self.push(format!("finished:{}:{}:{}", role.label(), reason.label(), outcome.label()));
    }
    fn phase_finished(&self, role: ShutdownRole, phase: ShutdownPhase, _elapsed: Duration) {
        self.push(format!("phase:{}:{}", role.label(), phase.label()));
    }
}

/// 代替投递存储：sending 围栏与结果落库各自只写一次。
#[derive(Default)]
struct DeliveryLedger {
    sending: AtomicBool,
    /// `(远端成功已证明, 落库时刻)`；`None` 表示结果未落库。
    outcome: Mutex<Option<(bool, Instant)>>,
}

impl DeliveryLedger {
    fn outcome(&self) -> Option<(bool, Instant)> {
        *self.outcome.lock().expect("ledger lock")
    }
}

/// 准备期 owner 的观察结果。
#[derive(Default)]
struct PreparingProbe {
    cancelled: AtomicBool,
    entered_after_cancel: AtomicBool,
}

fn client(server: &MailDouble) -> MoogleMailClient {
    MoogleMailClient::with_config(MoogleMailClientConfig {
        endpoint: server.endpoint(),
        maximum_response_bytes: 8 * 1024 * 1024,
        minimum_request_interval: Duration::from_millis(1),
        // 必须长于替身的 POST 延迟，排空结果只由 Worker 预算裁决。
        request_timeout: Duration::from_secs(30),
    })
    .expect("loopback client")
}

fn cookie() -> MoogleMailCookie {
    MoogleMailCookie::new("ipb_member_id=fixture; ipb_pass_hash=fixture").expect("fixture cookie")
}

/// 按 handoff 协议执行一次发送；走到哪一步由账本与替身计数体现。
async fn send_owner(
    client: &MoogleMailClient,
    activity: ExternalWriteActivity,
    ledger: &DeliveryLedger,
) {
    let cookie = cookie();
    let target = client.compose(WORLD).expect("compose target");
    // 准备期只读，关闭先发生时由活动取消，不写任何持久状态。
    let Some(page) = activity.run(client.get(&target, &cookie)).await else { return };
    let state = MoogleMailParser
        .parse_compose_state(&page.expect("compose page"), WORLD)
        .expect("compose state");
    // 围栏调用前登记；关闭先赢时不发起围栏，原 claim 由租约恢复。
    if !activity.enter_critical() {
        return;
    }
    ledger.sending.store(true, Ordering::SeqCst);
    let fields = MoogleMailMessageFields {
        recipient: "FixtureRecipient".into(),
        subject: "fixture".into(),
        body: "fixture".into(),
    };
    let form = MoogleMailForm::send(state.token(), &fields).expect("send form");
    let response = client.post(&target, &cookie, form).await;
    let proven = response
        .ok()
        .is_some_and(|bytes| MoogleMailParser.prove_send_completed(&bytes, WORLD).is_ok());
    *ledger.outcome.lock().expect("ledger lock") = Some((proven, Instant::now()));
    // 只在结果落库后释放；未调用即 drop 时观察者只记为未收口。
    activity.complete();
}

/// 尚未登记关键段的准备期工作：关闭后应立即取消，且不能再登记。
async fn preparing_owner(activity: ExternalWriteActivity, probe: &PreparingProbe) {
    if activity.run(std::future::pending::<()>()).await.is_some() {
        return;
    }
    probe.cancelled.store(true, Ordering::SeqCst);
    if activity.enter_critical() {
        probe.entered_after_cancel.store(true, Ordering::SeqCst);
    }
}

/// POST 抵达替身即说明发送已越过 sending 围栏；替身在响应前延迟。
async fn wait_for_post(server: &MailDouble) {
    while server.write_requests() == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

struct Outcome {
    result: anyhow::Result<()>,
    forced: bool,
    closed_at: Instant,
    drained_at: Instant,
}

/// 与 `bootstrap::run` 相同的顺序：先 close，再保持原 Worker future 的所有权排空。
async fn stop_during_send(
    settings: ShutdownSettings,
    post_delay: Duration,
    ledger: &DeliveryLedger,
    probe: &PreparingProbe,
    metrics: &Arc<RecordingShutdown>,
    server: &MailDouble,
) -> Outcome {
    server.set_fault(Some(Fault::SlowWriteSucceeded(post_delay)));
    let client = client(server);
    let shutdown = WorkerShutdown::new(settings, Arc::clone(metrics) as Arc<dyn ShutdownRecorder>);
    let sending = shutdown.lifecycle.activity(CriticalWriteKind::DeliverySend);
    let preparing = shutdown.lifecycle.activity(CriticalWriteKind::DeliverySend);
    let mut signals = TerminationSignals::subscribe().expect("subscribe termination signals");
    let mut worker = Box::pin(async {
        tokio::join!(send_owner(&client, sending, ledger), preparing_owner(preparing, probe));
        Ok::<(), anyhow::Error>(())
    });
    tokio::select! {
        biased;
        _ = &mut worker => panic!("worker finished before the stop request"),
        () = wait_for_post(server) => {}
    }
    let closed_at = Instant::now();
    shutdown.close();
    let mut forced = false;
    let result =
        shutdown.drain(&mut signals, &mut worker, ShutdownReason::Signal, &mut forced).await;
    let drained_at = Instant::now();
    // 强制收口时由进程退出丢弃在途 future；这里显式丢弃，模拟同一时刻。
    drop(worker);
    Outcome { result, forced, closed_at, drained_at }
}

#[tokio::test]
async fn stop_during_send_waits_until_result_is_persisted() {
    let server = MailDouble::start().await.expect("mail double");
    let ledger = DeliveryLedger::default();
    let probe = PreparingProbe::default();
    let metrics = Arc::new(RecordingShutdown::default());
    // 普通排空 1 秒短于剩余的 POST 时间，只有关键段预算能等到结果落库。
    let outcome = stop_during_send(
        ShutdownSettings { drain_timeout_seconds: 1, critical_write_timeout_seconds: 10 },
        Duration::from_secs(2),
        &ledger,
        &probe,
        &metrics,
        &server,
    )
    .await;

    outcome.result.expect("drain completes");
    assert!(!outcome.forced, "critical budget must not force exit");
    assert!(ledger.sending.load(Ordering::SeqCst));
    let (proven, persisted_at) = ledger.outcome().expect("result persisted before exit");
    assert!(proven, "the real success page proves the send");
    assert!(persisted_at <= outcome.drained_at, "exit only after the result is persisted");
    assert!(
        outcome.drained_at.duration_since(outcome.closed_at) > Duration::from_secs(1),
        "the in-flight send outlived the ordinary drain budget"
    );
    assert!(probe.cancelled.load(Ordering::SeqCst), "unfenced work is abandoned on close");
    assert!(!probe.entered_after_cancel.load(Ordering::SeqCst));
    assert_eq!(server.write_requests(), 1, "no resend");
    assert_eq!(metrics.count("entered:delivery_send"), 1);
    assert_eq!(metrics.count("released:delivery_send"), 1);
    assert_eq!(metrics.count("phase:worker:critical"), 1);
    assert_eq!(metrics.count("finished:worker:signal:completed"), 1);
}

#[tokio::test]
async fn exhausted_critical_budget_leaves_unknown_outcome_without_resend() {
    let server = MailDouble::start().await.expect("mail double");
    let ledger = DeliveryLedger::default();
    let probe = PreparingProbe::default();
    let metrics = Arc::new(RecordingShutdown::default());
    let outcome = stop_during_send(
        ShutdownSettings { drain_timeout_seconds: 1, critical_write_timeout_seconds: 1 },
        Duration::from_secs(6),
        &ledger,
        &probe,
        &metrics,
        &server,
    )
    .await;

    assert!(outcome.result.is_err(), "budget exhaustion is reported as a forced exit");
    assert!(outcome.forced);
    assert!(
        outcome.drained_at.duration_since(outcome.closed_at) < Duration::from_secs(5),
        "the critical budget bounds the wait"
    );
    // sending 围栏已持久，结果未落库：只能人工对账，不补写失败、不自动重发。
    assert!(ledger.sending.load(Ordering::SeqCst));
    assert!(ledger.outcome().is_none());
    assert_eq!(server.write_requests(), 1);
    assert_eq!(metrics.count("finished:worker:signal:timed_out"), 1);
    // 丢弃在途 future 时活动以未收口释放，计数不泄漏。
    assert_eq!(metrics.count("released:delivery_send"), 1);
}

#[tokio::test]
async fn legacy_path_without_critical_budget_cuts_in_flight_send() {
    let server = MailDouble::start().await.expect("mail double");
    let ledger = DeliveryLedger::default();
    let probe = PreparingProbe::default();
    let metrics = Arc::new(RecordingShutdown::default());
    // `critical_write_timeout_seconds = 0` 是旧路径：只有普通排空，用于复现该场景。
    let outcome = stop_during_send(
        ShutdownSettings { drain_timeout_seconds: 1, critical_write_timeout_seconds: 0 },
        Duration::from_secs(3),
        &ledger,
        &probe,
        &metrics,
        &server,
    )
    .await;

    assert!(outcome.result.is_err());
    assert!(outcome.forced);
    assert!(ledger.sending.load(Ordering::SeqCst));
    assert!(ledger.outcome().is_none(), "D8: the fenced send is cut before its result lands");
    assert_eq!(metrics.count("phase:worker:critical"), 0);
    assert_eq!(metrics.count("finished:worker:signal:timed_out"), 1);
}

/// 领取前的后台清理由同一写生命周期保护：关闭入口后、进入关键段前的扫描必须立即取消，
/// 不能把无界 SQL 扫描带入停机排空。
#[tokio::test]
async fn preclaim_background_cleanup_is_cancelled_by_shutdown_close() {
    let metrics = Arc::new(RecordingShutdown::default());
    let shutdown = WorkerShutdown::new(
        ShutdownSettings { drain_timeout_seconds: 1, critical_write_timeout_seconds: 1 },
        metrics,
    );
    let pending = shutdown.lifecycle.background(std::future::pending::<()>());
    tokio::pin!(pending);
    shutdown.close();
    let result = tokio::time::timeout(Duration::from_secs(1), &mut pending)
        .await
        .expect("关闭必须唤醒后台清理");
    assert!(result.is_none(), "未进入关键段的后台清理必须被取消");
}
