//! PostgreSQL 竞价事实与 lot 最终状态写入。

use super::super::mapping::*;
use super::super::*;

pub(in crate::database::competition) async fn insert_bid(
    transaction: &mut PostgresCompetitionTransaction,
    bid: &AcceptedBid,
) -> Result<(), CompetitionTransactionError> {
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        r#"INSERT INTO public.bids
            (id, command_id, fact_index, lot_id, bidder_user_id, amount, mechanism,
             effective_end_at, accepted_state_version)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
    )
    .bind(bid.id.as_uuid())
    .bind(bid.command_id.as_uuid())
    .bind(i32_version(bid.fact_index, "竞价事实序号")?)
    .bind(bid.lot_id.as_uuid())
    .bind(bid.bidder_id.as_uuid())
    .bind(bid.amount.value())
    .bind(bid_mechanism(bid.mechanism))
    .bind(bid.effective_end_at)
    .bind(i64_version(bid.state_version, "竞价状态版本")?)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

pub(in crate::database::competition) async fn update_lot(
    transaction: &mut PostgresCompetitionTransaction,
    plan: &CompetitionPlan,
) -> Result<(), CompetitionTransactionError> {
    let previous_version = plan
        .final_lot
        .state_version
        .checked_sub(plan.public_bids.len() as u64)
        .ok_or_else(|| invalid_state("lot 状态版本回退失败"))?;
    // 同一命令可先接受其他用户出价，再由原领先者的代理反超；即使最终领先者不变，
    // 连续领先区间也必须从本次事实重新开始，与追加式 bid 重建规则一致。
    let leadership_interrupted =
        plan.public_bids.iter().any(|bid| Some(bid.bidder_id) != plan.final_lot.leader_id);
    let transaction = transaction.transaction_mut()?;
    let result = sqlx::query(
        r#"UPDATE public.lots
         SET current_price = $2, leader_user_id = $3,
             leader_since = CASE
                 WHEN leader_user_id IS DISTINCT FROM $3 OR $12 THEN CURRENT_TIMESTAMP
                 WHEN $3 IS NULL THEN NULL
                 ELSE leader_since
             END,
             leader_proxy_last = $4, effective_end_at = $5,
             slow_authorization_id = $6, slow_due_at = $7, slow_generation = $8,
             last_proxy_sequence = $9, state_version = $10, updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND state_version = $11"#,
    )
    .bind(plan.final_lot.lot_id.as_uuid())
    .bind(plan.final_lot.current_price.map(BidAmount::value))
    .bind(plan.final_lot.leader_id.map(auction_domain::UserId::as_uuid))
    .bind(plan.final_lot.leader_proxy_last)
    .bind(plan.final_lot.effective_end_at)
    .bind(plan.final_lot.slow_authorization_id.map(ProxyAuthorizationId::as_uuid))
    .bind(plan.final_lot.slow_due_at)
    .bind(i64_version(plan.final_lot.slow_generation, "慢速代理世代")?)
    .bind(i64_version(plan.final_lot.last_proxy_sequence, "代理序号")?)
    .bind(i64_version(plan.final_lot.state_version, "lot 状态版本")?)
    .bind(i64_version(previous_version, "lot 前一版本")?)
    .bind(leadership_interrupted)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if result.rows_affected() != 1 {
        return Err(CompetitionTransactionError::Failed("lot 状态版本与数据库不一致".to_owned()));
    }
    Ok(())
}
