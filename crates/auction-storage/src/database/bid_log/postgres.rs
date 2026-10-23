//! PostgreSQL Public 竞价日志的快照查询与安全行映射。

use auction_application::{
    ApplicationError, PublicBidLogCursor, PublicBidLogEntry, PublicBidLogListRequest,
    PublicBidLogPage,
};
use sqlx::Row;

use super::super::PostgresStore;
use super::common::{cursor_values, entry_cursor, storage_error, validate_entries};

pub(super) async fn list_postgres(
    store: &PostgresStore,
    request: PublicBidLogListRequest,
) -> Result<Option<PublicBidLogPage>, ApplicationError> {
    let use_boundary = matches!(request.cursor.as_ref(), Some(PublicBidLogCursor::Boundary { .. }));
    let (
        boundary_created_at,
        boundary_lot_id,
        boundary_state_version,
        boundary_fact_index,
        boundary_id,
    ) = cursor_values(&request);
    // 仅额外探测行采用饱和加法；i64::MAX 已覆盖可表示行数，不改变请求及响应数量。
    let query_limit =
        if request.cursor.is_some() { request.limit.saturating_add(1) } else { request.limit };

    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启公开竞价日志读取事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置公开竞价日志隔离级别失败"))?;

    let projection = sqlx::query(
        "SELECT a.id
         FROM public.auctions AS a
         JOIN public.worlds AS w ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
         WHERE a.id = $1
           AND a.status IN ('live', 'final', 'settled')
           AND ($2::text IS NULL OR a.world_id = $2)",
    )
    .bind(request.auction_id)
    .bind(request.world_id.as_deref())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取公开竞价日志拍卖 projection 失败"))?;
    if projection.is_none() {
        transaction.commit().await.map_err(|_| storage_error("提交公开竞价日志读取事务失败"))?;
        return Ok(None);
    }

    // 保留每条已接受事实；普通出价及同事务代理回应都公开，避免隐藏触发报价后造成自抬价错觉。
    let rows = sqlx::query(
        "SELECT
            b.id,
            l.auction_id,
            b.lot_id,
            l.category_number AS lot_number,
            e.name AS equipment_name,
            e.category AS equipment_category,
            users.username AS bidder_name,
            CASE
                WHEN b.mechanism = 'proxy' THEN 'proxy'
                WHEN forum_command.command_id IS NOT NULL THEN 'forum'
                ELSE NULL
            END AS source,
            CASE
                WHEN b.mechanism = 'ordinary' THEN forum_command.post_number
                ELSE NULL
            END AS source_post_number,
            CASE
                WHEN b.mechanism = 'ordinary' THEN forum_command.thread_id
                ELSE NULL
            END AS source_thread_id,
            CASE
                WHEN b.mechanism = 'ordinary' THEN forum_command.post_id
                ELSE NULL
            END AS source_post_id,
            b.amount,
            b.accepted_state_version,
            b.fact_index,
            b.mechanism,
            b.created_at,
            COALESCE(l.leader_user_id = b.bidder_user_id AND l.current_price = b.amount, false)::boolean AS valid
         FROM public.bids AS b
         JOIN public.lots AS l ON l.id = b.lot_id
         JOIN public.auctions AS a ON a.id = l.auction_id
         JOIN public.worlds AS w ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
         JOIN public.equipment AS e ON e.id = l.equipment_id
         JOIN public.users AS users ON users.id = b.bidder_user_id
         JOIN public.auction_commands AS command ON command.command_id = b.command_id
         LEFT JOIN public.auction_forum_bid_commands AS forum_command
           ON forum_command.command_id = b.command_id
          AND forum_command.auction_id = a.id
         WHERE a.id = $1
           AND a.status IN ('live', 'final', 'settled')
           AND l.status IN ('live', 'sold', 'unsold')
           AND ($2::text IS NULL OR a.world_id = $2)
           AND ($3::uuid IS NULL OR l.id = $3)
           AND (
                $4::text IS NULL
                OR lower(
                    CASE e.category
                        WHEN 'One-Handed' THEN 'One'
                        WHEN 'Two-Handed' THEN 'Two'
                        WHEN 'Staff' THEN 'Sta'
                        WHEN 'Shield' THEN 'Shd'
                        WHEN 'Cloth' THEN 'Clo'
                        WHEN 'Light' THEN 'Lig'
                        WHEN 'Heavy' THEN 'Hea'
                        ELSE 'Lot'
                    END || CASE
                        WHEN l.category_number < 10 THEN '0' || l.category_number::text
                        ELSE l.category_number::text
                    END
                ) LIKE '%' || lower($4) || '%'
           )
           AND command.command_kind IN (
                'place_bid', 'create_proxy', 'create_proxy_authorization',
                'execute_slow_response', 'final_proxy_transition'
           )
           AND (
                NOT $5::boolean
                OR b.created_at < $6
                OR (
                    b.created_at = $6
                    AND b.lot_id < $7
                )
                OR (
                    b.created_at = $6
                    AND b.lot_id = $7
                    AND b.accepted_state_version < $8
                )
                OR (
                    b.created_at = $6
                    AND b.lot_id = $7
                    AND b.accepted_state_version = $8
                    AND b.fact_index < $9
                )
                OR (
                    b.created_at = $6
                    AND b.lot_id = $7
                    AND b.accepted_state_version = $8
                    AND b.fact_index = $9
                    AND b.id < $10
                )
           )
         ORDER BY b.created_at DESC, b.lot_id DESC,
                  b.accepted_state_version DESC, b.fact_index DESC, b.id DESC
         LIMIT $11 OFFSET $12",
    )
    .bind(request.auction_id)
    .bind(request.world_id.as_deref())
    .bind(request.lot_id)
    .bind(request.lot_code.as_deref())
    .bind(use_boundary)
    .bind(boundary_created_at)
    .bind(boundary_lot_id)
    .bind(boundary_state_version)
    .bind(boundary_fact_index)
    .bind(boundary_id)
    .bind(query_limit)
    .bind(if request.cursor.is_some() { 0 } else { request.offset })
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 公开竞价日志失败"))?;

    let mut entries = rows.into_iter().map(map_postgres_entry).collect::<Result<Vec<_>, _>>()?;
    validate_entries(&entries, &request)?;
    let entry_count =
        i64::try_from(entries.len()).map_err(|_| storage_error("公开竞价日志页大小溢出"))?;
    let has_more = request.cursor.is_some() && entry_count > request.limit;
    if has_more {
        entries.truncate(request.limit as usize);
    }
    let next_cursor = has_more.then(|| entries.last().map(entry_cursor)).flatten();
    let total = if request.cursor.is_some() {
        None
    } else {
        // total 必须复用明细的可见性规则，避免页数和游标投影错位。
        Some(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*)
                 FROM public.bids AS b
                 JOIN public.lots AS l ON l.id = b.lot_id
                 JOIN public.auctions AS a ON a.id = l.auction_id
                 JOIN public.worlds AS w
                   ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
                 JOIN public.equipment AS e ON e.id = l.equipment_id
                 JOIN public.users AS users ON users.id = b.bidder_user_id
                 JOIN public.auction_commands AS command ON command.command_id = b.command_id
                 WHERE a.id = $1
                   AND a.status IN ('live', 'final', 'settled')
                   AND l.status IN ('live', 'sold', 'unsold')
                   AND ($2::text IS NULL OR a.world_id = $2)
                   AND ($3::uuid IS NULL OR l.id = $3)
                   AND (
                        $4::text IS NULL
                        OR lower(
                            CASE e.category
                                WHEN 'One-Handed' THEN 'One'
                                WHEN 'Two-Handed' THEN 'Two'
                                WHEN 'Staff' THEN 'Sta'
                                WHEN 'Shield' THEN 'Shd'
                                WHEN 'Cloth' THEN 'Clo'
                                WHEN 'Light' THEN 'Lig'
                                WHEN 'Heavy' THEN 'Hea'
                                ELSE 'Lot'
                            END || CASE
                                WHEN l.category_number < 10 THEN '0' || l.category_number::text
                                ELSE l.category_number::text
                            END
                        ) LIKE '%' || lower($4) || '%'
                   )
                   AND command.command_kind IN (
                        'place_bid', 'create_proxy', 'create_proxy_authorization',
                        'execute_slow_response', 'final_proxy_transition'
                   )",
            )
            .bind(request.auction_id)
            .bind(request.world_id.as_deref())
            .bind(request.lot_id)
            .bind(request.lot_code.as_deref())
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| storage_error("统计 PostgreSQL 公开竞价日志失败"))?,
        )
    };
    let offset = if request.cursor.is_some() { 0 } else { request.offset };
    transaction.commit().await.map_err(|_| storage_error("提交公开竞价日志读取事务失败"))?;
    Ok(Some(PublicBidLogPage {
        entries,
        total,
        limit: request.limit,
        offset,
        next_cursor,
        has_more,
    }))
}

fn map_postgres_entry(row: sqlx::postgres::PgRow) -> Result<PublicBidLogEntry, ApplicationError> {
    Ok(PublicBidLogEntry {
        id: row.try_get("id").map_err(|_| storage_error("读取 PostgreSQL 竞价事实标识失败"))?,
        auction_id: row
            .try_get("auction_id")
            .map_err(|_| storage_error("读取 PostgreSQL 竞价拍卖标识失败"))?,
        lot_id: row
            .try_get("lot_id")
            .map_err(|_| storage_error("读取 PostgreSQL 竞价 lot 标识失败"))?,
        lot_number: i64::from(
            row.try_get::<i32, _>("lot_number")
                .map_err(|_| storage_error("读取 PostgreSQL 竞价 lot 序号失败"))?,
        ),
        amount: row.try_get("amount").map_err(|_| storage_error("读取 PostgreSQL 竞价金额失败"))?,
        equipment_name: row
            .try_get("equipment_name")
            .map_err(|_| storage_error("读取 PostgreSQL 竞价装备名称失败"))?,
        equipment_category: row
            .try_get("equipment_category")
            .map_err(|_| storage_error("读取 PostgreSQL 竞价装备分类失败"))?,
        bidder_name: row
            .try_get("bidder_name")
            .map_err(|_| storage_error("读取 PostgreSQL 竞价用户名称失败"))?,
        source: row.try_get("source").map_err(|_| storage_error("读取 PostgreSQL 竞价来源失败"))?,
        source_post_number: row
            .try_get("source_post_number")
            .map_err(|_| storage_error("读取 PostgreSQL 论坛竞价楼层号失败"))?,
        source_thread_id: row
            .try_get("source_thread_id")
            .map_err(|_| storage_error("读取 PostgreSQL 论坛竞价主题标识失败"))?,
        source_post_id: row
            .try_get("source_post_id")
            .map_err(|_| storage_error("读取 PostgreSQL 论坛竞价帖子标识失败"))?,
        valid: row
            .try_get("valid")
            .map_err(|_| storage_error("读取 PostgreSQL 竞价有效标志失败"))?,
        accepted_state_version: row
            .try_get("accepted_state_version")
            .map_err(|_| storage_error("读取 PostgreSQL 竞价状态版本失败"))?,
        fact_index: row
            .try_get("fact_index")
            .map_err(|_| storage_error("读取 PostgreSQL 竞价事实序号失败"))?,
        mechanism: row
            .try_get("mechanism")
            .map_err(|_| storage_error("读取 PostgreSQL 竞价机制失败"))?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| storage_error("读取 PostgreSQL 竞价创建时间失败"))?,
    })
}
