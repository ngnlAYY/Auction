//! 读取追加式 user_renames 事实，输出安全投影，不修改历史改名证据。

use auction_application::{
    AdminRenameListRequest, AdminRenamePage, AdminRenameReader, AdminRenameRecord,
    ApplicationError, ApplicationFuture,
};

use super::DatabaseStore;
use crate::database::storage_error;

impl AdminRenameReader for DatabaseStore {
    fn list(&self, request: AdminRenameListRequest) -> ApplicationFuture<'_, AdminRenamePage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(storage_error("Admin 改名历史查询参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store, request).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    store: &super::PostgresStore,
    request: AdminRenameListRequest,
) -> Result<AdminRenamePage, ApplicationError> {
    use chrono::{DateTime, Utc};
    use sqlx::Row;

    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|_| storage_error("开启 Admin 改名历史读取事务失败"))?;

    let name = request.name.as_deref().filter(|value| !value.is_empty());
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM public.user_renames
         WHERE ($1::uuid IS NULL OR user_id = $1)
           AND ($2::text IS NULL
                OR strpos(lower(previous_username), lower($2)) > 0
                OR strpos(lower(next_username), lower($2)) > 0)",
    )
    .bind(request.user_id)
    .bind(name)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| storage_error("统计 Admin 改名历史失败"))?;

    let rows = sqlx::query(
        "SELECT rename.id, rename.user_id, usr.state_version AS user_state_version,
                rename.previous_username, rename.next_username, rename.actor,
                rename.created_at, rename.reverted_at
         FROM public.user_renames AS rename
         JOIN public.users AS usr ON usr.id = rename.user_id
         WHERE ($1::uuid IS NULL OR rename.user_id = $1)
           AND ($2::text IS NULL
                OR strpos(lower(rename.previous_username), lower($2)) > 0
                OR strpos(lower(rename.next_username), lower($2)) > 0)
         ORDER BY rename.created_at DESC, rename.id DESC
         LIMIT $3 OFFSET $4",
    )
    .bind(request.user_id)
    .bind(name)
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 Admin 改名历史失败"))?;

    let entries = rows
        .into_iter()
        .map(|row| {
            let record = AdminRenameRecord {
                id: row.try_get("id").map_err(|_| storage_error("读取改名记录标识失败"))?,
                user_id: row
                    .try_get("user_id")
                    .map_err(|_| storage_error("读取改名用户标识失败"))?,
                user_state_version: row
                    .try_get("user_state_version")
                    .map_err(|_| storage_error("读取改名用户版本失败"))?,
                previous_username: row
                    .try_get("previous_username")
                    .map_err(|_| storage_error("读取改名前用户名失败"))?,
                next_username: row
                    .try_get("next_username")
                    .map_err(|_| storage_error("读取改名后用户名失败"))?,
                actor: row.try_get("actor").map_err(|_| storage_error("读取改名 actor 失败"))?,
                created_at: row
                    .try_get::<DateTime<Utc>, _>("created_at")
                    .map_err(|_| storage_error("读取改名时间失败"))?,
                reverted_at: row
                    .try_get::<Option<DateTime<Utc>>, _>("reverted_at")
                    .map_err(|_| storage_error("读取改名回滚时间失败"))?,
            };
            if record.validate() {
                Ok(record)
            } else {
                Err(storage_error("Admin 改名历史安全投影无效"))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    transaction.commit().await.map_err(|_| storage_error("提交 Admin 改名历史读取事务失败"))?;
    let page = AdminRenamePage { entries, total, limit: request.limit, offset: request.offset };
    if page.validate(&request) {
        Ok(page)
    } else {
        Err(storage_error("Admin 改名历史分页结果无效"))
    }
}
