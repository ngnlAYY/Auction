//! SQLite 生命周期上下文、最终 lot 和代理资格读取。

use super::super::mapping::*;
use super::super::*;

pub(super) async fn list_final_lot_ids_for_update(
    transaction: &mut SqliteCompetitionTransaction,
    auction_id: AuctionId,
) -> Result<Vec<LotId>, CompetitionTransactionError> {
    let rows = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "SELECT id FROM lots
             WHERE auction_id = ? AND status = 'live'
             ORDER BY lot_number",
        )
        .bind(uuid_blob(auction_id.as_uuid()))
        .fetch_all(&mut *connection)
        .await
        .map_err(map_sql_error)?
    };
    rows.into_iter()
        .map(|row| {
            uuid_from_blob(row.try_get("id").map_err(map_sql_error)?, "Lite final lot 标识")
                .map(LotId::from_uuid)
        })
        .collect()
}

pub(super) async fn proxy_user_is_eligible_for_update(
    transaction: &mut SqliteCompetitionTransaction,
    user_id: UserId,
) -> Result<bool, CompetitionTransactionError> {
    let status = {
        let connection = transaction.connection_mut()?;
        sqlx::query_scalar::<_, String>("SELECT status FROM users WHERE id = ?")
            .bind(uuid_blob(user_id.as_uuid()))
            .fetch_optional(&mut *connection)
            .await
            .map_err(map_sql_error)?
            .ok_or_else(|| invalid_state("Lite 活动代理用户不存在"))?
    };
    let blacklist_active = {
        let connection = transaction.connection_mut()?;
        sqlx::query_scalar::<_, i64>(
            "SELECT CASE WHEN revoked_at IS NULL THEN 1 ELSE 0 END
             FROM user_blacklist
             WHERE user_id = ?",
        )
        .bind(uuid_blob(user_id.as_uuid()))
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_sql_error)?
        .unwrap_or(0)
            != 0
    };
    Ok(status == "active" && !blacklist_active)
}

pub(super) async fn load_lifecycle_context_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    auction_id: AuctionId,
) -> Result<Option<AuctionLifecycleContext>, CompetitionTransactionError> {
    let row = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "SELECT
                a.id, a.title, a.status, a.state_version, a.created_at, a.updated_at,
                a.scheduled_end_at, a.effective_end_at,
                a.cancelled_at, a.cancelled_reason, a.cancelled_by,
                a.world_id, a.sequence, a.preview_scheduled_at, a.scheduled_open_at,
                a.preview_manual_open,
                json_extract(a.policy_snapshot, '$.scheduled_end_at') AS policy_scheduled_end_at,
                CAST(json_extract(a.policy_snapshot, '$.final_phase_lead_seconds') AS INTEGER)
                    AS policy_final_phase_lead_seconds,
                (SELECT round.local_week FROM scheduled_rounds AS round
                 WHERE round.auction_id = a.id LIMIT 1) AS scheduled_round_local_week
             FROM auctions AS a
             WHERE a.id = ?",
        )
        .bind(uuid_blob(auction_id.as_uuid()))
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_sql_error)?
    };
    let Some(row) = row else {
        return Ok(None);
    };
    let id = uuid_from_blob(row.try_get("id").map_err(map_sql_error)?, "Lite 拍卖标识格式无效")?;
    let auction = Auction {
        id: AuctionId::from_uuid(id),
        title: row.try_get("title").map_err(map_sql_error)?,
        status: parse_auction_status(row.try_get::<String, _>("status").map_err(map_sql_error)?)?,
        state_version: positive_u64(
            row.try_get("state_version").map_err(map_sql_error)?,
            "Lite 拍卖状态版本无效",
        )?,
        created_at: parse_timestamp(
            row.try_get("created_at").map_err(map_sql_error)?,
            "Lite 拍卖创建时间格式无效",
        )?,
        updated_at: parse_timestamp(
            row.try_get("updated_at").map_err(map_sql_error)?,
            "Lite 拍卖更新时间格式无效",
        )?,
        scheduled_end_at: parse_optional_timestamp(
            row.try_get("scheduled_end_at").map_err(map_sql_error)?,
            "Lite 拍卖名义结束时间格式无效",
        )?,
        effective_end_at: parse_optional_timestamp(
            row.try_get("effective_end_at").map_err(map_sql_error)?,
            "Lite 拍卖有效结束时间格式无效",
        )?,
        cancelled_at: parse_optional_timestamp(
            row.try_get("cancelled_at").map_err(map_sql_error)?,
            "Lite 拍卖取消时间格式无效",
        )?,
        cancelled_reason: row.try_get("cancelled_reason").map_err(map_sql_error)?,
        cancelled_by: row.try_get("cancelled_by").map_err(map_sql_error)?,
    };
    let preview_scheduled_at = parse_timestamp(
        row.try_get("preview_scheduled_at").map_err(map_sql_error)?,
        "Lite 预览计划时间格式无效",
    )?;
    let scheduled_open_at = parse_timestamp(
        row.try_get("scheduled_open_at").map_err(map_sql_error)?,
        "Lite 开拍计划时间格式无效",
    )?;
    let policy_scheduled_end_at = parse_timestamp(
        row.try_get("policy_scheduled_end_at").map_err(map_sql_error)?,
        "Lite 策略结束时间格式无效",
    )?;
    let final_phase_lead_seconds: i64 =
        row.try_get("policy_final_phase_lead_seconds").map_err(map_sql_error)?;
    if final_phase_lead_seconds <= 0 {
        return Err(invalid_state("Lite 最终阶段领先时间无效"));
    }
    let scheduled_round_local_week = row
        .try_get::<Option<String>, _>("scheduled_round_local_week")
        .map_err(map_sql_error)?
        .map(|value| {
            NaiveDate::parse_from_str(&value, "%Y-%m-%d")
                .map_err(|_| invalid_state("Lite 周轮次日期格式无效"))
        })
        .transpose()?;
    Ok(Some(AuctionLifecycleContext {
        auction,
        world_id: row.try_get("world_id").map_err(map_sql_error)?,
        sequence: row.try_get("sequence").map_err(map_sql_error)?,
        preview_scheduled_at,
        scheduled_open_at,
        policy_scheduled_end_at,
        policy_final_phase_lead: Duration::seconds(final_phase_lead_seconds),
        preview_manual_open: row.try_get::<i64, _>("preview_manual_open").map_err(map_sql_error)?
            != 0,
        scheduled_round_local_week,
    }))
}
