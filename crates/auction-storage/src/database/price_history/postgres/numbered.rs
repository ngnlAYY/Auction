//! PostgreSQL 价格历史编号页：一次筛选同时供给分页和完整统计。

use auction_application::{
    ApplicationError, PublicPriceHistoryListRequest, PublicPriceHistoryPage,
    PublicPriceHistoryStats,
};
use sqlx::Row;
use uuid::Uuid;

use super::super::mapping::{
    history_name_patterns, storage_error, validate_entries, validate_stats,
};
use super::map_postgres_entry;
use crate::database::PostgresStore;

pub(super) async fn list(
    store: &PostgresStore,
    request: PublicPriceHistoryListRequest,
) -> Result<PublicPriceHistoryPage, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|_| storage_error("开启公开价格历史读取事务失败"))?;

    // 编号页仍需返回同一快照内的完整统计；仅物化筛选、排序和金额所需的窄行，避免两次
    // 执行完整过滤联表。装备等级只为当前页读取，空页通过统计行保留 total/stats。
    // 中位数先选中间一至两笔再按 numeric 求均值，不将 bigint 价格转为浮点或先整数求和。
    // 旧整数 median 显式半数取偶，保留原 float8 转 bigint 的数值口径。
    let rows = sqlx::query(concat!(
        "WITH matching_lots AS MATERIALIZED (
            SELECT l.id AS lot_id, l.current_price AS price, l.effective_end_at AS closed_at,
                   a.sequence AS auction_sequence, l.lot_number AS ordering_lot_number
            ",
        price_history_scope_sql!(),
        "), ranked_prices AS (
            SELECT price,
                   row_number() OVER (ORDER BY price, lot_id) AS price_rank,
                   count(*) OVER () AS sample_count
            FROM matching_lots
         ), median_value AS (
            SELECT COALESCE(avg(price::numeric), 0::numeric) AS exact_price
            FROM ranked_prices
            WHERE price_rank = sample_count / 2 + sample_count % 2
               OR price_rank = sample_count / 2 + 1
         ), statistics AS (
            SELECT count(*)::bigint AS total,
                   COALESCE(min(price), 0)::bigint AS minimum_price,
                   COALESCE(max(price), 0)::bigint AS maximum_price,
                   (SELECT (floor(exact_price) + CASE
                       WHEN exact_price - floor(exact_price) = 0.5
                        AND mod(floor(exact_price), 2) = 1 THEN 1
                       ELSE 0 END)::bigint FROM median_value) AS median_price,
                   (SELECT trim_scale(exact_price)::text FROM median_value) AS median_exact
            FROM matching_lots
         ), page_lots AS (
            SELECT lot_id, price, closed_at, auction_sequence, ordering_lot_number
            FROM matching_lots
            ORDER BY CASE WHEN $11::text = 'closed_asc' THEN closed_at END ASC,
                     CASE WHEN $11::text = 'price_desc' THEN price END DESC,
                     CASE WHEN $11::text = 'price_asc' THEN price END ASC,
                     closed_at DESC, auction_sequence DESC, ordering_lot_number, lot_id
            LIMIT $9 OFFSET $10
         )
         SELECT statistics.total, statistics.minimum_price, statistics.maximum_price,
                statistics.median_price, statistics.median_exact,
                page_lots.lot_id, a.id AS auction_id, a.world_id,
                page_lots.auction_sequence, page_lots.ordering_lot_number,
                l.category_number AS lot_number,
                e.name AS equipment_name, e.category AS equipment_category,
                COALESCE(e.attributes -> 'level', 'null'::jsonb) AS equipment_level,
                buyer.username AS buyer_name,
                COALESCE(submission.seller, '—')::text AS seller_name,
                page_lots.price, page_lots.closed_at
         FROM statistics
         LEFT JOIN page_lots ON TRUE
         LEFT JOIN public.lots AS l ON l.id = page_lots.lot_id
         LEFT JOIN public.auctions AS a ON a.id = l.auction_id
         LEFT JOIN public.equipment AS e ON e.id = l.equipment_id
         LEFT JOIN public.users AS buyer ON buyer.id = l.leader_user_id
         LEFT JOIN public.auction_submissions AS submission ON submission.id = l.submission_id
         ORDER BY CASE WHEN $11::text = 'closed_asc' THEN page_lots.closed_at END ASC,
                  CASE WHEN $11::text = 'price_desc' THEN page_lots.price END DESC,
                  CASE WHEN $11::text = 'price_asc' THEN page_lots.price END ASC,
                  page_lots.closed_at DESC, page_lots.auction_sequence DESC,
                  page_lots.ordering_lot_number, page_lots.lot_id"
    ))
    .bind(history_name_patterns(&request))
    .bind(request.world_id.as_deref())
    .bind(request.closed_from)
    .bind(request.closed_before)
    .bind(request.minimum_price)
    .bind(request.maximum_price)
    .bind(request.buyer.as_deref())
    .bind(request.seller.as_deref())
    .bind(request.limit)
    .bind(request.offset)
    .bind(request.sort.as_str())
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 公开价格历史编号页失败"))?;

    let stats =
        map_stats(rows.first().ok_or_else(|| storage_error("公开价格历史缺少编号页统计"))?)?;
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let lot_id: Option<Uuid> = row
            .try_get("lot_id")
            .map_err(|_| storage_error("读取 PostgreSQL 历史 lot 标识失败"))?;
        if lot_id.is_some() {
            entries.push(map_postgres_entry(row)?);
        }
    }
    validate_entries(&entries)?;
    transaction.commit().await.map_err(|_| storage_error("提交公开价格历史读取事务失败"))?;
    Ok(PublicPriceHistoryPage {
        entries,
        total: Some(stats.count),
        stats: Some(stats),
        limit: request.limit,
        offset: request.offset,
        next_cursor: None,
        has_more: false,
    })
}

fn map_stats(row: &sqlx::postgres::PgRow) -> Result<PublicPriceHistoryStats, ApplicationError> {
    validate_stats(PublicPriceHistoryStats {
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
        median_exact: Some(
            row.try_get("median_exact")
                .map_err(|_| storage_error("读取 PostgreSQL 历史精确中位价失败"))?,
        ),
    })
}
