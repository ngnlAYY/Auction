//! 约束覆盖用例的执行与清单核对。

use std::collections::BTreeSet;

use sqlx::PgPool;

use super::super::schema::additional_rows;
use super::super::support::{
    attempt_start_proof_and_custody, controls_notice_and_handoff, quarantine_release_fixture,
    seed_accepted, seed_settlement_recovery_handoff, succeeds,
};

#[derive(Clone, Copy)]
pub struct Case {
    pub table: &'static str,
    pub constraint: &'static str,
    pub state: &'static str,
    pub sql: &'static str,
}

pub async fn seed_graph(pool: &PgPool) {
    seed_accepted(pool).await;
    succeeds(pool, &additional_rows()).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    succeeds(pool, &controls_notice_and_handoff()).await;
    succeeds(pool, &quarantine_release_fixture()).await;
    seed_settlement_recovery_handoff(pool).await;
}

pub async fn run_case(pool: &PgPool, case: &Case) {
    let script = format!("ALTER TABLE public.{} DISABLE TRIGGER USER;\n{}", case.table, case.sql);
    let error = match super::super::support::execute(pool, &script).await {
        Ok(()) => panic!(
            "{} 应被约束 {} 拒绝，但事务已提交：{}",
            case.constraint, case.constraint, case.sql
        ),
        Err(error) => error,
    };
    let db_error = error.as_database_error().expect("期望服务端 SQLSTATE");
    assert_eq!(
        db_error.code().as_deref(),
        Some(case.state),
        "{} 的 SQLSTATE 不符；约束 {:?}；SQL：{}",
        case.constraint,
        db_error.constraint(),
        case.sql
    );
    assert_eq!(
        db_error.constraint(),
        Some(case.constraint),
        "{} 的约束名不符；SQL：{}",
        case.constraint,
        case.sql
    );
}

pub async fn assert_coverage(pool: &PgPool, cases: &[Case], exceptions: &[(&str, &str)]) {
    let actual: Vec<String> = sqlx::query_scalar(
        "SELECT conname::text FROM pg_constraint
         WHERE connamespace = 'public'::regnamespace
           AND conname LIKE 'c4\\_%' ESCAPE '\\'
           AND contype IN ('c', 'u', 'f')
         ORDER BY conname",
    )
    .fetch_all(pool)
    .await
    .expect("读取 c4 命名约束");
    let mut expected: BTreeSet<&str> = cases.iter().map(|case| case.constraint).collect();
    expected.extend(exceptions.iter().map(|(name, _)| *name));
    let missing: Vec<&str> =
        actual.iter().map(String::as_str).filter(|name| !expected.contains(name)).collect();
    assert!(missing.is_empty(), "以下命名约束没有违反行或豁免说明：{missing:?}");
    let stale: Vec<&&str> =
        expected.iter().filter(|name| !actual.iter().any(|actual| actual == **name)).collect();
    assert!(stale.is_empty(), "以下用例指向不存在的约束：{stale:?}");
}
