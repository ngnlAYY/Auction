//! PostgreSQL 最终阶段计划写入。

use super::super::record::previous_lifecycle_version;
use super::super::*;

pub(super) async fn apply_final_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    proxy_plans: Vec<auction_application::AuctionFinalProxyPlan>,
) -> Result<(), CompetitionTransactionError> {
    let updated = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.auctions
             SET status = 'final', final_phase_at = $1, state_version = $2, updated_at = $1
             WHERE id = $3 AND status = 'live' AND state_version = $4",
        )
        .bind(auction.updated_at)
        .bind(i64_version(auction.state_version, "最终阶段状态版本")?)
        .bind(auction.id.as_uuid())
        .bind(previous_lifecycle_version(auction)?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }
    for proxy_plan in proxy_plans {
        if proxy_plan.command.aggregate_id != proxy_plan.competition.final_lot.lot_id.as_uuid() {
            return Err(invalid_state("最终阶段代理子命令与 lot 计划不一致"));
        }
        transaction.insert_command_inner(proxy_plan.command).await?;
        transaction.apply_plan_inner(proxy_plan.competition, command.request_id.clone()).await?;
    }
    if command.actor.actor_type == auction_application::AuctionLifecycleActorType::Admin {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.scheduled_rounds
             SET final_command_id = $1, final_completed_at = $2,
                 lease_owner = NULL, lease_until = NULL,
                 state_version = state_version + 1, updated_at = $2
             WHERE auction_id = $3 AND status = 'active'
               AND open_completed_at IS NOT NULL AND final_completed_at IS NULL",
        )
        .bind(command.command_id.as_uuid())
        .bind(auction.updated_at)
        .bind(auction.id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}
