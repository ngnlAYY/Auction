//! 寄售存储公共底座的 PG18 集成测试入口。

use crate::support::TestDatabase;

async fn run_scenario(scenario: &str) {
    let database =
        TestDatabase::create().await.unwrap_or_else(|error| panic!("建临时数据库失败：{error}"));
    let maintenance = database
        .maintenance_store()
        .await
        .unwrap_or_else(|error| panic!("连接维护账号失败：{error}"));
    let store = database.store().await.unwrap_or_else(|error| panic!("连接运行账号失败：{error}"));
    store
        .test_consignment_transactions(&maintenance, scenario)
        .await
        .unwrap_or_else(|error| panic!("寄售事务场景失败：{error}"));
}

// 测试函数名带 `postgres_` 前缀，标准入口 `mise run test-db` 才会选中；场景名仍是源码内分派用的原名。
macro_rules! scenario {
    ($test:ident, $scenario:literal) => {
        #[tokio::test]
        async fn $test() {
            run_scenario($scenario).await;
        }
    };
}

scenario!(postgres_command_replay, "command_replay");
scenario!(postgres_command_dual_target, "command_dual_target");
scenario!(postgres_same_seller, "same_seller");
scenario!(postgres_same_equipment, "same_equipment");
scenario!(postgres_reversed_keys, "reversed_keys");
scenario!(postgres_session_cleanup, "session_cleanup");
scenario!(postgres_retry_policy, "retry_policy");
scenario!(postgres_compare_all_fields, "compare_all_fields");
scenario!(postgres_atomic_combinations, "atomic_combinations");
scenario!(postgres_missing_fact_rejected, "missing_fact_rejected");

#[tokio::test]
#[should_panic(expected = "会写入进程级审计健康计数的场景只能在 postgres_audit 测试目标中运行")]
async fn postgres_rejects_audit_failure_scenario() {
    run_scenario("audit_failure_rolls_back").await;
}

scenario!(postgres_constraint_mapping, "constraint_mapping");
scenario!(postgres_anonymous_registration, "anonymous_registration");
scenario!(postgres_error_detail_absent, "error_detail_absent");
scenario!(postgres_commit_unknown_sqlstate, "commit_unknown_sqlstate");
scenario!(postgres_commit_unknown_recovers_by_replay, "commit_unknown_recovers_by_replay");
scenario!(postgres_registration_replays_with_old_key, "registration_replays_with_old_key");
scenario!(postgres_admin_delist_requires_private_reason, "admin_delist_requires_private_reason");
scenario!(postgres_withdraw_replay, "withdraw_replay");
scenario!(postgres_admin_delist_replay, "admin_delist_replay");
scenario!(postgres_lookup_rescheduled_replay, "lookup_rescheduled_replay");
scenario!(postgres_auto_close_replay, "auto_close_replay");
scenario!(postgres_listing_row_is_locked, "listing_row_is_locked");
scenario!(postgres_actual_lock_order, "actual_lock_order");
scenario!(postgres_cancel_while_taking_locks, "cancel_while_taking_locks");
