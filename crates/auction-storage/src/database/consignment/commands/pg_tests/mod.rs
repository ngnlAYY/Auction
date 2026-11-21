//! 由 PostgreSQL 集成测试入口驱动的寄售内部事务场景。

mod diagnostics;
mod fixture;
mod locks;
mod replays;
mod writes;

use crate::database::DatabaseStore;

impl DatabaseStore {
    /// 只供 `test-db` 集成入口在已迁移的临时数据库中调用内部寄售场景。
    #[doc(hidden)]
    pub async fn test_consignment_transactions(
        &self,
        maintenance: &Self,
        scenario: &str,
    ) -> Result<(), String> {
        let result = match scenario {
            "command_replay" => locks::command_replay(self, maintenance).await,
            "command_dual_target" => locks::command_dual_target(self, maintenance).await,
            "same_seller" => locks::same_seller(self, maintenance).await,
            "same_equipment" => locks::same_equipment(self, maintenance).await,
            "reversed_keys" => locks::reversed_keys(self, maintenance).await,
            "session_cleanup" => locks::session_cleanup(self, maintenance).await,
            "listing_row_is_locked" => locks::listing_row_is_locked(self, maintenance).await,
            "actual_lock_order" => locks::actual_lock_order(self, maintenance).await,
            "cancel_while_taking_locks" => {
                locks::cancel_while_taking_locks(self, maintenance).await
            }
            "retry_policy" => writes::retry_policy(self, maintenance).await,
            "compare_all_fields" => writes::compare_all_fields(self, maintenance).await,
            "atomic_combinations" => writes::atomic_combinations(self, maintenance).await,
            "missing_fact_rejected" => writes::missing_fact_rejected(self, maintenance).await,
            "constraint_mapping" => writes::constraint_mapping(self, maintenance).await,
            "error_detail_absent" => diagnostics::verify(self).await,
            "commit_unknown_sqlstate" => diagnostics::commit_unknown_sqlstate(self).await,
            "commit_unknown_recovers_by_replay" => {
                diagnostics::commit_unknown_recovers_by_replay(self, maintenance).await
            }
            "registration_replays_with_old_key" => {
                replays::registration_replays_with_old_key(self, maintenance).await
            }
            "admin_delist_requires_private_reason" => {
                replays::admin_delist_requires_private_reason(self, maintenance).await
            }
            "withdraw_replay" => replays::withdraw_replay(self, maintenance).await,
            "admin_delist_replay" => replays::admin_delist_replay(self, maintenance).await,
            "lookup_rescheduled_replay" => {
                replays::lookup_rescheduled_replay(self, maintenance).await
            }
            "auto_close_replay" => replays::auto_close_replay(self, maintenance).await,

            _ => return Err("未知寄售测试场景".into()),
        };
        result.map_err(|error| error.to_string())
    }
}
