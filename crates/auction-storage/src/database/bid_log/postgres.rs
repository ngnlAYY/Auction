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
    let query_limit = if request.cursor.is_some() {
        request.limit.checked_add(1).ok_or_else(|| storage_error("公开竞价日志分页上限溢出"))?
    } else {
        request.limit
    };

    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启公开竞价日志读取事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置公开竞价日志隔离级别失败"))?;

    let projection = sqlx::query(
        "SELECT a.id
         FROM public.auctions AS a
         JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
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

    // Public 只压缩同一普通命令内被代理立即反击的展示行；内部竞价事实与账户记录保持完整。
    let rows = sqlx::query(
        "SELECT
            b.id,
            l.auction_id,
            b.lot_id,
            l.lot_number,
            e.name AS equipment_name,
            e.category AS equipment_category,
            users.username AS bidder_name,
            b.amount,
            b.accepted_state_version,
            b.fact_index,
            b.mechanism,
            b.created_at,
            COALESCE(l.leader_user_id = b.bidder_user_id AND l.current_price = b.amount, false)::boolean AS valid
         FROM public.bids AS b
         JOIN public.lots AS l ON l.id = b.lot_id
         JOIN public.auctions AS a ON a.id = l.auction_id
         JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
         JOIN public.equipment AS e ON e.id = l.equipment_id
         JOIN public.users AS users ON users.id = b.bidder_user_id
         JOIN public.auction_commands AS command ON command.command_id = b.command_id
         WHERE a.id = $1
           AND a.status IN ('live', 'final', 'settled')
           AND l.status IN ('live', 'sold', 'unsold')
           AND ($2::text IS NULL OR a.world_id = $2)
           AND ($3::uuid IS NULL OR l.id = $3)
           AND command.command_kind IN (
                'place_bid', 'create_proxy', 'create_proxy_authorization',
                'execute_slow_response', 'final_proxy_transition'
           )
           AND NOT (
                command.command_kind = 'place_bid'
                AND b.mechanism = 'ordinary'
                AND EXISTS (
                    SELECT 1
                    FROM public.bids AS proxy_response
                    WHERE proxy_response.command_id = b.command_id
                      AND proxy_response.lot_id = b.lot_id
                      AND proxy_response.fact_index = b.fact_index + 1
                      AND proxy_response.accepted_state_version = b.accepted_state_version + 1
                      AND proxy_response.bidder_user_id <> b.bidder_user_id
                      AND proxy_response.amount >= b.amount
                      AND proxy_response.mechanism = 'proxy'
                )
           )
           AND (
                NOT $4::boolean
                OR b.created_at < $5
                OR (
                    b.created_at = $5
                    AND b.lot_id < $6
                )
                OR (
                    b.created_at = $5
                    AND b.lot_id = $6
                    AND b.accepted_state_version < $7
                )
                OR (
                    b.created_at = $5
                    AND b.lot_id = $6
                    AND b.accepted_state_version = $7
                    AND b.fact_index < $8
                )
                OR (
                    b.created_at = $5
                    AND b.lot_id = $6
                    AND b.accepted_state_version = $7
                    AND b.fact_index = $8
                    AND b.id < $9
                )
           )
         ORDER BY b.created_at DESC, b.lot_id DESC,
                  b.accepted_state_version DESC, b.fact_index DESC, b.id DESC
         LIMIT $10 OFFSET $11",
    )
    .bind(request.auction_id)
    .bind(request.world_id.as_deref())
    .bind(request.lot_id)
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
                 JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
                 JOIN public.equipment AS e ON e.id = l.equipment_id
                 JOIN public.users AS users ON users.id = b.bidder_user_id
                 JOIN public.auction_commands AS command ON command.command_id = b.command_id
                 WHERE a.id = $1
                   AND a.status IN ('live', 'final', 'settled')
                   AND l.status IN ('live', 'sold', 'unsold')
                   AND ($2::text IS NULL OR a.world_id = $2)
                   AND ($3::uuid IS NULL OR l.id = $3)
                   AND command.command_kind IN (
                        'place_bid', 'create_proxy', 'create_proxy_authorization',
                        'execute_slow_response', 'final_proxy_transition'
                   )
                   AND NOT (
                        command.command_kind = 'place_bid'
                        AND b.mechanism = 'ordinary'
                        AND EXISTS (
                            SELECT 1
                            FROM public.bids AS proxy_response
                            WHERE proxy_response.command_id = b.command_id
                              AND proxy_response.lot_id = b.lot_id
                              AND proxy_response.fact_index = b.fact_index + 1
                              AND proxy_response.accepted_state_version = b.accepted_state_version + 1
                              AND proxy_response.bidder_user_id <> b.bidder_user_id
                              AND proxy_response.amount >= b.amount
                              AND proxy_response.mechanism = 'proxy'
                        )
                   )",
            )
            .bind(request.auction_id)
            .bind(request.world_id.as_deref())
            .bind(request.lot_id)
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
