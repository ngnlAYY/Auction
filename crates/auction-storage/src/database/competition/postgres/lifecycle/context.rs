//! PostgreSQL 拍卖生命周期上下文锁定与安全投影。

use super::*;

pub(super) async fn load_lifecycle_context_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    auction_id: AuctionId,
) -> Result<Option<AuctionLifecycleContext>, CompetitionTransactionError> {
    let row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"SELECT
                a.id, a.title, a.status, a.state_version, a.created_at, a.updated_at,
                a.scheduled_end_at, a.effective_end_at,
                a.cancelled_at, a.cancelled_reason, a.cancelled_by,
                a.world_id, a.sequence, a.preview_scheduled_at, a.scheduled_open_at,
                a.preview_manual_open,
                (a.policy_snapshot ->> 'scheduled_end_at')::timestamptz
                    AS policy_scheduled_end_at,
                (a.policy_snapshot ->> 'final_phase_lead_seconds')::bigint
                    AS policy_final_phase_lead_seconds,
                (SELECT round.local_week
                 FROM public.scheduled_rounds AS round
                 WHERE round.auction_id = a.id
                 LIMIT 1) AS scheduled_round_local_week
             FROM public.auctions AS a
             WHERE a.id = $1
             FOR UPDATE OF a"#,
        )
        .bind(auction_id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(row) = row else {
        return Ok(None);
    };
    let state_version =
        positive_u64(row.try_get("state_version").map_err(map_sql_error)?, "拍卖状态版本无效")?;
    let auction = Auction {
        id: AuctionId::from_uuid(row.try_get("id").map_err(map_sql_error)?),
        title: row.try_get("title").map_err(map_sql_error)?,
        status: parse_auction_status(row.try_get::<String, _>("status").map_err(map_sql_error)?)?,
        state_version,
        created_at: row.try_get("created_at").map_err(map_sql_error)?,
        updated_at: row.try_get("updated_at").map_err(map_sql_error)?,
        scheduled_end_at: row.try_get("scheduled_end_at").map_err(map_sql_error)?,
        effective_end_at: row.try_get("effective_end_at").map_err(map_sql_error)?,
        cancelled_at: row.try_get("cancelled_at").map_err(map_sql_error)?,
        cancelled_reason: row.try_get("cancelled_reason").map_err(map_sql_error)?,
        cancelled_by: row.try_get("cancelled_by").map_err(map_sql_error)?,
    };
    let final_phase_lead_seconds: i64 =
        row.try_get("policy_final_phase_lead_seconds").map_err(map_sql_error)?;
    if final_phase_lead_seconds <= 0 {
        return Err(invalid_state("最终阶段领先时间无效"));
    }
    Ok(Some(AuctionLifecycleContext {
        auction,
        world_id: row.try_get("world_id").map_err(map_sql_error)?,
        sequence: row.try_get("sequence").map_err(map_sql_error)?,
        preview_scheduled_at: row.try_get("preview_scheduled_at").map_err(map_sql_error)?,
        scheduled_open_at: row.try_get("scheduled_open_at").map_err(map_sql_error)?,
        policy_scheduled_end_at: row.try_get("policy_scheduled_end_at").map_err(map_sql_error)?,
        policy_final_phase_lead: Duration::seconds(final_phase_lead_seconds),
        preview_manual_open: row.try_get("preview_manual_open").map_err(map_sql_error)?,
        scheduled_round_local_week: row
            .try_get::<Option<NaiveDate>, _>("scheduled_round_local_week")
            .map_err(map_sql_error)?,
    }))
}
