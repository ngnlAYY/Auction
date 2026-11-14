//! PostgreSQL 拍卖生命周期上下文锁定与安全投影。

use crate::database::auction::competition::PostgresCompetitionTransaction;
use crate::database::auction::competition::lifecycle::map_sql_error;
use crate::database::auction::competition::mapping::duration;
use crate::database::auction::competition::mapping::invalid_state;
use crate::database::auction::competition::mapping::parse_auction_status;
use crate::database::auction::competition::mapping::positive_u64;
use auction_application::auction::AuctionLifecycleContext;
use auction_application::core::CompetitionTransactionError;
use auction_domain::{Auction, AuctionId, AuctionStatus};
use chrono::NaiveDate;
use serde_json::Value;
use sqlx::Row;

pub(super) async fn load_lifecycle_context_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    auction_id: AuctionId,
) -> Result<Option<AuctionLifecycleContext>, CompetitionTransactionError> {
    let row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"WITH target_auction AS MATERIALIZED (
                   SELECT auction.id, auction.world_id
                   FROM public.auctions AS auction
                   WHERE auction.id = $1
               ),
               locked_world AS MATERIALIZED (
                   SELECT world.id, world.enabled
                   FROM public.worlds AS world
                   INNER JOIN target_auction ON target_auction.world_id = world.id
                   FOR UPDATE OF world
               ),
               locked_round AS MATERIALIZED (
                   SELECT round.local_week, round.schedule_snapshot
                   FROM public.scheduled_rounds AS round
                   INNER JOIN target_auction ON target_auction.id = round.auction_id
                   INNER JOIN locked_world ON locked_world.id = target_auction.world_id
                   FOR UPDATE OF round
               )
             SELECT
                a.id, a.title, a.status, a.state_version, a.created_at, a.updated_at,
                a.scheduled_end_at, a.effective_end_at,
                a.cancelled_at, a.cancelled_reason, a.cancelled_by,
                a.world_id, a.sequence, a.preview_scheduled_at, a.scheduled_open_at,
                a.preview_opened_at, a.preview_manual_open, a.preview_revision,
                a.opened_at, a.final_phase_at, a.settled_at,
                a.policy_snapshot,
                locked_world.enabled AS world_enabled,
                (a.policy_snapshot ->> 'scheduled_end_at')::timestamptz
                    AS policy_scheduled_end_at,
                (a.policy_snapshot ->> 'final_phase_lead_seconds')::bigint
                    AS policy_final_phase_lead_seconds,
                locked_round.local_week AS scheduled_round_local_week,
                locked_round.schedule_snapshot
             FROM public.auctions AS a
             INNER JOIN target_auction ON target_auction.id = a.id
             INNER JOIN locked_world ON locked_world.id = a.world_id
             LEFT JOIN locked_round ON TRUE
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
    auction_observability::database_transaction_phase(if auction.status == AuctionStatus::Final {
        auction_observability::DatabaseTransactionPhase::Final
    } else {
        auction_observability::DatabaseTransactionPhase::NonFinal
    });
    let final_phase_lead_seconds: i64 =
        row.try_get("policy_final_phase_lead_seconds").map_err(map_sql_error)?;
    if final_phase_lead_seconds <= 0 {
        return Err(invalid_state("最终阶段领先时间无效"));
    }
    let schedule_source = row
        .try_get::<Option<Value>, _>("schedule_snapshot")
        .map_err(map_sql_error)?
        .map(serde_json::from_value::<auction_domain::AuctionScheduleSource>)
        .transpose()
        .map_err(|_| invalid_state("周排期来源快照无效"))?;
    if let Some(source) = &schedule_source {
        source.resolve().map_err(|_| invalid_state("周排期来源无法解析"))?;
    }
    Ok(Some(AuctionLifecycleContext {
        schedule_source,
        auction,
        world_id: row.try_get("world_id").map_err(map_sql_error)?,
        world_enabled: row.try_get("world_enabled").map_err(map_sql_error)?,
        sequence: row.try_get("sequence").map_err(map_sql_error)?,
        preview_scheduled_at: row.try_get("preview_scheduled_at").map_err(map_sql_error)?,
        scheduled_open_at: row.try_get("scheduled_open_at").map_err(map_sql_error)?,
        preview_opened_at: row.try_get("preview_opened_at").map_err(map_sql_error)?,
        policy_scheduled_end_at: row.try_get("policy_scheduled_end_at").map_err(map_sql_error)?,
        policy_final_phase_lead: duration(final_phase_lead_seconds, "最终阶段领先时间无效")?,
        policy_settlement: crate::database::settlement::policy::decode(
            &row.try_get::<Value, _>("policy_snapshot").map_err(map_sql_error)?,
        )
        .map_err(invalid_state)?,
        preview_manual_open: row.try_get("preview_manual_open").map_err(map_sql_error)?,
        preview_revision: row.try_get("preview_revision").map_err(map_sql_error)?,
        opened_at: row.try_get("opened_at").map_err(map_sql_error)?,
        final_phase_at: row.try_get("final_phase_at").map_err(map_sql_error)?,
        settled_at: row.try_get("settled_at").map_err(map_sql_error)?,
        scheduled_round_local_week: row
            .try_get::<Option<NaiveDate>, _>("scheduled_round_local_week")
            .map_err(map_sql_error)?,
    }))
}
