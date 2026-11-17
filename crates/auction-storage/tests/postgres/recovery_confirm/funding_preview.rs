//! funding 预览识别成交撤销的精确收口：main 收回、isekai 取消与 COD 三次退回之后，原付款、
//! 卖方货款与 isekai 交付的 `failed` 都带着同一恢复记录在原付款投递当前 fence 上追加的
//! `cod_returned` 处置，不再按不明责任阻断，原付款投递的外发也不再算未收口的收款。只认这套
//! 精确事实：失败码换成普通失败、或失败码相符却没有处置的履约照常阻断。

use auction_application::recovery::{
    RecoveryCommandRepository, RecoveryConfirmationAction, RecoveryResolutionStore,
};
use auction_application::settlement::FundingReconciliationReader;
use auction_storage::DatabaseStore;
use auction_time::Timestamp;
use sqlx::PgPool;

use super::fixture::{Sale, SaleSpec, insert_reclaiming_sale};
use super::{confirmation, fulfillment};
use crate::support;

/// 预览只读，不需要已有资金账户；UID 形状合法即可。
const PREVIEW_UID: &str = "7654321";

/// 撤销收口不成立时会出现的四类 lot 阻断。
const CLOSURE_BLOCKERS: [&str; 4] = [
    ":payment_state:unproven",
    ":seller_state:unproven",
    ":handoff_unproven",
    ":payment_collection_unclosed",
];

async fn sale(store: &DatabaseStore, world: &'static str, sequence: i64, count: i32) -> Sale {
    insert_reclaiming_sale(
        store.postgres_pool(),
        SaleSpec {
            world,
            sequence,
            cod_return_count: count,
            cod_return_limit: 3,
            blacklist_satisfied: true,
        },
    )
    .await
}

/// 预览中属于该 lot、表示收口不成立的阻断。
async fn closure_blockers(store: &DatabaseStore, sale: &Sale) -> Vec<String> {
    // 预览阻断里的 lot ID 按 JSON 文本输出（带引号），按包含匹配。
    let lot = sale.lot_id.to_string();
    let preview = store.preview_funding(PREVIEW_UID.to_owned()).await.expect("读取 funding 预览");
    preview
        .blockers
        .into_iter()
        .filter(|blocker| {
            blocker.starts_with("lot:")
                && blocker.contains(&lot)
                && CLOSURE_BLOCKERS.iter().any(|suffix| blocker.ends_with(suffix))
        })
        .collect()
}

/// 把履约改成失败码不符的普通失败（处置事实仍在）：不再是精确收口。
async fn plain_failure(pool: &PgPool, obligations: &[uuid::Uuid]) {
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments SET last_error_code = 'send_rejected'
         WHERE obligation_id = ANY($1)",
    )
    .bind(obligations)
    .execute(pool)
    .await
    .expect("改为普通失败");
}

/// main 收回：原付款 `recovery_reclaimed`、卖方货款 `recovery_sale_voided` 与 `cod_returned`
/// 处置一起成立时不阻断；失败码换成普通失败即恢复阻断。
#[tokio::test]
async fn postgres_funding_preview_recognizes_main_reclaim_closure() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let sale = sale(&store, "main", 7_900_001, 3).await;
    let before = closure_blockers(&store, &sale).await;
    assert!(
        before.iter().any(|blocker| blocker.ends_with(":payment_collection_unclosed")),
        "收回前原付款已外发、尚未收口：{before:?}"
    );

    store
        .confirm_recovery(confirmation(&sale, RecoveryConfirmationAction::Reclaimed))
        .await
        .expect("确认 reclaimed");
    assert_eq!(closure_blockers(&store, &sale).await, Vec::<String>::new());

    plain_failure(pool, &[sale.payment, sale.seller]).await;
    let after = closure_blockers(&store, &sale).await;
    for suffix in
        [":payment_state:unproven", ":seller_state:unproven", ":payment_collection_unclosed"]
    {
        assert!(after.iter().any(|blocker| blocker.ends_with(suffix)), "{suffix}：{after:?}");
    }

    pool.close().await;
}

/// isekai 取消：载体付款 `recovery_cancelled`、交付与卖方货款 `recovery_sale_voided` 都由同一
/// 处置收口；交付换成普通失败即恢复 `handoff_unproven`。
#[tokio::test]
async fn postgres_funding_preview_recognizes_isekai_cancel_closure() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let sale = sale(&store, "isekai", 7_900_002, 1).await;

    store
        .confirm_recovery(confirmation(&sale, RecoveryConfirmationAction::Cancelled))
        .await
        .expect("确认 cancelled");
    let handoff = sale.handoff.expect("isekai 交付义务");
    assert_eq!(
        fulfillment(pool, handoff).await,
        ("failed".to_owned(), Some("recovery_sale_voided".to_owned()))
    );
    assert_eq!(closure_blockers(&store, &sale).await, Vec::<String>::new());

    plain_failure(pool, &[handoff]).await;
    let after = closure_blockers(&store, &sale).await;
    assert!(after.iter().any(|blocker| blocker.ends_with(":handoff_unproven")), "{after:?}");

    pool.close().await;
}

/// COD 第三次退回：Worker 自动退回卖家，原付款 `cod_returned_three_times`、卖方货款
/// `equipment_returned_to_seller` 由同一处置收口；失败码相符却没有处置的另一笔成交照常阻断。
#[tokio::test]
async fn postgres_funding_preview_recognizes_three_return_closure() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let sale = sale(&store, "main", 7_900_003, 3).await;

    let advance = store
        .advance_due_cod_return(Timestamp::now())
        .await
        .expect("推进 COD 退回")
        .expect("达到上限的退回必须推进");
    assert_eq!(advance.resolution.as_deref(), Some("returned_after_three_cod_returns"));
    assert_eq!(
        fulfillment(pool, sale.seller).await,
        ("failed".to_owned(), Some("equipment_returned_to_seller".to_owned()))
    );
    assert_eq!(closure_blockers(&store, &sale).await, Vec::<String>::new());

    // 没有恢复来源处置的同码失败不是收口：另一笔成交只改履约失败码。
    let forged = sale_without_disposition(&store).await;
    let blockers = closure_blockers(&store, &forged).await;
    for suffix in
        [":payment_state:unproven", ":seller_state:unproven", ":payment_collection_unclosed"]
    {
        assert!(blockers.iter().any(|blocker| blocker.ends_with(suffix)), "{suffix}：{blockers:?}");
    }

    pool.close().await;
}

/// 失败码与三次退回收口相同、但没有任何处置事实的成交。
async fn sale_without_disposition(store: &DatabaseStore) -> Sale {
    let pool = store.postgres_pool();
    let forged = sale(store, "main", 7_900_004, 2).await;
    for (obligation, code) in [
        (forged.payment, "cod_returned_three_times"),
        (forged.seller, "equipment_returned_to_seller"),
    ] {
        sqlx::query(
            "UPDATE public.settlement_obligation_fulfillments
             SET execution_state = 'failed', last_error_code = $2, completed_at = now()
             WHERE obligation_id = $1",
        )
        .bind(obligation)
        .bind(code)
        .execute(pool)
        .await
        .expect("只改履约失败码");
    }
    forged
}
