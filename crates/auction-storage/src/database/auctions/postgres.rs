//! PostgreSQL 完整档的公开拍卖查询。

use auction_application::{
    ApplicationError, CurrentAuctionSnapshot, PublicAuctionCursor, PublicAuctionListRequest,
    PublicAuctionPage, PublicRealtimeAuctionStatus,
};
use sqlx::Row;
use uuid::Uuid;

use super::super::PostgresStore;
use super::mapping::{
    map_postgres_current, map_postgres_summary, map_realtime_status, storage_error,
    validate_list_request,
};

pub(super) async fn list(
    store: &PostgresStore,
    request: PublicAuctionListRequest,
) -> Result<PublicAuctionPage, ApplicationError> {
    validate_list_request(&request)?;
    let use_cursor = request.cursor.is_some();
    let (use_boundary, cursor_sequence, cursor_id) = match request.cursor.as_ref() {
        Some(PublicAuctionCursor::Boundary { sequence, id }) => (true, *sequence, *id),
        Some(PublicAuctionCursor::Start) | None => (false, 0, Uuid::nil()),
    };
    let query_limit = if use_cursor {
        request.limit.checked_add(1).ok_or_else(|| storage_error("公开拍卖分页上限溢出"))?
    } else {
        request.limit
    };
    let rows = sqlx::query(
        "SELECT
            a.id,
            a.world_id,
            a.sequence,
            a.status,
            a.scheduled_open_at,
            a.opened_at,
            a.final_phase_at,
            MAX(l.effective_end_at) FILTER (WHERE l.status IN ('live', 'sold', 'unsold'))
                AS effective_end_at,
            a.settled_at,
            COUNT(l.id) FILTER (WHERE l.status IN ('live', 'sold', 'unsold'))::bigint AS lot_count,
            COUNT(l.id) FILTER (WHERE l.status = 'sold')::bigint AS sold_count,
            COUNT(l.id) FILTER (WHERE l.status = 'unsold')::bigint AS unsold_count,
            COALESCE(
                SUM(CASE WHEN l.status = 'sold' THEN COALESCE(ls.buyer_amount, l.current_price, 0) ELSE 0 END),
                0
            )::bigint AS sold_amount,
            a.state_version,
            COUNT(*) OVER ()::bigint AS total_count
         FROM public.auctions AS a
         INNER JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
         LEFT JOIN public.lots AS l ON l.auction_id = a.id
         LEFT JOIN public.lot_settlements AS ls ON ls.lot_id = l.id
         WHERE a.status = $1
           AND ($2::text IS NULL OR a.world_id = $2)
           AND (
                NOT $3::boolean
                OR a.sequence < $4
                OR (a.sequence = $4 AND a.id < $5)
           )
         GROUP BY a.id
         ORDER BY a.sequence DESC, a.id DESC
         LIMIT $6 OFFSET $7",
    )
    .bind(&request.status)
    .bind(request.world_id.as_deref())
    .bind(use_boundary)
    .bind(cursor_sequence)
    .bind(cursor_id)
    .bind(query_limit)
    .bind(if use_cursor { 0 } else { request.offset })
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error("读取公开拍卖列表失败"))?;

    let total = rows
        .first()
        .map(|row| row.try_get("total_count"))
        .transpose()
        .map_err(|_| storage_error("读取公开拍卖列表总数失败"))?
        .unwrap_or(0);
    let mut entries = rows.into_iter().map(map_postgres_summary).collect::<Result<Vec<_>, _>>()?;
    let entry_count =
        i64::try_from(entries.len()).map_err(|_| storage_error("公开拍卖页大小溢出"))?;
    let has_more = use_cursor && entry_count > request.limit;
    if has_more {
        entries.truncate(request.limit as usize);
    }
    let next_cursor = has_more
        .then(|| {
            entries.last().map(|entry| PublicAuctionCursor::Boundary {
                sequence: entry.sequence,
                id: entry.id,
            })
        })
        .flatten();

    Ok(PublicAuctionPage {
        entries,
        total: (!use_cursor).then_some(total),
        limit: request.limit,
        offset: if use_cursor { 0 } else { request.offset },
        next_cursor,
        has_more,
    })
}

pub(super) async fn current(
    store: &PostgresStore,
    world_id: &str,
) -> Result<Option<CurrentAuctionSnapshot>, ApplicationError> {
    let row = sqlx::query(
        "SELECT
            a.id,
            a.world_id,
            a.sequence,
            a.status,
            a.scheduled_open_at,
            MAX(l.effective_end_at) FILTER (WHERE l.status IN ('live', 'sold', 'unsold'))
                AS scheduled_ends_at,
            a.preview_revision,
            CASE
                WHEN a.status = 'preview' AND CURRENT_TIMESTAMP < a.scheduled_open_at THEN 'preview'
                WHEN a.status = 'preview' THEN 'opening'
                ELSE 'live'
            END AS presentation_mode,
            (a.status IN ('live', 'final')) AS bidding_open
         FROM public.auctions AS a
         INNER JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
         LEFT JOIN public.lots AS l ON l.auction_id = a.id
         WHERE a.world_id = $1
           AND a.status IN ('preview', 'live', 'final', 'settling')
         GROUP BY a.id
         ORDER BY
            CASE WHEN a.status IN ('live', 'final', 'settling') THEN 0 ELSE 1 END,
            a.sequence DESC,
            a.id DESC
         LIMIT 1",
    )
    .bind(world_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取当前公开拍卖失败"))?;

    row.map(map_postgres_current).transpose()
}

pub(super) async fn realtime_status(
    store: &PostgresStore,
    auction_id: Uuid,
) -> Result<Option<PublicRealtimeAuctionStatus>, ApplicationError> {
    let row = sqlx::query(
        "SELECT
             a.id,
             a.world_id,
             a.sequence,
             a.status,
             a.scheduled_open_at,
             MAX(l.effective_end_at) FILTER (WHERE l.status IN ('live', 'sold', 'unsold'))
                 AS scheduled_ends_at,
             a.preview_revision,
             CASE
                 WHEN a.status = 'preview' AND CURRENT_TIMESTAMP < a.scheduled_open_at THEN 'preview'
                 WHEN a.status = 'preview' THEN 'opening'
                 ELSE 'live'
             END AS presentation_mode,
             (a.status IN ('live', 'final')) AS bidding_open
         FROM public.auctions AS a
         INNER JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
         LEFT JOIN public.lots AS l ON l.auction_id = a.id
         WHERE a.id = $1
           AND a.status IN ('preview', 'live', 'final', 'settling', 'settled')
           AND (
               a.status <> 'preview'
               OR NOT EXISTS (
                   SELECT 1
                   FROM public.auctions AS active
                   WHERE active.world_id = a.world_id
                     AND active.status IN ('live', 'final', 'settling')
               )
           )
         GROUP BY a.id",
    )
    .bind(auction_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取 Public realtime 拍卖状态失败"))?;

    row.map(|row| {
        let status: String = row
            .try_get("status")
            .map_err(|_| storage_error("读取 Public realtime 拍卖状态失败"))?;
        if status == "settled" {
            return map_realtime_status(&status);
        }
        let snapshot = map_postgres_current(row)?;
        if snapshot.id != auction_id {
            return Err(storage_error("Public realtime 拍卖标识不一致"));
        }
        map_realtime_status(&snapshot.status)
    })
    .transpose()
}
