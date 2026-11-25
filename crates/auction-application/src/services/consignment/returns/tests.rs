//! 退件接管应用服务的核心测试：POST 许可只在首次提交时铸造；命令身份只由业务键决定。

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use auction_domain::CommandId;
use auction_domain::SettlementWorld;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::dispatch::AttemptKey;
use auction_domain::consignment::returns::{ReturnIntakeFacts, ReturnLeg};
use auction_time::Timestamp;
use uuid::Uuid;

use super::{ConsumeReturnInput, ReturnService, StartTakeInput};
use crate::ports::consignment::{
    AcquireReceipt, AcquireReturn, ConsignmentError, ConsignmentFuture, ConsumeReceipt,
    ConsumeReturn, ReturnIntakeStore, StartReceipt, StartReturnTake, WriteStart, WriteStartOutcome,
};

/// 假端口的 future 立即就绪，不需要运行时。
fn run<T>(future: impl Future<Output = T>) -> T {
    match std::pin::pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("假实现的 future 必须立即就绪"),
    }
}

/// 只回放预置起写结果、并记录收到的命令的假端口。
struct ScriptedStore {
    first: bool,
    started: Mutex<Vec<StartReturnTake>>,
    acquired: Mutex<Vec<CommandId>>,
}

impl ScriptedStore {
    fn new(first: bool) -> Arc<Self> {
        Arc::new(Self { first, started: Mutex::default(), acquired: Mutex::default() })
    }
}

impl ReturnIntakeStore for ScriptedStore {
    fn acquire_return(&self, command: AcquireReturn) -> ConsignmentFuture<'_, AcquireReceipt> {
        if let Ok(mut acquired) = self.acquired.lock() {
            acquired.push(command.command_id);
        }
        Box::pin(async move { Err(ConsignmentError::Unavailable) })
    }

    fn start_return_take(
        &self,
        command: StartReturnTake,
    ) -> ConsignmentFuture<'_, WriteStartOutcome> {
        let receipt = StartReceipt::started(
            command.command_id,
            Uuid::from_u128(1),
            command.work_id,
            command.attempt_id,
            command.fence,
        );
        let first = self.first;
        if let Ok(mut started) = self.started.lock() {
            started.push(command);
        }
        Box::pin(async move {
            Ok(if first {
                WriteStartOutcome::FirstCommitted(receipt)
            } else {
                WriteStartOutcome::AlreadyCommitted(receipt)
            })
        })
    }

    fn consume_return(&self, _command: ConsumeReturn) -> ConsignmentFuture<'_, ConsumeReceipt> {
        Box::pin(async move { Err(ConsignmentError::Unavailable) })
    }
}

fn input(fence: u64) -> StartTakeInput {
    StartTakeInput {
        intake_id: Uuid::from_u128(10),
        work_id: Uuid::from_u128(11),
        delivery_id: Uuid::from_u128(12),
        attempt_id: Uuid::from_u128(13),
        fence,
        claim_token: Uuid::from_u128(14),
        session_binding_digest: [5; 32],
        expected_manifest_digest: [6; 32],
    }
}

fn facts(mid: &str) -> ReturnIntakeFacts {
    ReturnIntakeFacts {
        k: OriginalMailKey::new("12345", SettlementWorld::Main, mid).expect("规范原信键"),
        contract_id: Uuid::from_u128(20),
        obligation_id: Uuid::from_u128(21),
        original_round: 1,
        original_attempt: AttemptKey { delivery_id: Uuid::from_u128(22), fence: 1 },
        sent_mid: "900".to_owned(),
        rts_observation_id: Uuid::from_u128(23),
        rts_returned_at: Timestamp::UNIX_EPOCH,
        leg: ReturnLeg::CashRefund,
        frozen_manifest_digest: [7; 32],
        session_id: Uuid::from_u128(24),
    }
}

#[test]
fn permit_is_minted_only_for_the_first_committed_start() {
    let first = ReturnService::new(ScriptedStore::new(true));
    let WriteStart::FirstCommitted { permit, .. } =
        run(first.start_take(input(1))).expect("首次提交")
    else {
        panic!("首次提交必须携带许可");
    };
    assert_eq!(
        (permit.work_id(), permit.delivery_id(), permit.fence()),
        (Uuid::from_u128(11), Uuid::from_u128(12), 1)
    );
    // 已有成功起写的工作：新 fence、新命令 ID 与重放都只得“已提交”，没有许可。
    let already = ReturnService::new(ScriptedStore::new(false));
    assert!(matches!(
        run(already.start_take(input(2))).expect("已提交"),
        WriteStart::AlreadyCommitted(_)
    ));
}

#[test]
fn start_command_id_follows_the_fence_and_acquire_id_only_the_mail_key() {
    let store = ScriptedStore::new(true);
    let service = ReturnService::new(store.clone());
    run(service.start_take(input(1))).expect("起写");
    run(service.start_take(input(2))).expect("起写");
    let ids: Vec<_> =
        store.started.lock().expect("记录锁").iter().map(|command| command.command_id).collect();
    assert_ne!(ids[0], ids[1], "过租后的新 fence 必须是新命令 ID，旧 fence 的拒绝不占它");

    // 同一 K 的接管命令 ID 不随 RTS 观察 ID 变化；换 K 必变。
    let mut other_observation = facts("557");
    other_observation.rts_observation_id = Uuid::from_u128(99);
    for candidate in [facts("557"), other_observation, facts("558")] {
        let _ = run(service.acquire(candidate));
    }
    let acquired = store.acquired.lock().expect("记录锁").clone();
    assert_eq!(acquired[0], acquired[1]);
    assert_ne!(acquired[0], acquired[2]);
}

#[test]
fn consume_digest_changes_with_the_proof_digest() {
    let base = ConsumeReturnInput {
        intake_id: Uuid::from_u128(10),
        original_attempt_id: Uuid::from_u128(13),
        fence: 1,
        observation_id: Uuid::from_u128(15),
        validator_version: "validator-v1".to_owned(),
        proof_digest: [8; 32],
    };
    let mut changed = base.clone();
    changed.proof_digest = [9; 32];
    let (first_id, first_hash) = super::encoding::consume(&base);
    let (second_id, second_hash) = super::encoding::consume(&changed);
    assert_eq!(first_id, second_id, "消费命令 ID 只由 intake 决定，异证明才在账本里冲突");
    assert_ne!(first_hash, second_hash);
}

/// 黄金向量：接管摘要的规范字节冻结；命令 ID 只由 K 决定，沿用旧值。
/// 纳入决定参数之前的摘要是 `a2f83f977a10dfd03dd0df0e179ec7cc8adec9f4863ec726e36801b91756bd07`。
#[test]
fn acquire_digest_golden_vector() {
    let (id, hash) = super::encoding::acquire(&facts("557"));
    let hex: String = hash.as_bytes().iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(id.as_uuid().to_string(), "1255593b-39c2-5b94-a0d2-37055431a595");
    assert_eq!(hex, "4ac141223b76a0deae12803f5439873eff82bb2304e7c15276beaaac92615fab");
}

/// 同 K 异决定参数（Sent MID、原 attempt、fence、冻结清单、原会话）摘要必不同，命令 ID 不变。
#[test]
fn acquire_digest_changes_with_every_decision_parameter() {
    let (base_id, base_hash) = super::encoding::acquire(&facts("557"));
    let mut variants = Vec::new();
    let mut other = facts("557");
    other.sent_mid = "901".to_owned();
    variants.push(other);
    let mut other = facts("557");
    other.original_attempt.delivery_id = Uuid::from_u128(98);
    variants.push(other);
    let mut other = facts("557");
    other.original_attempt.fence = 2;
    variants.push(other);
    let mut other = facts("557");
    other.frozen_manifest_digest = [8; 32];
    variants.push(other);
    let mut other = facts("557");
    other.session_id = Uuid::from_u128(97);
    variants.push(other);
    for changed in variants {
        let (id, hash) = super::encoding::acquire(&changed);
        assert_eq!(id, base_id, "命令 ID 只由 K 决定");
        assert_ne!(hash, base_hash, "决定参数不同必须是异请求");
    }
    // RTS 观察 ID 每次到达都可能不同，不进摘要。
    let mut observed = facts("557");
    observed.rts_observation_id = Uuid::from_u128(99);
    assert_eq!(super::encoding::acquire(&observed).1, base_hash);
}
