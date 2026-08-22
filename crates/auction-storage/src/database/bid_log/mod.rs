//! Public 竞价日志只读适配器。
//!
//! 竞价日志只读取已经接受的 append-only bid facts，并在同一快照内应用拍卖状态、lot 状态、
//! world 与命令种类过滤。keyset cursor 完整覆盖旧契约的时间、lot、状态版本、事实序号和
//! bid ID 排序，HTTP 层只接触应用投影。

use auction_application::{
    ApplicationError, ApplicationFuture, PublicBidLogCursor, PublicBidLogEntry,
    PublicBidLogListRequest, PublicBidLogPage, PublicBidLogReader, RepositoryError,
};
#[cfg(feature = "sqlite")]
use chrono::NaiveDateTime;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::DatabaseStore;

impl PublicBidLogReader for DatabaseStore {
    fn list_public_bid_log(
        &self,
        request: PublicBidLogListRequest,
    ) -> ApplicationFuture<'_, Option<PublicBidLogPage>> {
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
                'place_bid', 'create_proxy_authorization',
                'execute_slow_response', 'final_proxy_transition'
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
    let has_more = request.cursor.is_some() && entries.len() as i64 > request.limit;
    if has_more {
        entries.truncate(request.limit as usize);
    }
    let next_cursor = has_more.then(|| entries.last().map(entry_cursor)).flatten();
    let total = if request.cursor.is_some() {
        None
    } else {
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
                        'place_bid', 'create_proxy_authorization',
                        'execute_slow_response', 'final_proxy_transition'
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

#[cfg(feature = "sqlite")]
async fn list_sqlite(
    store: &super::SqliteStore,
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
                'place_bid', 'create_proxy_authorization',
                'execute_slow_response', 'final_proxy_transition'
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
    let has_more = request.cursor.is_some() && entries.len() as i64 > request.limit;
    if has_more {
        entries.truncate(request.limit as usize);
    }
    let next_cursor = has_more.then(|| entries.last().map(entry_cursor)).flatten();
    let total = if request.cursor.is_some() {
        None
    } else {
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
                        'place_bid', 'create_proxy_authorization',
                        'execute_slow_response', 'final_proxy_transition'
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

fn cursor_values(request: &PublicBidLogListRequest) -> (DateTime<Utc>, Uuid, i64, i32, Uuid) {
    match request.cursor.as_ref() {
        Some(PublicBidLogCursor::Boundary {
            created_at,
            lot_id,
            accepted_state_version,
            fact_index,
            id,
        }) => (*created_at, *lot_id, *accepted_state_version, *fact_index, *id),
        Some(PublicBidLogCursor::Start) | None => (Utc::now(), Uuid::nil(), 0, 0, Uuid::nil()),
    }
}

fn entry_cursor(entry: &PublicBidLogEntry) -> PublicBidLogCursor {
    PublicBidLogCursor::Boundary {
        created_at: entry.created_at,
        lot_id: entry.lot_id,
        accepted_state_version: entry.accepted_state_version,
        fact_index: entry.fact_index,
        id: entry.id,
    }
}

fn validate_entries(
    entries: &[PublicBidLogEntry],
    request: &PublicBidLogListRequest,
) -> Result<(), ApplicationError> {
    for entry in entries {
        if entry.id.is_nil()
            || entry.auction_id != request.auction_id
            || entry.lot_id.is_nil()
            || request.lot_id.is_some_and(|lot_id| entry.lot_id != lot_id)
            || entry.lot_number <= 0
            || entry.amount <= 0
            || entry.accepted_state_version <= 0
            || entry.fact_index <= 0
            || entry.equipment_name.is_empty()
            || entry.equipment_category.is_empty()
            || entry.bidder_name.is_empty()
            || entry.mechanism.is_empty()
            || entry.created_at.timestamp_micros() == 0
        {
            return Err(storage_error("公开竞价日志包含无效条目"));
        }
    }
    Ok(())
}

#[cfg(feature = "postgres")]
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

#[cfg(feature = "sqlite")]
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

#[cfg(feature = "sqlite")]
fn uuid_blob(id: Uuid) -> Vec<u8> {
    id.as_bytes().to_vec()
}

#[cfg(feature = "sqlite")]
fn uuid_from_blob(bytes: Vec<u8>, label: &str) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&bytes).map_err(|_| storage_error(label))
}

#[cfg(feature = "sqlite")]
fn sqlite_timestamp_seconds(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%d %H:%M:%S").to_string()
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
