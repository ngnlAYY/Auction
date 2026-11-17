//! 论坛回帖的持久化游标、输入校验与应用端口分派；不直接改变竞价状态。

use auction_application::core::ApplicationFuture;
use auction_application::forum::{
    ForumAuctionSnapshot, ForumBidCommandRecord, ForumBidderIdentity, ForumCommandOutcome,
    ForumCommandStart, ForumLotTarget, ForumRenderedPost, ForumRenderedThread, ForumReplyStore,
    ForumReplyTopic, NotifySubscription,
};
use auction_domain::{AuctionId, LotId, SettlementWorld, UserId};
use auction_time::Timestamp;
use uuid::Uuid;

use super::super::DatabaseStore;
use crate::database::storage_error;
use crate::database::storage_error_from;

mod commands;
mod fixed_posts;
mod identity;
mod queries;
mod subscribe;
mod validation;

use commands::{begin_command_postgres, finish_command_postgres};
use fixed_posts::{
    fail_topic_scan_postgres, finish_topic_scan_postgres, record_fixed_posts_progress_postgres,
    record_fixed_posts_update_postgres,
};
use identity::resolve_bidder_postgres;
use queries::{
    claim_published_topics_postgres, command_outcomes_postgres,
    earliest_live_lot_deadline_postgres, lot_targets_postgres, reply_commands_open_postgres,
    reply_cursor_postgres,
};
use subscribe::subscribe_notify_postgres;
use validation::{
    MAX_ERROR_CODE_BYTES, MAX_ERROR_DETAIL_BYTES, MAX_FORUM_ACCOUNT_ID_BYTES,
    MAX_PLAYER_NAME_BYTES, MAX_REPLY_BATCH_SIZE, MAX_THREAD_ID_BYTES, valid_command_record,
    valid_decimal_id, valid_optional_text, valid_processed_through, valid_text, valid_topic_layout,
    valid_topic_lease,
};

impl ForumReplyStore for DatabaseStore {
    fn claim_published_topics(
        &self,
        worker_id: String,
        now: Timestamp,
        lease_until: Timestamp,
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
            claim_published_topics_postgres(
                self,
                &worker_id,
                now,
                lease_until,
                &available_worlds,
                limit,
            )
            .await
        })
    }

    fn reply_commands_open(&self, auction_id: AuctionId) -> ApplicationFuture<'_, bool> {
        Box::pin(async move {
            if auction_id.is_nil() {
                return Err(storage_error("论坛回帖拍卖标识无效"));
            }
            reply_commands_open_postgres(self, auction_id).await
        })
    }

    fn lot_targets(&self, auction_id: AuctionId) -> ApplicationFuture<'_, Vec<ForumLotTarget>> {
        Box::pin(async move {
            if auction_id.is_nil() {
                return Err(storage_error("论坛回帖拍卖标识无效"));
            }
            lot_targets_postgres(self, auction_id).await
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
                return Err(auction_application::core::ApplicationError::ForumIdentityRejected(
                    "论坛回帖身份参数无效",
                ));
            }
            resolve_bidder_postgres(self, world, &external_account_id, &player_name).await
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
            begin_command_postgres(self, record).await
        })
    }

    fn subscribe_notify(
        &self,
        command_id: Uuid,
        auction_id: AuctionId,
        lot_id: LotId,
        user_id: UserId,
        post_id: String,
        lot_code: String,
    ) -> ApplicationFuture<'_, NotifySubscription> {
        Box::pin(async move {
            subscribe_notify_postgres(
                self, command_id, auction_id, lot_id, user_id, &post_id, &lot_code,
            )
            .await
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
            finish_command_postgres(self, command_id, &status, &error_code, &safe_detail).await
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
            command_outcomes_postgres(self, auction_id, &thread_id).await
        })
    }

    fn reply_cursor(
        &self,
        auction_id: AuctionId,
        thread_id: String,
    ) -> ApplicationFuture<'_, Option<auction_application::forum::ForumTopicCursor>> {
        Box::pin(async move {
            if auction_id.is_nil() || !valid_decimal_id(&thread_id, MAX_THREAD_ID_BYTES) {
                return Err(storage_error("论坛回帖游标查询参数无效"));
            }
            reply_cursor_postgres(self, auction_id, &thread_id).await
        })
    }

    fn earliest_live_lot_deadline(
        &self,
        auction_id: AuctionId,
    ) -> ApplicationFuture<'_, Option<Timestamp>> {
        Box::pin(async move {
            if auction_id.is_nil() {
                return Err(storage_error("论坛回帖截止查询参数无效"));
            }
            earliest_live_lot_deadline_postgres(self, auction_id).await
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
            super::sync::load_current_snapshot_postgres(self, auction_id).await
        })
    }

    fn record_fixed_posts_update(
        &self,
        topic: ForumReplyTopic,
        catalog: ForumRenderedThread,
        system_log: ForumRenderedPost,
        processed_through_post_number: i32,
        updated_at: Timestamp,
        next_scan_at: Timestamp,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if !valid_topic_lease(&topic, updated_at)
                || !valid_topic_layout(&topic)
                || !valid_processed_through(&topic, processed_through_post_number)
                || !super::sync::valid_fixed_post_update(&catalog, &system_log)
                || catalog.catalog_continuation.len() != topic.catalog_continuation.len()
                || updated_at.timestamp_nanos_opt().is_none()
                || next_scan_at.timestamp_nanos_opt().is_none()
                || next_scan_at <= updated_at
            {
                return Err(storage_error("论坛固定楼层更新参数无效"));
            }
            record_fixed_posts_update_postgres(
                self,
                &topic,
                &catalog,
                &system_log,
                processed_through_post_number,
                updated_at,
                next_scan_at,
            )
            .await
        })
    }

    fn record_fixed_posts_progress(
        &self,
        topic: ForumReplyTopic,
        catalog: ForumRenderedThread,
        system_log: ForumRenderedPost,
        processed_through_post_number: i32,
        updated_at: Timestamp,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if !valid_topic_lease(&topic, updated_at)
                || !valid_topic_layout(&topic)
                || !valid_processed_through(&topic, processed_through_post_number)
                || !super::sync::valid_fixed_post_update(&catalog, &system_log)
                || catalog.catalog_continuation.len() != topic.catalog_continuation.len()
                || updated_at.timestamp_nanos_opt().is_none()
            {
                return Err(storage_error("论坛固定楼层进度参数无效"));
            }
            record_fixed_posts_progress_postgres(
                self,
                &topic,
                &catalog,
                &system_log,
                processed_through_post_number,
                updated_at,
            )
            .await
        })
    }

    fn record_topic_page_count(
        &self,
        auction_id: AuctionId,
        page_count: u32,
        page_limit: u32,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            // 页数可以超过配置上限：超限失败本身就是要持久化的告警观察。
            if auction_id.is_nil() || page_count == 0 || !(1..=1_000).contains(&page_limit) {
                return Err(storage_error("论坛远端分页页数参数无效"));
            }
            sqlx::query(
                "UPDATE public.auction_forum_sync
                     SET remote_page_count = $2,
                         remote_page_limit = $3,
                         remote_page_count_observed_at = transaction_timestamp()
                     WHERE auction_id = $1",
            )
            .bind(auction_id.as_uuid())
            .bind(i32::try_from(page_count).unwrap_or(i32::MAX))
            .bind(i32::try_from(page_limit).unwrap_or(i32::MAX))
            .execute(self.pool())
            .await
            .map_err(|error| storage_error_from(error, "写入论坛远端分页页数失败"))?;
            Ok(())
        })
    }

    fn finish_topic_scan(
        &self,
        topic: ForumReplyTopic,
        processed_through_post_number: i32,
        scanned_at: Timestamp,
        next_scan_at: Timestamp,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if !valid_topic_lease(&topic, scanned_at)
                || !valid_topic_layout(&topic)
                || !valid_processed_through(&topic, processed_through_post_number)
                || scanned_at.timestamp_nanos_opt().is_none()
                || next_scan_at.timestamp_nanos_opt().is_none()
                || next_scan_at <= scanned_at
            {
                return Err(storage_error("论坛固定楼层扫描完成参数无效"));
            }
            finish_topic_scan_postgres(
                self,
                &topic,
                processed_through_post_number,
                scanned_at,
                next_scan_at,
            )
            .await
        })
    }

    fn fail_topic_scan(
        &self,
        topic: ForumReplyTopic,
        failed_at: Timestamp,
        retry_at: Timestamp,
        failure_code: String,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if !valid_topic_lease(&topic, failed_at)
                || failed_at.timestamp_nanos_opt().is_none()
                || retry_at.timestamp_nanos_opt().is_none()
                || retry_at <= failed_at
                || !valid_optional_text(&failure_code, MAX_ERROR_CODE_BYTES)
            {
                return Err(storage_error("论坛固定楼层扫描失败参数无效"));
            }
            fail_topic_scan_postgres(self, &topic, failed_at, retry_at, &failure_code).await
        })
    }
}
