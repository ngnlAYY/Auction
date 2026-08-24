//! SQLite Lite Public 成交价格历史查询与行映射。

use auction_application::{
    ApplicationError, PublicPriceHistoryCursor, PublicPriceHistoryEntry,
    PublicPriceHistoryListRequest, PublicPriceHistoryPage, PublicPriceHistoryStats,
};
use serde_json::Value;
use sqlx::Row;

use super::mapping::{
    cursor_values, entry_cursor, equipment_level, history_name_patterns, parse_timestamp,
    sqlite_timestamp, storage_error, uuid_blob, uuid_from_blob, validate_entries, validate_stats,
};
use crate::database::SqliteStore;

pub(super) async fn list_sqlite(
    store: &SqliteStore,
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
