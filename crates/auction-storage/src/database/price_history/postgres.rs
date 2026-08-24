//! PostgreSQL Public 成交价格历史查询与行映射。

use auction_application::{
    ApplicationError, PublicPriceHistoryCursor, PublicPriceHistoryEntry,
    PublicPriceHistoryListRequest, PublicPriceHistoryPage, PublicPriceHistoryStats,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::Row;

use super::mapping::{
    cursor_values, entry_cursor, equipment_level, history_name_patterns, storage_error,
    validate_entries, validate_stats,
};
use crate::database::PostgresStore;

pub(super) async fn list_postgres(
    store: &PostgresStore,
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
