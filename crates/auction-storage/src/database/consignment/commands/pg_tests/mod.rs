//! 由 PostgreSQL 集成测试入口驱动的寄售内部事务场景。

mod diagnostics;
mod fixture;
mod locks;
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
            "retry_policy" => writes::retry_policy(self, maintenance).await,
            "compare_all_fields" => writes::compare_all_fields(self, maintenance).await,
            "atomic_combinations" => writes::atomic_combinations(self, maintenance).await,
            "missing_fact_rejected" => writes::missing_fact_rejected(self, maintenance).await,
            "audit_failure_rolls_back" => writes::audit_failure_rolls_back(self, maintenance).await,
            "constraint_mapping" => writes::constraint_mapping(self, maintenance).await,
            "error_detail_absent" => diagnostics::verify(self).await,
            "commit_unknown_sqlstate" => diagnostics::commit_unknown_sqlstate(self).await,
            "system_identity" => writes::system_identity(),
            _ => return Err("未知寄售测试场景".into()),
        };
        result.map_err(|error| error.to_string())
    }
}
