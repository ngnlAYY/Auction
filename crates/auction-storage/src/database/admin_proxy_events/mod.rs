//! Admin 代理历史读取适配器。
//!
//! PostgreSQL 完整档从已提交的命令账本恢复代理计划，再把领域快照映射为安全的 Admin
//! projection。SQLite Lite 不声明这项完整管理查询能力，因此不会通过猜测字段或拼装空数据
//! 伪造成功结果。

#[cfg(feature = "postgres")]
use auction_application::{
    AdminProxyEvent, AdminProxyEventCursor, UNKNOWN_ADMIN_PROXY_EVENT_TOTAL,
};
use auction_application::{
    AdminProxyEventListRequest, AdminProxyEventPage, AdminProxyEventReader, ApplicationError,
    ApplicationFuture,
};
#[cfg(feature = "postgres")]
use auction_domain::{CompetitionPlan, ProxyAuthorization, ProxyMode, ProxyStatus};

use super::DatabaseStore;

impl AdminProxyEventReader for DatabaseStore {
    fn list_admin_proxy_events(
        &self,
        request: AdminProxyEventListRequest,
    ) -> ApplicationFuture<'_, Option<AdminProxyEventPage>> {
        Box::pin(async move {
            if !request.validate() {
                return Err(storage_error("Admin 代理历史参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "Admin 代理历史仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    store: &super::PostgresStore,
    request: AdminProxyEventListRequest,
) -> Result<Option<AdminProxyEventPage>, ApplicationError> {
    use std::collections::HashMap;

    use chrono::{DateTime, Utc};
    use serde_json::Value;
    use sqlx::Row;
    use uuid::Uuid;

    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启 Admin 代理历史读取事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置 Admin 代理历史隔离级别失败"))?;

    let visible = sqlx::query("SELECT id FROM public.lots WHERE id = $1")
        .bind(request.lot_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| storage_error("校验 Admin 代理历史 lot 边界失败"))?
        .is_some();
    if !visible {
        transaction.commit().await.map_err(|_| storage_error("提交 Admin 代理历史空结果失败"))?;
        return Ok(None);
    }

    let cursor_boundary = match request.cursor {
        Some(AdminProxyEventCursor::Boundary { created_at, command_id }) => {
            Some((created_at, command_id))
        }
        Some(AdminProxyEventCursor::Start) | None => None,
    };
    let query_limit =
        request.limit.checked_add(1).ok_or_else(|| storage_error("Admin 代理历史页大小溢出"))?;
    let rows = sqlx::query(
        "SELECT command_id, command_kind, result_payload, created_at
         FROM public.auction_commands
         WHERE aggregate_id = $1
           AND command_kind IN ('create_proxy', 'lower_proxy')
           AND result_code = 'accepted'
           AND (
               $4::timestamptz IS NULL
               OR created_at < $4
               OR (created_at = $4 AND command_id < $5::uuid)
           )
         ORDER BY created_at DESC, command_id DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(request.lot_id.as_uuid())
    .bind(query_limit)
    .bind(request.offset)
    .bind(cursor_boundary.map(|(created_at, _)| created_at))
    .bind(cursor_boundary.map(|(_, command_id)| command_id))
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 Admin 代理历史命令账本失败"))?;

    let has_more = rows.len() > request.limit as usize;
    let mut entries = Vec::with_capacity(request.limit as usize);
    let mut user_ids = Vec::with_capacity(rows.len());
    let mut pending: Vec<(Uuid, ProxyAuthorization, DateTime<Utc>)> =
        Vec::with_capacity(rows.len());

    for (index, row) in rows.into_iter().enumerate() {
        if index >= request.limit as usize {
            break;
        }

        let command_id: Uuid = row
            .try_get("command_id")
            .map_err(|_| storage_error("读取 Admin 代理历史命令标识失败"))?;
        let command_kind: String = row
            .try_get("command_kind")
            .map_err(|_| storage_error("读取 Admin 代理历史命令类型失败"))?;
        let payload: Value = row
            .try_get("result_payload")
            .map_err(|_| storage_error("读取 Admin 代理历史结果负载失败"))?;
        let created_at: DateTime<Utc> = row
            .try_get("created_at")
            .map_err(|_| storage_error("读取 Admin 代理历史创建时间失败"))?;
        let plan: CompetitionPlan = serde_json::from_value(payload)
            .map_err(|_| storage_error("Admin 代理历史结果负载格式无效"))?;
        plan.validate().map_err(|_| storage_error("Admin 代理历史结果违反领域不变量"))?;
        if plan.final_lot.lot_id != request.lot_id {
            return Err(storage_error("Admin 代理历史结果 lot 归属不一致"));
        }

        let state = *state_for_command(&plan, &command_kind, command_id)
            .ok_or_else(|| storage_error("Admin 代理历史缺少对应代理快照"))?;
        let user_id = state.user_id.as_uuid();
        user_ids.push(user_id);
        pending.push((command_id, state, created_at));
    }

    let mut usernames = HashMap::with_capacity(user_ids.len());
    if !user_ids.is_empty() {
        let user_rows = sqlx::query(
            "SELECT id, username
             FROM public.users
             WHERE id = ANY($1::uuid[])",
        )
        .bind(&user_ids)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| storage_error("读取 Admin 代理历史用户投影失败"))?;
        for row in user_rows {
            let user_id: Uuid =
                row.try_get("id").map_err(|_| storage_error("读取 Admin 代理历史用户标识失败"))?;
            let username: String = row
                .try_get("username")
                .map_err(|_| storage_error("读取 Admin 代理历史用户名失败"))?;
            usernames.insert(user_id, username);
        }
    }

    for (command_id, state, created_at) in pending {
        let user_id = state.user_id.as_uuid();
        let username =
            usernames.remove(&user_id).ok_or_else(|| storage_error("Admin 代理历史用户不存在"))?;
        let event = AdminProxyEvent {
            command_id,
            authorization_id: state.id.as_uuid(),
            authorization_sequence: i64_value(
                state.authorization_sequence,
                "Admin 代理历史授权序号溢出",
            )?,
            usage_number: i64_value(state.usage_number, "Admin 代理历史使用序号溢出")?,
            lot_id: state.lot_id,
            user_id,
            username,
            mode: proxy_mode(state.mode).to_owned(),
            maximum_amount: state.maximum_amount.value(),
            status: proxy_status(state.status).to_owned(),
            state_version: i64_value(state.state_version, "Admin 代理历史状态版本溢出")?,
            created_at,
        };
        if !event.validate(request.lot_id) {
            return Err(storage_error("Admin 代理历史安全投影无效"));
        }
        entries.push(event);
    }

    let total = if request.cursor.is_some() {
        UNKNOWN_ADMIN_PROXY_EVENT_TOTAL
    } else {
        sqlx::query_scalar(
            "SELECT count(*)
             FROM public.auction_commands
             WHERE aggregate_id = $1
               AND command_kind IN ('create_proxy', 'lower_proxy')
               AND result_code = 'accepted'",
        )
        .bind(request.lot_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| storage_error("读取 Admin 代理历史总数失败"))?
    };

    transaction.commit().await.map_err(|_| storage_error("提交 Admin 代理历史读取事务失败"))?;

    let next_cursor = has_more
        .then(|| {
            entries.last().map(|entry| AdminProxyEventCursor::Boundary {
                created_at: entry.created_at,
                command_id: entry.command_id,
            })
        })
        .flatten();
    let page = AdminProxyEventPage {
        lot_id: request.lot_id,
        entries,
        total,
        limit: request.limit,
        offset: request.offset,
        cursor: request.cursor,
        next_cursor,
        has_more,
    };
    if !page.validate(request) {
        return Err(storage_error("Admin 代理历史存储返回不变量失败"));
    }
    Ok(Some(page))
}

#[cfg(feature = "postgres")]
fn state_for_command<'a>(
    plan: &'a CompetitionPlan,
    command_kind: &str,
    command_id: uuid::Uuid,
) -> Option<&'a ProxyAuthorization> {
    match command_kind {
        "create_proxy" => {
            plan.proxy_states.iter().find(|state| state.creation_command_id.as_uuid() == command_id)
        }
        "lower_proxy" => {
            let authorization_id = plan
                .proxy_transitions
                .iter()
                .find(|transition| transition.command_id.as_uuid() == command_id)
                .map(|transition| transition.authorization_id)?;
            plan.proxy_states.iter().find(|state| state.id == authorization_id)
        }
        _ => None,
    }
}

#[cfg(feature = "postgres")]
fn proxy_mode(mode: ProxyMode) -> &'static str {
    match mode {
        ProxyMode::Fast => "fast",
        ProxyMode::Slow => "slow",
    }
}

#[cfg(feature = "postgres")]
fn proxy_status(status: ProxyStatus) -> &'static str {
    match status {
        ProxyStatus::Active => "active",
        ProxyStatus::FinalFast => "final_fast",
        ProxyStatus::Exhausted => "exhausted",
        ProxyStatus::Frozen => "frozen",
        ProxyStatus::Closed => "closed",
    }
}

#[cfg(feature = "postgres")]
fn i64_value(value: u64, message: &str) -> Result<i64, ApplicationError> {
    value.try_into().map_err(|_| storage_error(message))
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(auction_application::RepositoryError::Failed(message.to_owned()))
}
