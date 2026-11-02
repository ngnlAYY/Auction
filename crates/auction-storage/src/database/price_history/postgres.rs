//! PostgreSQL Public 成交价格历史查询与行映射。

use auction_application::{
    ApplicationError, PublicPriceHistoryCursor, PublicPriceHistoryEntry,
    PublicPriceHistoryListRequest, PublicPriceHistoryPage,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::Row;

use super::mapping::{
    cursor_values, entry_cursor, equipment_level, history_name_patterns, storage_error,
    validate_entries,
};
use crate::database::PostgresStore;

/// 成交价格历史的公开范围与筛选（$1 名称模式、$2 世界、$3/$4 成交时间、$5/$6 价格、$7 买家、
/// $8 卖家）；游标页与编号页共用，保证两种分页的可见集合一致。
macro_rules! price_history_scope_sql {
    () => {
        "FROM public.lots AS l
         JOIN public.auctions AS a ON a.id = l.auction_id
         JOIN public.worlds AS w ON w.id = a.world_id
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
         "
    };
}

mod numbered;

pub(super) async fn list_postgres(
    store: &PostgresStore,
    request: PublicPriceHistoryListRequest,
) -> Result<PublicPriceHistoryPage, ApplicationError> {
    if request.cursor.is_none() {
        return numbered::list(store, request).await;
    }
    let name_patterns = history_name_patterns(&request);
    let use_boundary =
        matches!(request.cursor.as_ref(), Some(PublicPriceHistoryCursor::Boundary { .. }));
    let (boundary_closed_at, boundary_sequence, boundary_lot_number, boundary_lot_id) =
        cursor_values(&request);
    // 仅额外探测行采用饱和加法；i64::MAX 已覆盖可表示行数，不改变请求及响应数量。
    let query_limit = request.limit.saturating_add(1);

    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|_| storage_error("开启公开价格历史读取事务失败"))?;
    let rows = sqlx::query(concat!(
        "SELECT
            l.id AS lot_id,
            a.id AS auction_id,
            a.world_id,
            a.sequence AS auction_sequence,
            l.lot_number AS ordering_lot_number,
            l.category_number AS lot_number,
            e.name AS equipment_name,
            e.category AS equipment_category,
            COALESCE(e.attributes -> 'level', 'null'::jsonb) AS equipment_level,
            buyer.username AS buyer_name,
            COALESCE(submission.seller, '—')::text AS seller_name,
            l.current_price AS price,
            l.effective_end_at AS closed_at
         ",
        price_history_scope_sql!(),
        "AND (
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
         LIMIT $14"
    ))
    .bind(name_patterns)
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
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 公开价格历史失败"))?;
    let mut entries = rows.into_iter().map(map_postgres_entry).collect::<Result<Vec<_>, _>>()?;
    validate_entries(&entries)?;
    let entry_count =
        i64::try_from(entries.len()).map_err(|_| storage_error("公开价格历史页大小溢出"))?;
    let has_more = entry_count > request.limit;
    if has_more {
        entries.truncate(request.limit as usize);
    }
    let next_cursor = has_more.then(|| entries.last().map(entry_cursor)).flatten();
    transaction.commit().await.map_err(|_| storage_error("提交公开价格历史读取事务失败"))?;
    Ok(PublicPriceHistoryPage {
        entries,
        total: None,
        stats: None,
        limit: request.limit,
        offset: 0,
        next_cursor,
        has_more,
    })
}

fn map_postgres_entry(
    row: sqlx::postgres::PgRow,
) -> Result<PublicPriceHistoryEntry, ApplicationError> {
    let level: sqlx::types::Json<Value> = row
        .try_get("equipment_level")
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
        ordering_lot_number: i64::from(
            row.try_get::<i32, _>("ordering_lot_number")
                .map_err(|_| storage_error("读取 PostgreSQL 历史内部顺序失败"))?,
        ),
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
        equipment_level: equipment_level(level.0),
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
