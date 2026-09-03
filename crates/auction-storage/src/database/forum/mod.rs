//! Public 论坛镜像读取适配器。
//!
//! 这里只读取 `auction_forum_sync` 的本地投影；论坛发布 Worker 和远程网关仍由独立 owner
//! 管理，读取路径不会把 pending/publishing 状态伪造成 published。

mod reply;
mod sync;

use auction_application::{
    ApplicationError, ApplicationFuture, PublicForumReader, PublicForumThread,
};
use uuid::Uuid;

use super::DatabaseStore;

impl PublicForumReader for DatabaseStore {
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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "公开论坛镜像仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
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
    .map_err(|_| storage_error("读取公开论坛镜像失败"))?;
    let Some(row) = row else {
        return Ok(None);
    };

    let thread = PublicForumThread {
        auction_id: row
            .try_get("auction_id")
            .map_err(|_| storage_error("读取公开论坛拍卖标识失败"))?,
        status: row.try_get("status").map_err(|_| storage_error("读取公开论坛同步状态失败"))?,
        source_revision: row
            .try_get("source_revision")
            .map_err(|_| storage_error("读取公开论坛源版本失败"))?,
        thread_url: row.try_get("thread_url").map_err(|_| storage_error("读取公开论坛链接失败"))?,
        title: row.try_get("title").map_err(|_| storage_error("读取公开论坛标题失败"))?,
        body: row.try_get("body").map_err(|_| storage_error("读取公开论坛正文失败"))?,
        content_revision: row
            .try_get("content_revision")
            .map_err(|_| storage_error("读取公开论坛内容版本失败"))?,
        processed_through_post_number: row
            .try_get("processed_through_post_number")
            .map_err(|_| storage_error("读取公开论坛处理楼层失败"))?,
        content_synced_at: row
            .try_get("content_synced_at")
            .map_err(|_| storage_error("读取公开论坛内容同步时间失败"))?,
        content_last_success_at: row
            .try_get("content_last_success_at")
            .map_err(|_| storage_error("读取公开论坛最近成功时间失败"))?,
        content_last_error_at: row
            .try_get("content_last_error_at")
            .map_err(|_| storage_error("读取公开论坛最近错误时间失败"))?,
        content_last_error_code: row
            .try_get("content_last_error_code")
            .map_err(|_| storage_error("读取公开论坛最近错误码失败"))?,
        updated_at: row
            .try_get::<DateTime<Utc>, _>("updated_at")
            .map_err(|_| storage_error("读取公开论坛更新时间失败"))?,
    };
    if thread.auction_id != auction_id {
        return Err(storage_error("公开论坛拍卖标识与请求不一致"));
    }
    thread.validate().map_err(storage_error)?;
    Ok(Some(thread))
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(auction_application::RepositoryError::Failed(message.to_owned()))
}
