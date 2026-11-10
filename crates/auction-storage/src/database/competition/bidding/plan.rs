//! PostgreSQL 竞价计划的事务编排。

use crate::database::competition::{
    PostgresCompetitionTransaction, invalid_state, plan_command_id,
};
use auction_application::core::{
    CompetitionCommandAuditContext, CompetitionTransactionError, SlowProxyExecutionContext,
};
use auction_domain::CompetitionPlan;
use chrono::{DateTime, Utc};
use std::sync::Arc;

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
        // 普通出价也可能在本次事务内耗尽/击败慢速代理；不能把 pending
        // 提醒只留给提交后的 outbox，否则“同一事务入队”不成立。
        let notifications = Arc::clone(&self.notifications);
        let public_base_url = self.public_base_url.clone();
        let mail_templates = Arc::clone(&self.mail_templates);
        let pending_wakeup = crate::database::lot_watch::resolve_pending(
            self.transaction_mut()?,
            plan.final_lot.lot_id.as_uuid(),
            occurred_at,
            notifications.as_ref(),
            public_base_url.as_deref(),
            mail_templates.as_ref(),
        )
        .await?;
        self.delivery_wakeup |= pending_wakeup;
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
        // 慢代理计划不产生 `bid_accepted` outbox 事件；反超成功要作废待定提醒，
        // 到期/Final 耗尽要在同一事务入队。resolve_pending 同时覆盖两种结果。
        let notifications = Arc::clone(&self.notifications);
        let public_base_url = self.public_base_url.clone();
        let mail_templates = Arc::clone(&self.mail_templates);
        let pending_wakeup = crate::database::lot_watch::resolve_pending(
            self.transaction_mut()?,
            plan.final_lot.lot_id.as_uuid(),
            occurred_at,
            notifications.as_ref(),
            public_base_url.as_deref(),
            mail_templates.as_ref(),
        )
        .await?;
        self.delivery_wakeup |= pending_wakeup;
        super::insert_audit_for_slow_proxy(self, plan, context).await
    }
}
