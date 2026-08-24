//! 论坛主题同步发布确认与失败退避的 PostgreSQL 状态推进。

use auction_application::{
    ApplicationError, ForumPublishedThread, ForumRenderedThread, ForumSyncClaim,
};
use chrono::{DateTime, Utc};

use super::super::super::PostgresStore;
use super::validation::storage_error;

pub(super) async fn publish_postgres(
    store: &PostgresStore,
    claim: &ForumSyncClaim,
    rendered: &ForumRenderedThread,
    published: &ForumPublishedThread,
    published_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
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
             last_error_code = NULL,
             last_error_detail = NULL,
             updated_at = $6
         WHERE auction_id = $1
           AND status = 'publishing'
           AND claim_token = $7
           AND claim_owner = $8
           AND claim_fencing = $9
           AND source_revision = $10
           AND lease_expires_at > $6",
    )
    .bind(claim.auction_id.as_uuid())
    .bind(&rendered.title)
    .bind(&rendered.body)
    .bind(&published.thread_id)
    .bind(&published.thread_url)
    .bind(published_at)
    .bind(claim.claim_token)
    .bind(&claim.worker_id)
    .bind(claim.claim_fencing)
    .bind(claim.source_revision)
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("写入论坛同步发布证明失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛同步发布 claim 已失效"));
    }
    Ok(())
}

pub(super) async fn fail_postgres(
    store: &PostgresStore,
    claim: &ForumSyncClaim,
    retry_at: DateTime<Utc>,
    failure_code: &str,
    safe_detail: &str,
) -> Result<(), ApplicationError> {
    let result = sqlx::query(
        "UPDATE public.auction_forum_sync
         SET status = 'failed',
             next_attempt_at = $2,
             lease_expires_at = NULL,
             claim_token = NULL,
             claim_owner = NULL,
             last_error_code = $3,
             last_error_detail = $4,
             updated_at = CURRENT_TIMESTAMP
         WHERE auction_id = $1
           AND status = 'publishing'
           AND claim_token = $5
           AND claim_owner = $6
           AND claim_fencing = $7
           AND source_revision = $8
           AND lease_expires_at > CURRENT_TIMESTAMP",
    )
    .bind(claim.auction_id.as_uuid())
    .bind(retry_at)
    .bind(failure_code)
    .bind(safe_detail)
    .bind(claim.claim_token)
    .bind(&claim.worker_id)
    .bind(claim.claim_fencing)
    .bind(claim.source_revision)
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("写入论坛同步失败退避状态失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛同步失败 claim 已失效"));
    }
    Ok(())
}
