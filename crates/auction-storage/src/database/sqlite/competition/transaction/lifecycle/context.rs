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
                a.preview_opened_at, a.preview_manual_open, a.preview_revision,
                a.opened_at, a.final_phase_at, a.settled_at,
                a.policy_snapshot,
                world.enabled AS world_enabled,
                json_extract(a.policy_snapshot, '$.scheduled_end_at') AS policy_scheduled_end_at,
                CAST(json_extract(a.policy_snapshot, '$.final_phase_lead_seconds') AS INTEGER)
                    AS policy_final_phase_lead_seconds,
                round.local_week AS scheduled_round_local_week,
                round.schedule_snapshot
             FROM auctions AS a
             INNER JOIN worlds AS world ON world.id = a.world_id
             LEFT JOIN scheduled_rounds AS round ON round.auction_id = a.id
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
    let preview_opened_at = parse_optional_timestamp(
        row.try_get("preview_opened_at").map_err(map_sql_error)?,
        "Lite 预览开放时间格式无效",
    )?;
    let opened_at = parse_optional_timestamp(
        row.try_get("opened_at").map_err(map_sql_error)?,
        "Lite 开拍时间格式无效",
    )?;
    let final_phase_at = parse_optional_timestamp(
        row.try_get("final_phase_at").map_err(map_sql_error)?,
        "Lite 最终阶段时间格式无效",
    )?;
    let settled_at = parse_optional_timestamp(
        row.try_get("settled_at").map_err(map_sql_error)?,
        "Lite 结算时间格式无效",
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
    let schedule_source = row
        .try_get::<Option<String>, _>("schedule_snapshot")
        .map_err(map_sql_error)?
        .map(|value| serde_json::from_str::<auction_application::AuctionScheduleSource>(&value))
        .transpose()
        .map_err(|_| invalid_state("Lite 周排期来源快照无效"))?;
    if let Some(source) = &schedule_source {
        source.resolve().map_err(|_| invalid_state("Lite 周排期来源无法解析"))?;
    }
    Ok(Some(AuctionLifecycleContext {
        schedule_source,
        auction,
        world_id: row.try_get("world_id").map_err(map_sql_error)?,
        world_enabled: row.try_get::<i64, _>("world_enabled").map_err(map_sql_error)? != 0,
        sequence: row.try_get("sequence").map_err(map_sql_error)?,
        preview_scheduled_at,
        scheduled_open_at,
        preview_opened_at,
        policy_scheduled_end_at,
        policy_final_phase_lead: Duration::seconds(final_phase_lead_seconds),
        policy_settlement: crate::database::settlement_policy::decode(
            &serde_json::from_str::<serde_json::Value>(
                &row.try_get::<String, _>("policy_snapshot").map_err(map_sql_error)?,
            )
            .map_err(|_| invalid_state("Lite 结算策略快照不是有效 JSON"))?,
        )
        .map_err(invalid_state)?,
        preview_manual_open: row.try_get::<i64, _>("preview_manual_open").map_err(map_sql_error)?
            != 0,
        preview_revision: row.try_get("preview_revision").map_err(map_sql_error)?,
        opened_at,
        final_phase_at,
        settled_at,
        scheduled_round_local_week,
    }))
}
