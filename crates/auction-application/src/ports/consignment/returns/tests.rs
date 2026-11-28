//! 退件接管端口回归（核心：Take 起写许可只发一次、接管判定失败关闭、消费只一次），用内存假实现。

use std::future::Future;
use std::task::{Context, Poll, Waker};

use auction_domain::CommandId;
use auction_domain::SettlementWorld;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::ProofValidatorStatus;
use auction_domain::consignment::dispatch::AttemptKey;
use auction_domain::consignment::returns::{
    ClaimFacts, KFinalDisposition, ReturnIntakeFacts, ReturnLeg, StartedRef, TakeProofFacts,
    return_intake_id, start_return_take_command_id,
};
use auction_time::Timestamp;
use uuid::Uuid;

use super::test_support::FakeReturns;
use super::{AcquireReceipt, AcquireReturn, ConsumeReturn, ReturnIntakeStore, StartReturnTake};
use crate::ports::consignment::deposit::{PostPermit, WriteStart, WriteStartOutcome};
use crate::ports::consignment::errors::ConsignmentError;
use crate::ports::consignment::errors::DepositBusinessRejection;
use crate::ports::core::RequestHash;

fn run<T>(future: impl Future<Output = T>) -> T {
    match std::pin::pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("假实现的 future 必须立即就绪"),
    }
}

fn id(byte: u8) -> Uuid {
    Uuid::from_bytes([byte; 16])
}

fn at(day: u32) -> Timestamp {
    Timestamp::from_ymd_hms(2026, 11, day, 0, 0, 0).unwrap()
}

fn key(mid: &str) -> OriginalMailKey {
    OriginalMailKey::new("42", SettlementWorld::Main, mid).unwrap()
}

fn intake_facts(mid: &str, leg: ReturnLeg) -> ReturnIntakeFacts {
    ReturnIntakeFacts {
        k: key(mid),
        contract_id: id(1),
        obligation_id: id(2),
        original_round: 1,
        original_attempt: AttemptKey { delivery_id: id(3), fence: 1 },
        sent_mid: "100".to_owned(),
        rts_observation_id: id(4),
        rts_returned_at: at(5),
        leg,
        frozen_manifest_digest: [9; 32],
        session_id: id(5),
    }
}

fn acquire(
    fake: &FakeReturns,
    mid: &str,
    leg: ReturnLeg,
    command: u8,
) -> Result<AcquireReceipt, ConsignmentError> {
    fake.seed_sent_at(id(2), at(1));
    run(fake.acquire_return(AcquireReturn {
        command_id: CommandId::from_uuid(id(command)),
        request_hash: RequestHash::new([command; 32]),
        facts: intake_facts(mid, leg),
    }))
}

fn claim(attempt: u8, fence: u64, lease_live: bool) -> ClaimFacts {
    ClaimFacts {
        current_fence: fence,
        current_attempt_id: id(attempt),
        current_claim_token: id(attempt + 100),
        lease_live,
        session_binding_digest: [5; 32],
        frozen_manifest_digest: [9; 32],
    }
}

fn start(
    fake: &FakeReturns,
    receipt: &AcquireReceipt,
    attempt: u8,
    fence: u64,
) -> Result<WriteStartOutcome, ConsignmentError> {
    let command_id =
        start_return_take_command_id(receipt.intake_id, receipt.work_id, id(attempt), fence);
    run(fake.start_return_take(StartReturnTake {
        command_id,
        request_hash: RequestHash::new([attempt; 32]),
        intake_id: receipt.intake_id,
        work_id: receipt.work_id,
        delivery_id: receipt.work_id,
        attempt_id: id(attempt),
        fence,
        claim_token: id(attempt + 100),
        session_binding_digest: [5; 32],
        expected_manifest_digest: [9; 32],
    }))
}

/// 应用服务的转换：只有首次提交携带许可（与第 4 期一致）。
fn into_write_start(
    outcome: WriteStartOutcome,
    receipt: &AcquireReceipt,
    fence: u64,
) -> WriteStart {
    match outcome {
        WriteStartOutcome::FirstCommitted(start) => WriteStart::FirstCommitted {
            permit: PostPermit::new(receipt.work_id, receipt.work_id, fence),
            receipt: start,
        },
        WriteStartOutcome::AlreadyCommitted(start) => WriteStart::AlreadyCommitted(start),
    }
}

#[test]
fn take_permit_is_issued_once_per_intake_and_work() {
    let fake = FakeReturns::default();
    let receipt = acquire(&fake, "200", ReturnLeg::Original, 10).unwrap();
    fake.set_claim(receipt.work_id, claim(20, 1, true));
    let first = start(&fake, &receipt, 20, 1).unwrap();
    assert!(matches!(into_write_start(first, &receipt, 1), WriteStart::FirstCommitted { .. }));
    // 同命令重放：只读原回执，不得再有许可。
    let replay = start(&fake, &receipt, 20, 1).unwrap();
    assert!(matches!(into_write_start(replay, &receipt, 1), WriteStart::AlreadyCommitted(_)));
    // 过租后新 fence、新命令 ID 即使完整有效，也拿不到许可，且回执指向原 attempt。
    fake.set_claim(receipt.work_id, claim(21, 2, true));
    let renewed = start(&fake, &receipt, 21, 2).unwrap();
    let WriteStart::AlreadyCommitted(original) = into_write_start(renewed, &receipt, 2) else {
        panic!("新 fence 不得得到许可");
    };
    assert_eq!((original.attempt_id, original.fence), (id(20), 1));
    assert_eq!(fake.counters().first_starts, 1);
}

#[test]
fn stale_or_expired_claims_never_start_and_cash_start_opens_the_uid_block() {
    let fake = FakeReturns::default();
    let receipt = acquire(&fake, "200", ReturnLeg::CashRefund, 10).unwrap();
    // 旧 fence、过租：确定拒绝，零起写、零阻断。
    fake.set_claim(receipt.work_id, claim(21, 2, true));
    assert_eq!(
        start(&fake, &receipt, 20, 1).unwrap_err(),
        ConsignmentError::Deposit(DepositBusinessRejection::FenceMismatch)
    );
    fake.set_claim(receipt.work_id, claim(22, 3, false));
    assert_eq!(start(&fake, &receipt, 22, 3).unwrap_err(), ConsignmentError::LeaseLost);
    assert_eq!(fake.counters().first_starts, 0);
    assert!(!fake.block_open(receipt.intake_id));
    // 正例：有效领取起写，同事务建立现金阻断。
    fake.set_claim(receipt.work_id, claim(23, 4, true));
    assert!(matches!(start(&fake, &receipt, 23, 4).unwrap(), WriteStartOutcome::FirstCommitted(_)));
    assert!(fake.block_open(receipt.intake_id));
    assert_eq!(fake.counters().blocks_opened, 1);
}

#[test]
fn acquire_is_unique_per_k_and_per_leg_and_fails_closed_on_contradictions() {
    let fake = FakeReturns::default();
    let first = acquire(&fake, "200", ReturnLeg::Original, 10).unwrap();
    assert_eq!(first.intake_id, return_intake_id(&key("200")));
    // 同 K 同事实重放：不新增行。
    let again = acquire(&fake, "200", ReturnLeg::Original, 11).unwrap();
    assert!(again.replayed);
    assert_eq!((again.intake_id, again.work_id), (first.intake_id, first.work_id));
    // 同轮同腿第二个不同 MID：冲突，不接第二件。
    assert_eq!(
        acquire(&fake, "201", ReturnLeg::Original, 12).unwrap_err(),
        ConsignmentError::Deposit(DepositBusinessRejection::DispositionConflict)
    );
    // 反例：K 已有相反终局、RTS 早于原发送，都不接管。
    fake.seed_k_final(&key("202"), KFinalDisposition::Other);
    assert!(acquire(&fake, "202", ReturnLeg::Carrier, 13).is_err());
    fake.seed_sent_at(id(2), at(9));
    assert!(
        run(fake.acquire_return(AcquireReturn {
            command_id: CommandId::from_uuid(id(14)),
            request_hash: RequestHash::new([14; 32]),
            facts: intake_facts("203", ReturnLeg::Carrier),
        }))
        .is_err()
    );
    assert_eq!(fake.counters().intakes, 1);
}

fn consume(
    fake: &FakeReturns,
    receipt: &AcquireReceipt,
    command: u8,
    observation: u8,
) -> Result<super::ConsumeReceipt, ConsignmentError> {
    run(fake.consume_return(ConsumeReturn {
        command_id: CommandId::from_uuid(id(command)),
        request_hash: RequestHash::new([command; 32]),
        intake_id: receipt.intake_id,
        original_attempt_id: id(20),
        fence: 1,
        observation_id: id(observation),
        validator_version: "v1".to_owned(),
        proof_digest: [3; 32],
    }))
}

fn proof(proof_id: u8, mid: &str, exact: bool) -> TakeProofFacts {
    TakeProofFacts {
        proof_id: id(proof_id),
        exact_take: exact,
        validated: true,
        validator_status: ProofValidatorStatus::SignedOff,
        validator_version: "v1".to_owned(),
        proof_digest: [3; 32],
        k: key(mid),
        attempt: StartedRef { attempt_id: id(20), fence: 1 },
    }
}

#[test]
fn consumption_happens_once_and_cash_is_returned_once() {
    let fake = FakeReturns::default();
    let receipt = acquire(&fake, "200", ReturnLeg::CashPayout, 10).unwrap();
    // 未起写、证明缺失或证明不足：不消费，保留核对。
    assert!(consume(&fake, &receipt, 30, 70).is_err());
    fake.set_claim(receipt.work_id, claim(20, 1, true));
    start(&fake, &receipt, 20, 1).unwrap();
    assert_eq!(
        consume(&fake, &receipt, 31, 70).unwrap_err(),
        ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven)
    );
    fake.seed_proof(id(71), proof(80, "200", false));
    assert!(consume(&fake, &receipt, 32, 71).is_err());
    assert_eq!(fake.counters().consumptions, 0);
    assert!(fake.block_open(receipt.intake_id));
    // 正例：精确 Take 证明到位，消费一次并回补现金、解除阻断。
    fake.seed_proof(id(72), proof(81, "200", true));
    let consumed = consume(&fake, &receipt, 33, 72).unwrap();
    assert!(!consumed.replayed);
    assert_eq!(fake.counters().cash_returns, 1);
    assert!(!fake.block_open(receipt.intake_id));
    // 重放与不同命令 ID 的再次消费都只读原结果，不产生第二次回补。
    assert!(consume(&fake, &receipt, 33, 72).unwrap().replayed);
    let other_command = consume(&fake, &receipt, 34, 72).unwrap();
    // 回执的命令 ID 是本次提交的那个，与生产账本按新命令 ID 记录重放一致。
    assert_eq!(other_command.command_id, CommandId::from_uuid(id(34)));
    assert_eq!(other_command.proof_id, consumed.proof_id);
    assert_eq!((fake.counters().consumptions, fake.counters().cash_returns), (1, 1));
}

#[test]
fn a_command_id_bound_to_another_operation_is_a_conflict() {
    let fake = FakeReturns::default();
    let receipt = acquire(&fake, "200", ReturnLeg::CashPayout, 10).unwrap();
    fake.set_claim(receipt.work_id, claim(20, 1, true));
    fake.seed_proof(id(90), proof(85, "200", true));
    // 接管已占用命令 ID 10 与同一摘要：换操作在生产账本里是身份冲突，不得重新执行或覆盖账本。
    assert_eq!(consume(&fake, &receipt, 10, 90).unwrap_err(), ConsignmentError::CommandIdConflict);
    assert_eq!((fake.counters().consumptions, fake.counters().cash_returns), (0, 0));
}

#[test]
fn a_proof_for_another_k_or_attempt_is_a_conflict_not_a_consumption() {
    let fake = FakeReturns::default();
    let receipt = acquire(&fake, "200", ReturnLeg::Original, 10).unwrap();
    fake.set_claim(receipt.work_id, claim(20, 1, true));
    start(&fake, &receipt, 20, 1).unwrap();
    fake.seed_proof(id(73), proof(82, "999", true));
    assert_eq!(
        consume(&fake, &receipt, 35, 73).unwrap_err(),
        ConsignmentError::Deposit(DepositBusinessRejection::EvidenceConflict)
    );
    assert_eq!(fake.counters().consumptions, 0);
}

#[test]
fn an_unsigned_validator_or_a_wrong_command_reference_never_consumes() {
    let fake = FakeReturns::default();
    let receipt = acquire(&fake, "200", ReturnLeg::CashPayout, 10).unwrap();
    fake.set_claim(receipt.work_id, claim(20, 1, true));
    start(&fake, &receipt, 20, 1).unwrap();
    // 验证器未签收：保留核对，零消费、零回补、阻断保留。
    let mut unsigned = proof(83, "200", true);
    unsigned.validator_status = ProofValidatorStatus::Unsigned;
    fake.seed_proof(id(74), unsigned);
    assert_eq!(
        consume(&fake, &receipt, 36, 74).unwrap_err(),
        ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven)
    );
    // 命令的 attempt、fence 或证明摘要与实际不一致：矛盾，不消费。
    fake.seed_proof(id(75), proof(84, "200", true));
    let with = |command: u8, mutate: fn(&mut ConsumeReturn)| {
        let mut request = ConsumeReturn {
            command_id: CommandId::from_uuid(id(command)),
            request_hash: RequestHash::new([command; 32]),
            intake_id: receipt.intake_id,
            original_attempt_id: id(20),
            fence: 1,
            observation_id: id(75),
            validator_version: "v1".to_owned(),
            proof_digest: [3; 32],
        };
        mutate(&mut request);
        run(fake.consume_return(request))
    };
    let conflict = ConsignmentError::Deposit(DepositBusinessRejection::EvidenceConflict);
    assert_eq!(with(37, |c| c.fence = 2).unwrap_err(), conflict);
    assert_eq!(with(38, |c| c.original_attempt_id = id(21)).unwrap_err(), conflict);
    assert_eq!(with(39, |c| c.proof_digest = [4; 32]).unwrap_err(), conflict);
    assert_eq!(fake.counters().consumptions, 0);
    assert!(fake.block_open(receipt.intake_id));
    // 正例：引用全部一致才消费一次。
    assert!(with(40, |_| {}).is_ok());
    assert_eq!(fake.counters().consumptions, 1);
}
