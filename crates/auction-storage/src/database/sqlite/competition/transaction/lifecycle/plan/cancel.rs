//! SQLite cancel 生命周期动作。

use super::super::super::mapping::*;
use super::super::super::*;

pub(super) async fn apply_cancel_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    auction: &Auction,
    successor: Option<&AuctionPreviewSuccessorPlan>,
) -> Result<u64, CompetitionTransactionError> {
    let previous_status: String = {
        let connection = transaction.connection_mut()?;
        sqlx::query_scalar("SELECT status FROM auctions WHERE id = ?")
            .bind(uuid_blob(auction.id.as_uuid()))
            .fetch_optional(&mut *connection)
            .await
            .map_err(map_sql_error)?
            .ok_or(CompetitionTransactionError::NotFound)?
    };
    if !matches!(previous_status.as_str(), "draft" | "preview") {
        return Err(CompetitionTransactionError::InvalidState);
    }
    if (previous_status == "preview") != successor.is_some() {
        return Err(invalid_state("Lite 取消计划与下一期草稿计划不一致"));
    }
    let now = sqlite_timestamp(auction.updated_at);
    let connection = transaction.connection_mut()?;
    let withdrawn = sqlx::query(
        "UPDATE lots
         SET status = 'withdrawn', slow_authorization_id = NULL, slow_due_at = NULL,
             state_version = state_version + 1, updated_at = ?
         WHERE auction_id = ? AND status = 'pending'",
    )
    .bind(&now)
    .bind(uuid_blob(auction.id.as_uuid()))
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE scheduled_rounds
         SET status = 'missed', auction_id = NULL, lease_owner = NULL, lease_until = NULL,
             state_version = state_version + 1, updated_at = ?
         WHERE auction_id = ? AND status = 'active' AND open_completed_at IS NULL",
    )
    .bind(&now)
    .bind(uuid_blob(auction.id.as_uuid()))
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    let updated = sqlx::query(
        "UPDATE auctions
         SET status = 'cancelled', state_version = ?, updated_at = ?,
             cancelled_at = ?, cancelled_reason = ?, cancelled_by = ?
         WHERE id = ? AND status = ? AND state_version = ?",
    )
    .bind(i64_version(auction.state_version, "Lite 取消状态版本")?)
    .bind(&now)
    .bind(auction.cancelled_at.map(sqlite_timestamp))
    .bind(&auction.cancelled_reason)
    .bind(&auction.cancelled_by)
    .bind(uuid_blob(auction.id.as_uuid()))
    .bind(previous_status)
    .bind(super::previous_lifecycle_version_sqlite(auction)?)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }
    if let Some(successor) = successor {
        super::successor::insert_successor_sqlite(transaction, auction, "cancelled", successor)
            .await?;
    }
    Ok(withdrawn.rows_affected())
}
