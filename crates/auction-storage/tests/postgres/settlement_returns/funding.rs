//! 结算退件补寄的资金守恒与派发换代保护（PostgreSQL 18，运行账号），全部经真实端口推进：
//! - 首寄：履约领取 → prepare/activate → 投递领取 → 现金门（同 session 证据；strict_cash 附 Main
//!   余额观察）预留与外发 → Sent 证明收口为 spent；
//! - 系统退回 → Admin 领回补寄 → 持久 Take 成功 → 原分配 spent → available（金额仍为 M）；
//! - 补寄：新派发编号与身份经同一现金门再支出 M；货款与退款 × strict_cash/record_only 全程净
//!   流出恰为 M，没有转移分录（退款不再做 seller→refund 转换）、没有第二个返还来源
//!   （`postgres_return_cash_resend_conserves_funding`；退款的 prepare/activate 附同 session 的
//!   装备在库证据）；
//! - 换代后旧派发的迟到成功/失败回调、人工 Sent 准入与投递台对账、以及旧派发的迟到退件都不能
//!   扣减或恢复新一轮资金（`postgres_old_dispatch_cannot_mutate_new_funding`）；
//! - 补寄派发写出结果未知时，经人工 Sent 准入确认后 spent，并按冻结证明结束本案
//!   （`postgres_return_resend_manual_sent_admission`）。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use auction_application::core::RequestHash;
use auction_application::delivery::{
    DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminPort, DeliveryClaimExclusions,
    DeliveryFenceOutcome, DeliveryPreparedExpectation, DeliverySendExpectation, DeliverySendResult,
    DeliverySentConfirmationPort, DeliveryTakeExpectation, DeliveryTransportError,
    DeliveryWorkClaim, DeliveryWorkStore, ManualSentEvidence, SentConfirmationCommand,
};
use auction_application::recovery::RecoveryWorkStore;
use auction_application::settlement::{
    INTERNAL_EVIDENCE_PROTOCOL, InternalEvidencePurpose, MailAccountEvidenceBinding,
    MailRuntimeBinding, SettlementDeliveryPreparation, SettlementFulfillmentClaim,
    SettlementFulfillmentStore, SettlementInternalEvidenceSnapshot, SettlementReturnWorkStore,
    VerifiedMailEquipmentPresence, VerifiedMainCashObservation,
};
use auction_domain::{
    CommandId, SettlementDispatchReference, SettlementReturnAction, SettlementReturnCaseStatus,
    SettlementWorld, resend_activate_command_id, resend_delivery_id, resend_prepare_command_id,
};
use auction_storage::DatabaseStore;
use chrono::{DateTime, Duration, DurationRound, Timelike, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::flows::handle;
use super::sweep::{MailSession, seed_mail_session, take_into_sending};
use super::{evidence, insert_inbound};
use crate::recovery_returns::{CodSale, insert_sale, settlement_minute};
use crate::support;

/// insert_sale 写入的卖方货款金额 M。
const AMOUNT: i64 = 90;
const UID: &str = "7321";
const BOT: &str = "Returns Bot";
const SELLER: &str = "Recovery seller";

/// 现金退件的两种义务：卖方货款（M = 90）与买方退款（M = 100）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Payout,
    Refund,
}

impl Kind {
    fn obligation(self, sale: &CodSale) -> Uuid {
        match self {
            Self::Payout => sale.seller_obligation,
            Self::Refund => sale.refund_obligation,
        }
    }

    fn amount(self) -> i64 {
        match self {
            Self::Payout => AMOUNT,
            Self::Refund => 100,
        }
    }

    fn purpose(self) -> &'static str {
        match self {
            Self::Payout => "seller",
            Self::Refund => "refund",
        }
    }

    fn template(self) -> &'static str {
        match self {
            Self::Payout => "settlement_seller_payout",
            Self::Refund => "settlement_buyer_refund",
        }
    }

    fn subject(self) -> &'static str {
        match self {
            Self::Payout => "Auction seller payout",
            Self::Refund => "Auction buyer refund",
        }
    }
}

/// 一笔经真实端口寄出、被退回并领回、补寄已进入发送的货款。
struct Resending {
    sale: CodSale,
    allocation: Uuid,
    original: Uuid,
    /// 首寄的投递 claim；换代后用于重放迟到回调。
    original_claim: DeliveryWorkClaim,
    case: Uuid,
    resend: Uuid,
    resend_claim: DeliveryWorkClaim,
    resend_expectation: Arc<DeliverySendExpectation>,
    /// 补寄激活后沿用的履约 claim（对账阶段）。
    resend_fulfillment: SettlementFulfillmentClaim,
}

async fn seed_funded_sale(
    pool: &PgPool,
    sequence: i64,
    strict: bool,
    kind: Kind,
) -> (CodSale, Uuid) {
    let (_, settled_at) = settlement_minute();
    let sale = insert_sale(pool, sequence, settled_at, 1).await;
    sqlx::query(
        "INSERT INTO public.settlement_funding_accounts (uid, status, accounting_mode)
         VALUES ($1, 'ready', $2)",
    )
    .bind(UID)
    .bind(if strict { "strict_cash" } else { "record_only" })
    .execute(pool)
    .await
    .expect("写入资金账户");
    sqlx::query(
        "INSERT INTO public.lot_settlements
            (lot_id, end_price, listed_price, buyer_amount, seller_amount, tax_amount,
             compensation_amount, hold_ppm, adjustment_ppm)
         VALUES ($1, $2, $2, 100, $2, 10, 0, 0, 0)",
    )
    .bind(sale.lot_id)
    .bind(AMOUNT)
    .execute(pool)
    .await
    .expect("写入结算明细");
    sqlx::query(
        "INSERT INTO public.settlement_funding_lots
            (lot_id, uid, payment_obligation_id, source_kind, disposition)
         VALUES ($1, $2, $3, 'collected', 'seller')",
    )
    .bind(sale.lot_id)
    .bind(UID)
    .bind(sale.payment_obligation)
    .execute(pool)
    .await
    .expect("写入资金化 lot");
    // 退款义务在成交撤销后才可执行；这里没有恢复记录，直接置为可执行的退款。
    if kind == Kind::Refund {
        sqlx::query(
            "UPDATE public.settlement_obligations SET business_state = 'ready' WHERE id = $1",
        )
        .bind(sale.refund_obligation)
        .execute(pool)
        .await
        .expect("退款可执行");
    }
    let allocation = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.settlement_funding_allocations
            (id, uid, purpose, lot_id, obligation_id, amount, state)
         VALUES ($1, $2, $3, $4, $5, $6, 'available')",
    )
    .bind(allocation)
    .bind(UID)
    .bind(kind.purpose())
    .bind(sale.lot_id)
    .bind(kind.obligation(&sale))
    .bind(kind.amount())
    .execute(pool)
    .await
    .expect("写入可用货款分配");
    for (obligation, state) in
        [(sale.payment_obligation, "satisfied"), (kind.obligation(&sale), "pending")]
    {
        sqlx::query(
            "INSERT INTO public.settlement_obligation_fulfillments
                (obligation_id, execution_state, dispatch_world, delivery_operation, completed_at)
             VALUES ($1, $2, 'main', 'send', CASE WHEN $2 = 'satisfied' THEN now() END)",
        )
        .bind(obligation)
        .bind(state)
        .execute(pool)
        .await
        .expect("写入履约");
    }
    (sale, allocation)
}

/// 同 session 证据：合成凭据版本与当前代际、资金账户 UID；strict_cash 另附 Main 余额观察。
fn delivery_evidence(
    session: &MailSession,
    claim: &DeliveryWorkClaim,
    strict: bool,
) -> SettlementInternalEvidenceSnapshot {
    let observed = Utc::now().duration_trunc(Duration::microseconds(1)).expect("截断到微秒");
    let mut identity = MailAccountEvidenceBinding {
        credential_id: session.credential,
        credential_version: 1,
        runtime: MailRuntimeBinding { generation: session.generation },
        authenticated_uid: UID.to_owned(),
        player_name: BOT.to_owned(),
        world: SettlementWorld::Main,
        observed_at: observed,
        protocol_version: INTERNAL_EVIDENCE_PROTOCOL,
        digest: [0; 32],
    };
    identity.digest = identity.compute_digest();
    let main_cash = strict.then(|| {
        let mut cash = VerifiedMainCashObservation {
            binding: identity.clone(),
            credits: 1_000_000,
            observed_at: observed,
            digest: [0; 32],
        };
        cash.digest = cash.compute_digest();
        cash
    });
    let mut evidence = SettlementInternalEvidenceSnapshot {
        operation_id: claim.command_id,
        purpose: InternalEvidencePurpose::DeliveryClaim {
            delivery_id: claim.delivery_id,
            claim_token: claim.lease_token,
            claim_fencing: claim.fencing_version,
            request_hash: claim.request_hash,
        },
        identity,
        equipment: None,
        main_cash,
        sent_collection: None,
        observed_from: observed,
        expires_at: observed + Duration::seconds(60),
        digest: [0; 32],
    };
    evidence.digest = evidence.compute_digest();
    evidence
}

/// 退款 prepare/activate 的 custody 证据：同 session 观察到成交装备仍在 Bot 库存，目的绑定当前
/// 履约 claim，操作 ID 为本阶段的 prepare 或 activate 命令。
fn custody_evidence(
    session: &MailSession,
    claim: &SettlementFulfillmentClaim,
    operation_id: Uuid,
) -> SettlementInternalEvidenceSnapshot {
    let observed = Utc::now().duration_trunc(Duration::microseconds(1)).expect("截断到微秒");
    let mut identity = MailAccountEvidenceBinding {
        credential_id: session.credential,
        credential_version: 1,
        runtime: MailRuntimeBinding { generation: session.generation },
        authenticated_uid: UID.to_owned(),
        player_name: BOT.to_owned(),
        world: claim.world,
        observed_at: observed,
        protocol_version: INTERNAL_EVIDENCE_PROTOCOL,
        digest: [0; 32],
    };
    identity.digest = identity.compute_digest();
    let mut equipment = VerifiedMailEquipmentPresence {
        binding: identity.clone(),
        equipment_id: claim.equipment_id,
        external_equipment_id: claim.external_equipment_id.clone(),
        category: "weapon_1handed".to_owned(),
        observed_at: observed,
        digest: [0; 32],
    };
    equipment.digest = equipment.compute_digest();
    let mut evidence = SettlementInternalEvidenceSnapshot {
        operation_id,
        purpose: InternalEvidencePurpose::SettlementClaim {
            obligation_id: claim.obligation_id.as_uuid(),
            claim_token: claim.claim_token,
            claim_fencing: claim.claim_fencing,
            state_version: claim.state_version,
        },
        identity,
        equipment: Some(equipment),
        main_cash: None,
        sent_collection: None,
        observed_from: observed,
        expires_at: observed + Duration::seconds(60),
        digest: [0; 32],
    };
    evidence.digest = evidence.compute_digest();
    evidence
}

/// 现金载荷；补寄另带版本化派发引用。
fn cash_payload(kind: Kind, obligation: Uuid, dispatch: Option<String>) -> Value {
    let mut variables =
        json!({"reference": obligation.to_string(), "amount": kind.amount().to_string()});
    if let Some(dispatch) = dispatch {
        variables["dispatch"] = json!(dispatch);
    }
    json!({
        "schema_version": 1, "operation": "send", "world": "main",
        "template": kind.template(), "variables": variables,
        "credits": kind.amount(), "cod": 0, "equipment_secret_refs": []
    })
}

/// 领取原义务的到期履约并以给定身份 prepare/activate 一次派发；返回激活后沿用的对账 claim。
async fn dispatch(
    store: &DatabaseStore,
    obligation: Uuid,
    preparation: impl FnOnce(String) -> SettlementDeliveryPreparation,
    activate: CommandId,
    custody: Option<&MailSession>,
) -> SettlementFulfillmentClaim {
    let claim = SettlementFulfillmentStore::claim_due(
        store,
        "returns-funding".to_owned(),
        Utc::now(),
        Utc::now() + Duration::minutes(5),
        10,
    )
    .await
    .expect("领取履约")
    .into_iter()
    .find(|claim| claim.obligation_id.as_uuid() == obligation)
    .expect("货款履约可被领取");
    let preparation = preparation(claim.recipient.clone());
    let deadline = Instant::now() + StdDuration::from_secs(30);
    let prepare_custody = custody
        .map(|session| custody_evidence(session, &claim, preparation.prepare_command_id.as_uuid()));
    let prepared = SettlementFulfillmentStore::prepare_delivery(
        store,
        claim,
        preparation,
        Utc::now(),
        prepare_custody.as_ref().map(|evidence| (evidence, deadline)),
    )
    .await
    .expect("准备投递");
    let activate_custody =
        custody.map(|session| custody_evidence(session, &prepared.next_claim, activate.as_uuid()));
    SettlementFulfillmentStore::activate_delivery(
        store,
        prepared.next_claim,
        activate,
        RequestHash::new([0x62; 32]),
        Utc::now(),
        activate_custody.as_ref().map(|evidence| (evidence, deadline)),
    )
    .await
    .expect("激活投递")
    .next_claim
}

/// 投递 Worker 领取指定投递，以冻结 Send 身份经现金门进入 sending。
async fn send_through_cash_gate(
    store: &DatabaseStore,
    session: &MailSession,
    delivery: Uuid,
    body_digest: [u8; 32],
    strict: bool,
    kind: Kind,
) -> (DeliveryWorkClaim, Arc<DeliverySendExpectation>) {
    let now = Utc::now();
    let grace = StdDuration::from_secs(60);
    DeliveryWorkStore::begin_claim_cycle(store, now, grace).await.expect("领取前清理");
    let claim = DeliveryWorkStore::claim_due(
        store,
        "returns-funding-delivery".to_owned(),
        now,
        now + Duration::minutes(2),
        grace,
        DeliveryClaimExclusions::default(),
        10,
    )
    .await
    .expect("领取投递")
    .into_iter()
    .find(|claim| claim.delivery_id == delivery)
    .expect("领取到货款投递");
    let expectation = Arc::new(DeliverySendExpectation {
        world_id: "main".to_owned(),
        recipient: claim.recipient.clone(),
        subject: kind.subject().to_owned(),
        body_digest,
        credits: kind.amount(),
        cod: 0,
        equipment_ids: Vec::new(),
        credential: Some(session.binding()),
    });
    let evidence = delivery_evidence(session, &claim, strict);
    let outcome = store
        .start_sending_authorized_until(
            claim.clone(),
            Utc::now(),
            &evidence,
            Instant::now() + StdDuration::from_secs(30),
            Some(&DeliveryPreparedExpectation::Send(Arc::clone(&expectation))),
        )
        .await
        .expect("现金门必须确定完成");
    assert!(matches!(outcome, DeliveryFenceOutcome::Sending), "现金门预留并进入 sending");
    (claim, expectation)
}

fn sent_result(
    claim: &DeliveryWorkClaim,
    expectation: &DeliverySendExpectation,
    mid: &str,
) -> DeliverySendResult {
    DeliverySendResult {
        external_reference: format!("mooglemail:main:send:{mid}"),
        result_metadata: json!({
            "operation": "send",
            "template": claim.payload["template"],
            "external_reference_proven": true,
            "sent_verified": true,
            "proof": "sent_exact_payload_v1",
            "send_expectation": serde_json::to_value(expectation).expect("冻结身份"),
            "sending_fencing_version": claim.fencing_version,
        }),
    }
}

/// 首寄经真实端口寄出 → 系统退回 → Admin 领回补寄 → Take 成功返还 → 补寄经现金门进入 sending。
async fn resending(
    store: &DatabaseStore,
    sequence: i64,
    mid: &str,
    strict: bool,
    kind: Kind,
) -> Resending {
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;
    let (sale, allocation) = seed_funded_sale(pool, sequence, strict, kind).await;
    let obligation = kind.obligation(&sale);
    let amount = kind.amount();
    // 退款的 prepare/activate 需要同 session 的装备在库证据。
    let custody = (kind == Kind::Refund).then_some(&session);

    // 首寄。
    let original = Uuid::new_v4();
    dispatch(
        store,
        obligation,
        |recipient| SettlementDeliveryPreparation {
            delivery_id: original,
            prepare_command_id: CommandId::from_uuid(Uuid::new_v4()),
            request_hash: RequestHash::new([0x61; 32]),
            recipient,
            payload: cash_payload(kind, obligation, None),
        },
        CommandId::from_uuid(Uuid::new_v4()),
        custody,
    )
    .await;
    let (original_claim, expectation) =
        send_through_cash_gate(store, &session, original, [0x51; 32], strict, kind).await;
    store
        .succeed(
            original_claim.clone(),
            sent_result(&original_claim, &expectation, "781001"),
            Utc::now(),
        )
        .await
        .expect("首寄 Sent 证明收口");
    assert_eq!(
        allocation_state(pool, allocation).await,
        ("spent".to_owned(), amount, Some(original))
    );

    // 系统退回：绑定首寄，履约转人工对账，案件待决定。
    let case = insert_inbound(
        pool,
        mid,
        Utc::now(),
        Some(("MoogleMail", format!("[RTS] {} [{obligation}]", kind.subject()))),
        evidence(obligation, amount, &[]),
    )
    .await;
    let consumed =
        store.consume_due_settlement_return(Utc::now()).await.expect("消费").expect("必须消费");
    assert_eq!((consumed.return_mail_id, consumed.delivery_id), (case, Some(original)));

    // 领回补寄：持久 Take 经投递端口以原会话执行并以成功页证明收口，返还原用途一次。
    assert_eq!(
        handle(store, case, 1, SettlementReturnAction::ReclaimAndResend).await,
        SettlementReturnCaseStatus::TakePending
    );
    store.activate_due_return_take(Utc::now()).await.expect("激活").expect("必须激活");
    let take: Uuid = sqlx::query_scalar(
        "SELECT delivery_id FROM public.settlement_return_actions WHERE return_mail_id = $1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取 Take 投递");
    let take_expectation = DeliveryTakeExpectation {
        world_id: "main".to_owned(),
        external_mail_id: mid.to_owned(),
        credential: Some(session.binding()),
    };
    let take_claim = take_into_sending(store, take, &take_expectation).await;
    store
        .succeed(
            take_claim,
            DeliverySendResult {
                external_reference: format!("mooglemail:main:take:{mid}"),
                result_metadata: json!({
                    "operation": "take", "external_mail_id": mid,
                    "proof": "action_removed_or_success_page",
                    "expected_credits": amount, "expected_cod": 0, "expected_attachment_ids": [],
                }),
            },
            Utc::now(),
        )
        .await
        .expect("Take 成功");
    let settled = store.settle_due_return_take(Utc::now()).await.expect("收口").expect("必须收口");
    assert_eq!(settled.status, SettlementReturnCaseStatus::ResendPending);
    assert_eq!(allocation_state(pool, allocation).await, ("available".to_owned(), amount, None));

    // 补寄：第 2 次派发的新身份经同一现金门再次预留与外发。
    let resend = resend_delivery_id(obligation, 2).expect("补寄投递 ID");
    let resend_fulfillment = dispatch(
        store,
        obligation,
        |recipient| SettlementDeliveryPreparation {
            delivery_id: resend,
            prepare_command_id: CommandId::from_uuid(
                resend_prepare_command_id(obligation, 2).expect("补寄 prepare ID"),
            ),
            request_hash: RequestHash::new([0x71; 32]),
            recipient,
            payload: cash_payload(
                kind,
                obligation,
                Some(
                    SettlementDispatchReference::for_resend(obligation, 2)
                        .expect("派发引用")
                        .encode(),
                ),
            ),
        },
        CommandId::from_uuid(resend_activate_command_id(obligation, 2).expect("补寄 activate ID")),
        custody,
    )
    .await;
    let (resend_claim, resend_expectation) =
        send_through_cash_gate(store, &session, resend, [0x52; 32], strict, kind).await;
    assert_eq!(
        allocation_state(pool, allocation).await,
        ("in_flight".to_owned(), amount, Some(resend))
    );
    Resending {
        sale,
        allocation,
        original,
        original_claim,
        case,
        resend,
        resend_claim,
        resend_expectation,
        resend_fulfillment,
    }
}

async fn allocation_state(pool: &PgPool, allocation: Uuid) -> (String, i64, Option<Uuid>) {
    sqlx::query_as(
        "SELECT state, amount, delivery_id FROM public.settlement_funding_allocations WHERE id = $1",
    )
    .bind(allocation)
    .fetch_one(pool)
    .await
    .expect("读取分配")
}

/// 账户的资金分录与来源汇总：(spent 合计, return 合计, transfer 条数, 来源条数, 返还事实条数)。
async fn ledger(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT
            COALESCE((SELECT sum(amount) FROM public.settlement_funding_entries
                      WHERE uid = $1 AND kind = 'spent'), 0)::bigint,
            COALESCE((SELECT sum(amount) FROM public.settlement_funding_entries
                      WHERE uid = $1 AND kind = 'return'), 0)::bigint,
            (SELECT count(*) FROM public.settlement_funding_entries
             WHERE uid = $1 AND kind = 'transfer'),
            (SELECT count(*) FROM public.settlement_funding_sources WHERE uid = $1),
            (SELECT count(*) FROM public.settlement_funding_returns)",
    )
    .bind(UID)
    .fetch_one(pool)
    .await
    .expect("读取资金分录")
}

/// 补寄成功后收口本案。
async fn finish_resend(store: &DatabaseStore, resending: &Resending) {
    store
        .succeed(
            resending.resend_claim.clone(),
            sent_result(&resending.resend_claim, &resending.resend_expectation, "781002"),
            Utc::now(),
        )
        .await
        .expect("补寄 Sent 证明收口");
    let done =
        store.complete_due_return_resend(Utc::now()).await.expect("收口补寄").expect("必须收口");
    assert_eq!(
        (done.return_mail_id, done.status),
        (resending.case, SettlementReturnCaseStatus::Resent)
    );
}

/// 货款与退款 × strict_cash/record_only：寄出 → 退回 → Take → 补寄，分配金额始终为 M、指针
/// 跟随当前派发；spent 合计 2M、return 合计 M，净流出恰为 M；没有转移分录（退款不再做 seller →
/// refund 转换），返还来源与返还事实各一。
#[tokio::test]
async fn postgres_return_cash_resend_conserves_funding() {
    for (kind, strict) in
        [(Kind::Payout, false), (Kind::Payout, true), (Kind::Refund, false), (Kind::Refund, true)]
    {
        let label = format!("{kind:?} strict={strict}");
        let amount = kind.amount();
        let database = support::TestDatabase::create().await.expect("克隆模板库");
        let store = database.store().await.expect("以运行账号连接");
        let pool = store.postgres_pool();
        let resending = resending(&store, 7_630_001, "998001", strict, kind).await;
        // 补寄进入 sending：只有首寄的 spent 与一次返还。
        assert_eq!(ledger(pool).await, (amount, amount, 0, 1, 1), "{label}");
        finish_resend(&store, &resending).await;
        assert_eq!(
            allocation_state(pool, resending.allocation).await,
            ("spent".to_owned(), amount, Some(resending.resend)),
            "{label}"
        );
        let (spent, returned, transfers, sources, returns) = ledger(pool).await;
        assert_eq!((spent, returned), (2 * amount, amount), "{label}");
        assert_eq!(spent - returned, amount, "净流出恰为 M（{label}）");
        assert_eq!(
            (transfers, sources, returns),
            (0, 1, 1),
            "没有转移、返还来源只有一条（{label}）"
        );
        let source: (String, Uuid, i64) = sqlx::query_as(
            "SELECT kind, source_key, amount FROM public.settlement_funding_sources WHERE uid = $1",
        )
        .bind(UID)
        .fetch_one(pool)
        .await
        .expect("读取返还来源");
        assert_eq!(source, ("returned_delivery".to_owned(), resending.original, amount), "{label}");
        // 分配只有本义务这一份，用途不变（退款没有再从货款转换）。
        let allocations: Vec<(String, i64)> = sqlx::query_as(
            "SELECT purpose, amount FROM public.settlement_funding_allocations WHERE uid = $1",
        )
        .bind(UID)
        .fetch_all(pool)
        .await
        .expect("读取分配");
        assert_eq!(allocations, [(kind.purpose().to_owned(), amount)], "{label}");
        let bindings: Vec<Uuid> = sqlx::query_scalar(
            "SELECT delivery_id FROM public.settlement_funding_delivery_bindings
             WHERE allocation_id = $1 ORDER BY recorded_at, delivery_id",
        )
        .bind(resending.allocation)
        .fetch_all(pool)
        .await
        .expect("读取派发资金绑定");
        assert!(
            bindings.contains(&resending.original) && bindings.contains(&resending.resend),
            "两次派发都留有只追加的资金绑定（{label}）：{bindings:?}"
        );
        pool.close().await;
    }
}

/// 补寄换代后，旧派发的迟到成功/失败回调、人工 Sent 准入、投递台对账与迟到退件都不能扣减或
/// 恢复新一轮资金；新一轮随后照常 spent。
#[tokio::test]
async fn postgres_old_dispatch_cannot_mutate_new_funding() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let resending = resending(&store, 7_630_011, "998011", false, Kind::Payout).await;
    let in_flight = ("in_flight".to_owned(), AMOUNT, Some(resending.resend));
    let before = ledger(pool).await;

    // 旧派发的迟到成功与失败回调：原 claim 已失效，拒绝且不触碰资金。
    let late = sent_result(&resending.original_claim, &resending.resend_expectation, "781001");
    assert!(store.succeed(resending.original_claim.clone(), late, Utc::now()).await.is_err());
    assert!(
        DeliveryWorkStore::fail(
            &store,
            resending.original_claim.clone(),
            DeliveryTransportError {
                failure_code: "write_unknown".to_owned(),
                safe_detail: "迟到的未知回调".to_owned(),
                outcome_unknown: true,
                external_reference: None,
                result_metadata: json!({"operation": "send", "write_started": true}),
            },
            Utc::now(),
        )
        .await
        .is_err()
    );

    // 旧派发的人工 Sent 准入：预览给出阻断项，确认失败关闭。
    let manual = manual_evidence(&resending, pool, resending.original).await;
    let preview = store
        .preview_sent_confirmation(resending.original, manual.clone())
        .await
        .expect("旧派发预览");
    assert!(!preview.eligible, "旧派发不具备人工 Sent 准入");
    let fence: i64 =
        sqlx::query_scalar("SELECT fencing_version FROM public.deliveries WHERE id = $1")
            .bind(resending.original)
            .fetch_one(pool)
            .await
            .expect("读取旧派发 fence");
    assert!(
        store
            .confirm_sent(SentConfirmationCommand {
                command_id: Uuid::new_v4(),
                delivery_id: resending.original,
                expected_fencing_version: fence,
                actor: "returns-operator".to_owned(),
                operator_identity: None,
                reason: "late Sent observation".to_owned(),
                evidence: manual,
                accept_collected_assets: false,
                accept_attempt_history: false,
            })
            .await
            .is_err()
    );

    // 投递台对旧派发的对账动作全部拒绝。
    for action in [
        DeliveryAdminAction::ReconcileSucceeded,
        DeliveryAdminAction::ReconcileFailed,
        DeliveryAdminAction::ReconcileRetry,
    ] {
        let (reference, failure) = match action {
            DeliveryAdminAction::ReconcileSucceeded => (Some("mooglemail:main:send:781001"), None),
            DeliveryAdminAction::ReconcileFailed => (None, Some("remote_rejected")),
            _ => (None, None),
        };
        let command = DeliveryAdminCommand {
            command_id: Uuid::new_v4(),
            delivery_id: resending.original,
            expected_fencing_version: fence,
            action,
            request_hash: action.request_hash(
                resending.original,
                fence,
                reference,
                failure,
                "admin-returns",
            ),
            external_reference: reference.map(str::to_owned),
            failure_code: failure.map(str::to_owned),
            actor_id: "admin-returns".to_owned(),
            request_id: None,
        };
        assert!(DeliveryAdminPort::execute(&store, command).await.is_err(), "{action:?} 拒绝");
    }

    // 旧派发的迟到退件：首寄已被本案绑定，按已退回隔离，不再返还资金。
    let late_return = insert_inbound(
        pool,
        "998012",
        Utc::now(),
        Some((
            "MoogleMail",
            format!("[RTS] Auction seller payout [{}]", resending.sale.seller_obligation),
        )),
        evidence(resending.sale.seller_obligation, AMOUNT, &[]),
    )
    .await;
    let consumed =
        store.consume_due_settlement_return(Utc::now()).await.expect("消费").expect("必须消费");
    assert_eq!((consumed.return_mail_id, consumed.delivery_id), (late_return, None));
    assert!(store.settle_due_return_take(Utc::now()).await.expect("收口").is_none());

    assert_eq!(allocation_state(pool, resending.allocation).await, in_flight, "新一轮资金不变");
    assert_eq!(ledger(pool).await, before, "没有新的支出、返还或来源");

    finish_resend(&store, &resending).await;
    let (spent, returned, ..) = ledger(pool).await;
    assert_eq!(spent - returned, AMOUNT, "净流出恰为 M");

    pool.close().await;
}

/// 人工 Sent 证据：不领取的原额仍在，正文未变；时间取当前尝试的 sending 分钟与终态时刻。
async fn manual_evidence(
    resending: &Resending,
    pool: &PgPool,
    delivery: Uuid,
) -> ManualSentEvidence {
    let (sending_at, completed_at): (Option<DateTime<Utc>>, Option<DateTime<Utc>>) =
        sqlx::query_as("SELECT sending_at, completed_at FROM public.deliveries WHERE id = $1")
            .bind(delivery)
            .fetch_one(pool)
            .await
            .expect("读取投递时窗");
    let sending_at = sending_at.expect("已进入 sending");
    let sent_minute =
        sending_at.with_second(0).and_then(|value| value.with_nanosecond(0)).expect("整分钟");
    ManualSentEvidence {
        schema_version: 1,
        world_id: "main".to_owned(),
        external_mail_id: "781002".to_owned(),
        sender_uid: UID.to_owned(),
        sender_name: BOT.to_owned(),
        recipient: SELLER.to_owned(),
        subject: resending.resend_expectation.subject.clone(),
        sent_minute,
        observed_at: completed_at.unwrap_or(sending_at),
        original_body_digest: resending.resend_expectation.body_digest,
        observed_body_digest: resending.resend_expectation.body_digest,
        remaining_credits: AMOUNT,
        cod_pending: 0,
        equipment_ids: Vec::new(),
        collected: false,
        receipt: String::new(),
    }
}

/// 补寄派发写出结果未知：履约转人工对账后，人工 Sent 准入按补寄的冻结身份与履约命令 ID 成立，
/// 确认把未知占用收为 spent，并以冻结证明结束本案。
#[tokio::test]
async fn postgres_return_resend_manual_sent_admission() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let resending = resending(&store, 7_630_021, "998021", false, Kind::Payout).await;
    DeliveryWorkStore::fail(
        &store,
        resending.resend_claim.clone(),
        DeliveryTransportError {
            failure_code: "write_unknown".to_owned(),
            safe_detail: "补寄写出结果未知".to_owned(),
            outcome_unknown: true,
            external_reference: None,
            result_metadata: json!({"operation": "send", "write_started": true}),
        },
        Utc::now(),
    )
    .await
    .expect("结果未知收口");
    assert_eq!(
        allocation_state(pool, resending.allocation).await,
        ("unknown".to_owned(), AMOUNT, Some(resending.resend))
    );
    // 对账阶段沿用激活时的 claim：投递结果未知时履约转人工对账。
    let claim = resending.resend_fulfillment.clone();
    assert_eq!((claim.dispatch_no, claim.delivery_id), (2, Some(resending.resend)));
    SettlementFulfillmentStore::manual_reconcile(
        &store,
        claim,
        "fulfillment_outcome_unknown".to_owned(),
    )
    .await
    .expect("履约转人工对账");

    let evidence = manual_evidence(&resending, pool, resending.resend).await;
    let preview = store
        .preview_sent_confirmation(resending.resend, evidence.clone())
        .await
        .expect("补寄预览");
    assert!(preview.eligible, "补寄派发可经人工 Sent 准入：{:?}", preview.blockers);
    store
        .confirm_sent(SentConfirmationCommand {
            command_id: Uuid::new_v4(),
            delivery_id: resending.resend,
            expected_fencing_version: preview.expected_fencing_version,
            actor: "returns-operator".to_owned(),
            operator_identity: None,
            reason: "manual Sent observation for resend".to_owned(),
            evidence,
            accept_collected_assets: false,
            accept_attempt_history: false,
        })
        .await
        .expect("人工 Sent 确认");
    assert_eq!(
        allocation_state(pool, resending.allocation).await,
        ("spent".to_owned(), AMOUNT, Some(resending.resend))
    );
    let done =
        store.complete_due_return_resend(Utc::now()).await.expect("收口补寄").expect("必须收口");
    assert_eq!(
        (done.return_mail_id, done.status),
        (resending.case, SettlementReturnCaseStatus::Resent)
    );
    let (spent, returned, transfers, ..) = ledger(pool).await;
    assert_eq!((spent - returned, transfers), (AMOUNT, 0), "净流出恰为 M，没有转移");

    pool.close().await;
}
