//! Admin 代理历史读取适配器。
//!
//! PostgreSQL 完整档把命令账本与追加式代理转换事实关联，兼容旧短命令名与 Auction-BF
//! authorization 命令名，同时避免直接暴露两代不同的账本 payload。SQLite Lite 不声明这项
//! 完整管理查询能力，因此不会通过猜测字段或拼装空数据伪造成功结果。

use super::DatabaseStore;
#[cfg(feature = "postgres")]
use auction_application::{
    AdminProxyEvent, AdminProxyEventCursor, UNKNOWN_ADMIN_PROXY_EVENT_TOTAL,
};
use auction_application::{
    AdminProxyEventListRequest, AdminProxyEventPage, AdminProxyEventReader, ApplicationError,
    ApplicationFuture,
};

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
    use chrono::{DateTime, Utc};
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
        "SELECT
            command.command_id,
            proxy_authorization.authorization_id,
            proxy_authorization.authorization_sequence,
            proxy_authorization.usage_number,
            proxy_authorization.lot_id,
            proxy_authorization.user_id,
            users.username,
            transition.new_mode AS mode,
            transition.new_maximum_amount AS maximum_amount,
            transition.new_status AS status,
            transition.resulting_state_version AS state_version,
            command.created_at
         FROM public.auction_commands AS command
         JOIN public.bid_proxy_authorization_transitions AS transition
           ON transition.origin_command_id = command.command_id
          AND (
              (command.command_kind IN ('create_proxy', 'create_proxy_authorization')
                  AND transition.transition_kind = 'created')
              OR (command.command_kind IN ('lower_proxy', 'lower_proxy_authorization')
                  AND transition.transition_kind = 'lowered')
              OR (command.command_kind = 'freeze_proxy_authorization'
                  AND transition.transition_kind = 'frozen')
          )
         JOIN public.bid_proxy_authorizations AS proxy_authorization
           ON proxy_authorization.authorization_id = transition.authorization_id
          AND proxy_authorization.lot_id = command.aggregate_id
         JOIN public.users AS users ON users.id = proxy_authorization.user_id
         WHERE command.aggregate_id = $1
           AND (
              (command.command_kind IN ('create_proxy', 'lower_proxy')
                  AND command.result_code = 'accepted')
              OR (command.command_kind = 'freeze_proxy_authorization'
                  AND command.result_code = 'accepted')
              OR (command.command_kind IN (
                      'create_proxy_authorization', 'lower_proxy_authorization'
                  ) AND command.result_code = 'OK')
           )
           AND (
               $4::timestamptz IS NULL
               OR command.created_at < $4
               OR (command.created_at = $4 AND command.command_id < $5::uuid)
           )
         ORDER BY command.created_at DESC, command.command_id DESC
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

    for (index, row) in rows.into_iter().enumerate() {
        if index >= request.limit as usize {
            break;
        }

        let command_id: Uuid = row
            .try_get("command_id")
            .map_err(|_| storage_error("读取 Admin 代理历史命令标识失败"))?;
        let created_at: DateTime<Utc> = row
            .try_get("created_at")
            .map_err(|_| storage_error("读取 Admin 代理历史创建时间失败"))?;
        let event = AdminProxyEvent {
            command_id,
            authorization_id: row
                .try_get("authorization_id")
                .map_err(|_| storage_error("读取 Admin 代理历史授权标识失败"))?,
            authorization_sequence: row
                .try_get("authorization_sequence")
                .map_err(|_| storage_error("读取 Admin 代理历史授权序号失败"))?,
            usage_number: row
                .try_get("usage_number")
                .map_err(|_| storage_error("读取 Admin 代理历史使用序号失败"))?,
            lot_id: row
                .try_get::<Uuid, _>("lot_id")
                .map(auction_domain::LotId::from_uuid)
                .map_err(|_| storage_error("读取 Admin 代理历史 lot 标识失败"))?,
            user_id: row
                .try_get("user_id")
                .map_err(|_| storage_error("读取 Admin 代理历史用户标识失败"))?,
            username: row
                .try_get("username")
                .map_err(|_| storage_error("读取 Admin 代理历史用户名失败"))?,
            mode: row.try_get("mode").map_err(|_| storage_error("读取 Admin 代理历史模式失败"))?,
            maximum_amount: row
                .try_get("maximum_amount")
                .map_err(|_| storage_error("读取 Admin 代理历史上限失败"))?,
            status: row
                .try_get("status")
                .map_err(|_| storage_error("读取 Admin 代理历史状态失败"))?,
            state_version: row
                .try_get("state_version")
                .map_err(|_| storage_error("读取 Admin 代理历史状态版本失败"))?,
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
             FROM public.auction_commands AS command
             JOIN public.bid_proxy_authorization_transitions AS transition
               ON transition.origin_command_id = command.command_id
              AND (
                  (command.command_kind IN ('create_proxy', 'create_proxy_authorization')
                      AND transition.transition_kind = 'created')
                  OR (command.command_kind IN ('lower_proxy', 'lower_proxy_authorization')
                      AND transition.transition_kind = 'lowered')
                  OR (command.command_kind = 'freeze_proxy_authorization'
                      AND transition.transition_kind = 'frozen')
              )
             JOIN public.bid_proxy_authorizations AS proxy_authorization
               ON proxy_authorization.authorization_id = transition.authorization_id
              AND proxy_authorization.lot_id = command.aggregate_id
             WHERE command.aggregate_id = $1
               AND (
                  (command.command_kind IN ('create_proxy', 'lower_proxy')
                      AND command.result_code = 'accepted')
                  OR (command.command_kind = 'freeze_proxy_authorization'
                      AND command.result_code = 'accepted')
                  OR (command.command_kind IN (
                          'create_proxy_authorization', 'lower_proxy_authorization'
                      ) AND command.result_code = 'OK')
               )",
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

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(auction_application::RepositoryError::Failed(message.to_owned()))
}
