//! 000134 全部命名 CHECK / UQ / FK 的最小违反行覆盖。

mod custody;
mod custody_controls;
mod existing;
mod foreign_keys;
mod mail;
mod mail_owner;
mod reexamined;
mod support;
mod work;

use std::collections::BTreeSet;

use super::support::execute;
use crate::support::TestDatabase;
use support::{assert_coverage, run_case, seed_graph};

#[tokio::test]
async fn postgres_consignment_deposit_named_constraints_have_minimal_violations() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_graph(pool).await;

    let mut cases = Vec::new();
    cases.extend_from_slice(mail::CASES);
    cases.extend(mail_owner::cases());
    cases.extend(reexamined::cases());
    cases.extend(custody::cases());
    cases.extend_from_slice(custody_controls::CASES);
    cases.extend(foreign_keys::cases());
    cases.extend(work::cases());
    cases.extend_from_slice(existing::CASES);
    for case in &cases {
        run_case(pool, case).await;
    }

    let mut exceptions: Vec<(&str, &str)> = Vec::new();
    exceptions.extend_from_slice(mail::EXCEPTIONS);
    exceptions.extend_from_slice(custody::EXCEPTIONS);
    exceptions.extend_from_slice(foreign_keys::EXCEPTIONS);
    exceptions.extend_from_slice(work::EXCEPTIONS);
    exceptions.extend_from_slice(existing::EXCEPTIONS);
    assert_coverage(pool, &cases, &exceptions).await;

    let names: BTreeSet<&str> = cases.iter().map(|case| case.constraint).collect();
    assert_eq!(names.len(), cases.len(), "同一约束不应有重复用例");
}

#[tokio::test]
async fn postgres_consignment_handoff_asset_season_constraint_is_enforced() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_graph(pool).await;
    let error = execute(
        pool,
        "ALTER TABLE public.consignment_equipment_handoffs
             DISABLE TRIGGER consignment_equipment_handoffs_append_only;
         ALTER TABLE public.consignment_equipment_handoffs
             DISABLE TRIGGER consignment_equipment_handoffs_no_truncate;
         UPDATE public.consignment_equipment_handoffs
         SET season_id='frozen-season'
         WHERE id='a3000000-0000-4000-8000-000000000001';",
    )
    .await
    .expect_err("错误季应被触发器拒绝");
    let db_error = error.as_database_error().expect("期望服务端 SQLSTATE");
    assert_eq!(db_error.code().as_deref(), Some("23514"));
    assert_eq!(db_error.constraint(), Some("c4_handoff_edge_asset_season"));
}
