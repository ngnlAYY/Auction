//! 从 135 升级到 136：既有拍卖义务、履约、投递与资金分配行逐字节不变，新列为空；旧分配键仍然唯一。

use auction_storage::postgres_migrate_to;
use sqlx::PgPool;

use super::super::contracts::upgrade::{LOT, legacy_rows, seed_legacy_settlement};
use super::super::deposit::support::{fails, succeeds};
use crate::support::TestDatabase;

async fn allocation_row(pool: &PgPool) -> String {
    sqlx::query_scalar(
        "SELECT (to_jsonb(row) - 'consignment_contract_id')::text
         FROM public.settlement_funding_allocations AS row WHERE lot_id IS NOT NULL",
    )
    .fetch_one(pool)
    .await
    .expect("读取旧分配行")
}

#[tokio::test]
async fn postgres_consignment_ledger136_upgrade_from_135_keeps_legacy_rows_and_allocation_key() {
    let database = TestDatabase::create_empty().await.expect("创建空库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    postgres_migrate_to(pool, 135).await.expect("迁移到 135");
    seed_legacy_settlement(pool).await;
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.settlement_funding_accounts (uid, status) VALUES ('8101', 'ready');
             INSERT INTO public.settlement_funding_allocations (id, uid, purpose, lot_id, amount)
             VALUES ('c6f10000-0000-4000-8000-000000000001', '8101', 'seller', '{LOT}', 5);"
        ),
    )
    .await;
    let before = (legacy_rows(pool).await, allocation_row(pool).await);

    postgres_migrate_to(pool, 136).await.expect("迁移到 136");

    assert_eq!(
        (legacy_rows(pool).await, allocation_row(pool).await),
        before,
        "既有义务、履约、投递与分配行必须逐字节不变"
    );
    let (contract_is_null, version): (bool, i64) = sqlx::query_as(
        "SELECT consignment_contract_id IS NULL,
                (SELECT max(version) FROM public._sqlx_migrations)
         FROM public.settlement_funding_allocations WHERE lot_id IS NOT NULL",
    )
    .fetch_one(pool)
    .await
    .expect("读取升级后的分配行");
    assert_eq!((contract_is_null, version), (true, 136));

    // 反例：拍卖分配的旧键 (uid, purpose, lot) 在新键下仍唯一。
    fails(
        pool,
        &format!(
            "INSERT INTO public.settlement_funding_allocations (id, uid, purpose, lot_id, amount)
             VALUES ('c6f10000-0000-4000-8000-000000000002', '8101', 'seller', '{LOT}', 1);"
        ),
        "23505",
        Some("c6_allocation_key_uq"),
    )
    .await;
    // 正例：同 UID、不同用途的账户级分配不受影响。
    succeeds(
        pool,
        "INSERT INTO public.settlement_funding_allocations (id, uid, purpose, amount)
         VALUES ('c6f10000-0000-4000-8000-000000000003', '8101', 'proprietary', 1);",
    )
    .await;
}
