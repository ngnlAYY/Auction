//! 固定 #1/#2 的本地正文证明更新。

use auction_application::{
    ApplicationError, ForumRenderedPost, ForumRenderedThread, ForumReplyTopic,
};
use chrono::{DateTime, Utc};

use super::super::super::PostgresStore;
use super::validation::storage_error;

pub(super) async fn record_fixed_posts_update_postgres(
    store: &PostgresStore,
    topic: &ForumReplyTopic,
    catalog: &ForumRenderedThread,
    system_log: &ForumRenderedPost,
    updated_at: DateTime<Utc>,
    next_scan_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let result = sqlx::query(
        "UPDATE public.auction_forum_sync
         SET title = $2,
             body = $3,
             system_log_body = $4,
             content_revision = content_revision + 1,
             content_synced_at = $5,
             content_next_scan_at = $6,
             content_lease_expires_at = NULL,
             content_claim_token = NULL,
             content_claim_owner = NULL,
             content_last_success_at = $5,
             content_last_error_at = NULL,
             content_last_error_code = NULL,
             updated_at = $5
         WHERE auction_id = $1
           AND status = 'published'
           AND thread_id = $7
           AND catalog_post_id = $8
           AND system_log_post_id = $9
           AND content_revision = $10
           AND content_claim_token = $11
           AND content_claim_owner = $12
           AND content_claim_fencing = $13
           AND content_lease_expires_at > $5",
    )
    .bind(topic.auction_id.as_uuid())
    .bind(&catalog.title)
    .bind(&catalog.body)
    .bind(&system_log.body)
    .bind(updated_at)
    .bind(next_scan_at)
    .bind(&topic.thread_id)
    .bind(&topic.catalog_post_id)
    .bind(&topic.system_log_post_id)
    .bind(topic.content_revision)
    .bind(topic.content_claim_token)
    .bind(&topic.content_claim_owner)
    .bind(topic.content_claim_fencing)
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("写入论坛固定楼层正文证明失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛固定楼层正文版本已变化"));
    }
    Ok(())
}

pub(super) async fn record_fixed_posts_progress_postgres(
    store: &PostgresStore,
    topic: &ForumReplyTopic,
    catalog: &ForumRenderedThread,
    system_log: &ForumRenderedPost,
    updated_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let result = sqlx::query(
        "UPDATE public.auction_forum_sync
         SET title = $2,
             body = $3,
             system_log_body = $4,
             content_revision = content_revision + 1,
             content_synced_at = $5,
             updated_at = $5
         WHERE auction_id = $1
           AND status = 'published'
           AND thread_id = $6
           AND catalog_post_id = $7
           AND system_log_post_id = $8
           AND content_revision = $9
           AND content_claim_token = $10
           AND content_claim_owner = $11
           AND content_claim_fencing = $12
           AND content_lease_expires_at > $5",
    )
    .bind(topic.auction_id.as_uuid())
    .bind(&catalog.title)
    .bind(&catalog.body)
    .bind(&system_log.body)
    .bind(updated_at)
    .bind(&topic.thread_id)
    .bind(&topic.catalog_post_id)
    .bind(&topic.system_log_post_id)
    .bind(topic.content_revision)
    .bind(topic.content_claim_token)
    .bind(&topic.content_claim_owner)
    .bind(topic.content_claim_fencing)
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("写入论坛固定楼层暂存进度失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛固定楼层暂存进度 claim 已失效"));
    }
    Ok(())
}

pub(super) async fn finish_topic_scan_postgres(
    store: &PostgresStore,
    topic: &ForumReplyTopic,
    scanned_at: DateTime<Utc>,
    next_scan_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let result = sqlx::query(
        "UPDATE public.auction_forum_sync
         SET content_next_scan_at = $3,
             content_lease_expires_at = NULL,
             content_claim_token = NULL,
             content_claim_owner = NULL,
             content_last_success_at = $2,
             content_last_error_at = NULL,
             content_last_error_code = NULL
         WHERE auction_id = $1
           AND status = 'published'
           AND thread_id = $4
           AND catalog_post_id = $5
           AND system_log_post_id = $6
           AND content_revision = $7
           AND content_claim_token = $8
           AND content_claim_owner = $9
           AND content_claim_fencing = $10
           AND content_lease_expires_at > $2",
    )
    .bind(topic.auction_id.as_uuid())
    .bind(scanned_at)
    .bind(next_scan_at)
    .bind(&topic.thread_id)
    .bind(&topic.catalog_post_id)
    .bind(&topic.system_log_post_id)
    .bind(topic.content_revision)
    .bind(topic.content_claim_token)
    .bind(&topic.content_claim_owner)
    .bind(topic.content_claim_fencing)
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("完成论坛固定楼层扫描失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛固定楼层扫描 claim 已失效"));
    }
    Ok(())
}

pub(super) async fn fail_topic_scan_postgres(
    store: &PostgresStore,
    topic: &ForumReplyTopic,
    failed_at: DateTime<Utc>,
    retry_at: DateTime<Utc>,
    failure_code: &str,
) -> Result<(), ApplicationError> {
    let result = sqlx::query(
        "UPDATE public.auction_forum_sync
         SET content_next_scan_at = $3,
             content_lease_expires_at = NULL,
             content_claim_token = NULL,
             content_claim_owner = NULL,
             content_last_error_at = $2,
             content_last_error_code = $4,
             updated_at = GREATEST(updated_at, $2)
         WHERE auction_id = $1
           AND status = 'published'
           AND thread_id = $5
           AND content_claim_token = $6
           AND content_claim_owner = $7
           AND content_claim_fencing = $8",
    )
    .bind(topic.auction_id.as_uuid())
    .bind(failed_at)
    .bind(retry_at)
    .bind(failure_code)
    .bind(&topic.thread_id)
    .bind(topic.content_claim_token)
    .bind(&topic.content_claim_owner)
    .bind(topic.content_claim_fencing)
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("记录论坛固定楼层扫描失败状态失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛固定楼层扫描失败 claim 已失效"));
    }
    Ok(())
}
