//! 使用 lots_due_slow_idx 读取当前到期调度，保留过期尚未清除的条目。

use auction_application::{SLI_CANDIDATE_LIMIT, SliPending, SliSnapshotError, SliWorld};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};

use super::{candidate_limit, storage};

pub(super) async fn read(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
) -> Result<[SliPending; SliWorld::ALL.len()], SliSnapshotError> {
    let rows = sqlx::query(
        "WITH due AS MATERIALIZED (
            SELECT auction_id,slow_due_at FROM public.lots
            WHERE status='live' AND slow_due_at IS NOT NULL AND slow_due_at <= $1
            ORDER BY slow_due_at,id LIMIT $2
         )
         SELECT auction.world_id,due.slow_due_at
         FROM due JOIN LATERAL (
            SELECT world_id FROM public.auctions WHERE id=due.auction_id LIMIT 1
         ) auction ON true",
    )
    .bind(now)
    .bind(candidate_limit())
    .fetch_all(connection)
    .await
    .map_err(storage)?;
    if rows.len() > SLI_CANDIDATE_LIMIT {
        return Err(SliSnapshotError::Limited);
    }
    let mut pending = [SliPending::default(); SliWorld::ALL.len()];
    for row in rows {
        let world: String = row.try_get("world_id").map_err(storage)?;
        let index = SliWorld::ALL
            .iter()
            .position(|candidate| candidate.label() == world)
            .ok_or(SliSnapshotError::InvalidData)?;
        let due: DateTime<Utc> = row.try_get("slow_due_at").map_err(storage)?;
        let age = (now - due).to_std().map_err(invalid_time)?.as_secs_f64();
        pending[index].count = pending[index].count.saturating_add(1);
        pending[index].oldest_seconds = pending[index].oldest_seconds.max(age);
    }
    Ok(pending)
}

fn invalid_time(_: chrono::OutOfRangeError) -> SliSnapshotError {
    SliSnapshotError::InvalidData
}
