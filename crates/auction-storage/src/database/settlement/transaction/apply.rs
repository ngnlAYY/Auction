//! 结算计划在锁内的应用：关闭 lot、写入义务、补偿账本、审计与实时事件。

use std::collections::HashSet;

use crate::database::PostgresCompetitionTransaction;
use crate::database::settlement::{
    close_settlement_lots, i32_version, i64_version, insert_compensation_ledger,
    insert_lot_settlements, insert_obligations, insert_recoveries, insert_settlement_audit,
    insert_settlement_realtime, invalid, map_competition_error, map_sql_error, positive_u64,
};
use auction_application::settlement::SettlementTransactionError;
use auction_domain::{
    AuctionId, CommandId, CompetitionPlan, ProxyStatus, RecoveryPolicy, SettlementPlan,
};
use chrono::{DateTime, Utc};
use sqlx::Row;

impl PostgresCompetitionTransaction {
    pub(in crate::database::settlement) async fn apply_settlement_inner(
        &mut self,
        auction_id: AuctionId,
        command_id: CommandId,
        plan: &SettlementPlan,
        proxy_closure_plans: Vec<CompetitionPlan>,
        recovery_policy: RecoveryPolicy,
        occurred_at: DateTime<Utc>,
    ) -> Result<(), SettlementTransactionError> {
        recovery_policy.validate().map_err(|_| invalid("恢复策略快照校验失败"))?;
        for line in &plan.lines {
            line.validate().map_err(|_| invalid("结算明细校验失败"))?;
        }
        for obligation in &plan.obligations {
            obligation.validate().map_err(|_| invalid("结算义务校验失败"))?;
        }
        validate_proxy_closure_plans(command_id, plan, &proxy_closure_plans)?;

        let auction_state_version = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let row = sqlx::query(
                r#"SELECT state_version
                   FROM public.auctions
                   WHERE id = $1 AND status = 'settling'
                   FOR UPDATE"#,
            )
            .bind(auction_id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?
            .ok_or(SettlementTransactionError::NotFound)?;
            positive_u64(
                row.try_get::<i64, _>("state_version").map_err(map_sql_error)?,
                "拍卖状态版本",
            )?
        };
        // 两个世界共用一个以 main Credits 计的补偿池；各世界结算在这一行上串行。
        let current_pool_version = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let row = sqlx::query(
                r#"SELECT balance, state_version
                   FROM public.compensation_pool
                   FOR UPDATE"#,
            )
            .fetch_one(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
            let current_balance: i64 = row.try_get("balance").map_err(map_sql_error)?;
            if current_balance != plan.opening_pool.value() {
                return Err(invalid("结算池余额与结算计划不一致"));
            }
            positive_u64(row.try_get("state_version").map_err(map_sql_error)?, "结算池状态版本")?
        };
        let closing_pool_version = if plan.compensation_pool_enabled {
            current_pool_version.checked_add(1).ok_or_else(|| invalid("结算池状态版本溢出"))?
        } else {
            current_pool_version
        };

        // 代理终态、lot 成交/流拍状态与金额事实必须在同一 Serializable 事务内提交。
        for closure_plan in proxy_closure_plans {
            self.apply_plan_inner(&closure_plan, None, occurred_at)
                .await
                .map_err(map_competition_error)?;
        }
        close_settlement_lots(self, auction_id, plan, occurred_at).await?;

        {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            if plan.compensation_pool_enabled {
                let result = sqlx::query(
                    r#"UPDATE public.compensation_pool
                       SET balance = $1, state_version = $2, updated_at = $5
                       WHERE balance = $3 AND state_version = $4"#,
                )
                .bind(plan.closing_pool.value())
                .bind(i64_version(closing_pool_version, "结算池状态版本")?)
                .bind(plan.opening_pool.value())
                .bind(i64_version(current_pool_version, "结算池状态版本")?)
                .bind(occurred_at)
                .execute(&mut **transaction)
                .await
                .map_err(map_sql_error)?;
                if result.rows_affected() != 1 {
                    return Err(invalid("结算池版本与数据库不一致"));
                }
            }

            sqlx::query(
                r#"INSERT INTO public.auction_settlements
                       (auction_id, command_id, policy_version, policy_mode, pool_enabled,
                        opening_pool_balance, closing_pool_balance, tax_total, discount_demand,
                        compensation_total, opening_pool_version, closing_pool_version, settled_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)"#,
            )
            .bind(auction_id.as_uuid())
            .bind(command_id.as_uuid())
            .bind(i32_version(plan.policy_version, "结算策略版本")?)
            .bind(plan.policy_mode.as_str())
            .bind(plan.compensation_pool_enabled)
            .bind(plan.opening_pool.value())
            .bind(plan.closing_pool.value())
            .bind(plan.tax_total.value())
            .bind(plan.discount_demand.value())
            .bind(plan.compensation_total.value())
            .bind(i64_version(current_pool_version, "结算池状态版本")?)
            .bind(i64_version(closing_pool_version, "结算池状态版本")?)
            .bind(occurred_at)
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
        }

        insert_lot_settlements(self, plan).await?;
        insert_compensation_ledger(self, command_id, plan, occurred_at).await?;
        insert_obligations(self, plan, occurred_at).await?;
        insert_recoveries(self, auction_id, plan, recovery_policy, occurred_at).await?;
        insert_settlement_audit(
            self,
            auction_id,
            command_id,
            current_pool_version,
            closing_pool_version,
            plan,
            occurred_at,
        )
        .await?;
        let next_version =
            auction_state_version.checked_add(1).ok_or_else(|| invalid("拍卖状态版本溢出"))?;

        {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let result = sqlx::query(
                r#"UPDATE public.auctions
                   SET status = 'settled', settled_at = $4,
                       settlement_complete_command_id = $5,
                       settlement_next_attempt_at = NULL,
                       settlement_last_error_code = NULL,
                       state_version = $2, updated_at = $4
                   WHERE id = $1 AND status = 'settling' AND state_version = $3"#,
            )
            .bind(auction_id.as_uuid())
            .bind(i64_version(next_version, "拍卖状态版本")?)
            .bind(i64_version(auction_state_version, "拍卖状态版本")?)
            .bind(occurred_at)
            .bind(command_id.as_uuid())
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
            if result.rows_affected() != 1 {
                return Err(invalid("拍卖状态版本与数据库不一致"));
            }
        }

        insert_settlement_realtime(self, auction_id, command_id, next_version, plan, occurred_at)
            .await
    }
}

fn validate_proxy_closure_plans(
    command_id: CommandId,
    settlement_plan: &SettlementPlan,
    closure_plans: &[CompetitionPlan],
) -> Result<(), SettlementTransactionError> {
    if closure_plans.is_empty() {
        return Ok(());
    }
    let lot_ids: HashSet<_> = settlement_plan.lines.iter().map(|line| line.lot_id).collect();
    let mut closed_lots = HashSet::with_capacity(closure_plans.len());
    for plan in closure_plans {
        plan.validate().map_err(|_| invalid("结算代理关闭计划校验失败"))?;
        let transition = match (plan.proxy_transitions.as_slice(), plan.proxy_states.as_slice()) {
            ([transition], [state]) => (transition, state),
            _ => return Err(invalid("结算代理关闭计划必须只包含一个终态转换")),
        };
        if !plan.public_bids.is_empty()
            || plan.active_proxy_id.is_some()
            || transition.0.command_id != command_id
            || transition.0.new_status != ProxyStatus::Closed
            || transition.1.status != ProxyStatus::Closed
            || transition.1.lot_id != plan.final_lot.lot_id
            || !lot_ids.contains(&plan.final_lot.lot_id)
            || !closed_lots.insert(plan.final_lot.lot_id)
        {
            return Err(invalid("结算代理关闭计划与结算命令或 lot 不一致"));
        }
    }
    Ok(())
}
