//! 所有目录页及系统更新楼的同事务正文证明更新。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

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
    processed_through_post_number: i32,
    updated_at: DateTime<Utc>,
    next_scan_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "开启论坛分页正文证明事务失败"))?;
    lock_terminal_version(&mut transaction, topic).await?;
    lock_content_claim(&mut transaction, topic).await?;
    let result = sqlx::query(
        "UPDATE public.auction_forum_sync
         SET title = $2,
             body = $3,
             system_log_body = $4,
             processed_through_post_number = $14,
             content_revision = content_revision + 1,
             content_synced_at = $5,
             content_next_scan_at = $6,
             terminal_synced_version = COALESCE($16, terminal_synced_version),
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
           AND catalog_page_count = $15
           AND content_claim_token = $11
           AND content_claim_owner = $12
           AND content_claim_fencing = $13
           AND content_lease_expires_at > $5
           AND content_lease_expires_at > clock_timestamp()
           AND ($16::bigint IS NULL OR (terminal_refresh_enabled AND terminal_synced_version IS NULL))",
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
    .bind(processed_through_post_number)
    .bind((topic.catalog_continuation.len() + 1) as i32)
    .bind(topic.confirmed_terminal_version)
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入论坛固定楼层正文证明失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛固定楼层正文版本已变化"));
    }
    super::super::catalog::update_pages(&mut transaction, topic, catalog).await?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交论坛分页正文证明失败"))?;
    Ok(())
}

pub(super) async fn record_fixed_posts_progress_postgres(
    store: &PostgresStore,
    topic: &ForumReplyTopic,
    catalog: &ForumRenderedThread,
    system_log: &ForumRenderedPost,
    processed_through_post_number: i32,
    updated_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "开启论坛分页进度事务失败"))?;
    lock_content_claim(&mut transaction, topic).await?;
    let result = sqlx::query(
        "UPDATE public.auction_forum_sync
         SET title = $2,
             body = $3,
             system_log_body = $4,
             processed_through_post_number = $13,
             content_revision = content_revision + 1,
             content_synced_at = $5,
             updated_at = $5
         WHERE auction_id = $1
           AND status = 'published'
           AND thread_id = $6
           AND catalog_post_id = $7
           AND system_log_post_id = $8
           AND content_revision = $9
           AND catalog_page_count = $14
           AND content_claim_token = $10
           AND content_claim_owner = $11
           AND content_claim_fencing = $12
           AND content_lease_expires_at > $5
           AND content_lease_expires_at > clock_timestamp()",
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
    .bind(processed_through_post_number)
    .bind((topic.catalog_continuation.len() + 1) as i32)
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入论坛固定楼层暂存进度失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛固定楼层暂存进度 claim 已失效"));
    }
    super::super::catalog::update_pages(&mut transaction, topic, catalog).await?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交论坛分页进度失败"))?;
    Ok(())
}

pub(super) async fn finish_topic_scan_postgres(
    store: &PostgresStore,
    topic: &ForumReplyTopic,
    processed_through_post_number: i32,
    scanned_at: DateTime<Utc>,
    next_scan_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "开启论坛扫描完成事务失败"))?;
    lock_terminal_version(&mut transaction, topic).await?;
    lock_content_claim(&mut transaction, topic).await?;
    let result = sqlx::query(
        "UPDATE public.auction_forum_sync
         SET processed_through_post_number = $11,
             content_next_scan_at = $3,
             terminal_synced_version = COALESCE($13, terminal_synced_version),
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
           AND catalog_page_count = $12
           AND content_claim_token = $8
           AND content_claim_owner = $9
           AND content_claim_fencing = $10
           AND content_lease_expires_at > $2
           AND content_lease_expires_at > clock_timestamp()
           AND ($13::bigint IS NULL OR (terminal_refresh_enabled AND terminal_synced_version IS NULL))",
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
    .bind(processed_through_post_number)
    .bind((topic.catalog_continuation.len() + 1) as i32)
    .bind(topic.confirmed_terminal_version)
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "完成论坛固定楼层扫描失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛固定楼层扫描 claim 已失效"));
    }
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交论坛扫描完成事务失败"))?;
    Ok(())
}

/// 先取得内容行锁，再由后续写入检查实时时钟；终态调用方已先锁 auction。
async fn lock_content_claim(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    topic: &ForumReplyTopic,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "SELECT auction_id FROM public.auction_forum_sync WHERE auction_id = $1 FOR UPDATE",
    )
    .bind(topic.auction_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "锁定论坛内容租约失败"))?;
    Ok(())
}

/// 只认 Worker 实际渲染并证明的版本，不从当前状态反推成功；锁序为 auction → sync。
async fn lock_terminal_version(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    topic: &ForumReplyTopic,
) -> Result<(), ApplicationError> {
    let Some(version) = topic.confirmed_terminal_version else { return Ok(()) };
    if version <= 0 {
        return Err(storage_error("论坛终态证明版本无效"));
    }
    let matched = sqlx::query_scalar::<_, bool>(
        "SELECT status IN ('settled', 'cancelled') AND state_version = $2
         FROM public.auctions WHERE id = $1 FOR SHARE",
    )
    .bind(topic.auction_id.as_uuid())
    .bind(version)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "锁定论坛终态版本失败"))?;
    if !matched {
        return Err(storage_error("论坛终态快照版本已变化"));
    }
    let pending = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM public.auction_forum_bid_commands
         WHERE auction_id = $1 AND status = 'pending')",
    )
    .bind(topic.auction_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "核对论坛终态 pending 边界失败"))?;
    if pending {
        return Err(storage_error("论坛终态仍有 pending 命令，不能完成"));
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
    .map_err(|error| storage_error_from(error, "记录论坛固定楼层扫描失败状态失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛固定楼层扫描失败 claim 已失效"));
    }
    Ok(())
}
