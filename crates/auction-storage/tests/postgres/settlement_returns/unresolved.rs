//! 原 Send 结果未知后被系统退回的现金退件（PostgreSQL 18，运行账号，全部经真实端口推进）：
//! - 原派发占用过资金分配但发送结果未知（分配 `unknown`）时，案件区分于“历史没有分配”：领回补寄、
//!   领回代管与留箱都以 `send_unresolved` 拒绝，不终结履约、不关闭原发送的 Sent 恢复入口；
//! - 自动 Sent 恢复（冻结尝试）与人工 Sent 证明在案件接管后仍能把 unknown 收口为 spent，且不把
//!   已由案件接管的履约改回等待领取；
//! - 收口后案件照常领回代管（资金返还一次、金额仍为 M）并可从代管补寄。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use auction_application::core::{ApplicationError, ApplicationFuture, Clock, RequestHash};
use auction_application::delivery::{
    DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminError, DeliveryAdminPort,
    DeliverySendExpectation, DeliverySendResult, DeliverySentConfirmationPort,
    DeliveryTakeExpectation, DeliveryTransportError, DeliveryWorkClaim, DeliveryWorkStore,
    ManualSentEvidence, SentConfirmationCommand,
};
use auction_application::mail::{
    MailboxMaintenanceReport, MailboxMaintenanceService, MoogleMailboxComposeState,
    MoogleMailboxDiscard, MoogleMailboxEvidenceStore, MoogleMailboxInboxObservation,
    MoogleMailboxMaintenance, MoogleMailboxNoticeExpectation, MoogleMailboxReturnTakeIdentity,
    MoogleMailboxReturnTakeObservation, MoogleMailboxSentEntries, MoogleMailboxSentEntry,
    MoogleMailboxSentState, MoogleMailboxUnprovenSend, sent_list_minute_start,
};
use auction_application::operations::ExternalWriteActivity;
use auction_application::recovery::RecoveryWorkStore;
use auction_application::settlement::{
    SettlementDeliveryPreparation, SettlementFulfillmentStore, SettlementReturnAdminPort,
    SettlementReturnCommand, SettlementReturnError, SettlementReturnWorkStore,
};
use auction_domain::{
    CommandId, SettlementReturnAction, SettlementReturnBlocker, SettlementReturnCaseStatus,
};
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::flows::handle;
use super::funding::{
    BOT, Kind, SELLER, UID, allocation_state, cash_payload, dispatch, ledger, seed_funded_sale,
    send_through_cash_gate,
};
use super::sweep::{MailSession, seed_mail_session, take_into_sending};
use super::{evidence, insert_inbound};
use crate::recovery_returns::{CodSale, insert_sale};
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
        Timestamp::now(),
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
        Timestamp::now(),
        Some(("MoogleMail", format!("[RTS] {} [{obligation}]", kind.subject()))),
        evidence(obligation, kind.amount(), &[]),
    )
    .await;
    let consumed = store
        .consume_due_settlement_return(Timestamp::now())
        .await
        .expect("消费")
        .expect("必须消费");
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
        Timestamp::now(),
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
    store
        .activate_due_return_take(Timestamp::now(), super::take_worlds())
        .await
        .expect("激活")
        .expect("必须激活");
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
            Timestamp::now(),
        )
        .await
        .expect("Take 成功");
    let settled =
        store.settle_due_return_take(Timestamp::now()).await.expect("收口").expect("必须收口");
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

        let (sending_at, fence): (Timestamp, i64) = sqlx::query_as(
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
                    "observed_at": Timestamp::now(),
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

    let (sending_at, completed_at, fence): (Timestamp, Timestamp, i64) = sqlx::query_as(
        "SELECT sending_at, completed_at, fencing_version FROM public.deliveries WHERE id = $1",
    )
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
    store
        .record_sent_sighting(
            returned.original,
            fence,
            &evidence.external_mail_id,
            json!({
                "mid": evidence.external_mail_id,
                "observed_at": evidence.observed_at,
                "collected": false,
                "remaining_equipment_count": 0,
                "remaining_equipment_ids": [],
            }),
        )
        .await
        .expect("Worker 记录货款邮件实见空装备清单");
    let preview = store
        .preview_sent_confirmation(returned.original, evidence.clone(), None)
        .await
        .expect("人工 Sent 预览");
    assert!(preview.eligible, "案件打开期间人工 Sent 准入成立：{:?}", preview.blockers);
    store
        .confirm_sent(
            SentConfirmationCommand {
                command_id: Uuid::new_v4(),
                delivery_id: returned.original,
                expected_fencing_version: preview.expected_fencing_version,
                actor: "returns-operator".to_owned(),
                operator_identity: None,
                reason: "manual Sent observation for the returned original".to_owned(),
                evidence,
                accept_collected_assets: false,
                accept_attempt_history: false,
            },
            None,
        )
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

async fn seed_manual_sent_equipment(
    pool: &PgPool,
) -> (SentConfirmationCommand, DeliverySendExpectation) {
    let sent_minute = sent_list_minute_start(Timestamp::now() - Duration::minutes(2));
    let sending_at = sent_minute + Duration::seconds(10);
    let completed_at = sent_minute + Duration::seconds(20);
    let observed_at = sent_minute + Duration::seconds(30);
    let sale = insert_sale(pool, 7_640_022, sent_minute, 1).await;
    let session = seed_mail_session(pool).await;
    sqlx::query(
        "INSERT INTO public.settlement_funding_accounts (uid, status, accounting_mode)
         VALUES ($1, 'ready', 'record_only')",
    )
    .bind(UID)
    .execute(pool)
    .await
    .expect("写入资金账户");
    sqlx::query(
        "INSERT INTO public.lot_settlements
            (lot_id, end_price, listed_price, buyer_amount, seller_amount, tax_amount,
             compensation_amount, hold_ppm, adjustment_ppm)
         VALUES ($1, 100, 90, 100, 90, 10, 0, 0, 0)",
    )
    .bind(sale.lot_id)
    .execute(pool)
    .await
    .expect("写入结算明细");

    let delivery_id = Uuid::new_v4();
    let prepare_id = Uuid::new_v4();
    let activate_id = Uuid::new_v4();
    let fence = 2;
    let item_name = "Recovery return equipment";
    let expectation = DeliverySendExpectation {
        world_id: "main".to_owned(),
        recipient: "Equipment Buyer".to_owned(),
        subject: format!("Auction equipment delivery [{}]", sale.payment_obligation),
        body_digest: [0x51; 32],
        credits: 0,
        cod: 100,
        equipment_ids: vec![sale.external_equipment_id],
        credential: Some(session.binding()),
    };
    sqlx::query(
        r#"INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             fencing_version, attempt_count, claimed_at, sending_at, completed_at, failure_code,
             result_metadata, activation_command_id, activation_request_digest, activated_at,
             created_at, updated_at)
         VALUES ($1, $2, decode(repeat('61', 32), 'hex'), 'settlement', $3, $4, $5,
                 'manual_reconcile', $6, 1, $7, $7, $8, 'write_unknown',
                 '{"operation":"send","write_started":true}', $9,
                 decode(repeat('62', 32), 'hex'), $10, $10, $8)"#,
    )
    .bind(delivery_id)
    .bind(prepare_id)
    .bind(sale.payment_obligation.to_string())
    .bind(&expectation.recipient)
    .bind(json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "template": "settlement_winner_equipment_cod",
        "variables": {
            "reference": sale.payment_obligation,
            "amount": "100",
            "item_name": item_name,
        },
        "credits": 0,
        "cod": 100,
        "equipment_secret_refs": [{"kind": "auction_equipment_key", "id": sale.equipment_id}],
    }))
    .bind(fence)
    .bind(sending_at)
    .bind(completed_at)
    .bind(activate_id)
    .bind(sent_minute)
    .execute(pool)
    .await
    .expect("写入结果未知的装备投递");
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, dispatch_world, delivery_operation, delivery_id,
             prepare_command_id, prepare_request_digest, activate_command_id, activate_request_digest,
             completed_at, created_at, updated_at)
         SELECT $2, 'manual_reconcile', 'main', 'send', id, command_id, request_digest,
                activation_command_id, activation_request_digest, completed_at, created_at, updated_at
         FROM public.deliveries WHERE id=$1",
    )
    .bind(delivery_id)
    .bind(sale.payment_obligation)
    .execute(pool)
    .await
    .expect("写入已转人工对账的履约");
    sqlx::query(
        "INSERT INTO public.delivery_send_attempts
            (delivery_id, fencing_version, command_id, request_digest, world_id, sending_at, expectation)
         SELECT id, fencing_version, command_id, request_digest, 'main', sending_at, $2
         FROM public.deliveries WHERE id=$1",
    )
    .bind(delivery_id)
    .bind(sqlx::types::Json(&expectation))
    .execute(pool)
    .await
    .expect("写入原发送围栏的冻结身份");

    let command = SentConfirmationCommand {
        command_id: Uuid::new_v4(),
        delivery_id,
        expected_fencing_version: fence,
        actor: "returns-operator".to_owned(),
        operator_identity: None,
        reason: "核对原 Sent 装备附件".to_owned(),
        evidence: ManualSentEvidence {
            schema_version: 1,
            world_id: "main".to_owned(),
            external_mail_id: "781022".to_owned(),
            sender_uid: UID.to_owned(),
            sender_name: BOT.to_owned(),
            recipient: expectation.recipient.clone(),
            subject: expectation.subject.clone(),
            sent_minute,
            observed_at,
            original_body_digest: expectation.body_digest,
            observed_body_digest: expectation.body_digest,
            remaining_credits: 0,
            cod_pending: expectation.cod,
            equipment_ids: expectation.equipment_ids.clone(),
            collected: false,
            receipt: String::new(),
        },
        accept_collected_assets: false,
        accept_attempt_history: false,
    };
    (command, expectation)
}

fn equipment_sweep_observation(evidence: &ManualSentEvidence) -> Value {
    json!({
        "mid": evidence.external_mail_id,
        "observed_at": evidence.observed_at,
        "collected": false,
        "remaining_equipment_count": evidence.equipment_ids.len(),
        "remaining_equipment_ids": evidence.equipment_ids,
    })
}

#[tokio::test]
async fn postgres_manual_sent_confirmation_after_admin_failed_reports_stale_fence() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let (command, _) = seed_manual_sent_equipment(pool).await;
    let delivery_id = command.delivery_id;
    let reference = format!("mailbox_sweep:{}", command.evidence.external_mail_id);
    assert!(
        store
            .record_sent_observation(
                delivery_id,
                command.expected_fencing_version,
                equipment_sweep_observation(&command.evidence),
            )
            .await
            .expect("写入原待确认观察")
    );
    let preview = store
        .preview_sent_confirmation(delivery_id, command.evidence.clone(), Some(&reference))
        .await
        .expect("预览判失败前的观察");
    assert!(preview.eligible, "{:?}", preview.blockers);
    assert_eq!(preview.expected_fencing_version, command.expected_fencing_version);

    let failure = store
        .execute(equipment_admin_action(
            &command,
            DeliveryAdminAction::ReconcileFailed,
            Some("verified_not_sent"),
        ))
        .await
        .expect("另一位管理员判失败");
    assert_eq!(failure.snapshot.status, "failed");
    assert_eq!(failure.snapshot.fencing_version, preview.expected_fencing_version + 1);
    assert!(matches!(
        store.confirm_sent(command, Some(&reference)).await,
        Err(ApplicationError::DeliveryAdmin(DeliveryAdminError::Stale))
    ));
    let current = store.detail(delivery_id).await.expect("读取判失败后的详情");
    assert_eq!(current.delivery.status, "failed");
    assert_eq!(current.delivery.fencing_version, failure.snapshot.fencing_version);
    assert!(current.delivery.external_reference.is_none());
    assert!(current.evidence.iter().all(|item| item.reference != reference));
    pool.close().await;
}

#[tokio::test]
async fn postgres_corrupt_sent_observed_at_is_unavailable_for_detail_and_confirmation() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let (command, _) = seed_manual_sent_equipment(pool).await;
    let delivery_id = command.delivery_id;
    let mut observation = equipment_sweep_observation(&command.evidence);
    assert!(
        store
            .record_sent_observation(
                delivery_id,
                command.expected_fencing_version,
                observation.clone()
            )
            .await
            .expect("写入完整观察")
    );
    let reference = format!("mailbox_sweep:{}", command.evidence.external_mail_id);
    assert!(
        store
            .preview_sent_confirmation(delivery_id, command.evidence.clone(), Some(&reference))
            .await
            .expect("预览完整观察")
            .eligible
    );

    observation["observed_at"] = json!("2026-11-31T01:20:00Z");
    assert!(
        store
            .record_sent_observation(delivery_id, command.expected_fencing_version, observation)
            .await
            .expect("模拟库内时间字段损坏")
    );
    assert!(matches!(
        store.detail_with_guard(delivery_id).await,
        Err(ApplicationError::DeliveryAdmin(DeliveryAdminError::Unavailable))
    ));
    for selected in [Some(reference.as_str()), None] {
        assert!(matches!(
            store.preview_sent_confirmation(delivery_id, command.evidence.clone(), selected).await,
            Err(ApplicationError::Serialization(_))
        ));
        assert!(matches!(
            store.confirm_sent(command.clone(), selected).await,
            Err(ApplicationError::Serialization(_))
        ));
    }
    assert_eq!(delivery_status(pool, delivery_id).await, "manual_reconcile");
    pool.close().await;
}

#[tokio::test]
async fn postgres_manual_sent_equipment_rejects_overwritten_evidence_and_same_count_wrong_eid() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let (mut command, expectation) = seed_manual_sent_equipment(pool).await;
    let delivery_id = command.delivery_id;
    let fence = command.expected_fencing_version;
    let observed_at = command.evidence.observed_at;
    let sent_minute = command.evidence.sent_minute;
    let sighting = |mid: &str, ids: &[String], recorded_at: Timestamp| {
        json!({
            "mid": mid,
            "observed_at": recorded_at,
            "sent_at": sent_minute,
            "recipient": expectation.recipient,
            "subject": expectation.subject,
            "body_digest": expectation.body_digest,
            "collected": false,
            "cod_pending": expectation.cod,
            "remaining_credits": 0,
            "remaining_equipment_count": ids.len(),
            "remaining_equipment_ids": ids,
        })
    };
    let first_mid = command.evidence.external_mail_id.as_str();
    let reference = format!("sent_reference_sighting:{first_mid}");
    assert!(
        store
            .record_sent_sighting(
                delivery_id,
                fence,
                first_mid,
                sighting(first_mid, &expectation.equipment_ids, observed_at),
            )
            .await
            .expect("Worker 首次实见冻结装备")
    );
    let preview = store
        .preview_sent_confirmation(delivery_id, command.evidence.clone(), Some(&reference))
        .await
        .expect("预览原观察");
    assert!(preview.eligible, "{:?}", preview.blockers);

    let collected_at = observed_at + Duration::seconds(1);
    let mut collected = sighting(first_mid, &[], collected_at);
    collected["collected"] = json!(true);
    collected["cod_pending"] = json!(0);
    assert!(
        store
            .record_sent_sighting(delivery_id, fence, first_mid, collected)
            .await
            .expect("Worker 同一 MID 覆盖为已领取")
    );
    assert!(matches!(
        store
            .preview_sent_confirmation(delivery_id, command.evidence.clone(), Some(&reference))
            .await,
        Err(ApplicationError::DeliveryAdmin(DeliveryAdminError::Stale))
    ));
    assert!(matches!(
        store.confirm_sent(command.clone(), Some(&reference)).await,
        Err(ApplicationError::DeliveryAdmin(DeliveryAdminError::Stale))
    ));

    let new_mid = "781023";
    let wrong_ids = vec!["1".to_owned()];
    assert_ne!(wrong_ids, expectation.equipment_ids);
    let wrong_at = collected_at + Duration::seconds(1);
    store
        .record_sent_sighting(delivery_id, fence, new_mid, sighting(new_mid, &wrong_ids, wrong_at))
        .await
        .expect("同一位置覆盖为另一封同数量错号的观察");
    assert!(matches!(
        store.confirm_sent(command.clone(), Some(&reference)).await,
        Err(ApplicationError::DeliveryAdmin(DeliveryAdminError::Stale))
    ));

    command.command_id = Uuid::new_v4();
    command.evidence.external_mail_id = new_mid.to_owned();
    command.evidence.observed_at = wrong_at;
    let reference = format!("sent_reference_sighting:{new_mid}");
    let preview = store
        .preview_sent_confirmation(delivery_id, command.evidence.clone(), Some(&reference))
        .await
        .expect("预览错号观察");
    assert_eq!(preview.blockers, ["observed_equipment_ids_mismatch"]);
    assert!(matches!(
        store.confirm_sent(command, Some(&reference)).await,
        Err(ApplicationError::FulfillmentDependencyMismatch)
    ));

    let state: (String, i64, Option<String>) = sqlx::query_as(
        "SELECT status, fencing_version, external_reference FROM public.deliveries WHERE id=$1",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("核对投递未推进");
    assert_eq!(state, ("manual_reconcile".to_owned(), fence, None));
    let bound: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.delivery_sent_bindings WHERE delivery_id=$1)",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("读取 MID 绑定");
    assert!(!bound);
    pool.close().await;
}

struct SweepClock;

impl Clock for SweepClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

struct FailedSentMailbox {
    entry: MoogleMailboxSentEntry,
    state: MoogleMailboxSentState,
    list_failed: AtomicBool,
    detail_failed: bool,
    list_reads: AtomicUsize,
    detail_reads: AtomicUsize,
}

impl FailedSentMailbox {
    fn new(command: &SentConfirmationCommand, expectation: &DeliverySendExpectation) -> Self {
        let entry = MoogleMailboxSentEntry {
            external_mail_id: command.evidence.external_mail_id.clone(),
            recipient: expectation.recipient.clone(),
            subject: expectation.subject.clone(),
            sent_at: command.evidence.sent_minute,
        };
        let state = MoogleMailboxSentState {
            external_mail_id: entry.external_mail_id.clone(),
            recipient: entry.recipient.clone(),
            subject: entry.subject.clone(),
            collected: false,
            cod_pending: expectation.cod,
            remaining_credits: 0,
            remaining_equipment_ids: expectation.equipment_ids.clone(),
            body_digest: expectation.body_digest,
            observation_credential: expectation.credential.clone(),
        };
        Self {
            entry,
            state,
            list_failed: AtomicBool::new(false),
            detail_failed: false,
            list_reads: AtomicUsize::new(0),
            detail_reads: AtomicUsize::new(0),
        }
    }
}

impl MoogleMailboxMaintenance for FailedSentMailbox {
    fn compose<'a>(&'a self, _: &'a str) -> ApplicationFuture<'a, MoogleMailboxComposeState> {
        Box::pin(async {
            Ok(MoogleMailboxComposeState {
                clean: true,
                recipient: String::new(),
                subject: String::new(),
                body: String::new(),
                credits: 0,
                cod: None,
                attachment_count: 0,
                equipment_ids: Vec::new(),
                body_digest: [0; 32],
                observation_credential: None,
            })
        })
    }

    fn discard_compose<'a>(
        &'a self,
        _: &'a str,
        _: &'a MoogleMailboxComposeState,
    ) -> ApplicationFuture<'a, MoogleMailboxDiscard> {
        panic!("失败投递取证不应外写")
    }

    fn sent_entries<'a>(&'a self, _: &'a str) -> ApplicationFuture<'a, MoogleMailboxSentEntries> {
        Box::pin(async move {
            self.list_reads.fetch_add(1, Ordering::Relaxed);
            if self.list_failed.load(Ordering::Relaxed) {
                return Err(ApplicationError::CapabilityUnavailable("测试 Sent 列表读取失败"));
            }
            Ok(MoogleMailboxSentEntries {
                entries: vec![self.entry.clone()],
                complete: true,
                ..MoogleMailboxSentEntries::default()
            })
        })
    }

    fn sent_state<'a>(
        &'a self,
        _: &'a str,
        entry: &'a MoogleMailboxSentEntry,
        _: Timestamp,
    ) -> ApplicationFuture<'a, MoogleMailboxSentState> {
        Box::pin(async move {
            assert_eq!(entry, &self.entry);
            self.detail_reads.fetch_add(1, Ordering::Relaxed);
            if self.detail_failed {
                return Err(ApplicationError::CapabilityUnavailable("测试 Sent 详情读取失败"));
            }
            Ok(self.state.clone())
        })
    }

    fn read_mail_observed<'a>(&'a self, _: &'a str) -> ApplicationFuture<'a, usize> {
        Box::pin(async { Ok(0) })
    }

    fn notice_expectation<'a>(
        &'a self,
        _: &'a MoogleMailboxUnprovenSend,
    ) -> ApplicationFuture<'a, Option<MoogleMailboxNoticeExpectation>> {
        panic!("装备 Sent 补采不应读取通知")
    }

    fn inbox_assets<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> ApplicationFuture<'a, MoogleMailboxInboxObservation> {
        panic!("装备 Sent 补采不应读取收件箱")
    }

    fn return_take_receipt<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a MoogleMailboxReturnTakeIdentity,
    ) -> ApplicationFuture<'a, MoogleMailboxReturnTakeObservation> {
        panic!("装备 Sent 补采不应读取退件")
    }
}

fn equipment_admin_action(
    evidence: &SentConfirmationCommand,
    action: DeliveryAdminAction,
    failure_code: Option<&str>,
) -> DeliveryAdminCommand {
    DeliveryAdminCommand {
        command_id: Uuid::new_v4(),
        delivery_id: evidence.delivery_id,
        expected_fencing_version: evidence.expected_fencing_version,
        action,
        external_reference: None,
        failure_code: failure_code.map(str::to_owned),
        actor_id: evidence.actor.clone(),
        request_id: None,
        request_hash: action.request_hash(
            evidence.delivery_id,
            evidence.expected_fencing_version,
            None,
            failure_code,
            &evidence.actor,
        ),
    }
}

async fn seed_admin_failed_equipment(
    store: &DatabaseStore,
) -> (SentConfirmationCommand, DeliverySendExpectation) {
    let (mut command, expectation) = seed_manual_sent_equipment(store.postgres_pool()).await;
    let failure = store
        .execute(equipment_admin_action(
            &command,
            DeliveryAdminAction::ReconcileFailed,
            Some("verified_not_sent"),
        ))
        .await
        .expect("Admin 判失败");
    assert_eq!(failure.snapshot.status, "failed");
    command.expected_fencing_version = failure.snapshot.fencing_version;
    command.evidence.observed_at = Timestamp::now();
    (command, expectation)
}

async fn failed_sent_sweep(service: &MailboxMaintenanceService) -> MailboxMaintenanceReport {
    service
        .sweep(
            "main",
            Instant::now() + std::time::Duration::from_secs(60),
            &ExternalWriteActivity::default(),
        )
        .await
        .expect("运行 Sent 巡检")
}

#[tokio::test]
async fn postgres_failed_sent_sweep_records_once_and_corrects_matching_equipment() {
    for eid_matches in [true, false] {
        let database = support::TestDatabase::create().await.expect("克隆模板库");
        let store = Arc::new(database.store().await.expect("以运行账号连接"));
        let pool = store.postgres_pool();
        let (mut command, expectation) = seed_admin_failed_equipment(&store).await;
        let missing = store
            .preview_sent_confirmation(command.delivery_id, command.evidence.clone(), None)
            .await
            .expect("补采前预览");
        assert_eq!(missing.blockers, ["missing_observed_equipment_ids"]);

        let mut mailbox = FailedSentMailbox::new(&command, &expectation);
        if !eid_matches {
            mailbox.state.remaining_equipment_ids = vec!["1".to_owned()];
            assert_ne!(mailbox.state.remaining_equipment_ids, expectation.equipment_ids);
        }
        let mailbox = Arc::new(mailbox);
        let service =
            MailboxMaintenanceService::new(mailbox.clone(), store.clone(), Arc::new(SweepClock));
        mailbox.list_failed.store(true, Ordering::Relaxed);
        assert_eq!(failed_sent_sweep(&service).await.sent_observed, 0);
        assert_eq!(mailbox.detail_reads.load(Ordering::Relaxed), 0);
        mailbox.list_failed.store(false, Ordering::Relaxed);
        let first = failed_sent_sweep(&service).await;
        assert_eq!((first.sent_observed, first.sent_recovered), (1, 0));
        assert_eq!(failed_sent_sweep(&service).await.sent_observed, 0);
        assert_eq!(mailbox.list_reads.load(Ordering::Relaxed), 3);
        assert_eq!(mailbox.detail_reads.load(Ordering::Relaxed), 1);

        let unchanged = || async {
            let row: (String, i64, Option<String>, String, Value) = sqlx::query_as(
                "SELECT d.status,d.fencing_version,d.external_reference,f.execution_state,
                        d.result_metadata->'sent_reference_sighting'
                 FROM public.deliveries d JOIN public.settlement_obligation_fulfillments f
                   ON f.delivery_id=d.id WHERE d.id=$1",
            )
            .bind(command.delivery_id)
            .fetch_one(pool)
            .await
            .expect("读取补采后的投递与履约");
            assert_eq!(
                (row.0.as_str(), row.1, row.2, row.3.as_str()),
                ("failed", command.expected_fencing_version, None, "failed")
            );
            row.4
        };
        let observed = unchanged().await;
        assert_eq!(
            observed["remaining_equipment_ids"],
            json!(mailbox.state.remaining_equipment_ids)
        );
        assert_eq!(observed["collected"], false);
        command.evidence.observed_at =
            serde_json::from_value(observed["observed_at"].clone()).expect("解码 Worker 实见时刻");
        let reference = format!("sent_reference_sighting:{}", command.evidence.external_mail_id);
        let preview = store
            .preview_sent_confirmation(
                command.delivery_id,
                command.evidence.clone(),
                Some(&reference),
            )
            .await
            .expect("预览失败更正");
        assert!(preview.corrects_admin_failure);
        assert_eq!(preview.eligible, eid_matches, "{:?}", preview.blockers);
        let confirmed = store.confirm_sent(command.clone(), Some(&reference)).await;
        if eid_matches {
            let confirmation = confirmed.expect("实见装备相符可以更正");
            assert_eq!(
                confirmation.external_reference,
                format!("mooglemail:main:send:{}", mailbox.entry.external_mail_id)
            );
            assert_eq!(delivery_status(pool, command.delivery_id).await, "succeeded");
        } else {
            assert_eq!(preview.blockers, ["observed_equipment_ids_mismatch"]);
            assert!(matches!(confirmed, Err(ApplicationError::FulfillmentDependencyMismatch)));
            unchanged().await;
            let bound: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM public.delivery_sent_bindings WHERE delivery_id=$1)",
            )
            .bind(command.delivery_id)
            .fetch_one(pool)
            .await
            .expect("核对没有 MID 绑定");
            assert!(!bound);
            assert!(matches!(
                store
                    .execute(equipment_admin_action(
                        &command,
                        DeliveryAdminAction::RetryFailed,
                        None
                    ))
                    .await,
                Err(ApplicationError::DeliveryAdmin(DeliveryAdminError::SentSighted)),
            ));
        }
        pool.close().await;
    }
}

#[tokio::test]
async fn postgres_failed_sent_sweep_respects_manual_budget_and_current_failure() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = Arc::new(database.store().await.expect("以运行账号连接"));
    let pool = store.postgres_pool();
    let (command, expectation) = seed_admin_failed_equipment(&store).await;
    let mut mailbox = FailedSentMailbox::new(&command, &expectation);
    mailbox.detail_failed = true;
    let mailbox = Arc::new(mailbox);
    let service =
        MailboxMaintenanceService::new(mailbox.clone(), store.clone(), Arc::new(SweepClock));
    let slots = store.failed_sent_slots(None, mailbox.entry.sent_at, 10).await.expect("取证候选");
    assert_eq!(slots.len(), 1);
    assert!(slots[0].target.is_some());
    let slots =
        store.failed_sent_slots(None, Timestamp::now(), 10).await.expect("可见范围外的候选");
    assert!(slots.is_empty());

    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, sending_at, completed_at, failure_code)
         SELECT gen_random_uuid(), gen_random_uuid(), decode(repeat('43',32),'hex'), 'settlement',
                fixture.reference, 'OtherBuyer', jsonb_build_object('operation','send','world','main',
                   'template','settlement_seller_payout','variables',jsonb_build_object('reference',fixture.reference),
                   'credits',100,'cod',0,'equipment_secret_refs','[]'::jsonb),
                'manual_reconcile', 2, 1, $1, $1, 'write_unknown'
         FROM (SELECT gen_random_uuid()::text AS reference FROM generate_series(1,20)) fixture",
    )
    .bind(mailbox.entry.sent_at)
    .execute(pool)
    .await
    .expect("填满原人工对账预算");
    let priority = failed_sent_sweep(&service).await;
    assert_eq!(priority.sent_candidates, 20);
    assert_eq!(mailbox.detail_reads.load(Ordering::Relaxed), 0);
    sqlx::query("UPDATE public.deliveries SET status='failed' WHERE recipient='OtherBuyer'")
        .execute(pool)
        .await
        .expect("移除人工对账测试候选");

    assert_eq!(failed_sent_sweep(&service).await.sent_state_failed, 1);
    assert_eq!(failed_sent_sweep(&service).await.sent_state_failed, 0);
    assert_eq!(mailbox.detail_reads.load(Ordering::Relaxed), 1);
    let persisted: (bool, i64) = sqlx::query_as(
        "SELECT d.result_metadata ? 'sent_reference_sighting',
                (SELECT count(*) FROM public.audit_records a WHERE a.entity_id=d.id::text
                 AND a.action='delivery.sent_failed_detail_attempt')
         FROM public.deliveries d WHERE d.id=$1",
    )
    .bind(command.delivery_id)
    .fetch_one(pool)
    .await
    .expect("核对失败读取仍持久登记且没有实见记录");
    assert_eq!(persisted, (false, 1));

    sqlx::query("UPDATE public.deliveries SET fencing_version=fencing_version+1 WHERE id=$1")
        .bind(command.delivery_id)
        .execute(pool)
        .await
        .expect("模拟失败结论已过期");
    let slots =
        store.failed_sent_slots(None, mailbox.entry.sent_at, 10).await.expect("过期失败轮转");
    assert!(slots.iter().all(|slot| slot.target.is_none()));
    assert_eq!(failed_sent_sweep(&service).await.sent_candidates, 0);
    assert_eq!(mailbox.detail_reads.load(Ordering::Relaxed), 1);
    assert_eq!(mailbox.list_reads.load(Ordering::Relaxed), 4);
    pool.close().await;
}
