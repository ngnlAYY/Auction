//! PostgreSQL 人工提前结算请求的原子生命周期写入。

use super::super::record::previous_lifecycle_version;
use super::super::*;
use super::final_phase::{
    apply_final_proxy_plans_postgres, clamp_live_lot_deadlines_postgres,
    mark_admin_final_schedule_postgres,
};

/// 立即封闭竞价并让既有 settlement scheduler 在事务提交后领取到期拍卖。
pub(super) async fn apply_early_settlement_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    proxy_plans: Vec<auction_application::AuctionFinalProxyPlan>,
    effective_end_at: DateTime<Utc>,
) -> Result<(), CompetitionTransactionError> {
    // 不放宽 auctions 的结束时间 shape；原始计划仍由 policy_snapshot 保存，当前列表示
    // 人工决定后的权威截止时间。
    let updated = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.auctions
             SET status = 'final', final_phase_at = COALESCE(final_phase_at, $1),
                 scheduled_end_at = CASE
                     WHEN scheduled_end_at IS NULL OR scheduled_end_at > $1 THEN $1
                     ELSE scheduled_end_at
                 END,
                 effective_end_at = CASE
                     WHEN effective_end_at IS NULL OR effective_end_at > $1 THEN $1
                     ELSE effective_end_at
                 END,
                 state_version = $2, updated_at = $1
             WHERE id = $3 AND status IN ('live', 'final') AND state_version = $4",
        )
        .bind(effective_end_at)
        .bind(i64_version(auction.state_version, "提前结算状态版本")?)
        .bind(auction.id.as_uuid())
        .bind(previous_lifecycle_version(auction)?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }

    apply_final_proxy_plans_postgres(transaction, command, auction, proxy_plans).await?;
    clamp_live_lot_deadlines_postgres(
        transaction,
        auction.id,
        effective_end_at,
        auction.updated_at,
    )
    .await?;
    mark_admin_final_schedule_postgres(transaction, command, auction).await?;
    Ok(())
}
