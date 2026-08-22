//! Public 成交价格历史只读适配器。
//!
//! PostgreSQL 使用已结算 auction/lot 与 settlement 记录构造完整成交事实；SQLite Lite
//! 没有完整结算账本，因此只在 settled/sold 公开状态边界内读取同一份 Lite 投影。两档都
//! 保留筛选、统计和混合 keyset 排序，HTTP 层不接触 SQL 行或装备原始属性。

use auction_application::{
    ApplicationError, ApplicationFuture, PublicPriceHistoryCursor, PublicPriceHistoryEntry,
    PublicPriceHistoryListRequest, PublicPriceHistoryPage, PublicPriceHistoryReader,
    PublicPriceHistoryStats, RepositoryError,
};
#[cfg(feature = "sqlite")]
use chrono::NaiveDateTime;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use super::DatabaseStore;

impl PublicPriceHistoryReader for DatabaseStore {
    fn list_public_price_history(
        &self,
        request: PublicPriceHistoryListRequest,
    ) -> ApplicationFuture<'_, PublicPriceHistoryPage> {
        Box::pin(async move {
            request.validate().map_err(storage_error)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => list_sqlite(store, request).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    store: &super::PostgresStore,
    request: PublicPriceHistoryListRequest,
) -> Result<PublicPriceHistoryPage, ApplicationError> {
    let name_patterns = history_name_patterns(&request);
    let use_boundary =
        matches!(request.cursor.as_ref(), Some(PublicPriceHistoryCursor::Boundary { .. }));
    let (boundary_closed_at, boundary_sequence, boundary_lot_number, boundary_lot_id) =
        cursor_values(&request);
    let query_limit = if request.cursor.is_some() {
        request.limit.checked_add(1).ok_or_else(|| storage_error("公开价格历史分页上限溢出"))?
    } else {
        request.limit
    };

    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启公开价格历史读取事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置公开价格历史隔离级别失败"))?;
    let rows = sqlx::query(
        "SELECT
            l.id AS lot_id,
            a.id AS auction_id,
            a.world_id,
            a.sequence AS auction_sequence,
            l.lot_number,
            e.name AS equipment_name,
            e.category AS equipment_category,
            e.attributes AS equipment_attributes,
            buyer.username AS buyer_name,
            COALESCE(submission.seller, '—')::text AS seller_name,
            l.current_price AS price,
            l.effective_end_at AS closed_at
         FROM public.lots AS l
         JOIN public.auctions AS a ON a.id = l.auction_id
         JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
         JOIN public.equipment AS e ON e.id = l.equipment_id
         JOIN public.users AS buyer ON buyer.id = l.leader_user_id
         LEFT JOIN public.auction_submissions AS submission ON submission.id = l.submission_id
         JOIN public.lot_settlements AS ls ON ls.lot_id = l.id
         JOIN public.auction_settlements AS settlement ON settlement.auction_id = a.id
         WHERE a.status = 'settled'
           AND l.status = 'sold'
           AND l.leader_user_id IS NOT NULL
           AND ($1::text[] IS NULL OR e.name ILIKE ALL($1::text[]))
           AND ($2::text IS NULL OR a.world_id = $2)
           AND ($3::timestamptz IS NULL OR l.effective_end_at >= $3)
           AND ($4::timestamptz IS NULL OR l.effective_end_at < $4)
           AND ($5::bigint IS NULL OR l.current_price >= $5)
           AND ($6::bigint IS NULL OR l.current_price <= $6)
           AND ($7::text IS NULL OR lower(buyer.username) = lower($7))
           AND ($8::text IS NULL OR lower(submission.seller) = lower($8))
           AND (
                NOT $9::boolean
                OR l.effective_end_at < $10
                OR (
                    l.effective_end_at = $10
                    AND (
                        a.sequence < $11
                        OR (a.sequence = $11 AND (
                            l.lot_number > $12
                            OR (l.lot_number = $12 AND l.id > $13)
                        ))
                    )
                )
           )
         ORDER BY l.effective_end_at DESC, a.sequence DESC, l.lot_number, l.id
         LIMIT $14 OFFSET $15",
    )
    .bind(name_patterns.clone())
    .bind(request.world_id.as_deref())
    .bind(request.closed_from)
    .bind(request.closed_before)
    .bind(request.minimum_price)
    .bind(request.maximum_price)
    .bind(request.buyer.as_deref())
    .bind(request.seller.as_deref())
    .bind(use_boundary)
    .bind(boundary_closed_at)
    .bind(boundary_sequence)
    .bind(boundary_lot_number)
    .bind(boundary_lot_id)
    .bind(query_limit)
    .bind(if request.cursor.is_some() { 0 } else { request.offset })
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 公开价格历史失败"))?;
    let mut entries = rows.into_iter().map(map_postgres_entry).collect::<Result<Vec<_>, _>>()?;
    validate_entries(&entries)?;
    let has_more = request.cursor.is_some() && entries.len() as i64 > request.limit;
    if has_more {
        entries.truncate(request.limit as usize);
    }
    let next_cursor = has_more.then(|| entries.last().map(entry_cursor)).flatten();
    let (total, stats) = if request.cursor.is_some() {
        (None, None)
    } else {
        let row = sqlx::query(
            "SELECT
                count(*)::bigint AS total,
                COALESCE(min(l.current_price), 0)::bigint AS minimum_price,
                COALESCE(max(l.current_price), 0)::bigint AS maximum_price,
                COALESCE(
                    percentile_cont(0.5) WITHIN GROUP (ORDER BY l.current_price),
                    0
                )::bigint AS median_price
             FROM public.lots AS l
             JOIN public.auctions AS a ON a.id = l.auction_id
             JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
             JOIN public.equipment AS e ON e.id = l.equipment_id
             JOIN public.users AS buyer ON buyer.id = l.leader_user_id
             LEFT JOIN public.auction_submissions AS submission ON submission.id = l.submission_id
             JOIN public.lot_settlements AS ls ON ls.lot_id = l.id
             JOIN public.auction_settlements AS settlement ON settlement.auction_id = a.id
             WHERE a.status = 'settled'
               AND l.status = 'sold'
               AND l.leader_user_id IS NOT NULL
               AND ($1::text[] IS NULL OR e.name ILIKE ALL($1::text[]))
               AND ($2::text IS NULL OR a.world_id = $2)
               AND ($3::timestamptz IS NULL OR l.effective_end_at >= $3)
               AND ($4::timestamptz IS NULL OR l.effective_end_at < $4)
               AND ($5::bigint IS NULL OR l.current_price >= $5)
               AND ($6::bigint IS NULL OR l.current_price <= $6)
               AND ($7::text IS NULL OR lower(buyer.username) = lower($7))
               AND ($8::text IS NULL OR lower(submission.seller) = lower($8))",
        )
        .bind(name_patterns)
        .bind(request.world_id.as_deref())
        .bind(request.closed_from)
        .bind(request.closed_before)
        .bind(request.minimum_price)
        .bind(request.maximum_price)
        .bind(request.buyer.as_deref())
        .bind(request.seller.as_deref())
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| storage_error("统计 PostgreSQL 公开价格历史失败"))?;
        let stats = map_postgres_stats(row)?;
        (Some(stats.count), Some(stats))
    };
    let offset = if request.cursor.is_some() { 0 } else { request.offset };
    transaction.commit().await.map_err(|_| storage_error("提交公开价格历史读取事务失败"))?;
    Ok(PublicPriceHistoryPage {
        entries,
        total,
        stats,
        limit: request.limit,
        offset,
        next_cursor,
        has_more,
    })
}

#[cfg(feature = "sqlite")]
async fn list_sqlite(
    store: &super::SqliteStore,
    request: PublicPriceHistoryListRequest,
) -> Result<PublicPriceHistoryPage, ApplicationError> {
    let patterns = history_name_patterns(&request).unwrap_or_default();
    let use_boundary =
        matches!(request.cursor.as_ref(), Some(PublicPriceHistoryCursor::Boundary { .. }));
    let (boundary_closed_at, boundary_sequence, boundary_lot_number, boundary_lot_id) =
        cursor_values(&request);
    let query_limit = if request.cursor.is_some() {
        request
            .limit
            .checked_add(1)
            .ok_or_else(|| storage_error("Lite 公开价格历史分页上限溢出"))?
    } else {
        request.limit
    };
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|_| storage_error("开启 Lite 公开价格历史读取事务失败"))?;
    let rows = sqlx::query(
        "WITH boundary AS (
            SELECT
                ? AS use_boundary,
                ? AS before_closed_at,
                ? AS before_auction_sequence,
                ? AS before_lot_number,
                ? AS before_lot_id
         )
         SELECT
            l.id AS lot_id,
            a.id AS auction_id,
            a.world_id,
            a.sequence AS auction_sequence,
            l.lot_number,
            e.name AS equipment_name,
            e.category AS equipment_category,
            e.attributes AS equipment_attributes,
            buyer.username AS buyer_name,
            COALESCE(submission.seller, '—') AS seller_name,
            l.current_price AS price,
            l.effective_end_at AS closed_at
         FROM lots AS l
         JOIN auctions AS a ON a.id = l.auction_id
         JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
         JOIN equipment AS e ON e.id = l.equipment_id
         JOIN users AS buyer ON buyer.id = l.leader_user_id
         LEFT JOIN auction_submissions AS submission ON submission.id = l.submission_id
         CROSS JOIN boundary
         WHERE a.status = 'settled'
           AND l.status = 'sold'
           AND l.leader_user_id IS NOT NULL
           AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
           AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
           AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
           AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
           AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
           AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
           AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
           AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
           AND (? IS NULL OR a.world_id = ?)
           AND (? IS NULL OR julianday(l.effective_end_at) >= julianday(?))
           AND (? IS NULL OR julianday(l.effective_end_at) < julianday(?))
           AND (? IS NULL OR l.current_price >= ?)
           AND (? IS NULL OR l.current_price <= ?)
           AND (? IS NULL OR lower(buyer.username) = lower(?))
           AND (? IS NULL OR lower(submission.seller) = lower(?))
           AND (
                boundary.use_boundary = 0
                OR julianday(l.effective_end_at) < julianday(boundary.before_closed_at)
                OR (
                    julianday(l.effective_end_at) = julianday(boundary.before_closed_at)
                    AND (
                        a.sequence < boundary.before_auction_sequence
                        OR (a.sequence = boundary.before_auction_sequence AND (
                            l.lot_number > boundary.before_lot_number
                            OR (l.lot_number = boundary.before_lot_number AND l.id > boundary.before_lot_id)
                        ))
                    )
                )
           )
         ORDER BY julianday(l.effective_end_at) DESC, a.sequence DESC, l.lot_number, l.id
         LIMIT ? OFFSET ?",
    )
    .bind(if use_boundary { 1_i64 } else { 0_i64 })
    .bind(sqlite_timestamp(boundary_closed_at))
    .bind(boundary_sequence)
    .bind(boundary_lot_number)
    .bind(uuid_blob(boundary_lot_id))
    .bind(patterns.first().cloned())
    .bind(patterns.first().cloned())
    .bind(patterns.get(1).cloned())
    .bind(patterns.get(1).cloned())
    .bind(patterns.get(2).cloned())
    .bind(patterns.get(2).cloned())
    .bind(patterns.get(3).cloned())
    .bind(patterns.get(3).cloned())
    .bind(patterns.get(4).cloned())
    .bind(patterns.get(4).cloned())
    .bind(patterns.get(5).cloned())
    .bind(patterns.get(5).cloned())
    .bind(patterns.get(6).cloned())
    .bind(patterns.get(6).cloned())
    .bind(patterns.get(7).cloned())
    .bind(patterns.get(7).cloned())
    .bind(request.world_id.as_deref())
    .bind(request.world_id.as_deref())
    .bind(request.closed_from.map(sqlite_timestamp))
    .bind(request.closed_from.map(sqlite_timestamp))
    .bind(request.closed_before.map(sqlite_timestamp))
    .bind(request.closed_before.map(sqlite_timestamp))
    .bind(request.minimum_price)
    .bind(request.minimum_price)
    .bind(request.maximum_price)
    .bind(request.maximum_price)
    .bind(request.buyer.as_deref())
    .bind(request.buyer.as_deref())
    .bind(request.seller.as_deref())
    .bind(request.seller.as_deref())
    .bind(query_limit)
    .bind(if request.cursor.is_some() { 0 } else { request.offset })
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 SQLite 公开价格历史失败"))?;
    let mut entries = rows.into_iter().map(map_sqlite_entry).collect::<Result<Vec<_>, _>>()?;
    validate_entries(&entries)?;
    let has_more = request.cursor.is_some() && entries.len() as i64 > request.limit;
    if has_more {
        entries.truncate(request.limit as usize);
    }
    let next_cursor = has_more.then(|| entries.last().map(entry_cursor)).flatten();
    let (total, stats) = if request.cursor.is_some() {
        (None, None)
    } else {
        let row = sqlx::query(
            "WITH filtered AS (
                SELECT l.current_price AS price
                FROM lots AS l
                JOIN auctions AS a ON a.id = l.auction_id
                JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
                JOIN equipment AS e ON e.id = l.equipment_id
                JOIN users AS buyer ON buyer.id = l.leader_user_id
                LEFT JOIN auction_submissions AS submission ON submission.id = l.submission_id
                WHERE a.status = 'settled'
                  AND l.status = 'sold'
                  AND l.leader_user_id IS NOT NULL
                  AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
                  AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
                  AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
                  AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
                  AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
                  AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
                  AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
                  AND (? IS NULL OR lower(e.name) LIKE lower(?) ESCAPE '\\')
                  AND (? IS NULL OR a.world_id = ?)
                  AND (? IS NULL OR julianday(l.effective_end_at) >= julianday(?))
                  AND (? IS NULL OR julianday(l.effective_end_at) < julianday(?))
                  AND (? IS NULL OR l.current_price >= ?)
                  AND (? IS NULL OR l.current_price <= ?)
                  AND (? IS NULL OR lower(buyer.username) = lower(?))
                  AND (? IS NULL OR lower(submission.seller) = lower(?))
            ), ranked AS (
                SELECT price,
                       row_number() OVER (ORDER BY price) AS position,
                       count(*) OVER () AS total
                FROM filtered
            )
            SELECT
                COALESCE(MAX(total), 0) AS total,
                COALESCE(MIN(price), 0) AS minimum_price,
                COALESCE(MAX(price), 0) AS maximum_price,
                COALESCE(
                    CAST(AVG(CASE
                        WHEN position = (total + 1) / 2 OR position = (total + 2) / 2
                        THEN price
                    END) AS INTEGER),
                    0
                ) AS median_price
            FROM ranked",
        )
        .bind(patterns.first().cloned())
        .bind(patterns.first().cloned())
        .bind(patterns.get(1).cloned())
        .bind(patterns.get(1).cloned())
        .bind(patterns.get(2).cloned())
        .bind(patterns.get(2).cloned())
        .bind(patterns.get(3).cloned())
        .bind(patterns.get(3).cloned())
        .bind(patterns.get(4).cloned())
        .bind(patterns.get(4).cloned())
        .bind(patterns.get(5).cloned())
        .bind(patterns.get(5).cloned())
        .bind(patterns.get(6).cloned())
        .bind(patterns.get(6).cloned())
        .bind(patterns.get(7).cloned())
        .bind(patterns.get(7).cloned())
        .bind(request.world_id.as_deref())
        .bind(request.world_id.as_deref())
        .bind(request.closed_from.map(sqlite_timestamp))
        .bind(request.closed_from.map(sqlite_timestamp))
        .bind(request.closed_before.map(sqlite_timestamp))
        .bind(request.closed_before.map(sqlite_timestamp))
        .bind(request.minimum_price)
        .bind(request.minimum_price)
        .bind(request.maximum_price)
        .bind(request.maximum_price)
        .bind(request.buyer.as_deref())
        .bind(request.buyer.as_deref())
        .bind(request.seller.as_deref())
        .bind(request.seller.as_deref())
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| storage_error("统计 SQLite 公开价格历史失败"))?;
        let stats = map_sqlite_stats(row)?;
        (Some(stats.count), Some(stats))
    };
    let offset = if request.cursor.is_some() { 0 } else { request.offset };
    transaction.commit().await.map_err(|_| storage_error("提交 Lite 公开价格历史读取事务失败"))?;
    Ok(PublicPriceHistoryPage {
        entries,
        total,
        stats,
        limit: request.limit,
        offset,
        next_cursor,
        has_more,
    })
}

fn cursor_values(request: &PublicPriceHistoryListRequest) -> (DateTime<Utc>, i64, i64, Uuid) {
    match request.cursor.as_ref() {
        Some(PublicPriceHistoryCursor::Boundary {
            closed_at,
            auction_sequence,
            lot_number,
            lot_id,
        }) => (*closed_at, *auction_sequence, *lot_number, *lot_id),
        Some(PublicPriceHistoryCursor::Start) | None => (Utc::now(), 0, 0, Uuid::nil()),
    }
}

fn entry_cursor(entry: &PublicPriceHistoryEntry) -> PublicPriceHistoryCursor {
    PublicPriceHistoryCursor::Boundary {
        closed_at: entry.closed_at,
        auction_sequence: entry.auction_sequence,
        lot_number: entry.lot_number,
        lot_id: entry.lot_id,
    }
}

fn history_name_patterns(request: &PublicPriceHistoryListRequest) -> Option<Vec<String>> {
    if request.name_terms.is_empty() {
        return None;
    }
    Some(
        request
            .name_terms
            .iter()
            .map(|term| {
                format!("%{}%", term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
            })
            .collect(),
    )
}

fn validate_entries(entries: &[PublicPriceHistoryEntry]) -> Result<(), ApplicationError> {
    if entries.iter().any(|entry| {
        entry.lot_id.is_nil()
            || entry.auction_id.is_nil()
            || entry.world_id.is_empty()
            || entry.world_id != entry.world_id.trim()
            || entry.auction_sequence <= 0
            || entry.lot_number <= 0
            || entry.equipment_name.is_empty()
            || entry.equipment_category.is_empty()
            || entry.equipment_level.is_some_and(|level| level < 0)
            || entry.buyer_name.is_empty()
            || entry.seller_name.is_empty()
            || entry.price <= 0
            || entry.closed_at.timestamp_micros() == 0
    }) {
        return Err(storage_error("公开价格历史包含无效条目"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn map_postgres_entry(
    row: sqlx::postgres::PgRow,
) -> Result<PublicPriceHistoryEntry, ApplicationError> {
    let attributes: sqlx::types::Json<Value> = row
        .try_get("equipment_attributes")
        .map_err(|_| storage_error("读取 PostgreSQL 历史装备属性失败"))?;
    Ok(PublicPriceHistoryEntry {
        lot_id: row
            .try_get("lot_id")
            .map_err(|_| storage_error("读取 PostgreSQL 历史 lot 标识失败"))?,
        auction_id: row
            .try_get("auction_id")
            .map_err(|_| storage_error("读取 PostgreSQL 历史拍卖标识失败"))?,
        world_id: row
            .try_get("world_id")
            .map_err(|_| storage_error("读取 PostgreSQL 历史世界标识失败"))?,
        auction_sequence: row
            .try_get("auction_sequence")
            .map_err(|_| storage_error("读取 PostgreSQL 历史拍卖序号失败"))?,
        lot_number: i64::from(
            row.try_get::<i32, _>("lot_number")
                .map_err(|_| storage_error("读取 PostgreSQL 历史 lot 序号失败"))?,
        ),
        equipment_name: row
            .try_get("equipment_name")
            .map_err(|_| storage_error("读取 PostgreSQL 历史装备名称失败"))?,
        equipment_category: row
            .try_get("equipment_category")
            .map_err(|_| storage_error("读取 PostgreSQL 历史装备分类失败"))?,
        equipment_level: equipment_level(attributes.0),
        buyer_name: row
            .try_get("buyer_name")
            .map_err(|_| storage_error("读取 PostgreSQL 历史买家名称失败"))?,
        seller_name: row
            .try_get("seller_name")
            .map_err(|_| storage_error("读取 PostgreSQL 历史卖家名称失败"))?,
        price: row.try_get("price").map_err(|_| storage_error("读取 PostgreSQL 历史成交价失败"))?,
        closed_at: row
            .try_get::<DateTime<Utc>, _>("closed_at")
            .map_err(|_| storage_error("读取 PostgreSQL 历史结束时间失败"))?,
    })
}

#[cfg(feature = "postgres")]
fn map_postgres_stats(
    row: sqlx::postgres::PgRow,
) -> Result<PublicPriceHistoryStats, ApplicationError> {
    let stats = PublicPriceHistoryStats {
        count: row.try_get("total").map_err(|_| storage_error("读取 PostgreSQL 历史总数失败"))?,
        minimum: row
            .try_get("minimum_price")
            .map_err(|_| storage_error("读取 PostgreSQL 历史最低价失败"))?,
        maximum: row
            .try_get("maximum_price")
            .map_err(|_| storage_error("读取 PostgreSQL 历史最高价失败"))?,
        median: row
            .try_get("median_price")
            .map_err(|_| storage_error("读取 PostgreSQL 历史中位价失败"))?,
    };
    validate_stats(stats)
}

#[cfg(feature = "sqlite")]
fn map_sqlite_entry(
    row: sqlx::sqlite::SqliteRow,
) -> Result<PublicPriceHistoryEntry, ApplicationError> {
    let attributes = serde_json::from_str::<Value>(
        &row.try_get::<String, _>("equipment_attributes")
            .map_err(|_| storage_error("读取 SQLite 历史装备属性失败"))?,
    )
    .map_err(|_| storage_error("SQLite 历史装备属性不是有效 JSON"))?;
    Ok(PublicPriceHistoryEntry {
        lot_id: uuid_from_blob(
            row.try_get("lot_id").map_err(|_| storage_error("读取 SQLite 历史 lot 标识失败"))?,
            "SQLite 历史 lot 标识",
        )?,
        auction_id: uuid_from_blob(
            row.try_get("auction_id").map_err(|_| storage_error("读取 SQLite 历史拍卖标识失败"))?,
            "SQLite 历史拍卖标识",
        )?,
        world_id: row
            .try_get("world_id")
            .map_err(|_| storage_error("读取 SQLite 历史世界标识失败"))?,
        auction_sequence: row
            .try_get("auction_sequence")
            .map_err(|_| storage_error("读取 SQLite 历史拍卖序号失败"))?,
        lot_number: row
            .try_get("lot_number")
            .map_err(|_| storage_error("读取 SQLite 历史 lot 序号失败"))?,
        equipment_name: row
            .try_get("equipment_name")
            .map_err(|_| storage_error("读取 SQLite 历史装备名称失败"))?,
        equipment_category: row
            .try_get("equipment_category")
            .map_err(|_| storage_error("读取 SQLite 历史装备分类失败"))?,
        equipment_level: equipment_level(attributes),
        buyer_name: row
            .try_get("buyer_name")
            .map_err(|_| storage_error("读取 SQLite 历史买家名称失败"))?,
        seller_name: row
            .try_get("seller_name")
            .map_err(|_| storage_error("读取 SQLite 历史卖家名称失败"))?,
        price: row.try_get("price").map_err(|_| storage_error("读取 SQLite 历史成交价失败"))?,
        closed_at: parse_timestamp(
            row.try_get("closed_at").map_err(|_| storage_error("读取 SQLite 历史结束时间失败"))?,
            "SQLite 历史结束时间",
        )?,
    })
}

#[cfg(feature = "sqlite")]
fn map_sqlite_stats(
    row: sqlx::sqlite::SqliteRow,
) -> Result<PublicPriceHistoryStats, ApplicationError> {
    let stats = PublicPriceHistoryStats {
        count: row.try_get("total").map_err(|_| storage_error("读取 SQLite 历史总数失败"))?,
        minimum: row
            .try_get("minimum_price")
            .map_err(|_| storage_error("读取 SQLite 历史最低价失败"))?,
        maximum: row
            .try_get("maximum_price")
            .map_err(|_| storage_error("读取 SQLite 历史最高价失败"))?,
        median: row
            .try_get("median_price")
            .map_err(|_| storage_error("读取 SQLite 历史中位价失败"))?,
    };
    validate_stats(stats)
}

fn equipment_level(attributes: Value) -> Option<i64> {
    attributes.get("level").and_then(Value::as_i64).filter(|level| *level >= 0)
}

fn validate_stats(
    stats: PublicPriceHistoryStats,
) -> Result<PublicPriceHistoryStats, ApplicationError> {
    if stats.count < 0
        || stats.count == 0 && (stats.minimum != 0 || stats.maximum != 0 || stats.median != 0)
        || stats.count > 0
            && (stats.minimum <= 0
                || stats.maximum < stats.minimum
                || stats.median < stats.minimum
                || stats.median > stats.maximum)
    {
        return Err(storage_error("公开价格历史统计无效"));
    }
    Ok(stats)
}

#[cfg(feature = "sqlite")]
fn uuid_blob(id: Uuid) -> Vec<u8> {
    id.as_bytes().to_vec()
}

#[cfg(feature = "sqlite")]
fn uuid_from_blob(bytes: Vec<u8>, label: &str) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&bytes).map_err(|_| storage_error(label))
}

#[cfg(feature = "sqlite")]
fn sqlite_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339()
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
