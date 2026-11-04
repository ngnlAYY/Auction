//! 从现有事件时间索引取得七天候选，再按主键关联 Final 命令。

use std::collections::BTreeMap;

use auction_application::{
    CREATE_PROXY_COMMAND_KIND, PLACE_BID_COMMAND_KIND, SLI_CANDIDATE_LIMIT, SLI_WINDOW_SECONDS,
    SLOW_PROXY_COMMAND_KIND, SliBidCapacity, SliBidKind, SliSnapshotError, SliWorld,
};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::{candidate_limit, storage};

const COMMAND_KINDS: [&str; SliBidKind::ALL.len()] =
    [PLACE_BID_COMMAND_KIND, CREATE_PROXY_COMMAND_KIND, SLOW_PROXY_COMMAND_KIND];

pub(super) async fn read(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
) -> Result<[[SliBidCapacity; SliBidKind::ALL.len()]; SliWorld::ALL.len()], SliSnapshotError> {
    let since = now
        .checked_sub_signed(chrono::Duration::seconds(SLI_WINDOW_SECONDS))
        .ok_or(SliSnapshotError::InvalidData)?;
    // LIMIT 位于 kind/replay 过滤之前，避免沿时间索引无限跳过无关事件。
    let candidates = sqlx::query(
        "SELECT created_at,
            CASE WHEN kind='lot.bid_accepted' AND replay_of_event_id IS NULL
                 THEN origin_bid_id END AS bid_id
         FROM public.realtime_events
         WHERE created_at >= $1 AND created_at <= $2
         ORDER BY created_at DESC,event_id DESC LIMIT $3",
    )
    .bind(since)
    .bind(now)
    .bind(candidate_limit())
    .fetch_all(&mut *connection)
    .await
    .map_err(storage)?;
    if candidates.len() > SLI_CANDIDATE_LIMIT {
        return Err(SliSnapshotError::Limited);
    }
    let mut bid_ids = Vec::new();
    let mut event_times = Vec::new();
    for row in candidates {
        if let Some(bid_id) = row.try_get::<Option<Uuid>, _>("bid_id").map_err(storage)? {
            bid_ids.push(bid_id);
            event_times.push(row.try_get::<DateTime<Utc>, _>("created_at").map_err(storage)?);
        }
    }
    // LATERAL 的 LIMIT 保留有界主键探测，避免构建全历史哈希表。
    let rows = sqlx::query(
        "SELECT bid.command_id,command.command_kind,auction.world_id,candidate.event_time
         FROM unnest($1::uuid[],$2::timestamptz[]) AS candidate(bid_id,event_time)
         JOIN LATERAL (
            SELECT command_id,lot_id,created_at FROM public.bids
            WHERE id=candidate.bid_id LIMIT 1
         ) bid ON true
         JOIN LATERAL (
            SELECT command_kind FROM public.auction_commands
            WHERE command_id=bid.command_id LIMIT 1
         ) command ON true
         JOIN LATERAL (
            SELECT auction_id FROM public.lots WHERE id=bid.lot_id LIMIT 1
         ) lot ON true
         JOIN LATERAL (
            SELECT world_id,final_phase_at FROM public.auctions
            WHERE id=lot.auction_id LIMIT 1
         ) auction ON true
         WHERE auction.final_phase_at IS NOT NULL AND bid.created_at >= auction.final_phase_at
           AND command.command_kind = ANY($3::text[])",
    )
    .bind(bid_ids)
    .bind(event_times)
    .bind(COMMAND_KINDS.as_slice())
    .fetch_all(&mut *connection)
    .await
    .map_err(storage)?;
    let mut commands = BTreeMap::new();
    for row in rows {
        let world: String = row.try_get("world_id").map_err(storage)?;
        let world = SliWorld::ALL
            .iter()
            .position(|candidate| candidate.label() == world)
            .ok_or(SliSnapshotError::InvalidData)?;
        let kind: String = row.try_get("command_kind").map_err(storage)?;
        let kind = COMMAND_KINDS
            .iter()
            .position(|candidate| *candidate == kind)
            .ok_or(SliSnapshotError::InvalidData)?;
        let command: Uuid = row.try_get("command_id").map_err(storage)?;
        let at: DateTime<Utc> = row.try_get("event_time").map_err(storage)?;
        let first = commands.entry((world, kind, command)).or_insert(at);
        *first = (*first).min(at);
    }
    let mut capacity = [[SliBidCapacity::default(); SliBidKind::ALL.len()]; SliWorld::ALL.len()];
    let mut seconds = BTreeMap::<_, u64>::new();
    let rate_since = now
        .checked_sub_signed(chrono::Duration::seconds(SliBidCapacity::RATE_WINDOW_SECONDS))
        .ok_or(SliSnapshotError::InvalidData)?;
    for ((world, kind, _command), at) in commands {
        if at > rate_since {
            capacity[world][kind].per_second += 1.0 / SliBidCapacity::RATE_WINDOW_SECONDS as f64;
        }
        let count = seconds.entry((world, kind, at.timestamp())).or_default();
        *count = count.saturating_add(1);
        capacity[world][kind].peak_per_second = capacity[world][kind].peak_per_second.max(*count);
    }
    Ok(capacity)
}
