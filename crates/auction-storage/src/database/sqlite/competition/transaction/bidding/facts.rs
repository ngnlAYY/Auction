//! SQLite 竞价事实与 lot 最终状态写入。

use super::super::mapping::*;
use super::super::*;

pub(in crate::database::sqlite::competition) async fn insert_bid(
    transaction: &mut SqliteCompetitionTransaction,
    bid: &AcceptedBid,
) -> Result<(), CompetitionTransactionError> {
    let connection = transaction.connection_mut()?;
    sqlx::query(
        "INSERT INTO bids
            (id, command_id, fact_index, lot_id, bidder_user_id, amount, mechanism,
             effective_end_at, accepted_state_version, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)",
    )
    .bind(uuid_blob(bid.id.as_uuid()))
    .bind(uuid_blob(bid.command_id.as_uuid()))
    .bind(i32_version(bid.fact_index, "竞价事实序号")?)
    .bind(uuid_blob(bid.lot_id.as_uuid()))
    .bind(uuid_blob(bid.bidder_id.as_uuid()))
    .bind(bid.amount.value())
    .bind(bid_mechanism(bid.mechanism))
    .bind(bid.effective_end_at.to_rfc3339())
    .bind(i64_version(bid.state_version, "竞价状态版本")?)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

pub(in crate::database::sqlite::competition) async fn update_lot(
    transaction: &mut SqliteCompetitionTransaction,
    plan: &CompetitionPlan,
) -> Result<(), CompetitionTransactionError> {
    let previous_version = plan
        .final_lot
        .state_version
        .checked_sub(plan.public_bids.len() as u64)
        .ok_or_else(|| invalid_state("lot 状态版本回退失败"))?;
    let connection = transaction.connection_mut()?;
    let result = sqlx::query(
        "UPDATE lots
         SET current_price = ?,
             leader_since = CASE
                 WHEN leader_user_id IS NOT ? THEN CURRENT_TIMESTAMP
                 WHEN ? IS NULL THEN NULL
                 ELSE leader_since
             END,
             leader_user_id = ?, leader_proxy_last = ?, effective_end_at = ?,
             slow_authorization_id = ?, slow_due_at = ?, slow_generation = ?,
             last_proxy_sequence = ?, state_version = ?, updated_at = CURRENT_TIMESTAMP
         WHERE id = ? AND state_version = ?",
    )
    .bind(plan.final_lot.current_price.map(BidAmount::value))
    .bind(plan.final_lot.leader_id.map(|id| uuid_blob(id.as_uuid())))
    .bind(plan.final_lot.leader_id.map(|id| uuid_blob(id.as_uuid())))
    .bind(plan.final_lot.leader_id.map(|id| uuid_blob(id.as_uuid())))
    .bind(if plan.final_lot.leader_proxy_last { 1_i64 } else { 0_i64 })
    .bind(plan.final_lot.effective_end_at.to_rfc3339())
    .bind(plan.final_lot.slow_authorization_id.map(|id| uuid_blob(id.as_uuid())))
    .bind(plan.final_lot.slow_due_at.map(|value| value.to_rfc3339()))
    .bind(i64_version(plan.final_lot.slow_generation, "慢速代理世代")?)
    .bind(i64_version(plan.final_lot.last_proxy_sequence, "代理序号")?)
    .bind(i64_version(plan.final_lot.state_version, "lot 状态版本")?)
    .bind(uuid_blob(plan.final_lot.lot_id.as_uuid()))
    .bind(i64_version(previous_version, "lot 前一版本")?)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    if result.rows_affected() != 1 {
        return Err(CompetitionTransactionError::Failed("lot 状态版本与数据库不一致".to_owned()));
    }
    Ok(())
}
