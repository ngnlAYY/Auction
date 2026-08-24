//! PostgreSQL 竞价计划的事务编排。

use super::super::*;

impl PostgresCompetitionTransaction {
    pub(crate) async fn apply_plan_inner(
        &mut self,
        plan: CompetitionPlan,
        request_id: Option<String>,
    ) -> Result<(), CompetitionTransactionError> {
        plan.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
        let command_id = plan_command_id(&plan)?;
        super::apply_proxy_changes(self, &plan).await?;
        for bid in &plan.public_bids {
            super::insert_bid(self, bid).await?;
            super::insert_audit_for_bid(self, bid, request_id.as_deref()).await?;
            super::insert_realtime_event(self, bid).await?;
        }
        super::insert_audit_for_proxy_changes(self, &plan, command_id, request_id.as_deref())
            .await?;
        super::update_lot(self, &plan).await
    }
}
