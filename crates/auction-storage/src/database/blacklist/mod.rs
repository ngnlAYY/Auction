//! Public 黑名单查询适配器。

use auction_application::{
    ApplicationError, ApplicationFuture, PublicBlacklistEntry, PublicBlacklistPage,
    PublicBlacklistReader, PublicBlacklistRequest,
};
#[cfg(feature = "sqlite")]
use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::Row;

use super::DatabaseStore;

impl PublicBlacklistReader for DatabaseStore {
    fn list_public_blacklist(
        &self,
        request: PublicBlacklistRequest,
    ) -> ApplicationFuture<'_, PublicBlacklistPage> {
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
    request: PublicBlacklistRequest,
) -> Result<PublicBlacklistPage, ApplicationError> {
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
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 公开黑名单失败"))?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM public.user_blacklist AS b
         JOIN public.users AS u ON u.id = b.user_id
         WHERE b.revoked_at IS NULL",
    )
    .fetch_one(store.pool())
    .await
    .map_err(|_| storage_error("统计 PostgreSQL 公开黑名单失败"))?;

    let entries = rows
        .into_iter()
        .map(|row| {
            Ok(PublicBlacklistEntry {
                username: row
                    .try_get("username")
                    .map_err(|_| storage_error("读取黑名单玩家名称失败"))?,
                public_reason: row
                    .try_get("public_reason")
                    .map_err(|_| storage_error("读取黑名单公开原因失败"))?,
                blacklisted_at: row
                    .try_get("blacklisted_at")
                    .map_err(|_| storage_error("读取黑名单时间失败"))?,
            })
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;
    Ok(PublicBlacklistPage { entries, total, limit: request.limit, offset: request.offset })
}

#[cfg(feature = "sqlite")]
async fn list_sqlite(
    store: &super::SqliteStore,
    request: PublicBlacklistRequest,
) -> Result<PublicBlacklistPage, ApplicationError> {
    let rows = sqlx::query(
        "SELECT u.username, b.public_reason, b.created_at AS blacklisted_at
         FROM user_blacklist AS b
         JOIN users AS u ON u.id = b.user_id
         WHERE b.revoked_at IS NULL
         ORDER BY b.created_at DESC, b.user_id
         LIMIT ? OFFSET ?",
    )
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error("读取 Lite 公开黑名单失败"))?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM user_blacklist AS b
         JOIN users AS u ON u.id = b.user_id
         WHERE b.revoked_at IS NULL",
    )
    .fetch_one(store.pool())
    .await
    .map_err(|_| storage_error("统计 Lite 公开黑名单失败"))?;

    let entries = rows
        .into_iter()
        .map(|row| {
            let blacklisted_at = parse_timestamp(
                row.try_get("blacklisted_at")
                    .map_err(|_| storage_error("读取 Lite 黑名单时间失败"))?,
            )?;
            Ok(PublicBlacklistEntry {
                username: row
                    .try_get("username")
                    .map_err(|_| storage_error("读取黑名单玩家名称失败"))?,
                public_reason: row
                    .try_get("public_reason")
                    .map_err(|_| storage_error("读取黑名单公开原因失败"))?,
                blacklisted_at,
            })
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;
    Ok(PublicBlacklistPage { entries, total, limit: request.limit, offset: request.offset })
}

#[cfg(feature = "sqlite")]
fn parse_timestamp(value: String) -> Result<DateTime<Utc>, ApplicationError> {
    if let Ok(value) = DateTime::parse_from_rfc3339(&value) {
        return Ok(value.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(&value, format) {
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(value, Utc));
        }
    }
    Err(storage_error("Lite 黑名单时间格式无效"))
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(auction_application::RepositoryError::Failed(message.to_owned()))
}
