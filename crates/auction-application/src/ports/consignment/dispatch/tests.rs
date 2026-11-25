//! 派发端口回归（核心：轮次推进、冻结身份一致、命令幂等、起写只发一次），用内存假实现。

use std::future::Future;
use std::task::{Context, Poll, Waker};

use auction_domain::CommandId;
use auction_domain::SettlementWorld;
use auction_domain::consignment::dispatch::{
    AttemptKey, DispatchClaimFacts, DispatchRoundKey, FrozenDispatchIdentity, RoundOrigin,
    dispatch_ref,
};
use auction_time::Timestamp;
use uuid::Uuid;

use super::test_support::FakeDispatch;
use super::{AuthorizeDispatch, DispatchStore, MarkWriteStarted, StartAttempt};
use crate::ports::consignment::deposit::WriteStartOutcome;
use crate::ports::consignment::errors::{ConsignmentError, DepositBusinessRejection};
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

fn command(byte: u8) -> CommandId {
    CommandId::from_uuid(id(byte))
}

fn hash(byte: u8) -> RequestHash {
    RequestHash::new([byte; 32])
}

fn round_key(round: u32) -> DispatchRoundKey {
    DispatchRoundKey { contract_id: id(1), obligation_id: id(2), round }
}

fn frozen(round: u32) -> FrozenDispatchIdentity {
    FrozenDispatchIdentity {
        rights_holder_identity: "buyer".to_owned(),
        bot_uid: "42".to_owned(),
        world: SettlementWorld::Main,
        subject: "[consign] x".to_owned(),
        template: "original".to_owned(),
        body_digest: [7; 32],
        round_ref: dispatch_ref(&round_key(round)),
        d0: Timestamp::from_ymd_hms(2026, 11, 1, 0, 0, 0).unwrap(),
        credits: 0,
        cod: 101,
        equipment_ids: vec!["900".to_owned()],
    }
}

fn authorize(command_byte: u8, round: u32, origin: RoundOrigin, delivery: u8) -> AuthorizeDispatch {
    AuthorizeDispatch {
        command_id: command(command_byte),
        request_hash: hash(command_byte),
        key: round_key(round),
        origin,
        delivery_id: id(delivery),
        frozen: frozen(round),
    }
}

#[test]
fn rounds_advance_only_with_a_closed_prior_round() {
    let fake = FakeDispatch::default();
    let first = run(fake.authorize_dispatch(authorize(10, 1, RoundOrigin::First, 50))).unwrap();
    assert_eq!(first.round, 1);
    // 反例：已有首轮时再建首轮、引用未封闭的接管建新轮，都不落行。
    let again = run(fake.authorize_dispatch(authorize(11, 1, RoundOrigin::First, 51)));
    assert!(again.is_err());
    let unclosed = RoundOrigin::Redispatch { closed_by_return_intake: id(90) };
    assert!(run(fake.authorize_dispatch(authorize(12, 2, unclosed, 52))).is_err());
    assert_eq!(fake.round_count(), 1);
    // 正例：引用已封闭上一轮的接管后得到第 2 轮。
    fake.close_round_by_intake(id(90));
    let second = run(fake.authorize_dispatch(authorize(13, 2, unclosed, 52))).unwrap();
    assert_eq!(second.round, 2);
}

#[test]
fn authorize_is_idempotent_by_command_id() {
    let fake = FakeDispatch::default();
    let first = run(fake.authorize_dispatch(authorize(10, 1, RoundOrigin::First, 50))).unwrap();
    let replay = run(fake.authorize_dispatch(authorize(10, 1, RoundOrigin::First, 50))).unwrap();
    assert!(!first.replayed && replay.replayed);
    assert_eq!(first.round_id, replay.round_id);
    // 反例：同 ID 异请求冲突，不返回原结果。
    let mut conflicting = authorize(10, 1, RoundOrigin::First, 50);
    conflicting.request_hash = hash(99);
    assert_eq!(
        run(fake.authorize_dispatch(conflicting)).unwrap_err(),
        ConsignmentError::CommandIdConflict
    );
    assert_eq!(fake.round_count(), 1);
}

fn start_attempt(
    fake: &FakeDispatch,
    command_byte: u8,
    fence: u64,
    body: u8,
) -> Result<(), ConsignmentError> {
    let round = round_key(1);
    let mut identity = frozen(1);
    identity.body_digest = [body; 32];
    run(fake.start_attempt(StartAttempt {
        command_id: command(command_byte),
        request_hash: hash(command_byte),
        round_id: auction_domain::consignment::dispatch::dispatch_round_id(&round),
        attempt: AttemptKey { delivery_id: id(50), fence },
        claim_token: id(60),
        frozen: identity,
    }))
    .map(|_| ())
}

fn claim(fence: u64, lease_live: bool) -> DispatchClaimFacts {
    DispatchClaimFacts { current_fence: fence, current_claim_token: id(60), lease_live }
}

fn try_mark(
    fake: &FakeDispatch,
    command_byte: u8,
    fence: u64,
) -> Result<WriteStartOutcome, ConsignmentError> {
    run(fake.mark_write_started(MarkWriteStarted {
        command_id: command(command_byte),
        request_hash: hash(command_byte),
        attempt: AttemptKey { delivery_id: id(50), fence },
        claim_token: id(60),
        credential: None,
    }))
}

fn mark(fake: &FakeDispatch, command_byte: u8, fence: u64) -> WriteStartOutcome {
    try_mark(fake, command_byte, fence).unwrap()
}

#[test]
fn attempt_must_match_the_frozen_identity_of_the_round() {
    let fake = FakeDispatch::default();
    run(fake.authorize_dispatch(authorize(10, 1, RoundOrigin::First, 50))).unwrap();
    // 反例：正文摘要被改动的尝试不登记。
    assert!(start_attempt(&fake, 20, 1, 8).is_err());
    // 正例：与轮冻结的身份一致才登记。
    assert!(start_attempt(&fake, 21, 1, 7).is_ok());
}

#[test]
fn a_delivery_has_exactly_one_first_write_start() {
    let fake = FakeDispatch::default();
    run(fake.authorize_dispatch(authorize(10, 1, RoundOrigin::First, 50))).unwrap();
    start_attempt(&fake, 21, 1, 7).unwrap();
    start_attempt(&fake, 22, 2, 7).unwrap();
    fake.set_claim(id(50), claim(1, true));
    assert!(matches!(mark(&fake, 30, 1), WriteStartOutcome::FirstCommitted(_)));
    // 同命令重放、新 fence、新命令 ID 都不再产生首次提交。
    assert!(matches!(mark(&fake, 30, 1), WriteStartOutcome::AlreadyCommitted(_)));
    fake.set_claim(id(50), claim(2, true));
    assert!(matches!(mark(&fake, 31, 2), WriteStartOutcome::AlreadyCommitted(_)));
    assert_eq!(fake.first_commits(), 1);
}

#[test]
fn a_stale_claim_never_gets_the_first_write_start() {
    let fake = FakeDispatch::default();
    run(fake.authorize_dispatch(authorize(10, 1, RoundOrigin::First, 50))).unwrap();
    start_attempt(&fake, 21, 1, 7).unwrap();
    start_attempt(&fake, 22, 2, 7).unwrap();
    // 没有领取事实、旧 fence、过租：确定拒绝，零首次提交。
    assert_eq!(
        try_mark(&fake, 30, 1).unwrap_err(),
        ConsignmentError::Deposit(DepositBusinessRejection::FenceMismatch)
    );
    fake.set_claim(id(50), claim(2, true));
    assert_eq!(
        try_mark(&fake, 31, 1).unwrap_err(),
        ConsignmentError::Deposit(DepositBusinessRejection::FenceMismatch)
    );
    fake.set_claim(id(50), claim(2, false));
    assert_eq!(try_mark(&fake, 32, 2).unwrap_err(), ConsignmentError::LeaseLost);
    assert_eq!(fake.first_commits(), 0);
    // 正例：当前且租约有效的领取得到首次许可。
    fake.set_claim(id(50), claim(2, true));
    assert!(matches!(mark(&fake, 33, 2), WriteStartOutcome::FirstCommitted(_)));
    assert_eq!(fake.first_commits(), 1);
}
