//! 000137 追加段的数据库最后一道门：退款类处置要求已转换的退款、补寄要求已消费的上一轮，
//! 账本只接受形状正确的 recheck / resolve 回执。

use super::super::contracts::fixture::CONTRACT;
use super::super::deposit::support::{fails, succeeds};
use super::super::ledger136::fixture::{Intake, seed_purchased};
use super::super::recovery::command_ids;
use crate::support::TestDatabase;

fn resolution_sql(action: &str, evidence: &str, dispatch: &str, command: &str) -> String {
    format!(
        "INSERT INTO public.consignment_order_resolutions
            (resolution_id, contract_id, action, evidence_id, dispatch_id, contract_version,
             resolved_by, command_id, resolved_at)
         VALUES ('7e300000-0000-4000-8000-000000000001', '{CONTRACT}', '{action}', '{evidence}',
                 {dispatch}, 1, 'fake-admin', '{command}', '2026-01-03T00:00:00Z');"
    )
}

fn ledger_sql(command: &str, operation: &str, body: &str, status: u16) -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{command}', 2, 'admin', 'fake-admin', '{operation}', '{CONTRACT}',
                 decode(repeat('e7', 32), 'hex'), 'contract', '{CONTRACT}', NULL, {status}, 'ok',
                 '{body}', '2026-01-03T00:00:00Z');"
    )
}

/// 反例：没有已转换的退款，退款类处置被拒；上一轮没有已消费的原件退件接管，补寄处置被拒。
#[tokio::test]
async fn postgres_consignment_order_resolution_table_guards_refund_and_redispatch_preconditions() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_purchased(pool).await;
    let commands = command_ids(pool, 1).await;
    let intake = Intake { n: 1, leg: "cash_refund" }.intake();
    fails(
        pool,
        &resolution_sql("refund_lost", &intake, "NULL", &commands[0]),
        "23514",
        Some("c7_resolution_refund_converted"),
    )
    .await;
    fails(
        pool,
        &resolution_sql(
            "redispatch_original",
            &intake,
            "'7e400000-0000-4000-8000-000000000001'",
            &commands[0],
        ),
        "23514",
        Some("c7_resolution_redispatch_closed"),
    )
    .await;
    // 轮 ID 只属于补寄：非补寄动作带轮 ID，形状由 CHECK 拒绝。
    fails(
        pool,
        &resolution_sql(
            "cancel_unpaid",
            &intake,
            "'7e400000-0000-4000-8000-000000000001'",
            &commands[0],
        ),
        "23514",
        Some("c7_resolution_dispatch_ck"),
    )
    .await;
}

/// 账本回执形状：缺 work_id / resolution_id 的 recheck / resolve 回执被拒；形状正确的通过。
#[tokio::test]
async fn postgres_consignment_order_ledger_rejects_malformed_recheck_and_resolve_receipts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_purchased(pool).await;
    let id = |n: u32| format!("7e5{n:05}-0000-4000-8000-000000000001");
    let head = |command: &str, operation: &str, disposition: &str| {
        format!(
            "\"schema_version\":2,\"operation\":\"{operation}\",\"disposition\":\"{disposition}\",\"command_id\":\"{command}\",\"contract_id\":\"{CONTRACT}\",\"contract_version\":1"
        )
    };
    let (a, b, c) = (id(1), id(2), id(3));
    fails(
        pool,
        &ledger_sql(&a, "recheck", &format!("{{{}}}", head(&a, "recheck", "recheck_queued")), 202),
        "23514",
        Some("consignment_command_receipt"),
    )
    .await;
    fails(
        pool,
        &ledger_sql(
            &b,
            "resolve",
            &format!(
                "{{{},\"action\":\"refund_lost\"}}",
                head(&b, "resolve", "resolution_recorded")
            ),
            200,
        ),
        "23514",
        Some("consignment_command_receipt"),
    )
    .await;
    succeeds(
        pool,
        &ledger_sql(
            &c,
            "recheck",
            &format!(
                "{{{},\"work_id\":\"7e600000-0000-4000-8000-000000000001\"}}",
                head(&c, "recheck", "recheck_queued")
            ),
            202,
        ),
    )
    .await;
}
