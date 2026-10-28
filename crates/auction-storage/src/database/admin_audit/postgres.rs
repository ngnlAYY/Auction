//! PostgreSQL Admin 审计查询。

use super::*;
use auction_application::{AdminAuditCursor, AdminAuditRecord};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Row, types::Json};

pub(super) async fn list_postgres(
    store: &super::super::PostgresStore,
    request: AdminAuditListRequest,
) -> Result<AdminAuditPage, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|_| storage_error("开启 Admin 审计读取事务失败"))?;

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
