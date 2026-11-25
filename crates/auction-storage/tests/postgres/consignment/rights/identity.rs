//! 权利 ID 的生产派生与迁移里的 `c7_right_id` 逐个对照（回填与新登记必须得到同一个 ID）。

use auction_domain::consignment::rights::{RightRole, right_id};
use uuid::Uuid;

use super::super::contracts::fixture::CONTRACT;
use crate::support::TestDatabase;

#[tokio::test]
async fn postgres_consignment_right_id_derivation_matches_the_sql_function() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let contract = Uuid::parse_str(CONTRACT).expect("合同");
    for role in [RightRole::Buyer, RightRole::Seller] {
        let from_sql: Uuid = sqlx::query_scalar("SELECT public.c7_right_id($1, $2)")
            .bind(contract)
            .bind(role.as_str())
            .fetch_one(owner.postgres_pool())
            .await
            .expect("SQL 派生");
        assert_eq!(right_id(contract, role), from_sql, "{role:?}");
    }
}
