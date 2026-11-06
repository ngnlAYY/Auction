//! Public 论坛镜像读取适配器；只读 `auction_forum_sync` 本地投影与心跳中的非敏感启用状态。
//! 远端发布和网关由独立 owner 管理，读取路径不得把待发布或未知状态伪造成成功或关闭。

mod catalog;
mod reply;
mod sync;

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::forum::{PublicForumReader, PublicForumThread};
use uuid::Uuid;

use super::DatabaseStore;
use crate::database::storage_error;

impl PublicForumReader for DatabaseStore {
    fn public_forum_sync_enabled(&self) -> ApplicationFuture<'_, Option<bool>> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => forum_sync_enabled_postgres(store).await,
            }
        })
    }

    fn public_forum_thread(
        &self,
        auction_id: Uuid,
    ) -> ApplicationFuture<'_, Option<PublicForumThread>> {
        Box::pin(async move {
            if auction_id.is_nil() {
                return Err(storage_error("公开论坛拍卖标识无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => forum_postgres(store, auction_id).await,
            }
        })
    }
}

/// 任一有效 Worker 明确启用即可返回 true；只有全部明确关闭才返回 false。
/// 旧版、缺失或损坏的布尔值不能成为关闭证据，过期和正在停止的实例不参与判断。
#[cfg(feature = "postgres")]
async fn forum_sync_enabled_postgres(
    store: &super::PostgresStore,
) -> Result<Option<bool>, ApplicationError> {
    sqlx::query_scalar(
        "SELECT CASE
             WHEN bool_or(details -> 'forum_sync_enabled' = 'true'::jsonb) THEN true
             WHEN bool_and(COALESCE(details -> 'forum_sync_enabled' = 'false'::jsonb, false))
                 THEN false
             ELSE NULL
         END
         FROM public.service_heartbeats
         WHERE service_name = 'auction-worker' AND process_role = 'worker'
           AND status IN ('starting', 'ready', 'degraded')
           AND expires_at > clock_timestamp()",
    )
    .fetch_one(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "读取公开论坛同步启用状态失败"))
}

#[cfg(feature = "postgres")]
async fn forum_postgres(
    store: &super::PostgresStore,
    auction_id: Uuid,
) -> Result<Option<PublicForumThread>, ApplicationError> {
    use chrono::{DateTime, Utc};
    use sqlx::Row;

    let row = sqlx::query(
        "SELECT auction_id, status, source_revision, thread_url, title, body,
                content_revision, processed_through_post_number, content_synced_at,
                content_last_success_at,
                content_last_error_at, content_last_error_code, updated_at
         FROM public.auction_forum_sync
         WHERE auction_id = $1",
    )
    .bind(auction_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "读取公开论坛镜像失败"))?;
    let Some(row) = row else {
        return Ok(None);
    };

    let thread = PublicForumThread {
        auction_id: row
            .try_get("auction_id")
            .map_err(|error| storage_error_from(error, "读取公开论坛拍卖标识失败"))?,
        status: row
            .try_get("status")
            .map_err(|error| storage_error_from(error, "读取公开论坛同步状态失败"))?,
        source_revision: row
            .try_get("source_revision")
            .map_err(|error| storage_error_from(error, "读取公开论坛源版本失败"))?,
        thread_url: row
            .try_get("thread_url")
            .map_err(|error| storage_error_from(error, "读取公开论坛链接失败"))?,
        title: row
            .try_get("title")
            .map_err(|error| storage_error_from(error, "读取公开论坛标题失败"))?,
        body: row
            .try_get("body")
            .map_err(|error| storage_error_from(error, "读取公开论坛正文失败"))?,
        content_revision: row
            .try_get("content_revision")
            .map_err(|error| storage_error_from(error, "读取公开论坛内容版本失败"))?,
        processed_through_post_number: row
            .try_get("processed_through_post_number")
            .map_err(|error| storage_error_from(error, "读取公开论坛处理楼层失败"))?,
        content_synced_at: row
            .try_get("content_synced_at")
            .map_err(|error| storage_error_from(error, "读取公开论坛内容同步时间失败"))?,
        content_last_success_at: row
            .try_get("content_last_success_at")
            .map_err(|error| storage_error_from(error, "读取公开论坛最近成功时间失败"))?,
        content_last_error_at: row
            .try_get("content_last_error_at")
            .map_err(|error| storage_error_from(error, "读取公开论坛最近错误时间失败"))?,
        content_last_error_code: row
            .try_get("content_last_error_code")
            .map_err(|error| storage_error_from(error, "读取公开论坛最近错误码失败"))?,
        updated_at: row
            .try_get::<DateTime<Utc>, _>("updated_at")
            .map_err(|error| storage_error_from(error, "读取公开论坛更新时间失败"))?,
    };
    if thread.auction_id != auction_id {
        return Err(storage_error("公开论坛拍卖标识与请求不一致"));
    }
    thread.validate().map_err(storage_error)?;
    Ok(Some(thread))
}
