//! PostgreSQL 竞价计划的事务编排。

use super::super::*;

impl PostgresCompetitionTransaction {
    /// 外部 Public 命令只追加一条 Auction-BF 命令级审计；前置资格冻结传入 `None`。
    pub(crate) async fn apply_public_command_plan_inner(
        &mut self,
        plan: &CompetitionPlan,
        audit: Option<CompetitionCommandAuditContext>,
        occurred_at: DateTime<Utc>,
    ) -> Result<(), CompetitionTransactionError> {
        plan.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
        super::apply_proxy_changes(self, plan, occurred_at).await?;
        let mut auction_id = None;
        for bid in &plan.public_bids {
            super::insert_bid(self, bid, occurred_at).await?;
            super::insert_realtime_event(self, &mut auction_id, bid).await?;
        }
        super::update_lot(self, plan, occurred_at).await?;
        if let Some(audit) = audit {
            super::insert_audit_for_public_command(self, plan, audit).await?;
        }
        Ok(())
    }

    /// 生命周期 final 子命令暂时保留独立审计路径，不能被外部 Public 命令的闭集误删。
    pub(crate) async fn apply_plan_inner(
        &mut self,
        plan: &CompetitionPlan,
        request_id: Option<String>,
        occurred_at: DateTime<Utc>,
    ) -> Result<(), CompetitionTransactionError> {
        plan.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
        let command_id = plan_command_id(plan)?;
        super::apply_proxy_changes(self, plan, occurred_at).await?;
        let mut auction_id = None;
        for bid in &plan.public_bids {
            super::insert_bid(self, bid, occurred_at).await?;
            super::insert_audit_for_bid(self, bid, request_id.as_deref()).await?;
            super::insert_realtime_event(self, &mut auction_id, bid).await?;
        }
        super::insert_audit_for_proxy_changes(self, plan, command_id, request_id.as_deref())
            .await?;
        super::update_lot(self, plan, occurred_at).await
    }

    /// 慢速代理计划沿用竞价事实/事件写入，但使用 Worker 专属旧版审计语义。
    pub(crate) async fn apply_slow_proxy_plan_inner(
        &mut self,
        plan: &CompetitionPlan,
        context: SlowProxyExecutionContext,
    ) -> Result<(), CompetitionTransactionError> {
        plan.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
        context.validate()?;
        let occurred_at = context.executed_at;
        super::apply_proxy_changes(self, plan, occurred_at).await?;
        let mut auction_id = None;
        for bid in &plan.public_bids {
            super::insert_bid(self, bid, occurred_at).await?;
            super::insert_realtime_event(self, &mut auction_id, bid).await?;
        }
        super::update_lot(self, plan, occurred_at).await?;
        super::insert_audit_for_slow_proxy(self, plan, context).await
    }
}
