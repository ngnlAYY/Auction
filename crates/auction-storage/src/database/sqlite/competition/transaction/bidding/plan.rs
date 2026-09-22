//! SQLite 竞价计划的事务编排。

use super::super::*;

impl SqliteCompetitionTransaction {
    /// Lite 与 PostgreSQL 一样按外部 Public 命令写唯一旧版审计。
    pub(crate) async fn apply_public_command_plan_inner(
        &mut self,
        plan: CompetitionPlan,
        audit: Option<CompetitionCommandAuditContext>,
        occurred_at: DateTime<Utc>,
    ) -> Result<(), CompetitionTransactionError> {
        plan.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
        super::apply_proxy_changes(self, &plan).await?;
        for bid in &plan.public_bids {
            super::insert_bid(self, bid, occurred_at).await?;
            super::insert_realtime_event(self, bid).await?;
        }
        super::update_lot(self, &plan, occurred_at).await?;
        if let Some(audit) = audit {
            super::insert_audit_for_public_command(self, &plan, audit).await?;
        }
        Ok(())
    }

    /// Lite 使用同一 `BEGIN IMMEDIATE` 写入慢速代理计划和 Worker 专属审计。
    pub(crate) async fn apply_slow_proxy_plan_inner(
        &mut self,
        plan: CompetitionPlan,
        context: SlowProxyExecutionContext,
    ) -> Result<(), CompetitionTransactionError> {
        plan.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
        context.validate()?;
        let occurred_at = context.executed_at;
        super::apply_proxy_changes(self, &plan).await?;
        for bid in &plan.public_bids {
            super::insert_bid(self, bid, occurred_at).await?;
            super::insert_realtime_event(self, bid).await?;
        }
        super::update_lot(self, &plan, occurred_at).await?;
        super::insert_audit_for_slow_proxy(self, &plan, context).await
    }
}
