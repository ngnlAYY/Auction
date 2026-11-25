//! 寄售派发 executor 与补寄消费端：只在持久起写首次提交后 POST 一次；结果未知转人工、零重发；
//! 崩溃重启不重复外写；补寄只授权处置记下的那一轮。
//!
//! 替身把“数据库”放在共享的 `Ledger` 里，不同 runtime 实例看到同一份持久事实，模拟重启。

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::consignment::{
    AttemptReceipt, AuthorizeDispatch, ConsignmentError, ConsignmentFuture, DispatchService,
    DispatchStore, EvidenceReceipt, MarkWriteStarted, PostPermit, RecordEvidence, RoundReceipt,
    ScanCursor, ScanPage, StartAttempt, StartReceipt, WriteStartOutcome,
};
use auction_application::delivery::DeliverySendCredentialBinding;
use auction_domain::SettlementWorld;
use auction_domain::consignment::dispatch::{
    AttemptKey, DispatchRoundKey, FrozenDispatchIdentity, dispatch_ref, dispatch_round_id,
};
use auction_time::Timestamp;
use tokio::sync::{Notify, watch};
use uuid::Uuid;

use crate::worker::consignment::{
    ConsignmentDispatchRuntime, DispatchExecution, DispatchLease, DispatchRound, DispatchTransport,
    DispatchWork, DispatchWorkSource, DownstreamRoundRuntime, DownstreamRoundSource,
    PendingDownstreamRound, PendingRedispatch, PreparedDispatch, RedispatchRuntime,
    RedispatchWorkSource, SentObservation, run_dispatch, run_downstream_rounds, run_redispatch,
};

const DELIVERY: Uuid = Uuid::from_u128(0xd1);

/// 共享的持久事实与远端写入记录。
#[derive(Default)]
struct Ledger {
    /// 按发生顺序记录的持久起写、远端 POST 与证据。
    events: Mutex<Vec<&'static str>>,
    /// 投递已进入 sending（起写已提交）。
    sending: AtomicBool,
    fence: AtomicU64,
    /// 起写已提交但回应丢失：调用方只看到错误。
    commit_unknown: AtomicBool,
    /// 衔接看到的是旧视图：投递已在 sending 仍按原尝试键与 token 给出租约。
    stale_lease: AtomicBool,
    /// 已建的轮。
    rounds: Mutex<HashSet<Uuid>>,
}

impl Ledger {
    fn record(&self, event: &'static str) {
        self.events.lock().unwrap().push(event);
    }

    fn events(&self) -> Vec<&'static str> {
        self.events.lock().unwrap().clone()
    }
}

struct Store(Arc<Ledger>);

impl DispatchStore for Store {
    fn authorize_dispatch(
        &self,
        command: AuthorizeDispatch,
    ) -> ConsignmentFuture<'_, RoundReceipt> {
        let round_id = dispatch_round_id(&command.key);
        let created = self.0.rounds.lock().unwrap().insert(round_id);
        Box::pin(async move {
            Ok(RoundReceipt {
                command_id: command.command_id,
                round_id,
                round: command.key.round,
                delivery_id: command.delivery_id,
                replayed: !created,
            })
        })
    }

    fn start_attempt(&self, command: StartAttempt) -> ConsignmentFuture<'_, AttemptReceipt> {
        Box::pin(async move {
            Ok(AttemptReceipt {
                command_id: command.command_id,
                attempt_id: Uuid::nil(),
                replayed: false,
            })
        })
    }

    fn mark_write_started(
        &self,
        command: MarkWriteStarted,
    ) -> ConsignmentFuture<'_, WriteStartOutcome> {
        let receipt = StartReceipt::started(
            command.command_id,
            Uuid::nil(),
            command.attempt.delivery_id,
            Uuid::nil(),
            command.attempt.fence,
        );
        let first = !self.0.sending.swap(true, Ordering::SeqCst);
        if first {
            self.0.record("write_started");
        }
        let lost = self.0.commit_unknown.load(Ordering::SeqCst);
        Box::pin(async move {
            match (first, lost) {
                (true, true) => Err(ConsignmentError::Unavailable),
                (true, false) => Ok(WriteStartOutcome::FirstCommitted(receipt)),
                (false, _) => Ok(WriteStartOutcome::AlreadyCommitted(receipt)),
            }
        })
    }

    fn record_evidence(&self, command: RecordEvidence) -> ConsignmentFuture<'_, EvidenceReceipt> {
        self.0.record("evidence");
        Box::pin(
            async move { Ok(EvidenceReceipt { command_id: command.command_id, replayed: false }) },
        )
    }
}

/// 最坏情况的来源：同一义务一直可领取；只有投递状态决定能否衔接。
struct Source(Arc<Ledger>);

impl DispatchWorkSource for Source {
    fn claim_next<'a>(
        &'a self,
        _worker_id: &'a str,
        _lease: Duration,
    ) -> ConsignmentFuture<'a, Option<DispatchWork>> {
        Box::pin(async { Ok(Some(work())) })
    }

    fn lease_round<'a>(&'a self, _work: &'a DispatchWork) -> ConsignmentFuture<'a, DispatchLease> {
        let sending = self.0.sending.load(Ordering::SeqCst);
        let lease = if sending && self.0.stale_lease.load(Ordering::SeqCst) {
            let fence = self.0.fence.load(Ordering::SeqCst);
            DispatchLease::Leased {
                attempt: AttemptKey { delivery_id: DELIVERY, fence },
                claim_token: Uuid::from_u128(u128::from(fence)),
            }
        } else if sending {
            DispatchLease::Settled
        } else {
            let fence = self.0.fence.fetch_add(1, Ordering::SeqCst) + 1;
            DispatchLease::Leased {
                attempt: AttemptKey { delivery_id: DELIVERY, fence },
                claim_token: Uuid::from_u128(u128::from(fence)),
            }
        };
        Box::pin(async move { Ok(lease) })
    }
}

#[derive(Clone, Copy)]
enum Remote {
    Proven,
    Unknown,
    /// POST 已发出，进程在读 Sent 之前崩溃。
    Crash,
}

struct Transport {
    ledger: Arc<Ledger>,
    remote: Remote,
}

impl DispatchTransport for Transport {
    fn prepare<'a>(
        &'a self,
        _work: &'a DispatchWork,
        _attempt: AttemptKey,
        _claim_token: Uuid,
    ) -> ConsignmentFuture<'a, Option<PreparedDispatch>> {
        Box::pin(async {
            Ok(Some(PreparedDispatch {
                credential: DeliverySendCredentialBinding {
                    credential_id: Uuid::from_u128(0xc1),
                    credential_version: 1,
                    runtime_generation: 1,
                },
                cash_evidence: None,
                handle: Box::new(()),
            }))
        })
    }

    fn execute(
        &self,
        _prepared: PreparedDispatch,
        permit: PostPermit,
    ) -> ConsignmentFuture<'_, DispatchExecution> {
        assert_eq!(permit.delivery_id(), DELIVERY);
        self.ledger.record("post");
        let remote = self.remote;
        Box::pin(async move {
            match remote {
                Remote::Proven => Ok(DispatchExecution::Proven(SentObservation {
                    sent_mid: "9000001".to_owned(),
                    observed_at: at(),
                    validator_version: "fake-validator".to_owned(),
                    evidence_digest: [7; 32],
                })),
                Remote::Unknown => Ok(DispatchExecution::Unknown),
                Remote::Crash => std::future::pending().await,
            }
        })
    }
}

fn at() -> Timestamp {
    Timestamp::parse_from_rfc3339("2026-01-01T04:00:00Z").unwrap()
}

fn key() -> DispatchRoundKey {
    DispatchRoundKey {
        contract_id: Uuid::from_u128(0xa1),
        obligation_id: Uuid::from_u128(0xb1),
        round: 1,
    }
}

fn frozen(key: &DispatchRoundKey) -> FrozenDispatchIdentity {
    FrozenDispatchIdentity {
        rights_holder_identity: "FakeBuyer".to_owned(),
        bot_uid: "12345".to_owned(),
        world: SettlementWorld::Main,
        subject: "fake subject".to_owned(),
        template: "consignment_cod".to_owned(),
        body_digest: [1; 32],
        round_ref: dispatch_ref(key),
        d0: at(),
        credits: 0,
        cod: 101,
        equipment_ids: vec!["42".to_owned()],
    }
}

fn work() -> DispatchWork {
    DispatchWork {
        obligation_id: key().obligation_id,
        claim_token: Uuid::from_u128(0xe1),
        claim_fencing: 1,
        key: key(),
        frozen: frozen(&key()),
    }
}

fn service(ledger: &Arc<Ledger>) -> Arc<DispatchService> {
    Arc::new(DispatchService::new(Arc::new(Store(ledger.clone()))))
}

fn executor(ledger: &Arc<Ledger>, remote: Remote) -> Arc<ConsignmentDispatchRuntime> {
    Arc::new(ConsignmentDispatchRuntime {
        work: Arc::new(Source(ledger.clone())),
        service: service(ledger),
        transport: Arc::new(Transport { ledger: ledger.clone(), remote }),
        worker_id: "fake-worker".to_owned(),
        lease_duration: Duration::from_secs(60),
        wakeup: Arc::new(Notify::new()),
        paused: Arc::new(|| false),
    })
}

/// 让循环跑过若干次空闲轮询。
async fn idle_rounds() {
    for _ in 0..5 {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_secs(6)).await;
    }
}

#[tokio::test]
async fn dispatch_posts_once_and_only_after_the_durable_write_start() {
    let ledger = Arc::new(Ledger::default());
    let runtime = executor(&ledger, Remote::Proven);
    assert_eq!(runtime.run_once().await.unwrap(), DispatchRound::Sent);
    assert_eq!(runtime.run_once().await.unwrap(), DispatchRound::Closed);
    assert_eq!(ledger.events(), ["write_started", "post", "evidence"]);

    // 反例：起写已提交但回应丢失，调用方拿不到许可，零 POST；即使衔接按旧视图再给出同一尝试，
    // 起写也只得到“已提交”，仍不发。
    let lost = Arc::new(Ledger::default());
    lost.commit_unknown.store(true, Ordering::SeqCst);
    lost.stale_lease.store(true, Ordering::SeqCst);
    let runtime = executor(&lost, Remote::Proven);
    assert_eq!(runtime.run_once().await.unwrap_err(), ConsignmentError::Unavailable);
    assert_eq!(runtime.run_once().await.unwrap(), DispatchRound::Closed);
    assert_eq!(lost.events(), ["write_started"]);
}

#[tokio::test(start_paused = true)]
async fn unknown_result_is_left_for_reconciliation_without_a_second_post() {
    let ledger = Arc::new(Ledger::default());
    let (drain, receiver) = watch::channel(false);
    let task = tokio::spawn(run_dispatch(executor(&ledger, Remote::Unknown), receiver, |_, _| {}));
    idle_rounds().await;
    drain.send_replace(true);
    task.await.unwrap();
    assert_eq!(ledger.events(), ["write_started", "post"], "结果未知只能转人工，不得重发");
}

#[tokio::test(start_paused = true)]
async fn restart_after_a_crash_mid_post_never_posts_again() {
    let ledger = Arc::new(Ledger::default());
    let (_crashed_drain, receiver) = watch::channel(false);
    let crashed = tokio::spawn(run_dispatch(executor(&ledger, Remote::Crash), receiver, |_, _| {}));
    idle_rounds().await;
    assert_eq!(ledger.events(), ["write_started", "post"]);
    crashed.abort();
    let _aborted = crashed.await;

    let (drain, receiver) = watch::channel(false);
    let restarted =
        tokio::spawn(run_dispatch(executor(&ledger, Remote::Proven), receiver, |_, _| {}));
    idle_rounds().await;
    drain.send_replace(true);
    restarted.await.unwrap();
    assert_eq!(ledger.events(), ["write_started", "post"], "重启后不得重复外写");
}

/// 只返回给定补寄待办的来源。
struct Pending(Vec<PendingRedispatch>);

impl RedispatchWorkSource for Pending {
    fn pending_redispatches(
        &self,
        _after: Option<ScanCursor>,
        _limit: u32,
    ) -> ConsignmentFuture<'_, ScanPage<PendingRedispatch>> {
        let pending = self.0.clone();
        Box::pin(async move { Ok(ScanPage::last(pending)) })
    }
}

#[tokio::test(start_paused = true)]
async fn redispatch_authorizes_only_the_round_id_queued_by_the_resolution() {
    let queued = DispatchRoundKey { round: 2, ..key() };
    let stray = DispatchRoundKey { obligation_id: Uuid::from_u128(0xb2), ..queued };
    let item = |key: DispatchRoundKey, dispatch_id| PendingRedispatch {
        dispatch_id,
        key,
        closed_by_return_intake: Uuid::from_u128(0xf1),
        delivery_id: DELIVERY,
        frozen: frozen(&key),
    };
    let ledger = Arc::new(Ledger::default());
    let runtime = Arc::new(RedispatchRuntime {
        source: Arc::new(Pending(vec![
            item(queued, dispatch_round_id(&queued)),
            // 反例：处置记下的轮 ID 不是这个轮键派生的，不得授权。
            item(stray, dispatch_round_id(&queued)),
        ])),
        service: service(&ledger),
        wakeup: Arc::new(Notify::new()),
    });
    let (drain, receiver) = watch::channel(false);
    let task = tokio::spawn(run_redispatch(runtime, receiver, |_, _| {}));
    idle_rounds().await;
    drain.send_replace(true);
    task.await.unwrap();
    let rounds = ledger.rounds.lock().unwrap().clone();
    assert_eq!(rounds, HashSet::from([dispatch_round_id(&queued)]));
}

/// 按页给出待办的来源：第一页带游标，第二页起没有后续；两页的内容由测试给定。
struct Paged<T> {
    first: Vec<T>,
    second: Vec<T>,
}

impl<T: Clone> Paged<T> {
    fn page(&self, after: Option<ScanCursor>) -> ScanPage<T> {
        match after {
            None => ScanPage {
                items: self.first.clone(),
                next: Some(ScanCursor {
                    sort_at: Timestamp::from_timestamp(0, 0).unwrap(),
                    tie_id: Uuid::from_u128(1),
                }),
            },
            Some(_) => ScanPage::last(self.second.clone()),
        }
    }
}

impl RedispatchWorkSource for Paged<PendingRedispatch> {
    fn pending_redispatches(
        &self,
        after: Option<ScanCursor>,
        _limit: u32,
    ) -> ConsignmentFuture<'_, ScanPage<PendingRedispatch>> {
        let page = self.page(after);
        Box::pin(async move { Ok(page) })
    }
}

impl DownstreamRoundSource for Paged<PendingDownstreamRound> {
    fn pending_downstream_rounds(
        &self,
        after: Option<ScanCursor>,
        _limit: u32,
    ) -> ConsignmentFuture<'_, ScanPage<PendingDownstreamRound>> {
        let page = self.page(after);
        Box::pin(async move { Ok(page) })
    }
}

/// 不变量 5：第一页的补寄项不写任何行（轮键与处置不符）、下一轮仍会被选中时，一次扫描按键集分页走完
/// 全部待办，第二页里排在后面的补寄同一轮被授权，不会被前面的项饿死；不符项仍不授权。
#[tokio::test(start_paused = true)]
async fn redispatch_scan_reaches_items_behind_a_full_page_of_unauthorizable_ones() {
    let queued = DispatchRoundKey { round: 2, ..key() };
    let stray = DispatchRoundKey { obligation_id: Uuid::from_u128(0xb2), ..queued };
    let item = |key: DispatchRoundKey, dispatch_id| PendingRedispatch {
        dispatch_id,
        key,
        closed_by_return_intake: Uuid::from_u128(0xf1),
        delivery_id: DELIVERY,
        frozen: frozen(&key),
    };
    let ledger = Arc::new(Ledger::default());
    let runtime = Arc::new(RedispatchRuntime {
        source: Arc::new(Paged {
            first: vec![item(stray, dispatch_round_id(&queued))],
            second: vec![item(queued, dispatch_round_id(&queued))],
        }),
        service: service(&ledger),
        wakeup: Arc::new(Notify::new()),
    });
    let (drain, receiver) = watch::channel(false);
    let task = tokio::spawn(run_redispatch(runtime, receiver, |_, _| {}));
    idle_rounds().await;
    drain.send_replace(true);
    task.await.unwrap();
    let rounds = ledger.rounds.lock().unwrap().clone();
    assert_eq!(rounds, HashSet::from([dispatch_round_id(&queued)]));
}

/// 不变量 5：同上，下游首轮第一页里轮号不是 1 的项（来源被改坏、不授权）不会挡住第二页的首轮。
#[tokio::test(start_paused = true)]
async fn downstream_scan_reaches_items_behind_a_full_page_of_unauthorizable_ones() {
    let first = key();
    let wrong_round = DispatchRoundKey { round: 2, ..first };
    let item = |key: DispatchRoundKey| PendingDownstreamRound {
        key,
        delivery_id: DELIVERY,
        frozen: frozen(&key),
    };
    let ledger = Arc::new(Ledger::default());
    let runtime = Arc::new(DownstreamRoundRuntime {
        source: Arc::new(Paged { first: vec![item(wrong_round)], second: vec![item(first)] }),
        service: service(&ledger),
        wakeup: Arc::new(Notify::new()),
    });
    let (drain, receiver) = watch::channel(false);
    let task = tokio::spawn(run_downstream_rounds(runtime, receiver, |_, _| {}));
    idle_rounds().await;
    drain.send_replace(true);
    task.await.unwrap();
    let rounds = ledger.rounds.lock().unwrap().clone();
    assert_eq!(rounds, HashSet::from([dispatch_round_id(&first)]));
}
