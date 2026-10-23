//! 论坛回帖的持久化游标、输入校验与应用端口分派；不直接改变竞价状态。

use auction_application::{
    ApplicationFuture, ForumActiveProxy, ForumAuctionSnapshot, ForumBidCommandRecord,
    ForumBidderIdentity, ForumCommandOutcome, ForumCommandStart, ForumLotTarget, ForumRenderedPost,
    ForumRenderedThread, ForumReplyStore, ForumReplyTopic,
};
use auction_domain::{AuctionId, LotId, SettlementWorld, UserId};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::super::DatabaseStore;

mod commands;
#[cfg(feature = "postgres")]
mod fixed_posts;
#[cfg(feature = "postgres")]
mod identity;
mod queries;
mod validation;

#[cfg(feature = "postgres")]
use commands::{begin_command_postgres, finish_command_postgres};
#[cfg(feature = "postgres")]
use fixed_posts::{
    fail_topic_scan_postgres, finish_topic_scan_postgres, record_fixed_posts_progress_postgres,
    record_fixed_posts_update_postgres,
};
#[cfg(feature = "postgres")]
use identity::resolve_bidder_postgres;
#[cfg(feature = "postgres")]
use queries::{
    active_proxy_postgres, claim_published_topics_postgres, command_outcomes_postgres,
    lot_targets_postgres, reply_commands_open_postgres,
};
use validation::{
    MAX_ERROR_CODE_BYTES, MAX_ERROR_DETAIL_BYTES, MAX_FIXED_POST_ID_BYTES,
    MAX_FORUM_ACCOUNT_ID_BYTES, MAX_PLAYER_NAME_BYTES, MAX_REPLY_BATCH_SIZE, MAX_THREAD_ID_BYTES,
    storage_error, valid_command_record, valid_decimal_id, valid_optional_text, valid_text,
};

impl ForumReplyStore for DatabaseStore {
    fn claim_published_topics(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        available_worlds: Vec<SettlementWorld>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<ForumReplyTopic>> {
        Box::pin(async move {
            if !valid_text(&worker_id, 256)
                || now.timestamp_nanos_opt().is_none()
                || lease_until.timestamp_nanos_opt().is_none()
                || lease_until <= now
                || limit == 0
                || limit > MAX_REPLY_BATCH_SIZE
                || available_worlds.len() > 2
            {
                return Err(storage_error("论坛回帖主题批量大小无效"));
            }
            if available_worlds.is_empty() {
                return Ok(Vec::new());
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    claim_published_topics_postgres(
                        store,
                        &worker_id,
                        now,
                        lease_until,
                        &available_worlds,
                        limit,
                    )
                    .await
                }
            }
        })
    }

    fn reply_commands_open(&self, auction_id: AuctionId) -> ApplicationFuture<'_, bool> {
        Box::pin(async move {
            if auction_id.is_nil() {
                return Err(storage_error("论坛回帖拍卖标识无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    reply_commands_open_postgres(store, auction_id).await
                }
            }
        })
    }

    fn lot_targets(&self, auction_id: AuctionId) -> ApplicationFuture<'_, Vec<ForumLotTarget>> {
        Box::pin(async move {
            if auction_id.is_nil() {
                return Err(storage_error("论坛回帖拍卖标识无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => lot_targets_postgres(store, auction_id).await,
            }
        })
    }

    fn resolve_bidder(
        &self,
        world: SettlementWorld,
        external_account_id: String,
        player_name: String,
    ) -> ApplicationFuture<'_, ForumBidderIdentity> {
        Box::pin(async move {
            if !valid_decimal_id(&external_account_id, MAX_FORUM_ACCOUNT_ID_BYTES)
                || !valid_text(&player_name, MAX_PLAYER_NAME_BYTES)
            {
                return Err(auction_application::ApplicationError::ForumIdentityRejected(
                    "论坛回帖身份参数无效",
                ));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    resolve_bidder_postgres(store, world, &external_account_id, &player_name).await
                }
            }
        })
    }

    fn active_proxy(
        &self,
        lot_id: LotId,
        user_id: UserId,
    ) -> ApplicationFuture<'_, Option<ForumActiveProxy>> {
        Box::pin(async move {
            if lot_id.is_nil() || user_id.is_nil() {
                return Err(storage_error("论坛回帖代理查询标识无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    active_proxy_postgres(store, lot_id, user_id).await
                }
            }
        })
    }

    fn begin_command(
        &self,
        record: ForumBidCommandRecord,
    ) -> ApplicationFuture<'_, ForumCommandStart> {
        Box::pin(async move {
            if !valid_command_record(&record) {
                return Err(storage_error("论坛回帖命令账本参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => begin_command_postgres(store, record).await,
            }
        })
    }

    fn finish_command(
        &self,
        command_id: Uuid,
        status: String,
        error_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if command_id.is_nil()
                || !matches!(status.as_str(), "applied" | "rejected")
                || !valid_optional_text(&error_code, MAX_ERROR_CODE_BYTES)
                || !valid_optional_text(&safe_detail, MAX_ERROR_DETAIL_BYTES)
            {
                return Err(storage_error("论坛回帖命令完成参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    finish_command_postgres(store, command_id, &status, &error_code, &safe_detail)
                        .await
                }
            }
        })
    }

    fn command_outcomes(
        &self,
        auction_id: AuctionId,
        thread_id: String,
    ) -> ApplicationFuture<'_, Vec<ForumCommandOutcome>> {
        Box::pin(async move {
            if auction_id.is_nil() || !valid_decimal_id(&thread_id, MAX_THREAD_ID_BYTES) {
                return Err(storage_error("论坛命令结果查询参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    command_outcomes_postgres(store, auction_id, &thread_id).await
                }
            }
        })
    }

    fn load_current_snapshot(
        &self,
        auction_id: AuctionId,
    ) -> ApplicationFuture<'_, ForumAuctionSnapshot> {
        Box::pin(async move {
            if auction_id.is_nil() {
                return Err(storage_error("论坛刷新快照拍卖标识无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    super::sync::load_current_snapshot_postgres(store, auction_id).await
                }
            }
        })
    }

    fn record_fixed_posts_update(
        &self,
        topic: ForumReplyTopic,
        catalog: ForumRenderedThread,
        system_log: ForumRenderedPost,
        processed_through_post_number: i32,
        updated_at: DateTime<Utc>,
        next_scan_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if topic.auction_id.is_nil()
                || !valid_decimal_id(&topic.thread_id, MAX_THREAD_ID_BYTES)
                || !valid_decimal_id(&topic.catalog_post_id, MAX_FIXED_POST_ID_BYTES)
                || !valid_decimal_id(&topic.system_log_post_id, MAX_FIXED_POST_ID_BYTES)
                || !super::catalog::valid_pages(
                    &topic.catalog_post_id,
                    &topic.system_log_post_id,
                    &topic.catalog_continuation,
                )
                || topic.content_revision <= 0
                || processed_through_post_number < topic.system_log_post_number() as i32
                || topic.content_claim_token.is_nil()
                || !valid_text(&topic.content_claim_owner, 256)
                || topic.content_claim_fencing <= 0
                || topic.content_lease_until <= updated_at
                || !super::sync::valid_fixed_post_update(&catalog, &system_log)
                || catalog.catalog_continuation.len() != topic.catalog_continuation.len()
                || updated_at.timestamp_nanos_opt().is_none()
                || next_scan_at.timestamp_nanos_opt().is_none()
                || next_scan_at <= updated_at
            {
                return Err(storage_error("论坛固定楼层更新参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    record_fixed_posts_update_postgres(
                        store,
                        &topic,
                        &catalog,
                        &system_log,
                        processed_through_post_number,
                        updated_at,
                        next_scan_at,
                    )
                    .await
                }
            }
        })
    }

    fn record_fixed_posts_progress(
        &self,
        topic: ForumReplyTopic,
        catalog: ForumRenderedThread,
        system_log: ForumRenderedPost,
        processed_through_post_number: i32,
        updated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if topic.auction_id.is_nil()
                || !valid_decimal_id(&topic.thread_id, MAX_THREAD_ID_BYTES)
                || !valid_decimal_id(&topic.catalog_post_id, MAX_FIXED_POST_ID_BYTES)
                || !valid_decimal_id(&topic.system_log_post_id, MAX_FIXED_POST_ID_BYTES)
                || !super::catalog::valid_pages(
                    &topic.catalog_post_id,
                    &topic.system_log_post_id,
                    &topic.catalog_continuation,
                )
                || topic.content_revision <= 0
                || processed_through_post_number < topic.system_log_post_number() as i32
                || topic.content_claim_token.is_nil()
                || !valid_text(&topic.content_claim_owner, 256)
                || topic.content_claim_fencing <= 0
                || topic.content_lease_until <= updated_at
                || !super::sync::valid_fixed_post_update(&catalog, &system_log)
                || catalog.catalog_continuation.len() != topic.catalog_continuation.len()
                || updated_at.timestamp_nanos_opt().is_none()
            {
                return Err(storage_error("论坛固定楼层进度参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    record_fixed_posts_progress_postgres(
                        store,
                        &topic,
                        &catalog,
                        &system_log,
                        processed_through_post_number,
                        updated_at,
                    )
                    .await
                }
            }
        })
    }

    fn finish_topic_scan(
        &self,
        topic: ForumReplyTopic,
        processed_through_post_number: i32,
        scanned_at: DateTime<Utc>,
        next_scan_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if topic.auction_id.is_nil()
                || !valid_decimal_id(&topic.thread_id, MAX_THREAD_ID_BYTES)
                || !valid_decimal_id(&topic.catalog_post_id, MAX_FIXED_POST_ID_BYTES)
                || !valid_decimal_id(&topic.system_log_post_id, MAX_FIXED_POST_ID_BYTES)
                || !super::catalog::valid_pages(
                    &topic.catalog_post_id,
                    &topic.system_log_post_id,
                    &topic.catalog_continuation,
                )
                || topic.content_revision <= 0
                || processed_through_post_number < topic.system_log_post_number() as i32
                || topic.content_claim_token.is_nil()
                || !valid_text(&topic.content_claim_owner, 256)
                || topic.content_claim_fencing <= 0
                || scanned_at.timestamp_nanos_opt().is_none()
                || next_scan_at.timestamp_nanos_opt().is_none()
                || next_scan_at <= scanned_at
                || topic.content_lease_until <= scanned_at
            {
                return Err(storage_error("论坛固定楼层扫描完成参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    finish_topic_scan_postgres(
                        store,
                        &topic,
                        processed_through_post_number,
                        scanned_at,
                        next_scan_at,
                    )
                    .await
                }
            }
        })
    }

    fn fail_topic_scan(
        &self,
        topic: ForumReplyTopic,
        failed_at: DateTime<Utc>,
        retry_at: DateTime<Utc>,
        failure_code: String,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if topic.auction_id.is_nil()
                || !valid_decimal_id(&topic.thread_id, MAX_THREAD_ID_BYTES)
                || topic.content_claim_token.is_nil()
                || !valid_text(&topic.content_claim_owner, 256)
                || topic.content_claim_fencing <= 0
                || failed_at.timestamp_nanos_opt().is_none()
                || retry_at.timestamp_nanos_opt().is_none()
                || retry_at <= failed_at
                || topic.content_lease_until <= failed_at
                || !valid_optional_text(&failure_code, MAX_ERROR_CODE_BYTES)
            {
                return Err(storage_error("论坛固定楼层扫描失败参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    fail_topic_scan_postgres(store, &topic, failed_at, retry_at, &failure_code)
                        .await
                }
            }
        })
    }
}
