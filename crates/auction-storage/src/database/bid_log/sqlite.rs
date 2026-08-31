//! SQLite Lite Public 竞价日志查询、类型转换和安全行映射。

use auction_application::{
    ApplicationError, PublicBidLogCursor, PublicBidLogEntry, PublicBidLogListRequest,
    PublicBidLogPage,
};
use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::super::SqliteStore;
use super::common::{cursor_values, entry_cursor, storage_error, validate_entries};

pub(super) async fn list_sqlite(
    store: &SqliteStore,
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
        request
            .limit
            .checked_add(1)
            .ok_or_else(|| storage_error("Lite 公开竞价日志分页上限溢出"))?
    } else {
        request.limit
    };
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|_| storage_error("开启 Lite 公开竞价日志读取事务失败"))?;
    let projection = sqlx::query(
        "SELECT a.id
         FROM auctions AS a
         JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
         WHERE a.id = ?
           AND a.status IN ('live', 'final', 'settled')
           AND (? IS NULL OR a.world_id = ?)",
    )
    .bind(uuid_blob(request.auction_id))
    .bind(request.world_id.as_deref())
    .bind(request.world_id.as_deref())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 Lite 公开竞价日志拍卖 projection 失败"))?;
    if projection.is_none() {
        transaction
            .commit()
            .await
            .map_err(|_| storage_error("提交 Lite 公开竞价日志读取事务失败"))?;
        return Ok(None);
    }

    let lot_id_blob = request.lot_id.map(uuid_blob);
    // Lite 与完整档复用同一公开可见性规则，不能让部署档位改变竞价历史语义。
    let rows = sqlx::query(
        "WITH boundary AS (
            SELECT
                ? AS use_boundary,
                ? AS before_created_at,
                ? AS before_lot_id,
                ? AS before_state_version,
                ? AS before_fact_index,
                ? AS before_id
         )
         SELECT
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
            COALESCE(l.leader_user_id = b.bidder_user_id AND l.current_price = b.amount, 0) AS valid
         FROM bids AS b
         JOIN lots AS l ON l.id = b.lot_id
         JOIN auctions AS a ON a.id = l.auction_id
         JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
         JOIN equipment AS e ON e.id = l.equipment_id
         JOIN users AS users ON users.id = b.bidder_user_id
         JOIN auction_commands AS command ON command.command_id = b.command_id
         CROSS JOIN boundary
         WHERE a.id = ?
           AND a.status IN ('live', 'final', 'settled')
           AND l.status IN ('live', 'sold', 'unsold')
           AND (? IS NULL OR a.world_id = ?)
           AND (? IS NULL OR l.id = ?)
           AND command.command_kind IN (
                'place_bid', 'create_proxy', 'create_proxy_authorization',
                'execute_slow_response', 'final_proxy_transition'
           )
           AND NOT (
                command.command_kind = 'place_bid'
                AND b.mechanism = 'ordinary'
                AND EXISTS (
                    SELECT 1
                    FROM bids AS proxy_response
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
                boundary.use_boundary = 0
                OR b.created_at < boundary.before_created_at
                OR (
                    b.created_at = boundary.before_created_at
                    AND b.lot_id < boundary.before_lot_id
                )
                OR (
                    b.created_at = boundary.before_created_at
                    AND b.lot_id = boundary.before_lot_id
                    AND b.accepted_state_version < boundary.before_state_version
                )
                OR (
                    b.created_at = boundary.before_created_at
                    AND b.lot_id = boundary.before_lot_id
                    AND b.accepted_state_version = boundary.before_state_version
                    AND b.fact_index < boundary.before_fact_index
                )
                OR (
                    b.created_at = boundary.before_created_at
                    AND b.lot_id = boundary.before_lot_id
                    AND b.accepted_state_version = boundary.before_state_version
                    AND b.fact_index = boundary.before_fact_index
                    AND b.id < boundary.before_id
                )
           )
         ORDER BY b.created_at DESC, b.lot_id DESC,
                  b.accepted_state_version DESC, b.fact_index DESC, b.id DESC
         LIMIT ? OFFSET ?",
    )
    .bind(if use_boundary { 1_i64 } else { 0_i64 })
    .bind(sqlite_timestamp_seconds(boundary_created_at))
    .bind(uuid_blob(boundary_lot_id))
    .bind(boundary_state_version)
    .bind(i64::from(boundary_fact_index))
    .bind(uuid_blob(boundary_id))
    .bind(uuid_blob(request.auction_id))
    .bind(request.world_id.as_deref())
    .bind(request.world_id.as_deref())
    .bind(lot_id_blob.as_deref())
    .bind(lot_id_blob.as_deref())
    .bind(query_limit)
    .bind(if request.cursor.is_some() { 0 } else { request.offset })
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 SQLite 公开竞价日志失败"))?;

    let mut entries = rows.into_iter().map(map_sqlite_entry).collect::<Result<Vec<_>, _>>()?;
    validate_entries(&entries, &request)?;
    let entry_count =
        i64::try_from(entries.len()).map_err(|_| storage_error("SQLite 公开竞价日志页大小溢出"))?;
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
                 FROM bids AS b
                 JOIN lots AS l ON l.id = b.lot_id
                 JOIN auctions AS a ON a.id = l.auction_id
                 JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
                 JOIN equipment AS e ON e.id = l.equipment_id
                 JOIN users AS users ON users.id = b.bidder_user_id
                 JOIN auction_commands AS command ON command.command_id = b.command_id
                 WHERE a.id = ?
                   AND a.status IN ('live', 'final', 'settled')
                   AND l.status IN ('live', 'sold', 'unsold')
                   AND (? IS NULL OR a.world_id = ?)
                   AND (? IS NULL OR l.id = ?)
                   AND command.command_kind IN (
                        'place_bid', 'create_proxy', 'create_proxy_authorization',
                        'execute_slow_response', 'final_proxy_transition'
                   )
                   AND NOT (
                        command.command_kind = 'place_bid'
                        AND b.mechanism = 'ordinary'
                        AND EXISTS (
                            SELECT 1
                            FROM bids AS proxy_response
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
            .bind(uuid_blob(request.auction_id))
            .bind(request.world_id.as_deref())
            .bind(request.world_id.as_deref())
            .bind(lot_id_blob.as_deref())
            .bind(lot_id_blob.as_deref())
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| storage_error("统计 SQLite 公开竞价日志失败"))?,
        )
    };
    let offset = if request.cursor.is_some() { 0 } else { request.offset };
    transaction.commit().await.map_err(|_| storage_error("提交 Lite 公开竞价日志读取事务失败"))?;
    Ok(Some(PublicBidLogPage {
        entries,
        total,
        limit: request.limit,
        offset,
        next_cursor,
        has_more,
    }))
}

fn map_sqlite_entry(row: sqlx::sqlite::SqliteRow) -> Result<PublicBidLogEntry, ApplicationError> {
    let fact_index = i32::try_from(
        row.try_get::<i64, _>("fact_index")
            .map_err(|_| storage_error("读取 SQLite 竞价事实序号失败"))?,
    )
    .map_err(|_| storage_error("SQLite 竞价事实序号超出范围"))?;
    Ok(PublicBidLogEntry {
        id: uuid_from_blob(
            row.try_get("id").map_err(|_| storage_error("读取 SQLite 竞价事实标识失败"))?,
            "SQLite 竞价事实标识",
        )?,
        auction_id: uuid_from_blob(
            row.try_get("auction_id").map_err(|_| storage_error("读取 SQLite 竞价拍卖标识失败"))?,
            "SQLite 竞价拍卖标识",
        )?,
        lot_id: uuid_from_blob(
            row.try_get("lot_id").map_err(|_| storage_error("读取 SQLite 竞价 lot 标识失败"))?,
            "SQLite 竞价 lot 标识",
        )?,
        lot_number: row
            .try_get("lot_number")
            .map_err(|_| storage_error("读取 SQLite 竞价 lot 序号失败"))?,
        amount: row.try_get("amount").map_err(|_| storage_error("读取 SQLite 竞价金额失败"))?,
        equipment_name: row
            .try_get("equipment_name")
            .map_err(|_| storage_error("读取 SQLite 竞价装备名称失败"))?,
        equipment_category: row
            .try_get("equipment_category")
            .map_err(|_| storage_error("读取 SQLite 竞价装备分类失败"))?,
        bidder_name: row
            .try_get("bidder_name")
            .map_err(|_| storage_error("读取 SQLite 竞价用户名称失败"))?,
        valid: row
            .try_get::<i64, _>("valid")
            .map_err(|_| storage_error("读取 SQLite 竞价有效标志失败"))?
            != 0,
        accepted_state_version: row
            .try_get("accepted_state_version")
            .map_err(|_| storage_error("读取 SQLite 竞价状态版本失败"))?,
        fact_index,
        mechanism: row
            .try_get("mechanism")
            .map_err(|_| storage_error("读取 SQLite 竞价机制失败"))?,
        created_at: parse_timestamp(
            row.try_get("created_at").map_err(|_| storage_error("读取 SQLite 竞价创建时间失败"))?,
            "SQLite 竞价创建时间",
        )?,
    })
}

fn uuid_blob(id: Uuid) -> Vec<u8> {
    id.as_bytes().to_vec()
}

fn uuid_from_blob(bytes: Vec<u8>, label: &str) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&bytes).map_err(|_| storage_error(label))
}

fn sqlite_timestamp_seconds(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%d %H:%M:%S").to_string()
}

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
