//! 寄售存储公共底座的 PG18 集成测试入口。

use crate::support::TestDatabase;

macro_rules! scenario {
    ($name:ident) => {
        #[tokio::test]
        async fn $name() {
            let database = TestDatabase::create().await.expect("建临时数据库");
            let maintenance = database.maintenance_store().await.expect("连接维护账号");
            let store = database.store().await.expect("连接运行账号");
            store
                .test_consignment_transactions(&maintenance, stringify!($name))
                .await
                .expect("寄售事务场景通过");
        }
    };
}

scenario!(command_replay);
scenario!(command_dual_target);
scenario!(same_seller);
scenario!(same_equipment);
scenario!(reversed_keys);
scenario!(session_cleanup);
scenario!(retry_policy);
scenario!(compare_all_fields);
scenario!(atomic_combinations);
scenario!(missing_fact_rejected);
scenario!(audit_failure_rolls_back);
scenario!(constraint_mapping);
scenario!(error_detail_absent);
scenario!(commit_unknown_sqlstate);
scenario!(commit_unknown_recovers_by_replay);
scenario!(registration_replays_with_old_key);
scenario!(admin_delist_requires_private_reason);
scenario!(withdraw_replay);
scenario!(admin_delist_replay);
scenario!(lookup_rescheduled_replay);
scenario!(auto_close_replay);
scenario!(listing_row_is_locked);
scenario!(actual_lock_order);
scenario!(cancel_while_taking_locks);
