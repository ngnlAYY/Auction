//! Admin 审计读取适配器。
//!
//! 两个后端都只读追加式 `audit_records`，不向管理端暴露数据库内部行。数字页的总数与记录
//! 在同一读取事务中取得；keyset 页只返回稳定的时间/ID 下一页边界。

use auction_application::{
    AdminAuditCursor, AdminAuditListRequest, AdminAuditPage, AdminAuditPort, AdminAuditRecord,
    ApplicationError, ApplicationFuture,
};

use super::DatabaseStore;

impl AdminAuditPort for DatabaseStore {
    fn list(&self, request: AdminAuditListRequest) -> ApplicationFuture<'_, AdminAuditPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(ApplicationError::AdminAudit(
                    auction_application::AdminAuditError::InvalidRequest,
                ));
            }
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
    request: AdminAuditListRequest,
) -> Result<AdminAuditPage, ApplicationError> {
    use chrono::{DateTime, Utc};
    use serde_json::Value;
    use sqlx::{Row, types::Json};

    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启 Admin 审计读取事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置 Admin 审计读取隔离级别失败"))?;

    let offset = request.offset();
    let actor = request.actor.as_deref().filter(|value| !value.is_empty());
    let actor_type = request.actor_type.as_deref().filter(|value| !value.is_empty());
    let action = request.action.as_deref().filter(|value| !value.is_empty());
    let entity = request.entity.as_deref().filter(|value| !value.is_empty());
    let source = request.source.as_deref().filter(|value| !value.is_empty());
    let result = request.result.as_deref().filter(|value| !value.is_empty());
    let request_id = request.request_id.as_deref().filter(|value| !value.is_empty());
    let (rows, total, has_more) = if let Some(offset) = offset {
        let total: i64 = sqlx::query_scalar(
            "SELECT count(*)
             FROM public.audit_records
             WHERE ($1::text IS NULL OR actor_id = $1)
               AND ($2::text IS NULL OR actor_type = $2)
               AND ($3::text IS NULL OR action = $3)
               AND ($4::text IS NULL OR entity_type || ':' || entity_id = $4)
               AND ($5::text IS NULL OR source = $5)
               AND ($6::text IS NULL OR result = $6)
               AND ($7::text IS NULL OR request_id = $7)
               AND ($8::timestamptz IS NULL OR occurred_at >= $8)
               AND ($9::timestamptz IS NULL OR occurred_at <= $9)",
        )
        .bind(actor)
        .bind(actor_type)
        .bind(action)
        .bind(entity)
        .bind(source)
        .bind(result)
        .bind(request_id)
        .bind(request.occurred_from)
        .bind(request.occurred_to)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| storage_error("统计 Admin 审计记录失败"))?;
        let rows = sqlx::query(
            "SELECT id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                    result, request_id, source, metadata
             FROM public.audit_records
             WHERE ($1::text IS NULL OR actor_id = $1)
               AND ($2::text IS NULL OR actor_type = $2)
               AND ($3::text IS NULL OR action = $3)
               AND ($4::text IS NULL OR entity_type || ':' || entity_id = $4)
               AND ($5::text IS NULL OR source = $5)
               AND ($6::text IS NULL OR result = $6)
               AND ($7::text IS NULL OR request_id = $7)
               AND ($8::timestamptz IS NULL OR occurred_at >= $8)
               AND ($9::timestamptz IS NULL OR occurred_at <= $9)
             ORDER BY occurred_at DESC, id DESC
             LIMIT $10 OFFSET $11",
        )
        .bind(actor)
        .bind(actor_type)
        .bind(action)
        .bind(entity)
        .bind(source)
        .bind(result)
        .bind(request_id)
        .bind(request.occurred_from)
        .bind(request.occurred_to)
        .bind(request.page_size)
        .bind(offset)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| storage_error("读取 Admin 审计数字页失败"))?;
        (rows, Some(total), false)
    } else {
        let query_limit = request
            .page_size
            .checked_add(1)
            .ok_or_else(|| storage_error("Admin 审计 keyset 页大小溢出"))?;
        let cursor_time = request.cursor.map(|cursor| cursor.occurred_at);
        let cursor_id = request.cursor.map(|cursor| cursor.id);
        let rows = sqlx::query(
            "SELECT id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                    result, request_id, source, metadata
             FROM public.audit_records
             WHERE ($1::text IS NULL OR actor_id = $1)
               AND ($2::text IS NULL OR actor_type = $2)
               AND ($3::text IS NULL OR action = $3)
               AND ($4::text IS NULL OR entity_type || ':' || entity_id = $4)
               AND ($5::text IS NULL OR source = $5)
               AND ($6::text IS NULL OR result = $6)
               AND ($7::text IS NULL OR request_id = $7)
               AND ($8::timestamptz IS NULL OR occurred_at >= $8)
               AND ($9::timestamptz IS NULL OR occurred_at <= $9)
               AND ($10::timestamptz IS NULL
                    OR occurred_at < $10
                    OR (occurred_at = $10 AND id < $11::uuid))
             ORDER BY occurred_at DESC, id DESC
             LIMIT $12",
        )
        .bind(actor)
        .bind(actor_type)
        .bind(action)
        .bind(entity)
        .bind(source)
        .bind(result)
        .bind(request_id)
        .bind(request.occurred_from)
        .bind(request.occurred_to)
        .bind(cursor_time)
        .bind(cursor_id)
        .bind(query_limit)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| storage_error("读取 Admin 审计 keyset 页面失败"))?;
        let has_more = rows.len() > request.page_size as usize;
        (rows, None, has_more)
    };

    let entries = rows
        .into_iter()
        .take(request.page_size as usize)
        .map(|row| {
            let metadata: Json<Value> = row
                .try_get("metadata")
                .map_err(|_| storage_error("读取 Admin 审计 metadata 失败"))?;
            let record = AdminAuditRecord {
                id: row.try_get("id").map_err(|_| storage_error("读取 Admin 审计标识失败"))?,
                occurred_at: row
                    .try_get::<DateTime<Utc>, _>("occurred_at")
                    .map_err(|_| storage_error("读取 Admin 审计时间失败"))?,
                actor_type: row
                    .try_get("actor_type")
                    .map_err(|_| storage_error("读取 Admin 审计 actor 类型失败"))?,
                actor_id: row
                    .try_get("actor_id")
                    .map_err(|_| storage_error("读取 Admin 审计 actor 标识失败"))?,
                action: row
                    .try_get("action")
                    .map_err(|_| storage_error("读取 Admin 审计动作失败"))?,
                entity_type: row
                    .try_get("entity_type")
                    .map_err(|_| storage_error("读取 Admin 审计实体类型失败"))?,
                entity_id: row
                    .try_get("entity_id")
                    .map_err(|_| storage_error("读取 Admin 审计实体标识失败"))?,
                result: row
                    .try_get("result")
                    .map_err(|_| storage_error("读取 Admin 审计结果失败"))?,
                request_id: row
                    .try_get("request_id")
                    .map_err(|_| storage_error("读取 Admin 审计请求标识失败"))?,
                source: row
                    .try_get("source")
                    .map_err(|_| storage_error("读取 Admin 审计来源失败"))?,
                metadata: metadata.0,
            };
            if record.validate() {
                Ok(record)
            } else {
                Err(storage_error("Admin 审计安全投影无效"))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    transaction.commit().await.map_err(|_| storage_error("提交 Admin 审计读取事务失败"))?;
    let next_cursor = has_more
        .then(|| {
            entries
                .last()
                .map(|entry| AdminAuditCursor { occurred_at: entry.occurred_at, id: entry.id })
        })
        .flatten();
    let page = AdminAuditPage {
        entries,
        page: request.page,
        page_size: request.page_size,
        total,
        next_cursor,
        has_more,
    };
    if page.validate(&request) {
        Ok(page)
    } else {
        Err(storage_error("Admin 审计分页结果无效"))
    }
}

#[cfg(feature = "sqlite")]
async fn list_sqlite(
    store: &super::SqliteStore,
    request: AdminAuditListRequest,
) -> Result<AdminAuditPage, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|_| storage_error("开启 SQLite Admin 审计读取事务失败"))?;
    let actor = request.actor.as_deref().filter(|value| !value.is_empty());
    let actor_type = request.actor_type.as_deref().filter(|value| !value.is_empty());
    let action = request.action.as_deref().filter(|value| !value.is_empty());
    let entity = request.entity.as_deref().filter(|value| !value.is_empty());
    let source = request.source.as_deref().filter(|value| !value.is_empty());
    let result = request.result.as_deref().filter(|value| !value.is_empty());
    let request_id = request.request_id.as_deref().filter(|value| !value.is_empty());
    let occurred_from = request.occurred_from.map(sqlite_timestamp);
    let occurred_to = request.occurred_to.map(sqlite_timestamp);
    let offset = request.offset();

    macro_rules! bind_filters {
        ($query:expr) => {
            $query
                .bind(actor)
                .bind(actor)
                .bind(actor_type)
                .bind(actor_type)
                .bind(action)
                .bind(action)
                .bind(entity)
                .bind(entity)
                .bind(source)
                .bind(source)
                .bind(result)
                .bind(result)
                .bind(request_id)
                .bind(request_id)
                .bind(occurred_from.as_deref())
                .bind(occurred_from.as_deref())
                .bind(occurred_to.as_deref())
                .bind(occurred_to.as_deref())
        };
    }

    let (rows, total, has_more) = if let Some(offset) = offset {
        let total: i64 = bind_filters!(sqlx::query_scalar(
            "SELECT count(*)
             FROM audit_records
             WHERE (? IS NULL OR actor_id = ?)
               AND (? IS NULL OR actor_type = ?)
               AND (? IS NULL OR action = ?)
               AND (? IS NULL OR entity_type || ':' || entity_id = ?)
               AND (? IS NULL OR source = ?)
               AND (? IS NULL OR result = ?)
               AND (? IS NULL OR request_id = ?)
               AND (? IS NULL OR datetime(occurred_at) >= datetime(?))
               AND (? IS NULL OR datetime(occurred_at) <= datetime(?))"
        ))
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| storage_error("统计 SQLite Admin 审计记录失败"))?;
        let rows = bind_filters!(sqlx::query(
            "SELECT id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                    result, request_id, source, metadata
             FROM audit_records
             WHERE (? IS NULL OR actor_id = ?)
               AND (? IS NULL OR actor_type = ?)
               AND (? IS NULL OR action = ?)
               AND (? IS NULL OR entity_type || ':' || entity_id = ?)
               AND (? IS NULL OR source = ?)
               AND (? IS NULL OR result = ?)
               AND (? IS NULL OR request_id = ?)
               AND (? IS NULL OR datetime(occurred_at) >= datetime(?))
               AND (? IS NULL OR datetime(occurred_at) <= datetime(?))
             ORDER BY datetime(occurred_at) DESC, id DESC LIMIT ? OFFSET ?"
        ))
        .bind(request.page_size)
        .bind(offset)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| storage_error("读取 SQLite Admin 审计数字页失败"))?;
        (rows, Some(total), false)
    } else {
        let cursor_time = request.cursor.map(|cursor| sqlite_timestamp(cursor.occurred_at));
        let cursor_id = request.cursor.map(|cursor| cursor.id.as_bytes().to_vec());
        let rows = bind_filters!(sqlx::query(
            "SELECT id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                    result, request_id, source, metadata
             FROM audit_records
             WHERE (? IS NULL OR actor_id = ?)
               AND (? IS NULL OR actor_type = ?)
               AND (? IS NULL OR action = ?)
               AND (? IS NULL OR entity_type || ':' || entity_id = ?)
               AND (? IS NULL OR source = ?)
               AND (? IS NULL OR result = ?)
               AND (? IS NULL OR request_id = ?)
               AND (? IS NULL OR datetime(occurred_at) >= datetime(?))
               AND (? IS NULL OR datetime(occurred_at) <= datetime(?))
               AND (? IS NULL OR datetime(occurred_at) < datetime(?)
                    OR (datetime(occurred_at) = datetime(?) AND id < ?))
             ORDER BY datetime(occurred_at) DESC, id DESC LIMIT ?"
        ))
        .bind(cursor_time.as_deref())
        .bind(cursor_time.as_deref())
        .bind(cursor_time.as_deref())
        .bind(cursor_id.as_deref())
        .bind(
            request
                .page_size
                .checked_add(1)
                .ok_or_else(|| storage_error("SQLite Admin 审计页大小溢出"))?,
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| storage_error("读取 SQLite Admin 审计 keyset 页面失败"))?;
        let has_more = rows.len() > request.page_size as usize;
        (rows, None, has_more)
    };

    let entries = rows
        .into_iter()
        .take(request.page_size as usize)
        .map(map_sqlite_record)
        .collect::<Result<Vec<_>, _>>()?;
    transaction.commit().await.map_err(|_| storage_error("提交 SQLite Admin 审计读取事务失败"))?;
    let next_cursor = has_more
        .then(|| {
            entries
                .last()
                .map(|entry| AdminAuditCursor { occurred_at: entry.occurred_at, id: entry.id })
        })
        .flatten();
    let page = AdminAuditPage {
        entries,
        page: request.page,
        page_size: request.page_size,
        total,
        next_cursor,
        has_more,
    };
    if page.validate(&request) {
        Ok(page)
    } else {
        Err(storage_error("SQLite Admin 审计分页结果无效"))
    }
}

#[cfg(feature = "sqlite")]
fn map_sqlite_record(row: sqlx::sqlite::SqliteRow) -> Result<AdminAuditRecord, ApplicationError> {
    use serde_json::Value;
    use sqlx::Row;
    use uuid::Uuid;

    let id: Vec<u8> =
        row.try_get("id").map_err(|_| storage_error("读取 SQLite Admin 审计标识失败"))?;
    let id = Uuid::from_slice(&id).map_err(|_| storage_error("SQLite Admin 审计标识格式无效"))?;
    let timestamp: String =
        row.try_get("occurred_at").map_err(|_| storage_error("读取 SQLite Admin 审计时间失败"))?;
    let occurred_at = parse_sqlite_timestamp(&timestamp)?;
    let metadata_text: String = row
        .try_get("metadata")
        .map_err(|_| storage_error("读取 SQLite Admin 审计 metadata 失败"))?;
    let metadata: Value = serde_json::from_str(&metadata_text)
        .map_err(|_| storage_error("SQLite Admin 审计 metadata 格式无效"))?;
    let record = AdminAuditRecord {
        id,
        occurred_at,
        actor_type: row
            .try_get("actor_type")
            .map_err(|_| storage_error("读取 SQLite Admin 审计 actor 类型失败"))?,
        actor_id: row
            .try_get("actor_id")
            .map_err(|_| storage_error("读取 SQLite Admin 审计 actor 标识失败"))?,
        action: row
            .try_get("action")
            .map_err(|_| storage_error("读取 SQLite Admin 审计动作失败"))?,
        entity_type: row
            .try_get("entity_type")
            .map_err(|_| storage_error("读取 SQLite Admin 审计实体类型失败"))?,
        entity_id: row
            .try_get("entity_id")
            .map_err(|_| storage_error("读取 SQLite Admin 审计实体标识失败"))?,
        result: row
            .try_get("result")
            .map_err(|_| storage_error("读取 SQLite Admin 审计结果失败"))?,
        request_id: row
            .try_get("request_id")
            .map_err(|_| storage_error("读取 SQLite Admin 审计请求标识失败"))?,
        source: row
            .try_get("source")
            .map_err(|_| storage_error("读取 SQLite Admin 审计来源失败"))?,
        metadata,
    };
    if record.validate() {
        Ok(record)
    } else {
        Err(storage_error("SQLite Admin 审计安全投影无效"))
    }
}

#[cfg(feature = "sqlite")]
fn sqlite_timestamp(value: chrono::DateTime<chrono::Utc>) -> String {
    value.naive_utc().format("%Y-%m-%d %H:%M:%S%.f").to_string()
}

#[cfg(feature = "sqlite")]
fn parse_sqlite_timestamp(value: &str) -> Result<chrono::DateTime<chrono::Utc>, ApplicationError> {
    use chrono::{DateTime, NaiveDateTime, Utc};
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value, format) {
            return Ok(DateTime::from_naive_utc_and_offset(parsed, Utc));
        }
    }
    Err(storage_error("SQLite Admin 审计时间格式无效"))
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(auction_application::RepositoryError::Failed(message.to_owned()))
}
