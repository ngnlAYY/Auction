//! SQLite final phase 生命周期动作。

use super::super::super::mapping::*;
use super::super::super::*;

pub(super) async fn apply_final_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    proxy_plans: Vec<auction_application::AuctionFinalProxyPlan>,
) -> Result<(), CompetitionTransactionError> {
    let now = sqlite_timestamp(auction.updated_at);
    let updated = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "UPDATE auctions
             SET status = 'final', final_phase_at = ?, state_version = ?, updated_at = ?
             WHERE id = ? AND status = 'live' AND state_version = ?",
        )
        .bind(&now)
        .bind(i64_version(auction.state_version, "Lite 最终阶段状态版本")?)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .bind(super::previous_lifecycle_version_sqlite(auction)?)
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?
    };
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }
    for proxy_plan in proxy_plans {
        if proxy_plan.command.aggregate_id != proxy_plan.competition.final_lot.lot_id.as_uuid() {
            return Err(invalid_state("Lite 最终阶段代理子命令与 lot 计划不一致"));
        }
        transaction.insert_command_inner(proxy_plan.command).await?;
        transaction.apply_plan_inner(proxy_plan.competition, command.request_id.clone()).await?;
    }
    if command.actor.actor_type == auction_application::AuctionLifecycleActorType::Admin {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "UPDATE scheduled_rounds
             SET final_command_id = ?, final_completed_at = ?,
                 lease_owner = NULL, lease_until = NULL,
                 state_version = state_version + 1, updated_at = ?
             WHERE auction_id = ? AND status = 'active'
               AND open_completed_at IS NOT NULL AND final_completed_at IS NULL",
        )
        .bind(uuid_blob(command.command_id.as_uuid()))
        .bind(&now)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}
