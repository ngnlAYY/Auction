//! 退件 Take executor：只在持久起写首次提交后 Take 一次；证明未获证（验证器未签收）保留核对、
//! 零第二次 Take；崩溃重启不重复外写。替身把“数据库”放在共享的 `Ledger` 里，模拟重启。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::consignment::{
    AcquireReceipt, AcquireReturn, ConsignmentError, ConsignmentFuture, ConsumeReceipt,
    ConsumeReturn, DepositBusinessRejection, PostPermit, ReturnIntakeStore, ReturnService,
    StartReceipt, StartReturnTake, WriteStartOutcome,
};
use auction_domain::consignment::returns::ReturnLeg;
use tokio::sync::{Notify, watch};
use uuid::Uuid;

use crate::worker::consignment::{
    ClaimedTake, PreparedTake, ReturnTakeRuntime, ReturnTakeTransport, ReturnTakeWorkSource,
    TakeExecution, TakeProof, TakeRound, run_return_takes,
};

const WORK: Uuid = Uuid::from_u128(0x71);

#[derive(Default)]
struct Ledger {
    events: Mutex<Vec<&'static str>>,
    started: AtomicBool,
    /// 起写已提交但回应丢失。
    commit_unknown: AtomicBool,
    /// 消费事务判证据不足（验证器未签收）。
    unsigned: AtomicBool,
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

impl ReturnIntakeStore for Store {
    fn acquire_return(&self, _command: AcquireReturn) -> ConsignmentFuture<'_, AcquireReceipt> {
        panic!("Take executor 不接管")
    }

    fn start_return_take(
        &self,
        command: StartReturnTake,
    ) -> ConsignmentFuture<'_, WriteStartOutcome> {
        let receipt = StartReceipt::started(
            command.command_id,
            Uuid::nil(),
            command.work_id,
            command.attempt_id,
            command.fence,
        );
        let first = !self.0.started.swap(true, Ordering::SeqCst);
        if first {
            self.0.record("take_started");
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

    fn consume_return(&self, command: ConsumeReturn) -> ConsignmentFuture<'_, ConsumeReceipt> {
        self.0.record("consume");
        let unsigned = self.0.unsigned.load(Ordering::SeqCst);
        Box::pin(async move {
            if unsigned {
                return Err(DepositBusinessRejection::EvidenceUnproven.into());
            }
            Ok(ConsumeReceipt {
                command_id: command.command_id,
                intake_id: command.intake_id,
                proof_id: Uuid::nil(),
                leg: ReturnLeg::Original,
                replayed: false,
            })
        })
    }
}

/// 最坏情况的来源：同一 Take 工作一直可领取。
struct Source;

impl ReturnTakeWorkSource for Source {
    fn claim_next<'a>(
        &'a self,
        _worker_id: &'a str,
        _lease: Duration,
    ) -> ConsignmentFuture<'a, Option<ClaimedTake>> {
        Box::pin(async {
            Ok(Some(ClaimedTake {
                intake_id: Uuid::from_u128(0x61),
                work_id: WORK,
                delivery_id: Uuid::from_u128(0x81),
                attempt_id: Uuid::from_u128(0x91),
                fence: 1,
                claim_token: Uuid::from_u128(0xa1),
            }))
        })
    }
}

#[derive(Clone, Copy)]
enum Remote {
    Proven,
    /// Take 已发出，进程在读证明之前崩溃。
    Crash,
}

struct Transport {
    ledger: Arc<Ledger>,
    remote: Remote,
}

impl ReturnTakeTransport for Transport {
    fn prepare<'a>(
        &'a self,
        _take: &'a ClaimedTake,
    ) -> ConsignmentFuture<'a, Option<PreparedTake>> {
        Box::pin(async {
            Ok(Some(PreparedTake {
                session_binding_digest: [3; 32],
                expected_manifest_digest: [4; 32],
                handle: Box::new(()),
            }))
        })
    }

    fn execute(
        &self,
        _prepared: PreparedTake,
        permit: PostPermit,
    ) -> ConsignmentFuture<'_, TakeExecution> {
        assert_eq!(permit.work_id(), WORK);
        self.ledger.record("take_post");
        let remote = self.remote;
        Box::pin(async move {
            match remote {
                Remote::Proven => Ok(TakeExecution::Proven(TakeProof {
                    observation_id: Uuid::from_u128(0xb1),
                    validator_version: "fake-validator".to_owned(),
                    proof_digest: [5; 32],
                })),
                Remote::Crash => std::future::pending().await,
            }
        })
    }
}

fn executor(ledger: &Arc<Ledger>, remote: Remote) -> Arc<ReturnTakeRuntime> {
    Arc::new(ReturnTakeRuntime {
        work: Arc::new(Source),
        service: Arc::new(ReturnService::new(Arc::new(Store(ledger.clone())))),
        transport: Arc::new(Transport { ledger: ledger.clone(), remote }),
        worker_id: "fake-worker".to_owned(),
        lease_duration: Duration::from_secs(60),
        wakeup: Arc::new(Notify::new()),
        paused: Arc::new(|| false),
    })
}

async fn idle_rounds() {
    for _ in 0..5 {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_secs(6)).await;
    }
}

#[tokio::test]
async fn take_runs_once_after_the_durable_start_and_consumes_the_proof() {
    let ledger = Arc::new(Ledger::default());
    let runtime = executor(&ledger, Remote::Proven);
    assert_eq!(runtime.run_once().await.unwrap(), TakeRound::Consumed);
    assert_eq!(runtime.run_once().await.unwrap(), TakeRound::Closed);
    assert_eq!(ledger.events(), ["take_started", "take_post", "consume"]);

    // 反例：起写已提交但回应丢失，拿不到许可，零 Take；再领取同一工作也只得到“已提交”。
    let lost = Arc::new(Ledger::default());
    lost.commit_unknown.store(true, Ordering::SeqCst);
    let runtime = executor(&lost, Remote::Proven);
    assert_eq!(runtime.run_once().await.unwrap_err(), ConsignmentError::Unavailable);
    assert_eq!(runtime.run_once().await.unwrap(), TakeRound::Closed);
    assert_eq!(lost.events(), ["take_started"]);
}

#[tokio::test(start_paused = true)]
async fn unsigned_proof_is_kept_for_reconciliation_without_a_second_take() {
    let ledger = Arc::new(Ledger::default());
    ledger.unsigned.store(true, Ordering::SeqCst);
    let (drain, receiver) = watch::channel(false);
    let task =
        tokio::spawn(run_return_takes(executor(&ledger, Remote::Proven), receiver, |_, _| {}));
    idle_rounds().await;
    drain.send_replace(true);
    task.await.unwrap();
    assert_eq!(ledger.events(), ["take_started", "take_post", "consume"], "未签收只保留核对");
}

#[tokio::test(start_paused = true)]
async fn restart_after_a_crash_mid_take_never_takes_again() {
    let ledger = Arc::new(Ledger::default());
    let (_crashed_drain, receiver) = watch::channel(false);
    let crashed =
        tokio::spawn(run_return_takes(executor(&ledger, Remote::Crash), receiver, |_, _| {}));
    idle_rounds().await;
    crashed.abort();
    let _aborted = crashed.await;
    let (drain, receiver) = watch::channel(false);
    let restarted =
        tokio::spawn(run_return_takes(executor(&ledger, Remote::Proven), receiver, |_, _| {}));
    idle_rounds().await;
    drain.send_replace(true);
    restarted.await.unwrap();
    assert_eq!(ledger.events(), ["take_started", "take_post"], "重启后不得重复 Take");
}
