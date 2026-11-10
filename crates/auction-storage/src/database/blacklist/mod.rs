//! Public 黑名单查询适配器。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::public::{
    PublicBlacklistEntry, PublicBlacklistPage, PublicBlacklistReader, PublicBlacklistRequest,
};
use sqlx::Row;

use super::DatabaseStore;
use crate::database::storage_error;

impl PublicBlacklistReader for DatabaseStore {
    fn list_public_blacklist(
        &self,
        request: PublicBlacklistRequest,
    ) -> ApplicationFuture<'_, PublicBlacklistPage> {
        Box::pin(async move {
            request.validate().map_err(storage_error)?;
            list_postgres(self, request).await
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    store: &super::DatabaseStore,
    request: PublicBlacklistRequest,
) -> Result<PublicBlacklistPage, ApplicationError> {
    // 页数据与总数共享只读快照，避免并发封禁/解封使同一响应自相矛盾。
    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "开启公开黑名单快照失败"))?;
    let rows = sqlx::query(
        "SELECT u.username, b.public_reason, b.created_at AS blacklisted_at
         FROM public.user_blacklist AS b
         JOIN public.users AS u ON u.id = b.user_id
         WHERE b.revoked_at IS NULL
         ORDER BY b.created_at DESC, b.user_id
         LIMIT $1 OFFSET $2",
    )
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL 公开黑名单失败"))?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM public.user_blacklist AS b
         JOIN public.users AS u ON u.id = b.user_id
         WHERE b.revoked_at IS NULL",
    )
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "统计 PostgreSQL 公开黑名单失败"))?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交公开黑名单快照失败"))?;

    let entries = rows
        .into_iter()
        .map(|row| {
            Ok(PublicBlacklistEntry {
                username: row
                    .try_get("username")
                    .map_err(|error| storage_error_from(error, "读取黑名单玩家名称失败"))?,
                public_reason: row
                    .try_get("public_reason")
                    .map_err(|error| storage_error_from(error, "读取黑名单公开原因失败"))?,
                blacklisted_at: row
                    .try_get("blacklisted_at")
                    .map_err(|error| storage_error_from(error, "读取黑名单时间失败"))?,
            })
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;
    Ok(PublicBlacklistPage { entries, total, limit: request.limit, offset: request.offset })
}
