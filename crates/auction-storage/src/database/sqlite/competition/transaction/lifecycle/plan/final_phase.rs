//! SQLite final phase 生命周期动作。

use crate::database::lifecycle_audit::final_proxy_audit_record;

use super::super::super::mapping::*;
use super::super::super::*;
use super::super::record::insert_lifecycle_audit;

pub(super) async fn apply_final_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    proxy_plans: Vec<auction_application::AuctionFinalProxyPlan>,
    effective_end_at: Option<DateTime<Utc>>,
) -> Result<(), CompetitionTransactionError> {
    let now = sqlite_timestamp(auction.updated_at);
    // 上下文已把历史格式解析为 UTC；拍卖层直接比较完整精度，不能通过文本或毫秒取整延长期限。
    let scheduled_end_at = auction.scheduled_end_at.map(|deadline| {
        sqlite_timestamp(effective_end_at.map_or(deadline, |cap| deadline.min(cap)))
    });
    let clamped_end_at = auction.effective_end_at.map(|deadline| {
        sqlite_timestamp(effective_end_at.map_or(deadline, |cap| deadline.min(cap)))
    });
    let effective_end_at = effective_end_at.map(|deadline| {
        (
            sqlite_timestamp(deadline),
            deadline.naive_utc().format("%Y-%m-%d %H:%M:%S%.9f").to_string(),
        )
    });
    // Lite 同样保留 effective >= scheduled；人工 Final 必须同步收紧两个拍卖层时间列。
    let updated = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "UPDATE auctions
             SET status = 'final', final_phase_at = ?,
                 scheduled_end_at = ?, effective_end_at = ?,
                 state_version = ?, updated_at = ?
             WHERE id = ? AND status = 'live' AND state_version = ?",
        )
        .bind(&now)
        .bind(&scheduled_end_at)
        .bind(&clamped_end_at)
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
        let audit = final_proxy_audit_record(command, auction.updated_at, &proxy_plan)?;
        transaction.insert_command_inner(proxy_plan.command).await?;
        transaction
            .apply_public_command_plan_inner(proxy_plan.competition, None, auction.updated_at)
            .await?;
        insert_lifecycle_audit(transaction, audit).await?;
    }
    if let Some((effective_end_at, precise_end_at)) = effective_end_at {
        let connection = transaction.connection_mut()?;
        // julianday 只保留毫秒；并列时仅对已知 UTC 写入格式补齐小数比较，未知 offset 不当作 UTC。
        sqlx::query(
            "UPDATE lots
             SET effective_end_at = ?1, state_version = state_version + 1, updated_at = ?2
             WHERE auction_id = ?3 AND status = 'live'
               AND (julianday(effective_end_at) > julianday(?1)
                    OR (julianday(effective_end_at) = julianday(?1)
                        AND CASE WHEN substr(effective_end_at, -6) = '+00:00'
                                      OR substr(effective_end_at, -1) = 'Z'
                                      OR (substr(effective_end_at, 11, 1) = ' '
                                          AND substr(effective_end_at, 20) NOT GLOB '*[+-]*')
                            THEN substr(replace(replace(replace(effective_end_at,
                                'T', ' '), '+00:00', ''), 'Z', '')
                                || CASE WHEN substr(effective_end_at, 20, 1) = '.'
                                    THEN '000000000' ELSE '.000000000' END, 1, 29)
                        END > ?4))",
        )
        .bind(&effective_end_at)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .bind(&precise_end_at)
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
    }
    // Worker 的 Final 与轮次完成标记必须原子提交，不能在两次写入之间留下不可再次领取的轮次。
    {
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
