//! Admin 确认 `equipment_recovered` 同事务收口依赖履约（设计 5b）的 PostgreSQL 18 用例。
//!
//! - 从真实回收夹具执行确认：载体付款证明保留，从未开始的卖方货款与已寄出的交付收口为
//!   `failed`，原交付当前 fence 上恰好一条 `recovery_equipment_recovered` 处置；随后买方退款经
//!   真实资金转换、prepare 与 activate 进入发送队列，命令重放不重复收口；
//! - 原交付与本轮 Take 链路不符、卖方货款已开始或已有支出、交付已满足、存在更新派发或原交付没有
//!   可信 Sent 证明时拒绝确认，什么都不改；
//! - 告警分类口径与长期未领取的计数、清单都按同一收口事实排除这两条终态，legacy 与 shadow
//!   仍按迁移前口径计入。

pub(crate) mod fixture;

use std::time::Duration as StdDuration;

use auction_application::core::{ApplicationError, RequestHash};
use auction_application::operations::{
    AuditFailureMode, DispositionReadMode, OperationsAlertCollectionOverdueRead,
    OperationsAlertSourceReader,
};
use auction_application::recovery::{
    RecoveryCommandError, RecoveryCommandRepository, RecoveryConfirmationAction,
};
use auction_application::runtime_switches::{RuntimeSwitchRole, RuntimeSwitches};
use auction_application::settlement::{
    FundingReconciliationReader, SettlementDeliveryPreparation, SettlementFulfillmentClaim,
    SettlementFulfillmentState, SettlementFulfillmentStore, SettlementInternalCommand,
    SettlementInternalCommitOutcome, SettlementInternalFulfillmentStore,
};
use auction_domain::CommandId;
use auction_storage::DatabaseStore;
use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use self::fixture::{
    FUNDING_UID, Recovered, evidence, insert_funding, insert_recovered_sale, insert_relist_target,
};
use super::{closure_audits, confirmation, dispositions, fulfillment};
use crate::support;

/// 证据交给存储事务时附带的本地截止时刻。
fn deadline() -> std::time::Instant {
    std::time::Instant::now() + StdDuration::from_secs(60)
}

async fn claim_refund(store: &DatabaseStore, refund: Uuid) -> SettlementFulfillmentClaim {
    let now = Utc::now();
    SettlementFulfillmentStore::claim_due(
        store,
        "recovered-refund-worker".to_owned(),
        now,
        now + Duration::minutes(5),
        20,
    )
    .await
    .expect("领取履约")
    .into_iter()
    .find(|claim| claim.obligation_id.as_uuid() == refund)
    .expect("equipment_recovered 后的买方退款可以领取")
}

async fn recipient_version(pool: &PgPool, claim: &SettlementFulfillmentClaim) -> Option<u64> {
    let user = claim.recipient_user_id.expect("退款收件人是注册用户");
    let version: i64 = sqlx::query_scalar("SELECT state_version FROM public.users WHERE id = $1")
        .bind(user)
        .fetch_one(pool)
        .await
        .expect("读取收件人版本");
    u64::try_from(version).ok()
}

async fn recovery_state(pool: &PgPool, recovery_id: Uuid) -> (String, Option<String>) {
    sqlx::query_as("SELECT stage, resolution FROM public.settlement_recoveries WHERE id = $1")
        .bind(recovery_id)
        .fetch_one(pool)
        .await
        .expect("读取恢复记录")
}

async fn business_state(pool: &PgPool, obligation: Uuid) -> String {
    sqlx::query_scalar("SELECT business_state FROM public.settlement_obligations WHERE id = $1")
        .bind(obligation)
        .fetch_one(pool)
        .await
        .expect("读取义务业务状态")
}

/// 从真实回收夹具确认 `equipment_recovered`，再走退款的资金转换、prepare 与 activate。
#[tokio::test]
async fn postgres_equipment_recovered_closes_dependents_and_funds_refund() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let recovered = insert_recovered_sale(pool, 7_800_001).await;
    insert_relist_target(pool, 7_800_001).await;
    let credential = insert_funding(pool, &recovered).await;
    let sale = &recovered.sale;
    let payment_before = fulfillment(pool, sale.payment).await;

    let command = confirmation(sale, RecoveryConfirmationAction::EquipmentRecovered);
    let result = store.confirm_recovery(command.clone()).await.expect("确认 equipment_recovered");
    assert_eq!(
        (result.stage.as_str(), result.resolution.as_str()),
        ("refunding", "equipment_recovered")
    );

    // 载体付款与其证明保留；卖方货款与原交付收口，原交付投递与 Take 证明不改写。
    assert_eq!(fulfillment(pool, sale.payment).await, payment_before);
    assert_eq!(
        fulfillment(pool, sale.seller).await,
        ("failed".to_owned(), Some("recovery_sale_voided".to_owned()))
    );
    assert_eq!(
        fulfillment(pool, recovered.handoff).await,
        ("failed".to_owned(), Some("recovery_equipment_recovered".to_owned()))
    );
    assert_eq!(
        dispositions(pool, recovered.handoff_delivery).await,
        vec![(
            "recovery_equipment_recovered".to_owned(),
            Some("recovery_equipment_recovered".to_owned()),
            "admin".to_owned()
        )]
    );
    assert_eq!(closure_audits(pool, sale).await, 2);
    for delivery in [recovered.handoff_delivery, recovered.take_delivery] {
        let status: String =
            sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
                .bind(delivery)
                .fetch_one(pool)
                .await
                .expect("读取投递");
        assert_eq!(status, "succeeded", "原交付与 Take 投递不改写");
    }
    assert_eq!(business_state(pool, sale.refund).await, "ready");

    // 同一命令重放返回原结果，不重复收口、处置或审计。
    let replayed = store.confirm_recovery(command).await.expect("重放确认");
    assert_eq!(replayed.state_version, result.state_version);
    assert_eq!(dispositions(pool, recovered.handoff_delivery).await.len(), 1);
    assert_eq!(closure_audits(pool, sale).await, 2);

    // 买方退款：真实资金转换把卖方用途改为退款用途，不新增收入。
    let claim = claim_refund(&store, sale.refund).await;
    let version = recipient_version(pool, &claim).await;
    let funding =
        SettlementInternalCommand::external_refund(claim.clone(), version).expect("退款资金命令");
    let outcome = store
        .prepare_external_refund(&funding, &evidence(funding.command_id, &claim, credential))
        .await
        .expect("退款资金转换事务");
    assert!(
        matches!(outcome, SettlementInternalCommitOutcome::Committed),
        "资金转换必须提交，而不是延后或回滚"
    );
    let (disposition,): (String,) =
        sqlx::query_as("SELECT disposition FROM public.settlement_funding_lots WHERE lot_id = $1")
            .bind(recovered.lot_id)
            .fetch_one(pool)
            .await
            .expect("读取资金化 lot");
    assert_eq!(disposition, "refund");
    let allocations: Vec<(String, i64)> = sqlx::query_as(
        "SELECT purpose, amount FROM public.settlement_funding_allocations
         WHERE uid = $1 AND lot_id = $2 ORDER BY purpose",
    )
    .bind(FUNDING_UID)
    .bind(recovered.lot_id)
    .fetch_all(pool)
    .await
    .expect("读取资金分配");
    assert_eq!(
        allocations,
        vec![("refund".to_owned(), 100), ("seller".to_owned(), 0), ("tax".to_owned(), 0)]
    );

    // 准备与激活：同一 claim 进入发送队列。
    let preparation = SettlementDeliveryPreparation {
        delivery_id: Uuid::new_v4(),
        prepare_command_id: CommandId::from_uuid(Uuid::new_v4()),
        request_hash: RequestHash::from_canonical_bytes(b"recovered-refund-prepare"),
        recipient: claim.recipient.clone(),
        payload: serde_json::json!({
            "schema_version": 1,
            "operation": "send",
            "world": "main",
            "template": "settlement_buyer_refund",
            "variables": {"amount": "100", "reference": sale.refund.to_string()},
            "credits": 100,
            "cod": 0,
            "equipment_secret_refs": [],
        }),
    };
    let custody = evidence(preparation.prepare_command_id.as_uuid(), &claim, credential);
    let prepared = store
        .prepare_delivery(claim.clone(), preparation, Utc::now(), Some((&custody, deadline())))
        .await
        .expect("退款 prepare");
    let next = prepared.next_claim;
    assert_eq!(next.state, SettlementFulfillmentState::Prepared);
    let activate = CommandId::from_uuid(Uuid::new_v4());
    let custody = evidence(activate.as_uuid(), &next, credential);
    store
        .activate_delivery(
            next.clone(),
            activate,
            RequestHash::from_canonical_bytes(b"recovered-refund-activate"),
            Utc::now(),
            Some((&custody, deadline())),
        )
        .await
        .expect("退款 activate");
    assert_eq!(fulfillment(pool, sale.refund).await, ("enqueued".to_owned(), None));
    let (status,): (String,) = sqlx::query_as("SELECT status FROM public.deliveries WHERE id = $1")
        .bind(prepared.delivery_id)
        .fetch_one(pool)
        .await
        .expect("读取退款投递");
    assert_eq!(status, "pending");

    // funding 预览只认精确收口：两条收口的 failed 不再是不明责任；失败码不符即恢复阻断。
    let lot = recovered.lot_id.to_string();
    let unproven = |blockers: &[String]| {
        blockers
            .iter()
            .filter(|blocker| {
                blocker.contains(&lot)
                    && (blocker.ends_with(":handoff_unproven")
                        || blocker.ends_with(":seller_state:unproven"))
            })
            .count()
    };
    let preview = store.preview_funding(FUNDING_UID.to_owned()).await.expect("funding 预览");
    assert_eq!(unproven(&preview.blockers), 0, "{:?}", preview.blockers);
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments SET last_error_code = 'send_rejected'
         WHERE obligation_id = ANY($1)",
    )
    .bind(vec![recovered.handoff, sale.seller])
    .execute(pool)
    .await
    .expect("改为普通失败");
    let preview = store.preview_funding(FUNDING_UID.to_owned()).await.expect("funding 预览");
    assert_eq!(unproven(&preview.blockers), 2, "{:?}", preview.blockers);

    store.postgres_pool().close().await;
}

/// 任一冲突都让整个确认事务回滚：恢复记录、履约、处置与退款激活都保持原样。
#[tokio::test]
async fn postgres_equipment_recovered_rejects_conflicting_delivery() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let cases: [(&str, &str); 6] = [
        (
            "Take 投递取回的不是这封退件",
            "UPDATE public.deliveries SET external_reference = 'mooglemail:isekai:take:999999'
             WHERE id = $1",
        ),
        (
            "原交付没有可信 Sent 证明",
            "UPDATE public.deliveries SET result_metadata = '{\"operation\":\"send\"}'::jsonb
             WHERE id = $2",
        ),
        (
            "交付义务已有更新派发",
            "INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status)
             VALUES (gen_random_uuid(), gen_random_uuid(), decode(repeat('e1', 32), 'hex'),
                     'settlement', $3::text, 'Recovery buyer',
                     '{\"operation\":\"send\"}'::jsonb, 'prepared')",
        ),
        (
            "交付已满足",
            "UPDATE public.settlement_obligation_fulfillments
             SET execution_state = 'satisfied', proof_kind = 'mooglemail_sent_collected',
                 proof_reference = '7654321', proof_digest = decode(repeat('e2', 32), 'hex'),
                 proof_observed_at = clock_timestamp(), completed_at = clock_timestamp()
             WHERE obligation_id = $3",
        ),
        (
            "卖方货款已开始执行",
            "UPDATE public.settlement_obligation_fulfillments
             SET prepare_command_id = gen_random_uuid(),
                 prepare_request_digest = decode(repeat('e3', 32), 'hex')
             WHERE obligation_id = $4",
        ),
        (
            "卖方货款已有支出",
            "UPDATE public.settlement_funding_allocations
             SET state = 'spent', delivery_id = $1
             WHERE obligation_id = $4",
        ),
    ];
    for (index, (label, setup)) in cases.into_iter().enumerate() {
        let sequence = 7_800_101 + i64::try_from(index).expect("用例序号");
        let recovered = insert_recovered_sale(pool, sequence).await;
        if index == 5 {
            insert_funding(pool, &recovered).await;
        }
        sqlx::query(sqlx::AssertSqlSafe(setup))
            .bind(recovered.take_delivery)
            .bind(recovered.handoff_delivery)
            .bind(recovered.handoff)
            .bind(recovered.sale.seller)
            .execute(pool)
            .await
            .expect(label);
        let sale = &recovered.sale;
        let before = (
            fulfillment(pool, sale.seller).await,
            fulfillment(pool, recovered.handoff).await,
            business_state(pool, sale.refund).await,
        );
        let error = store
            .confirm_recovery(confirmation(sale, RecoveryConfirmationAction::EquipmentRecovered))
            .await
            .expect_err(label);
        assert!(
            matches!(
                error,
                ApplicationError::RecoveryCommand(RecoveryCommandError::NotConfirmable)
            ),
            "{label}: {error:?}"
        );
        assert_eq!(recovery_state(pool, sale.recovery_id).await, ("reclaiming".to_owned(), None));
        assert_eq!(
            (
                fulfillment(pool, sale.seller).await,
                fulfillment(pool, recovered.handoff).await,
                business_state(pool, sale.refund).await,
            ),
            before,
            "{label}"
        );
        assert!(dispositions(pool, recovered.handoff_delivery).await.is_empty(), "{label}");
        assert_eq!(closure_audits(pool, sale).await, 0, "{label}");
    }

    store.postgres_pool().close().await;
}

async fn snapshot_at(
    store: &DatabaseStore,
    mode: DispositionReadMode,
) -> auction_application::operations::OperationsAlertSnapshot {
    store
        .clone()
        .with_runtime_switches(
            RuntimeSwitches::new(RuntimeSwitchRole::Worker, AuditFailureMode::Shadow, true)
                .with_disposition_read_mode_baseline(mode),
        )
        .snapshot(
            Utc::now(),
            OperationsAlertCollectionOverdueRead::CountWithItems(StdDuration::from_secs(3_600)),
        )
        .await
        .expect("读取告警快照")
}

/// 确认前，已寄出两小时的交付是长期未领取；确认后交付与卖方货款按精确收口事实退出长期未领取
/// 的计数与清单，分类口径不计失败，legacy 与 shadow 仍按迁移前口径计入两条失败。
#[tokio::test]
async fn postgres_equipment_recovered_leaves_failure_alerts_and_collection_lists() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let recovered: Recovered = insert_recovered_sale(pool, 7_800_201).await;
    insert_relist_target(pool, 7_800_201).await;

    let before = snapshot_at(&store, DispositionReadMode::Classified).await;
    let fulfillment_before = before.fulfillment.expect("履约水位");
    assert_eq!(fulfillment_before.collection_overdue, 1);
    assert_eq!(fulfillment_before.failed, 0);
    assert_eq!(
        before.collection_overdue_items.iter().map(|item| item.obligation_id).collect::<Vec<_>>(),
        vec![recovered.handoff]
    );

    store
        .confirm_recovery(confirmation(
            &recovered.sale,
            RecoveryConfirmationAction::EquipmentRecovered,
        ))
        .await
        .expect("确认 equipment_recovered");

    for (mode, failed) in [
        (DispositionReadMode::Classified, 0),
        (DispositionReadMode::Shadow, 2),
        (DispositionReadMode::Legacy, 2),
    ] {
        let after = snapshot_at(&store, mode).await;
        let fulfillment_after = after.fulfillment.expect("履约水位");
        assert_eq!(fulfillment_after.failed, failed, "{mode:?}");
        assert_eq!(fulfillment_after.collection_overdue, 0, "{mode:?}");
        assert_eq!(fulfillment_after.awaiting_collection, 0, "{mode:?}");
        assert!(after.collection_overdue_items.is_empty(), "{mode:?}");
    }

    store.postgres_pool().close().await;
}
