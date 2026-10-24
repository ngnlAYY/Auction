//! 论坛主题同步发布确认与失败退避的 PostgreSQL 状态推进。

use auction_application::{
    ApplicationError, ForumPublishedLayout, ForumRenderedPost, ForumRenderedThread, ForumSyncClaim,
};
use chrono::{DateTime, Utc};

use super::super::super::PostgresStore;
use super::validation::storage_error;

pub(super) async fn publish_postgres(
    store: &PostgresStore,
    claim: &ForumSyncClaim,
    rendered: &ForumRenderedThread,
    system_log: &ForumRenderedPost,
    published: &ForumPublishedLayout,
    published_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启论坛分页发布证明事务失败"))?;
    // 锁等待结束后才检查实时租约，不能凭远端回执的旧时间提交过期 claim。
    sqlx::query(
        "SELECT auction_id FROM public.auction_forum_sync WHERE auction_id = $1 FOR UPDATE",
    )
    .bind(claim.auction_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("锁定论坛发布租约失败"))?;
    let page_count = (rendered.catalog_continuation.len() + 1) as i32;
    let result = sqlx::query(
        "UPDATE public.auction_forum_sync
         SET status = 'published',
             lease_expires_at = NULL,
             claim_token = NULL,
             claim_owner = NULL,
             title = $2,
             body = $3,
             thread_id = $4,
             thread_url = $5,
             catalog_post_id = $6,
             system_log_post_id = $7,
             system_log_body = $8,
             catalog_page_count = $14,
             processed_through_post_number = $14 + 1,
             content_revision = CASE WHEN content_revision < 1 THEN 1 ELSE content_revision + 1 END,
             content_synced_at = $9,
             content_last_success_at = $9,
             content_last_error_at = NULL,
             content_last_error_code = NULL,
             last_error_code = NULL,
             last_error_detail = NULL,
             updated_at = $9
         WHERE auction_id = $1
           AND status = 'publishing'
           AND claim_token = $10
           AND claim_owner = $11
           AND claim_fencing = $12
           AND source_revision = $13
           AND catalog_page_count IN (1, $14)
           AND (catalog_post_id IS NULL OR (catalog_post_id = $6 AND system_log_post_id = $7))
           AND lease_expires_at > $9
           AND lease_expires_at > clock_timestamp()",
    )
    .bind(claim.auction_id.as_uuid())
    .bind(&rendered.title)
    .bind(&rendered.body)
    .bind(&published.thread.thread_id)
    .bind(&published.thread.thread_url)
    .bind(&published.catalog_post_id)
    .bind(&published.system_log_post_id)
    .bind(&system_log.body)
    .bind(published_at)
    .bind(claim.claim_token)
    .bind(&claim.worker_id)
    .bind(claim.claim_fencing)
    .bind(claim.source_revision)
    .bind(page_count)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("写入论坛同步发布证明失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛同步发布 claim 或固定楼层身份已失效"));
    }
    super::super::catalog::publish_pages(
        &mut transaction,
        claim.auction_id.as_uuid(),
        rendered,
        published,
    )
    .await?;
    transaction.commit().await.map_err(|_| storage_error("提交论坛分页发布证明失败"))?;
    Ok(())
}

pub(super) async fn fail_postgres(
    store: &PostgresStore,
    claim: &ForumSyncClaim,
    retry_at: DateTime<Utc>,
    failure_code: &str,
    safe_detail: &str,
) -> Result<(), ApplicationError> {
    // 至少推进到本次 claim 的应用时钟，避免数据库时钟轻微落后时重复越过同一进程恢复水位。
    let result = sqlx::query(
        "UPDATE public.auction_forum_sync
         SET status = 'failed',
             next_attempt_at = $2,
             lease_expires_at = NULL,
             claim_token = NULL,
             claim_owner = NULL,
             last_error_code = $3,
             last_error_detail = $4,
             updated_at = GREATEST(CURRENT_TIMESTAMP, $9)
         WHERE auction_id = $1
           AND status = 'publishing'
           AND claim_token = $5
           AND claim_owner = $6
           AND claim_fencing = $7
           AND source_revision = $8",
    )
    .bind(claim.auction_id.as_uuid())
    .bind(retry_at)
    .bind(failure_code)
    .bind(safe_detail)
    .bind(claim.claim_token)
    .bind(&claim.worker_id)
    .bind(claim.claim_fencing)
    .bind(claim.source_revision)
    .bind(claim.claimed_at)
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("写入论坛同步失败退避状态失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛同步失败 claim 已失效"));
    }
    Ok(())
}
