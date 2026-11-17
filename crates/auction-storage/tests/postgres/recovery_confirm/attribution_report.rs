//! 恢复报告给出发送归属不成立的具体原因（PostgreSQL 18，运行账号）：报告在只读快照内按确认与
//! Worker 推进同一判定求值，归属成立时为空；原因随新的持久事实（Sent 观察到的远端 MID）消失。

use auction_application::recovery::{RecoveryReportReader, RecoveryResolutionStore};
use auction_domain::AuctionId;
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};

use super::binding::{created_at, freeze_current_attempt, insert_attempt};
use super::fixture::{Sale, SaleSpec, insert_reclaiming_sale};
use crate::support;

async fn blocker(store: &DatabaseStore, sale: &Sale) -> Option<String> {
    store
        .recovery_report(AuctionId::from_uuid(sale.auction_id))
        .await
        .expect("读取恢复报告")
        .expect("场次存在")
        .recoveries
        .into_iter()
        .find(|entry| entry.id.as_uuid() == sale.recovery_id)
        .expect("报告包含该恢复记录")
        .send_attribution_blocker
}

/// 归属成立时没有原因；退件之后又有一次确定未外写的尝试时，Worker 阻断与报告给出同一原因
/// `send_after_return`；被退回那次发送补上 Sent MID 后原因消失；原投递进入发送时为
/// `send_in_flight`。
#[tokio::test]
async fn postgres_recovery_report_explains_send_attribution_blockers() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let sale = insert_reclaiming_sale(
        pool,
        SaleSpec {
            world: "main",
            sequence: 7_920_001,
            cod_return_count: 3,
            cod_return_limit: 3,
            blacklist_satisfied: true,
        },
    )
    .await;
    assert_eq!(blocker(&store, &sale).await, None, "原始夹具的归属成立");

    let at = created_at(pool, sale.recovery_id).await;
    insert_attempt(pool, sale.payment_delivery, 1, at + Duration::minutes(5)).await;
    freeze_current_attempt(pool, sale.payment_delivery, 2, at + Duration::minutes(40)).await;
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'failed', failure_code = 'send_rejected', external_reference = NULL,
             completed_at = sending_at,
             result_metadata = '{\"operation\":\"send\",\"write_started\":false,\
                                 \"outcome_unknown\":false}'::jsonb
         WHERE id = $1",
    )
    .bind(sale.payment_delivery)
    .execute(pool)
    .await
    .expect("退件之后确定未外写的一次尝试");
    assert_eq!(blocker(&store, &sale).await.as_deref(), Some("send_after_return"));
    store.advance_due_cod_return(Timestamp::now()).await.expect("推进").expect("阻断");
    assert_eq!(blocker(&store, &sale).await.as_deref(), Some("send_after_return"));

    sqlx::query(
        "INSERT INTO public.delivery_sent_bindings
            (world_id, external_mail_id, delivery_id, fencing_version, expectation_fencing_version)
         VALUES ('main', '882001', $1, 1, 1)",
    )
    .bind(sale.payment_delivery)
    .execute(pool)
    .await
    .expect("补上 Sent 绑定");
    assert_eq!(blocker(&store, &sale).await, None, "Sent MID 证明归属成立");

    sqlx::query(
        "UPDATE public.deliveries SET status = 'sending', failure_code = NULL, completed_at = NULL,
             result_metadata = '{}'::jsonb, lease_token = gen_random_uuid(),
             lease_expires_at = now() + interval '5 minutes'
         WHERE id = $1",
    )
    .bind(sale.payment_delivery)
    .execute(pool)
    .await
    .expect("原投递进入发送");
    assert_eq!(blocker(&store, &sale).await.as_deref(), Some("send_in_flight"));

    pool.close().await;
}
