//! 结算退件 Take 结果未知（`take_unknown`）的机器可验证出口，PostgreSQL 18 用例，以运行账号执行：
//! - Take 投递经投递端口按 Worker 的真实顺序推进（领取 → 以原会话冻结 Take 身份进入 sending →
//!   结果未知收口为人工核对），案件经正常收口停在 `take_unknown`；
//! - 完成证明：邮箱巡检在原 Take 会话里由 integrations 核对过领取回执（`take_receipt_proven`），
//!   同一事务按正常领回证明记账：领回事实与资金返还各一次（金额仍为 M），再按计划授权补寄；
//!   重放不再改写；只凭“附件已为空”（旧口径 `attachments_absent`）不再收口；
//! - 仍在未领：投递重新排队、动作与案件回到 Take 待执行，由投递端口再执行一次 Take 后正常收口，
//!   领回与返还仍各一次；
//! - 无法判定的完成证明绑定：邮件整体缺失、预期资产不符、换凭据版本、同邮件存在其他处置意图，
//!   以及正常收口尚未接手（`take_pending`）时，都不改写任何事实；邮箱维护在原 Take 会话内的观察
//!   另以 `take_observed` 事件留证，换账号或换凭据不写入任何事实。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration as StdDuration;

use auction_application::delivery::{
    DeliveryClaimExclusions, DeliveryFenceOutcome, DeliveryPreparedExpectation,
    DeliverySendCredentialBinding, DeliverySendResult, DeliveryTakeExpectation,
    DeliveryTransportError, DeliveryWorkClaim, DeliveryWorkStore,
};
use auction_application::mail::MoogleMailboxEvidenceStore;
use auction_application::settlement::SettlementReturnWorkStore;
use auction_domain::{SettlementReturnAction, SettlementReturnCaseStatus};
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::flows::{PAYOUT, Returned, count, handle, returned_payout, seed_spent_payout};
use crate::support;

/// 合成 Bot 邮件会话：active mail_cookie 凭据（main 范围）与当前运行代际。
pub(super) struct MailSession {
    pub(super) credential: Uuid,
    pub(super) generation: i64,
}

impl MailSession {
    pub(super) fn binding(&self) -> DeliverySendCredentialBinding {
        DeliverySendCredentialBinding {
            credential_id: self.credential,
            credential_version: 1,
            runtime_generation: self.generation,
        }
    }
}

pub(super) async fn seed_mail_session(pool: &PgPool) -> MailSession {
    let credential = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.credential_keys
            (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id, fingerprint,
             status, state_version, player_name)
         VALUES ($1, 'mail_cookie', 'mooglemail:main', 'returns-sweep', $2, $3, 'test-key',
                 $4, 'active', 1, 'Returns Bot')",
    )
    .bind(credential)
    .bind(vec![1_u8])
    .bind(vec![0_u8; 12])
    .bind(format!("returns-sweep-{credential}"))
    .execute(pool)
    .await
    .expect("写入合成邮件凭据");
    sqlx::query(
        "INSERT INTO public.mail_runtime_generation (scope, generation, activated_at)
         VALUES ('mail_cookie', 11, now())
         ON CONFLICT (scope) DO NOTHING",
    )
    .execute(pool)
    .await
    .expect("写入邮件运行代际");
    let generation = sqlx::query_scalar(
        "SELECT generation FROM public.mail_runtime_generation WHERE scope = 'mail_cookie'",
    )
    .fetch_one(pool)
    .await
    .expect("读取运行代际");
    MailSession { credential, generation }
}

/// 一件停在结果未知的现金退件 Take。
pub(super) struct UnknownTake {
    pub(super) returned: Returned,
    pub(super) allocation: Uuid,
    pub(super) mid: String,
    pub(super) take_delivery: Uuid,
    pub(super) fencing_version: i64,
    pub(super) expectation: DeliveryTakeExpectation,
}

/// 以投递端口领取唯一到期的 Take 投递，并以原会话冻结的 Take 身份进入 sending。
pub(super) async fn take_into_sending(
    store: &DatabaseStore,
    take_delivery: Uuid,
    expectation: &DeliveryTakeExpectation,
) -> DeliveryWorkClaim {
    let now = Timestamp::now();
    let grace = StdDuration::from_secs(60);
    DeliveryWorkStore::begin_claim_cycle(store, now, grace).await.expect("领取前清理");
    let claim = DeliveryWorkStore::claim_due(
        store,
        "returns-sweep-delivery".to_owned(),
        now,
        now + Duration::minutes(2),
        grace,
        DeliveryClaimExclusions::default(),
        10,
    )
    .await
    .expect("领取投递")
    .into_iter()
    .find(|claim| claim.delivery_id == take_delivery)
    .expect("领取到 Take 投递");
    let prepared = DeliveryPreparedExpectation::Take(Arc::new(expectation.clone()));
    let outcome = store
        .start_sending_authorized(claim.clone(), Timestamp::now(), None, Some(&prepared))
        .await
        .expect("Take 发送围栏必须确定完成");
    assert!(matches!(outcome, DeliveryFenceOutcome::Sending), "原会话 Take 身份进入 sending");
    claim
}

/// 建立一件现金退件，经 Admin 领回命令、Take 激活与投递端口推进到 Take 结果未知；`settle` 为真时
/// 再由正常收口把案件转入 `take_unknown`。
pub(super) async fn unknown_take(
    store: &DatabaseStore,
    session: &MailSession,
    sequence: i64,
    mid: &str,
    action: SettlementReturnAction,
    settle: bool,
) -> UnknownTake {
    let returned = returned_payout(store, sequence, mid).await;
    unknown_take_of(store, session, returned, mid, action, settle).await
}

/// 同 [`unknown_take`]，退件案件由调用方建立（例如证据带系统后缀事实）。
pub(super) async fn unknown_take_of(
    store: &DatabaseStore,
    session: &MailSession,
    returned: Returned,
    mid: &str,
    action: SettlementReturnAction,
    settle: bool,
) -> UnknownTake {
    let pool = store.postgres_pool();
    let allocation = seed_spent_payout(pool, &returned).await;
    // 货款的前置付款已满足：补寄的履约才可被领取。
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, dispatch_world, delivery_operation, completed_at)
         VALUES ($1, 'satisfied', 'main', 'send', now())",
    )
    .bind(returned.sale.payment_obligation)
    .execute(pool)
    .await
    .expect("前置付款已满足");
    assert_eq!(
        handle(store, returned.case, 1, action).await,
        SettlementReturnCaseStatus::TakePending
    );
    store
        .activate_due_return_take(Timestamp::now(), super::take_worlds())
        .await
        .expect("激活")
        .expect("必须激活");
    let take_delivery: Uuid = sqlx::query_scalar(
        "SELECT delivery_id FROM public.settlement_return_actions WHERE return_mail_id = $1",
    )
    .bind(returned.case)
    .fetch_one(pool)
    .await
    .expect("读取 Take 投递");
    let expectation = DeliveryTakeExpectation {
        world_id: "main".to_owned(),
        external_mail_id: mid.to_owned(),
        credential: Some(session.binding()),
    };
    let claim = take_into_sending(store, take_delivery, &expectation).await;
    let fencing_version = claim.fencing_version;
    DeliveryWorkStore::fail(
        store,
        claim,
        DeliveryTransportError {
            failure_code: "take_receipt_unproven".to_owned(),
            safe_detail: "Take 回执无法证明".to_owned(),
            outcome_unknown: true,
            external_reference: None,
            result_metadata: json!({"operation": "take", "write_started": true}),
        },
        Timestamp::now(),
    )
    .await
    .expect("结果未知收口为人工核对");
    if settle {
        let progress =
            store.settle_due_return_take(Timestamp::now()).await.expect("收口").expect("必须收口");
        assert_eq!(
            (progress.return_mail_id, progress.status),
            (returned.case, SettlementReturnCaseStatus::TakeUnknown)
        );
    }
    UnknownTake {
        returned,
        allocation,
        mid: mid.to_owned(),
        take_delivery,
        fencing_version,
        expectation,
    }
}

/// 巡检在原会话观察到的完成证明（与应用层写出的形状一致，预期资产取自 Take 载荷）。
pub(super) fn completed_observation(mid: &str, reason: &str, credits: i64) -> Value {
    json!({
        "observed_at": Timestamp::now(),
        "operation": "take",
        "proof": "action_recovered_inbox_v1",
        "external_reference_proven": true,
        "recovered_mid": mid,
        "reason": reason,
        "attachment_count": 0,
        "expected_credits": credits,
        "expected_cod": 0,
        "expected_attachment_ids": [],
    })
}

/// 案件、动作、Take 投递与资金的一组可比较快照。
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Facts {
    pub(super) case_status: String,
    pub(super) case_version: i64,
    pub(super) action_state: String,
    pub(super) delivery_status: String,
    pub(super) takes: i64,
    pub(super) funding_returns: i64,
    pub(super) allocation_state: String,
    pub(super) allocation_amount: i64,
    pub(super) return_entries: i64,
}

pub(super) async fn facts(pool: &PgPool, take: &UnknownTake) -> Facts {
    let case = take.returned.case;
    let (case_status, case_version): (String, i64) = sqlx::query_as(
        "SELECT status, case_version FROM public.settlement_return_cases WHERE return_mail_id = $1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取案件");
    let action_state: String = sqlx::query_scalar(
        "SELECT execution_state FROM public.settlement_return_actions WHERE return_mail_id = $1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取动作");
    let delivery_status: String =
        sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
            .bind(take.take_delivery)
            .fetch_one(pool)
            .await
            .expect("读取 Take 投递");
    let (allocation_state, allocation_amount): (String, i64) = sqlx::query_as(
        "SELECT state, amount FROM public.settlement_funding_allocations WHERE id = $1",
    )
    .bind(take.allocation)
    .fetch_one(pool)
    .await
    .expect("读取分配");
    Facts {
        case_status,
        case_version,
        action_state,
        delivery_status,
        takes: count(
            pool,
            "SELECT count(*) FROM public.settlement_return_takes WHERE return_mail_id = $1",
            case,
        )
        .await,
        funding_returns: count(
            pool,
            "SELECT count(*) FROM public.settlement_funding_returns WHERE return_mail_id = $1",
            case,
        )
        .await,
        allocation_state,
        allocation_amount,
        return_entries: count(
            pool,
            "SELECT count(*) FROM public.settlement_funding_entries
             WHERE kind = 'return' AND credit_id = $1",
            take.allocation,
        )
        .await,
    }
}

async fn last_event(pool: &PgPool, case: Uuid) -> (String, Value) {
    sqlx::query_as(
        "SELECT event_kind, details FROM public.settlement_return_events
         WHERE return_mail_id = $1 ORDER BY case_version DESC LIMIT 1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取最新事件")
}

/// 完成证明：附件已全部取走的观察在同一事务内按正常领回证明记账并授权补寄；返还只发生一次、
/// 金额仍为 M；重放与再次收口都不再改写。
#[tokio::test]
async fn postgres_return_take_unknown_completed_observation_settles_like_take_proof() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;
    let take = unknown_take(
        &store,
        &session,
        7_610_001,
        "995001",
        SettlementReturnAction::ReclaimAndResend,
        true,
    )
    .await;
    let before = facts(pool, &take).await;
    assert_eq!(
        (
            before.case_status.as_str(),
            before.action_state.as_str(),
            before.allocation_state.as_str()
        ),
        ("take_unknown", "manual_reconcile", "spent")
    );

    // 巡检候选：结算退件 Take 经原会话冻结身份成为可恢复候选，资产取自 Take 载荷。
    let candidates = MoogleMailboxEvidenceStore::unproven_sends(&store, "main", 200)
        .await
        .expect("读取巡检候选");
    let candidate = candidates
        .iter()
        .find(|slot| slot.delivery_id == take.take_delivery)
        .and_then(|slot| slot.send.as_ref())
        .expect("结算退件 Take 是可恢复候选");
    assert_eq!(candidate.origin_kind, "settlement_return_action");
    assert_eq!(candidate.recipient, "MoogleMail");
    assert_eq!((candidate.credits, candidate.cod), (PAYOUT, 0));
    assert_eq!(candidate.inbound_mail_id, take.mid);
    assert_eq!(candidate.take_expectation.as_ref(), Some(&take.expectation));

    let recovered = store
        .bind_recovered_action(
            take.take_delivery,
            take.fencing_version,
            take.mid.clone(),
            completed_observation(&take.mid, "take_receipt_proven", PAYOUT),
            &take.expectation,
            &session.binding(),
        )
        .await
        .expect("完成证明必须确定提交");
    assert!(recovered, "完成证明推进案件");
    let after = facts(pool, &take).await;
    assert_eq!(after.case_status, "resend_pending", "按计划授权补寄");
    assert_eq!(after.action_state, "satisfied");
    assert_eq!(after.delivery_status, "succeeded");
    assert_eq!((after.takes, after.funding_returns, after.return_entries), (1, 1, 1));
    assert_eq!(
        (after.allocation_state.as_str(), after.allocation_amount),
        ("available", PAYOUT),
        "金额是 M 不是 2M"
    );
    let (kind, details) = last_event(pool, take.returned.case).await;
    assert_eq!(kind, "take_proven");
    assert_eq!(details["proof"], "action_recovered_inbox_v1");
    assert_eq!(details["next"], "resend");
    let (reference, failure): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT d.external_reference, a.failure_code FROM public.settlement_return_actions a
         JOIN public.deliveries d ON d.id = a.delivery_id WHERE a.return_mail_id = $1",
    )
    .bind(take.returned.case)
    .fetch_one(pool)
    .await
    .expect("读取 Take 投递引用");
    assert_eq!(reference, Some(format!("mooglemail:main:take:{}", take.mid)));
    assert_eq!(failure, None);

    // 同一观察重放：投递已不在人工核对，不再改写；正常收口也没有到期项。
    assert!(
        !store
            .bind_recovered_action(
                take.take_delivery,
                take.fencing_version,
                take.mid.clone(),
                completed_observation(&take.mid, "take_receipt_proven", PAYOUT),
                &take.expectation,
                &session.binding(),
            )
            .await
            .expect("重放不报错")
    );
    assert!(store.settle_due_return_take(Timestamp::now()).await.expect("排空").is_none());
    let replayed = facts(pool, &take).await;
    assert_eq!(replayed, after, "重放不改写任何事实");

    pool.close().await;
}

/// 仍在未领：重新排队后动作与案件回到 Take 待执行，投递端口再执行一次 Take 并以成功页证明正常
/// 收口；领回事实与返还仍各一次，案件按计划代管。
#[tokio::test]
async fn postgres_return_take_unknown_still_present_requeues_without_double_take() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;
    let take = unknown_take(
        &store,
        &session,
        7_610_011,
        "995011",
        SettlementReturnAction::ReclaimAndHold,
        true,
    )
    .await;

    assert!(
        store
            .requeue_unproven(
                take.take_delivery,
                take.fencing_version,
                1,
                "take_assets_still_present",
                &take.expectation,
                &session.binding(),
            )
            .await
            .expect("重新排队必须确定提交"),
        "原资产仍在未领时重新排队"
    );
    let requeued = facts(pool, &take).await;
    assert_eq!(
        (
            requeued.case_status.as_str(),
            requeued.action_state.as_str(),
            requeued.delivery_status.as_str()
        ),
        ("take_pending", "enqueued", "pending")
    );
    assert_eq!((requeued.takes, requeued.funding_returns), (0, 0), "重新排队不记领回");
    assert_eq!(requeued.allocation_state, "spent", "重新排队不动资金");
    let (kind, details) = last_event(pool, take.returned.case).await;
    assert_eq!(kind, "take_activated");
    assert_eq!(
        (details["requeued"].clone(), details["sweep_retries"].clone()),
        (json!(true), json!(1))
    );
    let failure: Option<String> = sqlx::query_scalar(
        "SELECT failure_code FROM public.settlement_return_actions WHERE return_mail_id = $1",
    )
    .bind(take.returned.case)
    .fetch_one(pool)
    .await
    .expect("读取动作失败码");
    assert_eq!(failure, None);

    // 投递端口再执行一次 Take：执行前预检仍要求可领取且资产一致，成功页回执按正常证明收口。
    let claim = take_into_sending(&store, take.take_delivery, &take.expectation).await;
    assert!(claim.fencing_version > take.fencing_version, "再执行使用新的 fence");
    store
        .succeed(
            claim,
            DeliverySendResult {
                external_reference: format!("mooglemail:main:take:{}", take.mid),
                result_metadata: json!({
                    "operation": "take",
                    "external_mail_id": take.mid,
                    "proof": "action_removed_or_success_page",
                    "expected_credits": PAYOUT,
                    "expected_cod": 0,
                    "expected_attachment_ids": [],
                }),
            },
            Timestamp::now(),
        )
        .await
        .expect("Take 成功收口");
    let settled =
        store.settle_due_return_take(Timestamp::now()).await.expect("收口").expect("必须收口");
    assert_eq!(settled.status, SettlementReturnCaseStatus::Held);
    let done = facts(pool, &take).await;
    assert_eq!((done.takes, done.funding_returns, done.return_entries), (1, 1, 1));
    assert_eq!((done.allocation_state.as_str(), done.allocation_amount), ("available", PAYOUT));
    // 已重新执行并成功的投递不再是巡检候选，重复的重新排队不改写。
    assert!(
        !store
            .requeue_unproven(
                take.take_delivery,
                take.fencing_version,
                2,
                "take_assets_still_present",
                &take.expectation,
                &session.binding(),
            )
            .await
            .expect("不报错")
    );
    assert_eq!(facts(pool, &take).await, done);

    pool.close().await;
}

/// 无法判定的观察与不成立的前提都不改写事实：正常收口尚未接手（`take_pending`）、邮件整体缺失、
/// 预期资产不符、换凭据版本、同邮件存在其他处置意图。
#[tokio::test]
async fn postgres_return_take_unknown_undecidable_observations_change_nothing() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;
    let take = unknown_take(
        &store,
        &session,
        7_610_021,
        "995021",
        SettlementReturnAction::ReclaimAndHold,
        false,
    )
    .await;
    let bind = |metadata: Value, observation: DeliverySendCredentialBinding| {
        let store = &store;
        let take = &take;
        async move {
            store
                .bind_recovered_action(
                    take.take_delivery,
                    take.fencing_version,
                    take.mid.clone(),
                    metadata,
                    &take.expectation,
                    &observation,
                )
                .await
        }
    };

    // 投递已是人工核对、但正常收口尚未把案件转入结果未知：巡检不抢先改写 take_pending。
    let pending = facts(pool, &take).await;
    assert_eq!(pending.case_status, "take_pending");
    assert!(
        !bind(completed_observation(&take.mid, "take_receipt_proven", PAYOUT), session.binding())
            .await
            .expect("不报错")
    );
    assert_eq!(facts(pool, &take).await, pending);
    store.settle_due_return_take(Timestamp::now()).await.expect("收口").expect("必须收口");
    let unknown = facts(pool, &take).await;
    assert_eq!(unknown.case_status, "take_unknown");

    // 邮件整体缺失不能证明资产入账；只凭“附件已为空”（没有 integrations 的领取回执核对）同样
    // 不再是结算退件的完成证明。
    for weak in ["inbox_mail_absent", "attachments_absent"] {
        assert!(
            !bind(completed_observation(&take.mid, weak, PAYOUT), session.binding())
                .await
                .expect("不报错"),
            "{weak}"
        );
    }
    // 预期资产与动作冻结事实不符：锁内比对失败，整事务回滚（投递也不收口）。
    assert!(
        !bind(
            completed_observation(&take.mid, "take_receipt_proven", PAYOUT + 1),
            session.binding()
        )
        .await
        .expect("不报错")
    );
    // 换凭据版本的观察不是原 Take 会话。
    let mut other_version = session.binding();
    other_version.credential_version = 2;
    assert!(
        bind(completed_observation(&take.mid, "take_receipt_proven", PAYOUT), other_version)
            .await
            .is_err()
    );
    assert_eq!(facts(pool, &take).await, unknown, "无法判定的完成证明绑定不改写任何事实");

    // 同一邮件已有入站流程的处置意图（例如旧程序写入）：不重新执行 Take，保持结果未知。
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             fencing_version, attempt_count, completed_at, failure_code)
         VALUES ($1, $2, decode(repeat('5c', 32), 'hex'), 'mail_intake', $3, 'MoogleMail',
                 jsonb_build_object('schema_version', 1, 'operation', 'return', 'world', 'main',
                     'external_mail_id', $4::text),
                 'failed', 1, 1, now(), 'remote_rejected')",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4().to_string())
    .bind(&take.mid)
    .execute(pool)
    .await
    .expect("写入同邮件的其他处置意图");
    assert!(
        !store
            .requeue_unproven(
                take.take_delivery,
                take.fencing_version,
                1,
                "take_assets_still_present",
                &take.expectation,
                &session.binding(),
            )
            .await
            .expect("不报错"),
        "同邮件互斥成立时不重新执行 Take"
    );
    assert_eq!(facts(pool, &take).await, unknown);

    pool.close().await;
}
