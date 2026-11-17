//! 结算退件 Take 投递（`settlement_return_action`）归所属案件计数的 PostgreSQL 18 用例，以运行账号
//! 执行：投递积压、人工对账、失败与通知失败水位，以及导航待办的投递人工对账都不计入这类投递，
//! 由退件告警（`settlement_return_pending`/`settlement_return_take_unknown`）与案件待办承担；其他
//! 来源的同状态投递照常计数。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::delivery::{DeliveryTransportError, DeliveryWorkStore};
use auction_application::operations::{
    OperationsAlertCollectionOverdueRead, OperationsAlertConsole, OperationsAlertSnapshot,
    OperationsAlertSourceReader,
};
use auction_application::settlement::SettlementReturnWorkStore;
use auction_domain::SettlementReturnAction;
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use super::sweep::{seed_mail_session, take_into_sending, unknown_take};
use crate::support;

async fn snapshot(store: &auction_storage::DatabaseStore) -> OperationsAlertSnapshot {
    store
        .snapshot(Utc::now(), OperationsAlertCollectionOverdueRead::Skip)
        .await
        .expect("读取告警快照")
}

/// （积压, 人工对账, 失败）三项投递水位与通知失败数。
fn delivery_counts(snapshot: &OperationsAlertSnapshot) -> (i64, i64, i64, i64) {
    let delivery = snapshot.delivery.as_ref().expect("投递水位");
    let notification = snapshot.notification.as_ref().map_or(0, |value| value.failed_count);
    (delivery.pending, delivery.manual_reconcile, delivery.failed, notification)
}

/// （已绑定待处理, 隔离待核对, Take 结果未知合计）三类退件告警计数。
fn return_counts(snapshot: &OperationsAlertSnapshot) -> (i64, i64, i64) {
    let fulfillment = snapshot.fulfillment.as_ref().expect("履约水位");
    (
        fulfillment.settlement_return_pending,
        fulfillment.settlement_return_quarantined,
        fulfillment.settlement_return_take_unknown.iter().map(|reason| reason.count).sum(),
    )
}

/// Take 结果未知按原因的计数（只列非零原因）。
fn take_unknown_reasons(snapshot: &OperationsAlertSnapshot) -> Vec<(&'static str, i64)> {
    let fulfillment = snapshot.fulfillment.as_ref().expect("履约水位");
    auction_domain::SettlementReturnTakeUnknownReason::ALL
        .iter()
        .zip(fulfillment.settlement_return_take_unknown)
        .filter(|(_, watermark)| watermark.count > 0)
        .map(|(reason, watermark)| (reason.as_str(), watermark.count))
        .collect()
}

/// Take 投递在积压、结果未知与确定失败三种状态下都只按案件计数；导航待办的投递人工对账不含它，
/// 同状态的入站 Take 投递仍照常计数。
#[tokio::test]
async fn postgres_return_take_deliveries_count_only_under_their_case() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let session = seed_mail_session(pool).await;

    // 结果未知但正常收口尚未接手：Take 投递在人工对账，案件仍是领回中（待处理）。
    let _unknown = unknown_take(
        &store,
        &session,
        7_620_001,
        "996001",
        SettlementReturnAction::ReclaimAndHold,
        false,
    )
    .await;
    let counts = store.pending_counts().await.expect("导航待办");
    assert_eq!((counts.delivery_manual_reconcile, counts.settlement_returns), (0, 1));
    let observed = snapshot(&store).await;
    assert_eq!(delivery_counts(&observed), (0, 0, 0, 0), "Take 投递不进投递水位");
    assert_eq!(return_counts(&observed), (1, 0, 0), "由退件待处理告警承担");
    store.settle_due_return_take(Utc::now()).await.expect("收口").expect("必须收口");
    let observed = snapshot(&store).await;
    assert_eq!(delivery_counts(&observed), (0, 0, 0, 0));
    assert_eq!(return_counts(&observed), (0, 0, 1), "结果未知由结果未知告警承担");
    assert_eq!(store.pending_counts().await.expect("导航待办").delivery_manual_reconcile, 0);

    // 第二件：Take 已激活待执行（积压），再确定失败；两种状态都不计入投递积压、失败或通知失败。
    let _failed = unknown_take(
        &store,
        &session,
        7_620_002,
        "996002",
        SettlementReturnAction::ReclaimAndHold,
        true,
    )
    .await;
    assert_eq!(return_counts(&snapshot(&store).await), (0, 0, 2));
    let third = super::flows::returned_payout(&store, 7_620_003, "996003").await;
    super::flows::seed_spent_payout(pool, &third).await;
    super::flows::handle(&store, third.case, 1, SettlementReturnAction::ReclaimAndHold).await;
    store
        .activate_due_return_take(Utc::now(), super::take_worlds())
        .await
        .expect("激活")
        .expect("必须激活");
    let observed = snapshot(&store).await;
    assert_eq!(delivery_counts(&observed), (0, 0, 0, 0), "待执行的 Take 不算投递积压");
    assert_eq!(return_counts(&observed), (1, 0, 2));
    let take_delivery: Uuid = sqlx::query_scalar(
        "SELECT delivery_id FROM public.settlement_return_actions WHERE return_mail_id = $1",
    )
    .bind(third.case)
    .fetch_one(pool)
    .await
    .expect("读取 Take 投递");
    let expectation = auction_application::delivery::DeliveryTakeExpectation {
        world_id: "main".to_owned(),
        external_mail_id: "996003".to_owned(),
        credential: Some(session.binding()),
    };
    let claim = take_into_sending(&store, take_delivery, &expectation).await;
    // 确定失败但已开始写出（不能证明未外写）：不重启，案件转结果未知。确定未外写的失败由案件
    // owner 重新排队，见 `restart` 用例。
    DeliveryWorkStore::fail(
        &store,
        claim,
        DeliveryTransportError {
            failure_code: "take_remote_rejected".to_owned(),
            safe_detail: "Take 远端明确拒绝".to_owned(),
            outcome_unknown: false,
            external_reference: None,
            result_metadata: json!({"operation": "take", "write_started": true}),
        },
        Utc::now(),
    )
    .await
    .expect("确定失败收口");
    store.settle_due_return_take(Utc::now()).await.expect("收口").expect("必须收口");
    let observed = snapshot(&store).await;
    assert_eq!(delivery_counts(&observed), (0, 0, 0, 0), "失败的 Take 不算投递或通知失败");
    assert_eq!(return_counts(&observed), (0, 0, 3));
    // 原会话还没有观察的两件等待邮箱维护复核；以失败终结的 Take 巡检不复核，原因单列。
    assert_eq!(
        take_unknown_reasons(&observed),
        vec![("take_unverifiable", 1), ("awaiting_observation", 2)]
    );
    assert_eq!(store.pending_counts().await.expect("导航待办").settlement_returns, 3);

    // 对照：入站流程同状态的 Take 投递照常计入投递人工对账与导航待办。
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             fencing_version, attempt_count, sending_at, completed_at, failure_code)
         VALUES ($1, $2, decode(repeat('5d', 32), 'hex'), 'mail_intake', $3, 'SellerPlayer',
                 jsonb_build_object('schema_version', 1, 'operation', 'take', 'world', 'main',
                     'external_mail_id', '996099', 'attachment_ids', jsonb_build_array('4401')),
                 'manual_reconcile', 1, 1, now(), now(), 'write_unknown')",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4().to_string())
    .execute(pool)
    .await
    .expect("写入入站 Take 投递");
    assert_eq!(store.pending_counts().await.expect("导航待办").delivery_manual_reconcile, 1);
    assert_eq!(delivery_counts(&snapshot(&store).await).1, 1, "入站 Take 照常计入人工对账");

    pool.close().await;
}
