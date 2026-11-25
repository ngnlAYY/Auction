//! 推进循环的一次扫描按键集分页走完全部待办（PIWQ，统筹方裁定）：排在前面、推进不了的轮不会让最后一轮饿死；
//! 游标只在本次扫描内，不作为事实来源。
//!
//! 夹具用 `ledger136` 的已购种子：同一成交上退款轮（起写较早、远端未领取）与原件 COD 轮（起写较晚、远端已领取）
//! 同时待办；页大小取 1，一次扫描必须读到第二页并记下原件 COD 的领取证据。

use std::sync::Arc;

use auction_application::consignment::DispatchService;
use auction_domain::consignment::dispatch::{AttemptKey, dispatch_attempt_id};
use uuid::Uuid;

use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::{
    PAY_ATTEMPT, PAY_DELIVERY, PAY_MID, PURCHASED_AT, REFUND_ATTEMPT, REFUND_DELIVERY,
    original_dispatch_sql, refund_dispatch_sql, seed_purchased,
};
use super::fixture::{Collector, collection_runtime};
use crate::support::TestDatabase;

/// 证据写口按投递与 fence 派生尝试 ID；种子里的固定尝试 ID 换成派生值，fence 为 1。
fn derived_attempt(delivery: &str) -> String {
    let delivery_id = Uuid::parse_str(delivery).expect("夹具投递");
    dispatch_attempt_id(&AttemptKey { delivery_id, fence: 1 }).to_string()
}

/// 退款轮的起写时刻：早于原件轮，键集顺序为 [退款, 原件]。
const REFUND_STARTED: &str = "2026-01-01T03:00:00Z";

#[tokio::test]
async fn postgres_consignment_collection_scan_pages_through_to_the_last_round() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed_purchased(pool).await;
    succeeds(
        pool,
        &(refund_dispatch_sql()
            .replace(PURCHASED_AT, REFUND_STARTED)
            .replace(REFUND_ATTEMPT, &derived_attempt(REFUND_DELIVERY))
            + &original_dispatch_sql().replace(PAY_ATTEMPT, &derived_attempt(PAY_DELIVERY))
            + &format!(
                "UPDATE public.deliveries
                 SET status = 'succeeded', sending_at = '{PURCHASED_AT}',
                     completed_at = '{PURCHASED_AT}', attempt_count = 1, fencing_version = 1,
                     external_reference = 'mooglemail:main:send:' || id::text
                 WHERE id IN ('{PAY_DELIVERY}', '{REFUND_DELIVERY}');
                 UPDATE public.deliveries
                 SET activation_command_id = 'c6800000-0000-4000-8000-000000000001',
                     activation_request_digest = decode(repeat('c3', 32), 'hex'),
                     activated_at = '{PURCHASED_AT}'
                 WHERE id = '{PAY_DELIVERY}';"
            )),
    )
    .await;

    let service = Arc::new(DispatchService::new(Arc::new(runtime.clone())));
    let collector = Collector::collecting_only(PAY_MID);
    let round = collection_runtime(&runtime, service, &collector, 1).run_once().await;
    assert_eq!(collector.reads(), 2, "一次扫描读完两页");
    assert!(round.advanced >= 1, "最后一轮被推进");
    let recorded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_dispatch_evidence WHERE category = 'original_claim'",
    )
    .fetch_one(pool)
    .await
    .expect("证据数");
    assert_eq!(recorded, 1, "排在最后的原件 COD 轮记下领取证据");
}
