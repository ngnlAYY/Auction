//! Public 拍卖只读查询适配器。
//!
//! 这里把 PostgreSQL 完整档和 SQLite Lite 档的列表示差异收敛成应用层安全投影；HTTP 层
//! 不直接接触 SQL 行，也不根据后端类型分支。

use auction_application::{
    ApplicationError, ApplicationFuture, AuctionReader, CurrentAuctionSnapshot,
    PublicAuctionCursor, PublicAuctionListRequest, PublicAuctionPage, PublicAuctionSummary,
    RepositoryError,
};
#[cfg(feature = "sqlite")]
use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::DatabaseStore;

impl AuctionReader for DatabaseStore {
    fn list_public_auctions(
        &self,
        request: PublicAuctionListRequest,
    ) -> ApplicationFuture<'_, PublicAuctionPage> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => list_sqlite(store, request).await,
            }
        })
    }

    fn current_auction(
        &self,
        world_id: String,
    ) -> ApplicationFuture<'_, Option<CurrentAuctionSnapshot>> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => current_postgres(store, &world_id).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => current_sqlite(store, &world_id).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    store: &super::PostgresStore,
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

#[cfg(feature = "postgres")]
async fn current_postgres(
    store: &super::PostgresStore,
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

#[cfg(feature = "sqlite")]
async fn list_sqlite(
    store: &super::SqliteStore,
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

#[cfg(feature = "sqlite")]
async fn current_sqlite(
    store: &super::SqliteStore,
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

#[cfg(feature = "postgres")]
fn map_postgres_summary(
    row: sqlx::postgres::PgRow,
) -> Result<PublicAuctionSummary, ApplicationError> {
    Ok(PublicAuctionSummary {
        id: row.try_get("id").map_err(|_| storage_error("读取公开拍卖标识失败"))?,
        world_id: row.try_get("world_id").map_err(|_| storage_error("读取公开拍卖世界失败"))?,
        sequence: row.try_get("sequence").map_err(|_| storage_error("读取公开拍卖序号失败"))?,
        status: row.try_get("status").map_err(|_| storage_error("读取公开拍卖状态失败"))?,
        scheduled_open_at: row
            .try_get("scheduled_open_at")
            .map_err(|_| storage_error("读取公开拍卖开始时间失败"))?,
        opened_at: row
            .try_get("opened_at")
            .map_err(|_| storage_error("读取公开拍卖开放时间失败"))?,
        final_phase_at: row
            .try_get("final_phase_at")
            .map_err(|_| storage_error("读取公开拍卖终阶段时间失败"))?,
        effective_end_at: row
            .try_get("effective_end_at")
            .map_err(|_| storage_error("读取公开拍卖有效结束时间失败"))?,
        settled_at: row
            .try_get("settled_at")
            .map_err(|_| storage_error("读取公开拍卖结算时间失败"))?,
        lot_count: row
            .try_get("lot_count")
            .map_err(|_| storage_error("读取公开拍卖 lot 数量失败"))?,
        sold_count: row
            .try_get("sold_count")
            .map_err(|_| storage_error("读取公开拍卖售出数量失败"))?,
        unsold_count: row
            .try_get("unsold_count")
            .map_err(|_| storage_error("读取公开拍卖未售数量失败"))?,
        sold_amount: row
            .try_get("sold_amount")
            .map_err(|_| storage_error("读取公开拍卖售出金额失败"))?,
        state_version: row
            .try_get("state_version")
            .map_err(|_| storage_error("读取公开拍卖状态版本失败"))?,
    })
}

#[cfg(feature = "postgres")]
fn map_postgres_current(
    row: sqlx::postgres::PgRow,
) -> Result<CurrentAuctionSnapshot, ApplicationError> {
    Ok(CurrentAuctionSnapshot {
        id: row.try_get("id").map_err(|_| storage_error("读取当前拍卖标识失败"))?,
        world_id: row.try_get("world_id").map_err(|_| storage_error("读取当前拍卖世界失败"))?,
        sequence: row.try_get("sequence").map_err(|_| storage_error("读取当前拍卖序号失败"))?,
        status: row.try_get("status").map_err(|_| storage_error("读取当前拍卖状态失败"))?,
        opens_at: row
            .try_get("scheduled_open_at")
            .map_err(|_| storage_error("读取当前拍卖开始时间失败"))?,
        scheduled_ends_at: row
            .try_get("scheduled_ends_at")
            .map_err(|_| storage_error("读取当前拍卖结束时间失败"))?,
        presentation_mode: row
            .try_get("presentation_mode")
            .map_err(|_| storage_error("读取当前拍卖展示模式失败"))?,
        bidding_open: row
            .try_get("bidding_open")
            .map_err(|_| storage_error("读取当前拍卖竞价状态失败"))?,
        preview_revision: row
            .try_get("preview_revision")
            .map_err(|_| storage_error("读取当前拍卖预览版本失败"))?,
    })
}

#[cfg(feature = "sqlite")]
fn map_sqlite_summary(
    row: sqlx::sqlite::SqliteRow,
) -> Result<PublicAuctionSummary, ApplicationError> {
    Ok(PublicAuctionSummary {
        id: uuid_from_blob(
            row.try_get("id").map_err(|_| storage_error("读取 Lite 公开拍卖标识失败"))?,
            "公开拍卖标识",
        )?,
        world_id: row
            .try_get("world_id")
            .map_err(|_| storage_error("读取 Lite 公开拍卖世界失败"))?,
        sequence: row
            .try_get("sequence")
            .map_err(|_| storage_error("读取 Lite 公开拍卖序号失败"))?,
        status: row.try_get("status").map_err(|_| storage_error("读取 Lite 公开拍卖状态失败"))?,
        scheduled_open_at: parse_timestamp(
            row.try_get("scheduled_open_at")
                .map_err(|_| storage_error("读取 Lite 公开拍卖开始时间失败"))?,
            "公开拍卖开始时间",
        )?,
        opened_at: parse_optional_timestamp(
            row.try_get("opened_at")
                .map_err(|_| storage_error("读取 Lite 公开拍卖开放时间失败"))?,
            "公开拍卖开放时间",
        )?,
        final_phase_at: parse_optional_timestamp(
            row.try_get("final_phase_at")
                .map_err(|_| storage_error("读取 Lite 公开拍卖终阶段时间失败"))?,
            "公开拍卖终阶段时间",
        )?,
        effective_end_at: parse_optional_timestamp(
            row.try_get("effective_end_at")
                .map_err(|_| storage_error("读取 Lite 公开拍卖有效结束时间失败"))?,
            "公开拍卖有效结束时间",
        )?,
        settled_at: parse_optional_timestamp(
            row.try_get("settled_at")
                .map_err(|_| storage_error("读取 Lite 公开拍卖结算时间失败"))?,
            "公开拍卖结算时间",
        )?,
        lot_count: row
            .try_get("lot_count")
            .map_err(|_| storage_error("读取 Lite 公开拍卖 lot 数量失败"))?,
        sold_count: row
            .try_get("sold_count")
            .map_err(|_| storage_error("读取 Lite 公开拍卖售出数量失败"))?,
        unsold_count: row
            .try_get("unsold_count")
            .map_err(|_| storage_error("读取 Lite 公开拍卖未售数量失败"))?,
        sold_amount: row
            .try_get("sold_amount")
            .map_err(|_| storage_error("读取 Lite 公开拍卖售出金额失败"))?,
        state_version: row
            .try_get("state_version")
            .map_err(|_| storage_error("读取 Lite 公开拍卖状态版本失败"))?,
    })
}

#[cfg(feature = "sqlite")]
fn map_sqlite_current(
    row: sqlx::sqlite::SqliteRow,
) -> Result<CurrentAuctionSnapshot, ApplicationError> {
    Ok(CurrentAuctionSnapshot {
        id: uuid_from_blob(
            row.try_get("id").map_err(|_| storage_error("读取 Lite 当前拍卖标识失败"))?,
            "当前拍卖标识",
        )?,
        world_id: row
            .try_get("world_id")
            .map_err(|_| storage_error("读取 Lite 当前拍卖世界失败"))?,
        sequence: row
            .try_get("sequence")
            .map_err(|_| storage_error("读取 Lite 当前拍卖序号失败"))?,
        status: row.try_get("status").map_err(|_| storage_error("读取 Lite 当前拍卖状态失败"))?,
        opens_at: parse_timestamp(
            row.try_get("scheduled_open_at")
                .map_err(|_| storage_error("读取 Lite 当前拍卖开始时间失败"))?,
            "当前拍卖开始时间",
        )?,
        scheduled_ends_at: parse_optional_timestamp(
            row.try_get("scheduled_ends_at")
                .map_err(|_| storage_error("读取 Lite 当前拍卖结束时间失败"))?,
            "当前拍卖结束时间",
        )?,
        presentation_mode: row
            .try_get("presentation_mode")
            .map_err(|_| storage_error("读取 Lite 当前拍卖展示模式失败"))?,
        bidding_open: row
            .try_get::<i64, _>("bidding_open")
            .map_err(|_| storage_error("读取 Lite 当前拍卖竞价状态失败"))?
            != 0,
        preview_revision: row
            .try_get("preview_revision")
            .map_err(|_| storage_error("读取 Lite 当前拍卖预览版本失败"))?,
    })
}

#[cfg(feature = "sqlite")]
fn uuid_from_blob(bytes: Vec<u8>, label: &str) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&bytes).map_err(|_| storage_error(label))
}

#[cfg(feature = "sqlite")]
fn parse_optional_timestamp(
    value: Option<String>,
    label: &str,
) -> Result<Option<DateTime<Utc>>, ApplicationError> {
    value.map(|value| parse_timestamp(value, label)).transpose()
}

#[cfg(feature = "sqlite")]
fn parse_timestamp(value: String, label: &str) -> Result<DateTime<Utc>, ApplicationError> {
    if let Ok(value) = DateTime::parse_from_rfc3339(&value) {
        return Ok(value.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(&value, format) {
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(value, Utc));
        }
    }
    Err(storage_error(label))
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}

fn validate_list_request(request: &PublicAuctionListRequest) -> Result<(), ApplicationError> {
    if request.status.trim().is_empty()
        || request.status != request.status.trim()
        || request.limit < 1
        || request.offset < 0
        || request.cursor.is_some() && request.offset != 0
    {
        return Err(storage_error("公开拍卖列表参数无效"));
    }
    if request
        .world_id
        .as_deref()
        .is_some_and(|world| world.is_empty() || world.len() > 64 || world != world.trim())
    {
        return Err(storage_error("公开拍卖世界参数无效"));
    }
    Ok(())
}
