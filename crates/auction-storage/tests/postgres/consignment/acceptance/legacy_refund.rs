//! 旧在途退款验收（R4 §4.1 第 1、2 条）：隔离、补证、解除、拍卖原案件，全程不触碰寄售与资产外写。
//!
//! 保护不变量 3、4、6、8：旧退款退件只由拍卖原案件接收，寄售 intake、Take、Return、补寄与资金分录
//! 都不被创建；同一解除命令重放返回同一回执且不再写案件。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::settlement::SettlementReturnReason;
use auction_domain::CommandId;
use auction_domain::consignment::deposit::{LegacyAuctionOwner, ReleaseKind};
use auction_storage::postgres_migrate_to;
use serde_json::Value;
use sqlx::{AssertSqlSafe, PgPool};
use uuid::Uuid;

use super::super::deposit::envelopes::{repository, seed_base};
use super::super::deposit::owner_handoff::{
    MainReturn, assert_no_asset_side_effects, assert_return_case_written, count, create_quarantine,
    release, seed_buyer_refund,
};
use crate::support::TestDatabase;

/// 升级前后必须逐值不变的既有表；列集合取升级前（134）的列。
const LEGACY_TABLES: [&str; 6] = [
    "settlement_obligations",
    "settlement_obligation_fulfillments",
    "deliveries",
    "delivery_send_attempts",
    "inbound_mail",
    "mail_originals",
];

/// 寄售侧与资产外写侧在整个流程里必须保持为 0 的计数。
const ZERO_COUNTS: [&str; 11] = [
    "SELECT count(*) FROM public.consignment_contracts",
    "SELECT count(*) FROM public.consignment_deposit_items",
    "SELECT count(*) FROM public.consignment_custody_positions",
    "SELECT count(*) FROM public.mail_disposition_work",
    "SELECT count(*) FROM public.mail_disposition_write_starts",
    "SELECT count(*) FROM public.settlement_funding_entries",
    "SELECT count(*) FROM public.settlement_funding_returns",
    "SELECT count(*) FROM public.settlement_funding_allocations",
    "SELECT count(*) FROM public.settlement_obligations WHERE contract_id IS NOT NULL",
    // 寄售 Return 落点：只数寄售类 kind，拍卖原案件自身写入的 auction_* 终局与释放行不计入。
    "SELECT count(*) FROM public.mail_final_dispositions
     WHERE kind IN ('accepted', 'return_decided', 'archived_empty', 'acquire_return')",
    "SELECT count(*) FROM public.mail_quarantine_releases
     WHERE kind IN ('accepted', 'return_decided', 'archived_empty', 'acquire_return')",
];

async fn assert_all_zero(pool: &PgPool) {
    for query in ZERO_COUNTS {
        assert_eq!(count(pool, query).await, 0, "{query}");
    }
}

async fn max_migration(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT max(version) FROM public._sqlx_migrations")
        .fetch_one(pool)
        .await
        .expect("读取最高迁移号")
}

/// 一次事务解除并建拍卖原案件，随后用同一命令重放；返回案件数以便调用方继续断言。
async fn hand_off_to_original_case(database: &TestDatabase, pool: &PgPool, fixture: &MainReturn) {
    let repo = repository(database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(database, &repo, LegacyAuctionOwner::BuyerRefund, false).await;
    let deliveries_before = count(pool, "SELECT count(*) FROM public.deliveries").await;
    let attempts_before = count(pool, "SELECT count(*) FROM public.delivery_send_attempts").await;
    assert_all_zero(pool).await;

    let command_id = CommandId::from_uuid(Uuid::new_v4());
    let receipt =
        release(&repo, quarantine_id, observation_id, ReleaseKind::AuctionReturnCase, command_id)
            .await
            .expect("旧退款交接成功");
    assert_eq!(receipt.obligation_id, Some(fixture.obligation_id));
    assert_return_case_written(pool, fixture, SettlementReturnReason::ReturnedAssetsMatch).await;
    assert_no_asset_side_effects(pool, deliveries_before).await;
    assert_all_zero(pool).await;
    // 补寄与资产外写：没有新增投递与发送尝试。
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.delivery_send_attempts").await,
        attempts_before
    );

    let replay =
        release(&repo, quarantine_id, observation_id, ReleaseKind::AuctionReturnCase, command_id)
            .await
            .expect("同命令重放");
    assert_eq!(replay, receipt, "重放返回同一案件回执");
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 1);
    assert_all_zero(pool).await;
}

/// 在带寄售来源列的当前库上：隔离 → 补证 → 解除 → 拍卖原案件，寄售与外写计数全为 0，重放同一案件。
#[tokio::test]
async fn postgres_legacy_refund_hands_to_original_case_on_current_schema() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    assert!(max_migration(pool).await >= 135, "验收只在含寄售来源列的制品上有意义");
    seed_base(pool).await;
    let fixture = seed_buyer_refund(pool, 7_510_001).await;
    hand_off_to_original_case(&database, pool, &fixture).await;
    pool.close().await;
}

async fn legacy_columns(pool: &PgPool, table: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name = $1 ORDER BY ordinal_position",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .expect("读取列集合")
}

/// 只投影给定列的整表快照，逐行按文本排序，供升级前后逐值比较。
async fn snapshot(pool: &PgPool, table: &str, columns: &[String]) -> Value {
    sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT coalesce(jsonb_agg(projected ORDER BY projected::text), '[]'::jsonb)
         FROM (SELECT (SELECT jsonb_object_agg(entry.key, entry.value)
                       FROM jsonb_each(to_jsonb(t)) AS entry
                       WHERE entry.key = ANY($1)) AS projected
               FROM public.{table} AS t) AS rows"
    )))
    .bind(columns)
    .fetch_one(pool)
    .await
    .expect("读取表快照")
}

/// 升级版：用 134 建库，写入“升级前在途”的旧退款义务、首寄投递与原信，迁到当前最高版本后
/// 既有行逐值不变，再跑同一条交接流程。
#[tokio::test]
async fn postgres_legacy_refund_in_flight_across_upgrade_hands_to_original_case() {
    let database = TestDatabase::create_empty().await.expect("创建未迁移测试库");
    let maintenance = database.maintenance_store().await.expect("连接测试库 owner");
    postgres_migrate_to(maintenance.postgres_pool(), 134).await.expect("迁移到 134");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_buyer_refund(pool, 7_510_002).await;

    let mut before = Vec::new();
    for table in LEGACY_TABLES {
        let columns = legacy_columns(pool, table).await;
        let rows = snapshot(pool, table, &columns).await;
        if table != "delivery_send_attempts" {
            assert_ne!(rows, Value::Array(Vec::new()), "{table} 升级前必须有在途行");
        }
        before.push((table, columns, rows));
    }

    // i64::MAX 即“迁到当前最高版本”，不随后续迁移号写死。
    postgres_migrate_to(maintenance.postgres_pool(), i64::MAX).await.expect("迁移到当前最高版本");
    assert!(max_migration(pool).await >= 135);
    for (table, columns, rows) in &before {
        assert_eq!(&snapshot(pool, table, columns).await, rows, "{table} 升级后逐值不变");
    }

    hand_off_to_original_case(&database, pool, &fixture).await;
    pool.close().await;
}
