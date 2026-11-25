//! 资金支出侧夹具：已购成交、退款腿首派、现金腿支出、已起写未消费的现金退件阻断，以及 opening 最终写入口。
//!
//! `seed_unconsumed_cash_return_block` 交付给第 7 期复用：它构造“已起写、未消费”的现金退件阻断存量。

use auction_application::settlement::{
    CashLeg, CashPhase, FundingCashGate, FundingCommitOutcome, FundingError, FundingMutation,
    FundingReconciliationCommand, FundingReconciliationWriter, INTERNAL_EVIDENCE_PROTOCOL,
    InternalEvidencePurpose, MailAccountEvidenceBinding, MailRuntimeBinding, SettleCash,
    SettlementInternalEvidenceSnapshot, VerifiedMainCashObservation,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::funding::{BlockedLeg, FundingMode, OpeningBlock};
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_3};
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::{
    Intake, PRICE, PURCHASED_AT, REFUND_ATTEMPT, REFUND_DELIVERY, REFUND_MID, REFUND_ROUND,
    seed_purchased, send_sql,
};

pub const UID: &str = "7001";
pub const REFUND_ALLOCATION: &str = "c6c10000-0000-4000-8000-000000000001";
pub const CONSUMPTION: &str = "c6c20000-0000-4000-8000-000000000001";

pub fn id(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("夹具 UUID")
}

pub fn mutation(byte: u8) -> FundingMutation {
    FundingMutation { command_id: Uuid::from_bytes([byte; 16]), request_digest: [byte; 32] }
}

/// 退款腿的现金首派：Send、绑定 Sent MID，以及冻结金额等于 P 的轮次与尝试。
pub fn refund_dispatch_sql() -> String {
    send_sql(REFUND_DELIVERY, OBLIGATION_3, REFUND_MID, 0)
        + &format!(
            "INSERT INTO public.consignment_dispatch_rounds
                (contract_id, obligation_id, round, round_id, delivery_id, rights_holder_identity,
                 bot_uid, world_id, subject, template, body_digest, round_ref, d0, credits, cod,
                 prior_round, authorized_by_intake, authorized_by_consumption)
             VALUES ('{CONTRACT}', '{OBLIGATION_3}', 1, '{REFUND_ROUND}', '{REFUND_DELIVERY}',
                     'FakeSellerTwo', '12345', 'main', 'fake subject', 'consignment_cod',
                     decode(repeat('d1', 32), 'hex'), '{REFUND_ROUND}', '{PURCHASED_AT}', {PRICE},
                     0, NULL, NULL, NULL);
             INSERT INTO public.consignment_dispatch_attempts
                (delivery_id, fence, attempt_id, contract_id, obligation_id, round, leg,
                 started_at)
             VALUES ('{REFUND_DELIVERY}', 1, '{REFUND_ATTEMPT}', '{CONTRACT}', '{OBLIGATION_3}',
                     1, 'cash_refund', '{PURCHASED_AT}');"
        )
}

/// 一条退款腿现金命令；命令 ID 与摘要由 `byte` 决定。
pub fn cash(byte: u8, phase: CashPhase, amount: i64) -> SettleCash {
    SettleCash {
        mutation: mutation(byte),
        contract_id: id(CONTRACT),
        obligation_id: id(OBLIGATION_3),
        delivery_id: id(REFUND_DELIVERY),
        leg: CashLeg::Refund,
        phase,
        amount,
    }
}

/// 已购成交加退款腿首派，退款分配里已有 P 可用。
pub async fn seed_refund_ready(pool: &PgPool) {
    seed_purchased(pool).await;
    succeeds(pool, &refund_dispatch_sql()).await;
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.settlement_funding_allocations
                (id, uid, purpose, obligation_id, consignment_contract_id, amount)
             VALUES ('{REFUND_ALLOCATION}', '{UID}', 'refund', '{OBLIGATION_3}', '{CONTRACT}',
                     {PRICE});"
        ),
    )
    .await;
}

/// 退款已发出：reserve → in_flight → spent（原支出），命令 ID 取 1、2、3。
pub async fn seed_refund_spent(pool: &PgPool, store: &DatabaseStore) {
    seed_refund_ready(pool).await;
    for (byte, phase) in [(1, CashPhase::Reserve), (2, CashPhase::InFlight), (3, CashPhase::Spent)]
    {
        store.test_settle_cash(cash(byte, phase, PRICE)).await.expect("现金腿状态迁移");
    }
}

pub fn cash_refund_intake() -> Intake {
    Intake { n: 1, leg: "cash_refund" }
}

pub fn refund_block(intake: &Intake) -> OpeningBlock {
    OpeningBlock::new(
        UID.to_owned(),
        id(&intake.intake()),
        BlockedLeg::Refund,
        id(REFUND_DELIVERY),
        PRICE,
        FundingMode::StrictCash,
    )
    .expect("阻断记录")
}

/// 第 7 期复用：已发出的退款被 RTS 接管，Take 已起写、阻断已建立，但尚未消费、资金未回补。
pub async fn seed_unconsumed_cash_return_block(
    pool: &PgPool,
    store: &DatabaseStore,
) -> (Intake, Uuid) {
    seed_refund_spent(pool, store).await;
    let intake = cash_refund_intake();
    succeeds(pool, &intake.sql()).await;
    let block = store
        .test_open_cash_return_block(&intake.start_sql(), &refund_block(&intake))
        .await
        .expect("起写同事务建立阻断");
    (intake, block)
}

/// 精确 Take 证明加消费：证明与消费必须同一事务（消费守卫要求本事务写入的 take_proven 证明）。
pub fn consumption_sql(intake: &Intake) -> String {
    format!(
        "INSERT INTO public.mail_disposition_proofs
            (id, work_id, decision_id, delivery_id, fence, attempt_id, kind, asset_leg,
             observation_id, validator_version, digest, conclusion, proven_at)
         VALUES ('{proof}', '{work}', '{decision}', '{take}', 1, '{attempt}', 'exact_take',
                 'cash', '{observation}', 'v1', decode(repeat('a7', 32), 'hex'), 'take_proven',
                 '{PURCHASED_AT}');
         INSERT INTO public.consignment_return_consumptions
            (id, intake_id, contract_id, obligation_id, original_round, leg, bot_uid, world_id,
             external_mid, work_id, delivery_id, attempt_id, fence, proof_id)
         VALUES ('{CONSUMPTION}', '{intake_id}', '{CONTRACT}', '{OBLIGATION_3}', 1, '{leg}',
                 '12345', 'main', '{mid}', '{work}', '{take}', '{attempt}', 1, '{proof}');",
        proof = intake.proof(),
        work = intake.work(),
        decision = intake.decision(),
        take = intake.take_delivery(),
        attempt = intake.attempt(),
        observation = intake.observation(),
        intake_id = intake.intake(),
        leg = intake.leg,
        mid = intake.mid(),
    )
}

/// 对该 UID 的 opening 最终写入口（显式对账）；证据与预览摘要取合成值，门禁在预览比对之前。
pub async fn try_reconcile(
    store: &DatabaseStore,
    pool: &PgPool,
    uid: &str,
) -> Result<FundingCommitOutcome, FundingError> {
    let version: Option<i64> = sqlx::query_scalar(
        "SELECT state_version FROM public.settlement_funding_accounts WHERE uid = $1",
    )
    .bind(uid)
    .fetch_optional(pool)
    .await
    .expect("读取账户版本");
    // 尚无账户的 UID（首次开户）按版本 0 提交，与写端的 CAS 口径一致。
    reconcile_with(store, pool, uid, version.unwrap_or(0), [9; 32]).await
}

/// 同一 opening 写入口，但由调用方给出预期账户版本与预览摘要；取真实预览的摘要时就是一次可合法提交的对账。
pub async fn reconcile_with(
    store: &DatabaseStore,
    pool: &PgPool,
    uid: &str,
    version: i64,
    preview_digest: [u8; 32],
) -> Result<FundingCommitOutcome, FundingError> {
    sqlx::query(
        "INSERT INTO public.mail_runtime_generation (scope, generation, activated_at)
         VALUES ('mail_cookie', 11, now()) ON CONFLICT (scope) DO NOTHING",
    )
    .execute(pool)
    .await
    .expect("写入邮件运行代际");
    // 对账在 opening 门之后才锁凭据：用迁移夹具已有的活动邮件凭据，否则会先被判为证据过期，门禁不可分辨。
    let (credential_id, credential_version): (Uuid, i64) = sqlx::query_as(
        "SELECT id, state_version FROM public.credential_keys
         WHERE credential_kind = 'mail_cookie' AND status = 'active'",
    )
    .fetch_one(pool)
    .await
    .expect("读取活动邮件凭据");
    // 夹具凭据的作用域是 'main'；资金证据要求 'mooglemail:main'。
    sqlx::query("UPDATE public.credential_keys SET scope = 'mooglemail:main' WHERE id = $1")
        .bind(credential_id)
        .execute(pool)
        .await
        .expect("对齐凭据作用域");
    let generation: i64 = sqlx::query_scalar(
        "SELECT generation FROM public.mail_runtime_generation WHERE scope = 'mail_cookie'",
    )
    .fetch_one(pool)
    .await
    .expect("读取运行代际");
    let mut command = FundingReconciliationCommand {
        command_id: Uuid::from_bytes([0xe1; 16]),
        actor: "fake-operator".to_owned(),
        operator_identity: None,
        expected_uid: uid.to_owned(),
        expected_version: version,
        preview_digest,
        proprietary_credits: 0,
        compensation_credits: 0,
        confirm: String::new(),
        dedicated_account_no_external_liabilities: true,
    };
    command.confirm = command.confirmation();
    let observed = Timestamp::now().duration_trunc(Duration::microseconds(1)).expect("截断到微秒");
    let mut identity = MailAccountEvidenceBinding {
        credential_id,
        credential_version,
        runtime: MailRuntimeBinding { generation },
        authenticated_uid: uid.to_owned(),
        player_name: "Fake Bot".to_owned(),
        world: SettlementWorld::Main,
        observed_at: observed,
        protocol_version: INTERNAL_EVIDENCE_PROTOCOL,
        digest: [0; 32],
    };
    identity.digest = identity.compute_digest();
    let mut cash = VerifiedMainCashObservation {
        binding: identity.clone(),
        credits: 1_000_000,
        observed_at: observed,
        digest: [0; 32],
    };
    cash.digest = cash.compute_digest();
    let mut evidence = SettlementInternalEvidenceSnapshot {
        operation_id: command.command_id,
        purpose: InternalEvidencePurpose::FundingReconciliation {
            command_id: command.command_id,
            expected_account_version: u64::try_from(version).expect("账户版本非负"),
        },
        identity,
        equipment: None,
        main_cash: Some(cash),
        sent_collection: None,
        observed_from: observed,
        expires_at: observed + Duration::seconds(60),
        digest: [0; 32],
    };
    evidence.digest = evidence.compute_digest();
    store.reconcile_funding(&command, &evidence, FundingCashGate::V1).await
}

/// 该 UID 的 (opening 来源数, 分录数)。
pub async fn ledger_counts(pool: &PgPool, uid: &str) -> (i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.settlement_funding_sources WHERE uid = $1),
                (SELECT count(*) FROM public.settlement_funding_entries WHERE uid = $1)",
    )
    .bind(uid)
    .fetch_one(pool)
    .await
    .expect("读取账本计数")
}

/// 该 UID 下某种分录的 (条数, 金额合计)。
pub async fn entries_of(pool: &PgPool, uid: &str, kind: &str) -> (i64, i64) {
    sqlx::query_as(
        "SELECT count(*), COALESCE(sum(amount), 0)::bigint
         FROM public.settlement_funding_entries WHERE uid = $1 AND kind = $2",
    )
    .bind(uid)
    .bind(kind)
    .fetch_one(pool)
    .await
    .expect("读取分录")
}
