//! PostgreSQL 论坛回帖导入存储的模块装配。
//!
//! 论坛命令先落入本地账本，再由 Worker 调用现有竞价端口。主题/身份查询、命令账本、扫描
//! 游标和输入校验分别由子模块负责；本 owner 不直接改变竞价状态，SQLite Lite 明确不提供
//! 这条完整档能力。

use auction_application::{
    ApplicationFuture, ForumActiveProxy, ForumBidCommandRecord, ForumBidderIdentity,
    ForumCommandStart, ForumLotTarget, ForumReplyStore, ForumReplyTopic,
};
use auction_domain::{AuctionId, LotId, SettlementWorld, UserId};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::super::DatabaseStore;

mod commands;
mod cursor;
mod queries;
mod validation;

#[cfg(feature = "postgres")]
use commands::{begin_command_postgres, finish_command_postgres};
#[cfg(feature = "postgres")]
use cursor::mark_scanned_postgres;
#[cfg(feature = "postgres")]
use queries::{
    active_proxy_postgres, list_published_topics_postgres, lot_targets_postgres,
    resolve_bidder_postgres,
};
#[cfg(feature = "sqlite")]
use validation::capability_error;
use validation::{
    MAX_DISPLAY_NAME_BYTES, MAX_ERROR_CODE_BYTES, MAX_ERROR_DETAIL_BYTES,
    MAX_EXTERNAL_ACCOUNT_BYTES, MAX_REPLY_BATCH_SIZE, storage_error, valid_command_record,
    valid_optional_text, valid_text,
};

impl ForumReplyStore for DatabaseStore {
    fn list_published_topics(&self, limit: u16) -> ApplicationFuture<'_, Vec<ForumReplyTopic>> {
        Box::pin(async move {
            if limit == 0 || limit > MAX_REPLY_BATCH_SIZE {
                return Err(storage_error("论坛回帖主题批量大小无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    list_published_topics_postgres(store, limit).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn resolve_bidder(
        &self,
        world: SettlementWorld,
        external_account_id: String,
        display_name: String,
    ) -> ApplicationFuture<'_, ForumBidderIdentity> {
        Box::pin(async move {
            #[cfg(feature = "sqlite")]
            let _ = world;
            if !valid_text(&external_account_id, MAX_EXTERNAL_ACCOUNT_BYTES)
                || !valid_text(&display_name, MAX_DISPLAY_NAME_BYTES)
            {
                return Err(storage_error("论坛回帖身份参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    resolve_bidder_postgres(store, world, &external_account_id, &display_name).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn mark_scanned(
        &self,
        world: SettlementWorld,
        scanned_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            #[cfg(feature = "sqlite")]
            let _ = world;
            if scanned_at.timestamp_nanos_opt().is_none() {
                return Err(storage_error("论坛回帖扫描时间无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    mark_scanned_postgres(store, world, scanned_at).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }
}
