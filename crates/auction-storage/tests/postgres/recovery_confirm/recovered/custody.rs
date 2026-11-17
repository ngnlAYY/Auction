//! `equipment_recovered` 之后的买方退款被拒收退回、经退件案件领回代管时，恢复记录由恢复 owner
//! 以独立的“退款权益转代管”完成事实结束（PostgreSQL 18，运行账号）。全链走真实端口：确认
//! `equipment_recovered` → 退款资金转换与 prepare/activate → 投递经资金门寄出并以 Sent 证明收口 →
//! 系统退回并建立退件案件 → 领回代管 → 恢复完成 → 从代管补寄不被恢复状态阻断：第 2 次派发经
//! 领取、prepare、activate 进入发送队列，经资金门寄出并以 Sent 证明收口，案件结案为已补寄。
//! 外部 MoogleMail 不参与：发送围栏之后的结果由测试按生产形状写回，不代表真实远端行为。

use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use auction_application::core::RequestHash;
use auction_application::delivery::{
    DeliveryClaimExclusions, DeliveryFenceOutcome, DeliveryPreparedExpectation,
    DeliverySendCredentialBinding, DeliverySendExpectation, DeliverySendResult, DeliveryWorkStore,
};
use auction_application::recovery::{
    RecoveryCommandRepository, RecoveryConfirmationAction, RecoveryResolutionStore,
    RecoveryWorkStore,
};
use auction_application::settlement::{
    INTERNAL_EVIDENCE_PROTOCOL, InternalEvidencePurpose, MailAccountEvidenceBinding,
    MailRuntimeBinding, SettlementDeliveryPreparation, SettlementFulfillmentState,
    SettlementFulfillmentStore, SettlementInternalEvidenceSnapshot, SettlementReturnWorkStore,
};
use auction_domain::{
    CommandId, SettlementDispatchReference, SettlementReturnAction, SettlementReturnCaseStatus,
    SettlementWorld, resend_activate_command_id, resend_delivery_id, resend_prepare_command_id,
};
use auction_storage::DatabaseStore;
use chrono::{Duration, DurationRound, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::fixture::{
    FUNDING_UID, evidence as custody_evidence, insert_funding, insert_recovered_sale,
    insert_relist_target,
};
use super::{claim_refund, deadline, fund_and_activate_refund, recovery_state};
use crate::recovery_confirm::{confirmation, fulfillment};
use crate::settlement_returns::flows::{handle, take_through};
use crate::settlement_returns::{evidence, insert_inbound};
use crate::support;

/// 记账模式下投递领取的同 session 证据：资金账户 UID、共享邮件凭据与 generation 1；不观察余额。
fn delivery_evidence(
    credential: Uuid,
    claim: &auction_application::delivery::DeliveryWorkClaim,
) -> SettlementInternalEvidenceSnapshot {
    let observed = Utc::now().duration_trunc(Duration::microseconds(1)).expect("截断到微秒");
    let mut identity = MailAccountEvidenceBinding {
        credential_id: credential,
        credential_version: 1,
        runtime: MailRuntimeBinding { generation: 1 },
        authenticated_uid: FUNDING_UID.to_owned(),
        player_name: "Recovery Bot".to_owned(),
        world: SettlementWorld::Main,
        observed_at: observed,
        protocol_version: INTERNAL_EVIDENCE_PROTOCOL,
        digest: [0; 32],
    };
    identity.digest = identity.compute_digest();
    let mut snapshot = SettlementInternalEvidenceSnapshot {
        operation_id: claim.command_id,
        purpose: InternalEvidencePurpose::DeliveryClaim {
            delivery_id: claim.delivery_id,
            claim_token: claim.lease_token,
            claim_fencing: claim.fencing_version,
            request_hash: claim.request_hash,
        },
        identity,
        equipment: None,
        main_cash: None,
        sent_collection: None,
        observed_from: observed,
        expires_at: observed + Duration::seconds(60),
        digest: [0; 32],
    };
    snapshot.digest = snapshot.compute_digest();
    snapshot
}

/// 退款投递经资金门进入 sending 后以精确 Sent 证明（Sent MID 为 `mid`）收口：退款分配
/// reserve → in_flight → spent。
async fn send_refund(store: &DatabaseStore, credential: Uuid, delivery: Uuid, mid: &str) {
    let now = Utc::now();
    let grace = StdDuration::from_secs(60);
    DeliveryWorkStore::begin_claim_cycle(store, now, grace).await.expect("领取前清理");
    let claim = DeliveryWorkStore::claim_due(
        store,
        "recovered-refund-delivery".to_owned(),
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
    .expect("领取到退款投递");
    let expectation = Arc::new(DeliverySendExpectation {
        world_id: "main".to_owned(),
        recipient: claim.recipient.clone(),
        subject: "Auction buyer refund".to_owned(),
        body_digest: [0x53; 32],
        credits: 100,
        cod: 0,
        equipment_ids: Vec::new(),
        credential: Some(DeliverySendCredentialBinding {
            credential_id: credential,
            credential_version: 1,
            runtime_generation: 1,
        }),
    });
    let outcome = store
        .start_sending_authorized_until(
            claim.clone(),
            Utc::now(),
            &delivery_evidence(credential, &claim),
            Instant::now() + StdDuration::from_secs(30),
            Some(&DeliveryPreparedExpectation::Send(Arc::clone(&expectation))),
        )
        .await
        .expect("资金门必须确定完成");
    assert!(matches!(outcome, DeliveryFenceOutcome::Sending), "退款进入 sending");
    store
        .succeed(
            claim.clone(),
            DeliverySendResult {
                external_reference: format!("mooglemail:main:send:{mid}"),
                result_metadata: json!({
                    "operation": "send",
                    "template": claim.payload["template"],
                    "external_reference_proven": true,
                    "sent_verified": true,
                    "proof": "sent_exact_payload_v1",
                    "send_expectation": serde_json::to_value(expectation.as_ref()).expect("冻结身份"),
                    "sending_fencing_version": claim.fencing_version,
                }),
            },
            Utc::now(),
        )
        .await
        .expect("退款 Sent 证明收口");
}

async fn refund_allocation(pool: &PgPool, refund: Uuid) -> (String, String, i64) {
    sqlx::query_as(
        "SELECT purpose, state, amount FROM public.settlement_funding_allocations
         WHERE uid = $1 AND obligation_id = $2",
    )
    .bind(FUNDING_UID)
    .bind(refund)
    .fetch_one(pool)
    .await
    .expect("读取退款分配")
}

async fn audits(pool: &PgPool, recovery_id: Uuid, action: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.audit_records
         WHERE entity_type = 'settlement_recovery' AND entity_id = $1 AND action = $2",
    )
    .bind(recovery_id.to_string())
    .bind(action)
    .fetch_one(pool)
    .await
    .expect("读取恢复审计")
}

/// 真实链路 `equipment_recovered → refunding → 退款退回 → Take → held`：恢复记录以独立完成事实
/// 结束（不是 refund_completed），退款负债与资金用途保持；随后从代管补寄照常进行。
#[tokio::test]
async fn postgres_returned_refund_held_in_custody_completes_recovery_independently() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let recovered = insert_recovered_sale(pool, 7_800_101).await;
    insert_relist_target(pool, 7_800_101).await;
    let credential = insert_funding(pool, &recovered).await;
    let sale = &recovered.sale;
    let result = store
        .confirm_recovery(confirmation(sale, RecoveryConfirmationAction::EquipmentRecovered))
        .await
        .expect("确认 equipment_recovered");
    assert_eq!(result.stage.as_str(), "refunding");
    let refund_delivery = fund_and_activate_refund(&store, &recovered, credential).await;
    send_refund(&store, credential, refund_delivery, "782001").await;
    assert_eq!(
        refund_allocation(pool, sale.refund).await,
        ("refund".to_owned(), "spent".to_owned(), 100)
    );

    // 买方拒收退款：系统退件绑定退款的首寄，案件待决定；Admin 领回代管。
    let case = insert_inbound(
        pool,
        "996101",
        Utc::now(),
        Some(("MoogleMail", format!("[RTS] Auction buyer refund [{}]", sale.refund))),
        evidence(sale.refund, 100, &[]),
    )
    .await;
    let consumed =
        store.consume_due_settlement_return(Utc::now()).await.expect("消费").expect("必须消费");
    assert_eq!((consumed.return_mail_id, consumed.delivery_id), (case, Some(refund_delivery)));
    assert_eq!(
        handle(&store, case, 1, SettlementReturnAction::ReclaimAndHold).await,
        SettlementReturnCaseStatus::TakePending
    );
    take_through(&store, case, true).await;
    assert_eq!(
        fulfillment(pool, sale.refund).await,
        ("failed".to_owned(), Some("settlement_return_held".to_owned()))
    );
    assert_eq!(
        refund_allocation(pool, sale.refund).await,
        ("refund".to_owned(), "available".to_owned(), 100),
        "领回返还原退款用途一次"
    );
    assert_eq!(
        recovery_state(pool, sale.recovery_id).await,
        ("refunding".to_owned(), Some("equipment_recovered".to_owned()))
    );

    // 恢复 owner 以独立完成事实结束恢复记录；退款负债、资金用途与案件都不改写。
    let advance = store
        .complete_due_refund(Utc::now())
        .await
        .expect("恢复收口")
        .expect("退款代管必须让恢复记录完成");
    assert!(advance.completed);
    assert_eq!(
        recovery_state(pool, sale.recovery_id).await,
        ("completed".to_owned(), Some("equipment_recovered".to_owned()))
    );
    assert_eq!(
        audits(pool, sale.recovery_id, "auction.recovery.refund_custody_completed").await,
        1
    );
    assert_eq!(
        audits(pool, sale.recovery_id, "auction.recovery.refund_completed").await,
        0,
        "不冒充退款已完成"
    );
    let completed_at: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT completed_at FROM public.settlement_recoveries WHERE id = $1")
            .bind(sale.recovery_id)
            .fetch_one(pool)
            .await
            .expect("读取完成时刻");
    let proven_at: chrono::DateTime<Utc> = sqlx::query_scalar(
        "SELECT proven_at FROM public.settlement_return_takes WHERE return_mail_id = $1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取领回证明时刻");
    assert_eq!(completed_at, Some(proven_at), "完成时刻取领回证明时刻");
    assert_eq!(
        fulfillment(pool, sale.refund).await,
        ("failed".to_owned(), Some("settlement_return_held".to_owned()))
    );
    assert_eq!(
        refund_allocation(pool, sale.refund).await,
        ("refund".to_owned(), "available".to_owned(), 100)
    );
    assert!(store.complete_due_refund(Utc::now()).await.expect("排空").is_none(), "只完成一次");

    // 代管后补寄：恢复记录已完成不阻断补寄，退款履约以第 2 次派发重新可领取。
    let version: i64 = sqlx::query_scalar(
        "SELECT case_version FROM public.settlement_return_cases WHERE return_mail_id = $1",
    )
    .bind(case)
    .fetch_one(pool)
    .await
    .expect("读取案件版本");
    assert_eq!(
        handle(&store, case, version, SettlementReturnAction::ResendFromCustody).await,
        SettlementReturnCaseStatus::ResendPending
    );
    let claim = claim_refund(&store, sale.refund).await;
    assert_eq!((claim.dispatch_no, claim.delivery_id), (2, None));

    // 补寄继续走完 prepare/activate：第 2 次派发以确定性身份准备，正文带派发引用；准备与激活都
    // 附同 session 的装备在库证据，与首寄同一组门禁，不被已完成的恢复记录阻断。
    let resend = resend_delivery_id(sale.refund, 2).expect("补寄投递 ID");
    let prepare_command =
        CommandId::from_uuid(resend_prepare_command_id(sale.refund, 2).expect("补寄 prepare ID"));
    let preparation = SettlementDeliveryPreparation {
        delivery_id: resend,
        prepare_command_id: prepare_command,
        request_hash: RequestHash::from_canonical_bytes(b"recovered-refund-resend-prepare"),
        recipient: claim.recipient.clone(),
        payload: json!({
            "schema_version": 1,
            "operation": "send",
            "world": "main",
            "template": "settlement_buyer_refund",
            "variables": {
                "amount": "100",
                "reference": sale.refund.to_string(),
                "dispatch": SettlementDispatchReference::for_resend(sale.refund, 2)
                    .expect("派发引用")
                    .encode(),
            },
            "credits": 100,
            "cod": 0,
            "equipment_secret_refs": [],
        }),
    };
    let prepare_evidence = custody_evidence(prepare_command.as_uuid(), &claim, credential);
    let prepared = store
        .prepare_delivery(claim, preparation, Utc::now(), Some((&prepare_evidence, deadline())))
        .await
        .expect("补寄 prepare");
    assert_eq!(prepared.delivery_id, resend);
    assert_eq!(prepared.next_claim.state, SettlementFulfillmentState::Prepared);
    let activate =
        CommandId::from_uuid(resend_activate_command_id(sale.refund, 2).expect("补寄 activate ID"));
    let activate_evidence = custody_evidence(activate.as_uuid(), &prepared.next_claim, credential);
    store
        .activate_delivery(
            prepared.next_claim,
            activate,
            RequestHash::from_canonical_bytes(b"recovered-refund-resend-activate"),
            Utc::now(),
            Some((&activate_evidence, deadline())),
        )
        .await
        .expect("补寄 activate");
    assert_eq!(fulfillment(pool, sale.refund).await, ("enqueued".to_owned(), None));
    let (status, authorized_by): (String, Option<Uuid>) = sqlx::query_as(
        "SELECT delivery.status, dispatch.authorized_by
         FROM public.deliveries AS delivery
         JOIN public.settlement_fulfillment_dispatches AS dispatch
           ON dispatch.delivery_id = delivery.id
         WHERE delivery.id = $1 AND dispatch.obligation_id = $2 AND dispatch.dispatch_no = 2",
    )
    .bind(resend)
    .bind(sale.refund)
    .fetch_one(pool)
    .await
    .expect("读取补寄投递与派发历史");
    assert_eq!(
        (status.as_str(), authorized_by),
        ("pending", Some(case)),
        "补寄派发由本案授权并进入发送队列"
    );

    // 补寄经资金门寄出并以 Sent 证明收口：原退款用途再次支出原额，案件结案为已补寄。
    send_refund(&store, credential, resend, "782002").await;
    assert_eq!(
        refund_allocation(pool, sale.refund).await,
        ("refund".to_owned(), "spent".to_owned(), 100)
    );
    let done =
        store.complete_due_return_resend(Utc::now()).await.expect("收口补寄").expect("必须收口");
    assert_eq!((done.return_mail_id, done.status), (case, SettlementReturnCaseStatus::Resent));
    assert_eq!(
        recovery_state(pool, sale.recovery_id).await,
        ("completed".to_owned(), Some("equipment_recovered".to_owned())),
        "补寄不改写已完成的恢复记录"
    );

    store.postgres_pool().close().await;
}
