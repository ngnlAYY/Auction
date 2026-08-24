//! SQLite Lite Admin 审计查询。

use super::mapping::{map_sqlite_record, sqlite_timestamp};
use super::*;
use auction_application::AdminAuditCursor;

pub(super) async fn list_sqlite(
    store: &super::super::SqliteStore,
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
