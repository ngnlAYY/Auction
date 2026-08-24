//! SQLite Lite 档的公开拍卖查询。

use auction_application::{
    ApplicationError, CurrentAuctionSnapshot, PublicAuctionCursor, PublicAuctionListRequest,
    PublicAuctionPage,
};
use sqlx::Row;
use uuid::Uuid;

use super::super::SqliteStore;
use super::mapping::{
    map_sqlite_current, map_sqlite_summary, storage_error, validate_list_request,
};

pub(super) async fn list(
    store: &SqliteStore,
    request: PublicAuctionListRequest,
) -> Result<PublicAuctionPage, ApplicationError> {
    validate_list_request(&request)?;
    let use_cursor = request.cursor.is_some();
    let (use_boundary, cursor_sequence, cursor_id) = match request.cursor.as_ref() {
        Some(PublicAuctionCursor::Boundary { sequence, id }) => (true, *sequence, *id),
        Some(PublicAuctionCursor::Start) | None => (false, 0, Uuid::nil()),
    };
    let query_limit = if use_cursor {
        request.limit.checked_add(1).ok_or_else(|| storage_error("Lite 公开拍卖分页上限溢出"))?
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
            NULL AS opened_at,
            NULL AS final_phase_at,
            MAX(CASE WHEN l.status IN ('live', 'sold', 'unsold') THEN l.effective_end_at END)
                AS effective_end_at,
            a.settled_at,
            COUNT(CASE WHEN l.status IN ('live', 'sold', 'unsold') THEN l.id END) AS lot_count,
            COUNT(CASE WHEN l.status = 'sold' THEN l.id END) AS sold_count,
            COUNT(CASE WHEN l.status = 'unsold' THEN l.id END) AS unsold_count,
            COALESCE(SUM(CASE WHEN l.status = 'sold' THEN COALESCE(l.current_price, 0) ELSE 0 END), 0)
                AS sold_amount,
            a.state_version,
            COUNT(*) OVER () AS total_count
         FROM auctions AS a
         INNER JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
         LEFT JOIN lots AS l ON l.auction_id = a.id
         WHERE a.status = ?
           AND (? IS NULL OR a.world_id = ?)
           AND (
                ? = 0
                OR a.sequence < ?
                OR (a.sequence = ? AND a.id < ?)
           )
         GROUP BY a.id
         ORDER BY a.sequence DESC, a.id DESC
         LIMIT ? OFFSET ?",
    )
    .bind(&request.status)
    .bind(request.world_id.as_deref())
    .bind(request.world_id.as_deref())
    .bind(if use_boundary { 1_i64 } else { 0 })
    .bind(cursor_sequence)
    .bind(cursor_sequence)
    .bind(cursor_id.as_bytes().to_vec())
    .bind(query_limit)
    .bind(if use_cursor { 0 } else { request.offset })
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error("读取 Lite 公开拍卖列表失败"))?;

    let total = rows
        .first()
        .map(|row| row.try_get("total_count"))
        .transpose()
        .map_err(|_| storage_error("读取 Lite 公开拍卖列表总数失败"))?
        .unwrap_or(0);
    let mut entries = rows.into_iter().map(map_sqlite_summary).collect::<Result<Vec<_>, _>>()?;
    let has_more = use_cursor && entries.len() as i64 > request.limit;
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
    store: &SqliteStore,
    world_id: &str,
) -> Result<Option<CurrentAuctionSnapshot>, ApplicationError> {
    let row = sqlx::query(
        "SELECT
            a.id,
            a.world_id,
            a.sequence,
            a.status,
            a.scheduled_open_at,
            MAX(CASE WHEN l.status IN ('live', 'sold', 'unsold') THEN l.effective_end_at END)
                AS scheduled_ends_at,
            a.preview_revision,
            CASE
                WHEN a.status = 'preview' AND CURRENT_TIMESTAMP < a.scheduled_open_at THEN 'preview'
                WHEN a.status = 'preview' THEN 'opening'
                ELSE 'live'
            END AS presentation_mode,
            CASE WHEN a.status IN ('live', 'final') THEN 1 ELSE 0 END AS bidding_open
         FROM auctions AS a
         INNER JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
         LEFT JOIN lots AS l ON l.auction_id = a.id
         WHERE a.world_id = ?
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
    .map_err(|_| storage_error("读取 Lite 当前公开拍卖失败"))?;

    row.map(map_sqlite_current).transpose()
}
