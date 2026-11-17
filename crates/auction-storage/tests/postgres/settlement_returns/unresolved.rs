//! 原 Send 结果未知后被系统退回的现金退件（PostgreSQL 18，运行账号，全部经真实端口推进）：
//! - 原派发占用过资金分配但发送结果未知（分配 `unknown`）时，案件区分于“历史没有分配”：领回补寄、
//!   领回代管与留箱都以 `send_unresolved` 拒绝，不终结履约、不关闭原发送的 Sent 恢复入口；
//! - 自动 Sent 恢复（冻结尝试）与人工 Sent 证明在案件接管后仍能把 unknown 收口为 spent，且不把
//!   已由案件接管的履约改回等待领取；
//! - 收口后案件照常领回代管（资金返还一次、金额仍为 M）并可从代管补寄。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use auction_application::core::{ApplicationError, RequestHash};
use auction_application::delivery::{
    DeliverySendExpectation, DeliverySendResult, DeliverySentConfirmationPort,
    DeliveryTakeExpectation, DeliveryTransportError, DeliveryWorkClaim, DeliveryWorkStore,
    ManualSentEvidence, SentConfirmationCommand,
};
use auction_application::mail::{MoogleMailboxEvidenceStore, sent_list_minute_start};
use auction_application::recovery::RecoveryWorkStore;
use auction_application::settlement::{
    SettlementDeliveryPreparation, SettlementFulfillmentStore, SettlementReturnAdminPort,
    SettlementReturnCommand, SettlementReturnError, SettlementReturnWorkStore,
};
use auction_domain::{
    CommandId, SettlementReturnAction, SettlementReturnBlocker, SettlementReturnCaseStatus,
};
use auction_storage::DatabaseStore;
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::flows::handle;
use super::funding::{
    BOT, Kind, SELLER, UID, allocation_state, cash_payload, dispatch, ledger, seed_funded_sale,
    send_through_cash_gate,
};
use super::sweep::{MailSession, seed_mail_session, take_into_sending};
use super::{evidence, insert_inbound};
use crate::recovery_returns::CodSale;
use crate::support;

/// 原派发写出结果未知、随后被系统退回的一件现金退件。
struct Unresolved {
    sale: CodSale,
    allocation: Uuid,
    original: Uuid,
    claim: DeliveryWorkClaim,
    expectation: Arc<DeliverySendExpectation>,
    case: Uuid,
    session: MailSession,
}

/// 首寄经真实端口进入 sending 后写出结果未知（分配 unknown、履约人工对账），再由系统退回并被
/// 退件消费方绑定到首寄。
async fn unresolved_return(
    store: &DatabaseStore,
    sequence: i64,
    mid: &str,
    strict: bool,
    kind: Kind,
) -> Unresolved {
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;
    let (sale, allocation) = seed_funded_sale(pool, sequence, strict, kind).await;
    let obligation = kind.obligation(&sale);
    let custody = (kind == Kind::Refund).then_some(&session);
    let original = Uuid::new_v4();
    let fulfillment = dispatch(
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
    let (claim, expectation) =
        send_through_cash_gate(store, &session, original, [0x51; 32], strict, kind).await;
    DeliveryWorkStore::fail(
        store,
        claim.clone(),
        DeliveryTransportError {
            failure_code: "write_unknown".to_owned(),
            safe_detail: "首寄写出结果未知".to_owned(),
            outcome_unknown: true,
            external_reference: None,
            result_metadata: json!({"operation": "send", "write_started": true}),
        },
        Utc::now(),
    )
    .await
    .expect("结果未知收口为人工核对");
    SettlementFulfillmentStore::manual_reconcile(
        store,
        fulfillment,
        "fulfillment_outcome_unknown".to_owned(),
    )
    .await
    .expect("履约转人工对账");
    assert_eq!(
        allocation_state(pool, allocation).await,
        ("unknown".to_owned(), kind.amount(), Some(original))
    );
    let case = insert_inbound(
        pool,
        mid,
        Utc::now(),
        Some(("MoogleMail", format!("[RTS] {} [{obligation}]", kind.subject()))),
        evidence(obligation, kind.amount(), &[]),
    )
    .await;
    let consumed =
        store.consume_due_settlement_return(Utc::now()).await.expect("消费").expect("必须消费");
    assert_eq!((consumed.return_mail_id, consumed.delivery_id), (case, Some(original)));
    Unresolved { sale, allocation, original, claim, expectation, case, session }
}

async fn try_handle(
    store: &DatabaseStore,
    case: Uuid,
    version: i64,
    action: SettlementReturnAction,
) -> Result<SettlementReturnCaseStatus, ApplicationError> {
    let command = SettlementReturnCommand::new(
        CommandId::from_uuid(Uuid::new_v4()),
        case,
        version,
        action,
        "admin-returns",
        Utc::now(),
    )
    .expect("构造命令");
    store.handle_settlement_return(command).await.map(|result| result.status)
}

async fn fulfillment(pool: &PgPool, obligation: Uuid) -> (String, Option<Uuid>, i32) {
    sqlx::query_as(
        "SELECT execution_state, delivery_id, dispatch_no
         FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(obligation)
    .fetch_one(pool)
    .await
    .expect("读取履约")
}

async fn delivery_status(pool: &PgPool, delivery: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
        .bind(delivery)
        .fetch_one(pool)
        .await
        .expect("读取投递")
}

/// 未核实期间：详情把三种处置都列为 `send_unresolved`，命令以同一错误拒绝且不改写任何事实。
async fn assert_blocked_until_send_proven(store: &DatabaseStore, returned: &Unresolved) {
    let pool = store.postgres_pool();
    let detail = store.settlement_return_detail(returned.case).await.expect("详情");
    assert!(detail.allowed_actions.is_empty(), "{:?}", detail.allowed_actions);
    for action in [
        SettlementReturnAction::ReclaimAndResend,
        SettlementReturnAction::ReclaimAndHold,
        SettlementReturnAction::CloseInbox,
    ] {
        assert!(
            detail.blockers.contains(&(action, SettlementReturnBlocker::SendUnresolved)),
            "{action:?}: {:?}",
            detail.blockers
        );
        let rejected = try_handle(store, returned.case, 1, action).await;
        assert!(
            matches!(
                rejected,
                Err(ApplicationError::SettlementReturn(SettlementReturnError::SendUnresolved))
            ),
            "{action:?}: {rejected:?}"
        );
    }
    let (state, delivery, dispatch_no) =
        fulfillment(pool, returned.claim.origin_id.parse().expect("义务 ID")).await;
    assert_eq!(
        (state.as_str(), delivery, dispatch_no),
        ("manual_reconcile", Some(returned.original), 1)
    );
    assert_eq!(delivery_status(pool, returned.original).await, "manual_reconcile");
    assert_eq!(
        count_rows(pool, "SELECT count(*) FROM public.settlement_return_actions").await,
        0,
        "拒绝不建立 Take"
    );
}

async fn count_rows(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned())).fetch_one(pool).await.expect("计数")
}

/// Sent 收口后：领回代管照常返还资金一次（金额仍为 M），再从代管补寄到第 2 次派发。
async fn hold_then_resend(store: &DatabaseStore, returned: &Unresolved, mid: &str, kind: Kind) {
    let pool = store.postgres_pool();
    let amount = kind.amount();
    let obligation = kind.obligation(&returned.sale);
    assert_eq!(
        handle(store, returned.case, 1, SettlementReturnAction::ReclaimAndHold).await,
        SettlementReturnCaseStatus::TakePending
    );
    store.activate_due_return_take(Utc::now()).await.expect("激活").expect("必须激活");
    let take: Uuid = sqlx::query_scalar(
        "SELECT delivery_id FROM public.settlement_return_actions WHERE return_mail_id = $1",
    )
    .bind(returned.case)
    .fetch_one(pool)
    .await
    .expect("读取 Take 投递");
    let expectation = DeliveryTakeExpectation {
        world_id: "main".to_owned(),
        external_mail_id: mid.to_owned(),
        credential: Some(returned.session.binding()),
    };
    let take_claim = take_into_sending(store, take, &expectation).await;
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
    assert_eq!(settled.status, SettlementReturnCaseStatus::Held);
    assert_eq!(
        allocation_state(pool, returned.allocation).await,
        ("available".to_owned(), amount, None),
        "原分配 spent → available 只返还一次"
    );
    let (spent, refunded, transfers, sources, returns) = ledger(pool).await;
    assert_eq!((spent, refunded, transfers, sources, returns), (amount, amount, 0, 1, 1));
    assert_eq!(
        handle(
            store,
            returned.case,
            settled.case_version,
            SettlementReturnAction::ResendFromCustody
        )
        .await,
        SettlementReturnCaseStatus::ResendPending
    );
    let (state, delivery, dispatch_no) = fulfillment(pool, obligation).await;
    assert_eq!((state.as_str(), delivery, dispatch_no), ("pending", None, 2));
}

/// 原 Send 未知 → 系统退回 → 处置被拒 → 自动 Sent 恢复（冻结尝试）收口 unknown 且不改回等待领取
/// → 领回代管 → 从代管补寄；货款（strict_cash）与退款（record_only）两种现金义务。
#[tokio::test]
async fn postgres_unknown_original_send_requires_auto_sent_recovery_before_disposal() {
    for (kind, strict, sequence, mid) in
        [(Kind::Payout, true, 7_640_001, "996001"), (Kind::Refund, false, 7_640_011, "996011")]
    {
        let database = support::TestDatabase::create().await.expect("克隆模板库");
        let store = database.store().await.expect("以运行账号连接");
        let pool = store.postgres_pool();
        let returned = unresolved_return(&store, sequence, mid, strict, kind).await;
        assert_blocked_until_send_proven(&store, &returned).await;

        let (sending_at, fence): (DateTime<Utc>, i64) = sqlx::query_as(
            "SELECT sending_at, fencing_version FROM public.deliveries WHERE id = $1",
        )
        .bind(returned.original)
        .fetch_one(pool)
        .await
        .expect("读取首寄时窗");
        let sent_mid = format!("{}", 781_000 + sequence % 1_000);
        let recovered = store
            .bind_recovered_send(
                returned.original,
                fence,
                sent_mid.clone(),
                json!({
                    "observed_at": Utc::now(),
                    "sent_at": sent_list_minute_start(sending_at),
                    "operation": "send",
                    "proof": "sent_recovered_frozen_v1",
                    "external_reference_proven": true,
                    "sent_recovered": true,
                    "recovered_mid": sent_mid,
                    "collected": false,
                    "cod_pending": 0,
                    "recipient": returned.expectation.recipient,
                    "template": returned.claim.payload["template"],
                    "sending_fencing_version": returned.claim.fencing_version,
                    "observation_credential": returned.session.binding(),
                }),
                Some(&returned.expectation),
            )
            .await
            .expect("自动 Sent 恢复必须确定提交");
        assert!(recovered, "{kind:?}: 案件接管后自动 Sent 恢复仍能收口原发送");
        assert_eq!(delivery_status(pool, returned.original).await, "succeeded");
        assert_eq!(
            allocation_state(pool, returned.allocation).await,
            ("spent".to_owned(), kind.amount(), Some(returned.original)),
            "{kind:?}: unknown 收口为 spent"
        );
        let (state, delivery, _) = fulfillment(pool, kind.obligation(&returned.sale)).await;
        assert_eq!(
            (state.as_str(), delivery),
            ("manual_reconcile", Some(returned.original)),
            "{kind:?}: 已被退件绑定的履约不改回等待领取"
        );
        hold_then_resend(&store, &returned, mid, kind).await;
        pool.close().await;
    }
}

/// 同一序列经人工 Sent 证明收口：准入在案件打开期间成立，确认把 unknown 收为 spent，履约仍归
/// 案件所有，随后照常领回代管并补寄。
#[tokio::test]
async fn postgres_unknown_original_send_manual_sent_keeps_case_ownership() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let kind = Kind::Payout;
    let returned = unresolved_return(&store, 7_640_021, "996021", false, kind).await;
    assert_blocked_until_send_proven(&store, &returned).await;

    let (sending_at, completed_at): (DateTime<Utc>, DateTime<Utc>) =
        sqlx::query_as("SELECT sending_at, completed_at FROM public.deliveries WHERE id = $1")
            .bind(returned.original)
            .fetch_one(pool)
            .await
            .expect("读取首寄时窗");
    let evidence = ManualSentEvidence {
        schema_version: 1,
        world_id: "main".to_owned(),
        external_mail_id: "781021".to_owned(),
        sender_uid: UID.to_owned(),
        sender_name: BOT.to_owned(),
        recipient: SELLER.to_owned(),
        subject: returned.expectation.subject.clone(),
        sent_minute: sent_list_minute_start(sending_at),
        observed_at: completed_at,
        original_body_digest: returned.expectation.body_digest,
        observed_body_digest: returned.expectation.body_digest,
        remaining_credits: kind.amount(),
        cod_pending: 0,
        equipment_ids: Vec::new(),
        collected: false,
        receipt: String::new(),
    };
    let preview = store
        .preview_sent_confirmation(returned.original, evidence.clone())
        .await
        .expect("人工 Sent 预览");
    assert!(preview.eligible, "案件打开期间人工 Sent 准入成立：{:?}", preview.blockers);
    store
        .confirm_sent(SentConfirmationCommand {
            command_id: Uuid::new_v4(),
            delivery_id: returned.original,
            expected_fencing_version: preview.expected_fencing_version,
            actor: "returns-operator".to_owned(),
            operator_identity: None,
            reason: "manual Sent observation for the returned original".to_owned(),
            evidence,
            accept_collected_assets: false,
            accept_attempt_history: false,
        })
        .await
        .expect("人工 Sent 确认");
    assert_eq!(delivery_status(pool, returned.original).await, "succeeded");
    assert_eq!(
        allocation_state(pool, returned.allocation).await,
        ("spent".to_owned(), kind.amount(), Some(returned.original))
    );
    let (state, delivery, _) = fulfillment(pool, kind.obligation(&returned.sale)).await;
    assert_eq!(
        (state.as_str(), delivery),
        ("manual_reconcile", Some(returned.original)),
        "人工 Sent 证明不把已被退件绑定的履约改回等待领取"
    );
    hold_then_resend(&store, &returned, "996021", kind).await;
    pool.close().await;
}
