//! SQLite Lite 档的公开拍卖查询。
//!
//! 历史截止时间混用 UTC RFC3339 与空格格式；先按时刻排序，毫秒并列时仅对已知 UTC
//! 写入格式规范化并补齐小数精度，再返回原文。未知偏移量不进入 UTC 字符串次序。
//! 开拍展示判定同样按时间值比较，不能让 RFC3339 的 T 与历史空格词法决定状态。

use auction_application::{
    ApplicationError, CurrentAuctionSnapshot, PublicAuctionCursor, PublicAuctionListRequest,
    PublicAuctionPage, PublicRealtimeAuctionStatus,
};
use sqlx::Row;
use uuid::Uuid;

use super::super::SqliteStore;
use super::mapping::{
    map_realtime_status, map_sqlite_current, map_sqlite_summary, storage_error,
    validate_list_request,
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
    // 使用延迟只读事务共享同一快照；不为列表占用 BEGIN IMMEDIATE 写锁。
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开始 Lite 公开拍卖列表快照失败"))?;
    let total = if use_cursor {
        None
    } else {
        Some(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*)
                 FROM auctions AS a
                 JOIN worlds AS w ON w.id = a.world_id
                 WHERE a.status = ?
                   AND (w.enabled = 1 OR a.status = 'settled')
                   AND (? IS NULL OR a.world_id = ?)",
            )
            .bind(&request.status)
            .bind(request.world_id.as_deref())
            .bind(request.world_id.as_deref())
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| storage_error("读取 Lite 公开拍卖列表总数失败"))?,
        )
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
            (SELECT deadline.effective_end_at FROM lots AS deadline
             WHERE deadline.auction_id = a.id AND deadline.status IN ('live', 'sold', 'unsold')
               AND deadline.effective_end_at IS NOT NULL
             ORDER BY julianday(deadline.effective_end_at) DESC,
                 CASE WHEN substr(deadline.effective_end_at, -6) = '+00:00'
                           OR substr(deadline.effective_end_at, -1) = 'Z'
                           OR (substr(deadline.effective_end_at, 11, 1) = ' '
                               AND substr(deadline.effective_end_at, 20) NOT GLOB '*[+-]*')
                     THEN substr(replace(replace(replace(deadline.effective_end_at,
                         'T', ' '), '+00:00', ''), 'Z', '')
                         || CASE WHEN substr(deadline.effective_end_at, 20, 1) = '.'
                             THEN '000000000' ELSE '.000000000' END, 1, 29)
                 END DESC,
                 deadline.id DESC LIMIT 1)
                AS effective_end_at,
            a.settled_at,
            COUNT(CASE WHEN l.status IN ('live', 'sold', 'unsold') THEN l.id END) AS lot_count,
            COUNT(CASE WHEN l.status = 'sold' THEN l.id END) AS sold_count,
            COUNT(CASE WHEN l.status = 'unsold' THEN l.id END) AS unsold_count,
            COALESCE(SUM(CASE WHEN l.status = 'sold' THEN COALESCE(l.current_price, 0) ELSE 0 END), 0)
                AS sold_amount,
            a.state_version
         FROM auctions AS a
         INNER JOIN worlds AS w ON w.id = a.world_id
         LEFT JOIN lots AS l ON l.auction_id = a.id
         WHERE a.status = ?
           AND (w.enabled = 1 OR a.status = 'settled')
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
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 Lite 公开拍卖列表失败"))?;
    transaction.commit().await.map_err(|_| storage_error("结束 Lite 公开拍卖列表快照失败"))?;
    let mut entries = rows.into_iter().map(map_sqlite_summary).collect::<Result<Vec<_>, _>>()?;
    let entry_count =
        i64::try_from(entries.len()).map_err(|_| storage_error("Lite 公开拍卖页大小溢出"))?;
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
        total,
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
            (SELECT deadline.effective_end_at FROM lots AS deadline
             WHERE deadline.auction_id = a.id AND deadline.status IN ('live', 'sold', 'unsold')
               AND deadline.effective_end_at IS NOT NULL
             ORDER BY julianday(deadline.effective_end_at) DESC,
                 CASE WHEN substr(deadline.effective_end_at, -6) = '+00:00'
                           OR substr(deadline.effective_end_at, -1) = 'Z'
                           OR (substr(deadline.effective_end_at, 11, 1) = ' '
                               AND substr(deadline.effective_end_at, 20) NOT GLOB '*[+-]*')
                     THEN substr(replace(replace(replace(deadline.effective_end_at,
                         'T', ' '), '+00:00', ''), 'Z', '')
                         || CASE WHEN substr(deadline.effective_end_at, 20, 1) = '.'
                             THEN '000000000' ELSE '.000000000' END, 1, 29)
                 END DESC,
                 deadline.id DESC LIMIT 1)
                AS scheduled_ends_at,
            a.preview_revision,
            CASE
                WHEN a.status = 'preview' AND julianday('now') < julianday(a.scheduled_open_at) THEN 'preview'
                WHEN a.status = 'preview' THEN 'opening'
                ELSE 'live'
            END AS presentation_mode,
            CASE WHEN a.status IN ('live', 'final') THEN 1 ELSE 0 END AS bidding_open
         FROM auctions AS a
         INNER JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
         WHERE a.world_id = ?
           AND a.status IN ('preview', 'live', 'final', 'settling')
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

pub(super) async fn realtime_status(
    store: &SqliteStore,
    auction_id: Uuid,
) -> Result<Option<PublicRealtimeAuctionStatus>, ApplicationError> {
    let row = sqlx::query(
        "SELECT
             a.id,
             a.world_id,
             a.sequence,
             a.status,
             a.scheduled_open_at,
             (SELECT deadline.effective_end_at FROM lots AS deadline
              WHERE deadline.auction_id = a.id AND deadline.status IN ('live', 'sold', 'unsold')
                AND deadline.effective_end_at IS NOT NULL
              ORDER BY julianday(deadline.effective_end_at) DESC,
                 CASE WHEN substr(deadline.effective_end_at, -6) = '+00:00'
                           OR substr(deadline.effective_end_at, -1) = 'Z'
                           OR (substr(deadline.effective_end_at, 11, 1) = ' '
                               AND substr(deadline.effective_end_at, 20) NOT GLOB '*[+-]*')
                     THEN substr(replace(replace(replace(deadline.effective_end_at,
                         'T', ' '), '+00:00', ''), 'Z', '')
                         || CASE WHEN substr(deadline.effective_end_at, 20, 1) = '.'
                             THEN '000000000' ELSE '.000000000' END, 1, 29)
                 END DESC,
                 deadline.id DESC LIMIT 1)
                 AS scheduled_ends_at,
             a.preview_revision,
             CASE
                 WHEN a.status = 'preview' AND julianday('now') < julianday(a.scheduled_open_at) THEN 'preview'
                 WHEN a.status = 'preview' THEN 'opening'
                 ELSE 'live'
             END AS presentation_mode,
             CASE WHEN a.status IN ('live', 'final') THEN 1 ELSE 0 END AS bidding_open
         FROM auctions AS a
         INNER JOIN worlds AS w ON w.id = a.world_id
         WHERE a.id = ?
           AND (w.enabled = 1 OR a.status = 'settled')
           AND a.status IN ('preview', 'live', 'final', 'settling', 'settled')
           AND (
               a.status <> 'preview'
               OR NOT EXISTS (
                   SELECT 1
                   FROM auctions AS active
                   WHERE active.world_id = a.world_id
                     AND active.status IN ('live', 'final', 'settling')
               )
           )",
    )
    .bind(auction_id.as_bytes().to_vec())
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取 Lite Public realtime 拍卖状态失败"))?;

    row.map(|row| {
        let status: String = row
            .try_get("status")
            .map_err(|_| storage_error("读取 Lite Public realtime 拍卖状态失败"))?;
        if status == "settled" {
            return map_realtime_status(&status);
        }
        let snapshot = map_sqlite_current(row)?;
        if snapshot.id != auction_id {
            return Err(storage_error("Lite Public realtime 拍卖标识不一致"));
        }
        map_realtime_status(&snapshot.status)
    })
    .transpose()
}
