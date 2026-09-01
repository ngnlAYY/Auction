//! PostgreSQL 论坛主题同步存储的模块装配。
//!
//! `auction_forum_sync` 是远程主题投影的唯一 owner。领取、快照读取、发布确认和失败退避都
//! 使用持久化 token/owner/fencing；SQLite Lite 不提供该外部投影能力，明确返回能力缺失。

use auction_application::{
    ApplicationFuture, ForumAuctionSnapshot, ForumPublishedLayout, ForumRenderedPost,
    ForumRenderedThread, ForumSyncClaim, ForumSyncRecoveryItem, ForumSyncRecoveryPort,
    ForumSyncStore, ForumWriteRetryAuthorizationCommand, ForumWriteRetryAuthorizationResult,
    MAX_FORUM_SYNC_RECOVERY_ITEMS,
};
use chrono::{DateTime, Utc};

use super::super::DatabaseStore;

#[cfg(feature = "postgres")]
mod authorization;
#[cfg(feature = "postgres")]
mod claims;
#[cfg(feature = "postgres")]
mod publishing;
#[cfg(feature = "postgres")]
mod snapshot;
pub(super) mod validation;

#[cfg(feature = "postgres")]
use claims::claim_due_postgres;
#[cfg(feature = "postgres")]
use publishing::{fail_postgres, publish_postgres};
#[cfg(feature = "postgres")]
use snapshot::load_snapshot_postgres;
#[cfg(feature = "sqlite")]
use validation::capability_error;
use validation::{
    MAX_CLAIM_BATCH_SIZE, MAX_FAILURE_CODE_BYTES, MAX_FAILURE_DETAIL_BYTES, MAX_WORKER_ID_BYTES,
    storage_error, valid_claim, valid_rendered, valid_rendered_post, valid_text,
};
#[cfg(feature = "postgres")]
pub(super) fn lot_code(
    category: &str,
    lot_number: i32,
) -> Result<String, auction_application::ApplicationError> {
    validation::lot_code(category, lot_number)
}

#[cfg(feature = "postgres")]
pub(super) fn parse_world(
    value: &str,
) -> Result<auction_domain::SettlementWorld, auction_application::ApplicationError> {
    validation::parse_world(value)
}

#[cfg(feature = "postgres")]
pub(super) async fn load_current_snapshot_postgres(
    store: &super::super::PostgresStore,
    auction_id: auction_domain::AuctionId,
) -> Result<ForumAuctionSnapshot, auction_application::ApplicationError> {
    snapshot::load_current_snapshot_postgres(store, auction_id).await
}

pub(super) fn valid_fixed_post_update(
    catalog: &ForumRenderedThread,
    system_log: &ForumRenderedPost,
) -> bool {
    validation::valid_rendered(catalog) && validation::valid_rendered_post(system_log)
}

impl ForumSyncStore for DatabaseStore {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<ForumSyncClaim>> {
        Box::pin(async move {
            if !valid_text(&worker_id, MAX_WORKER_ID_BYTES)
                || now.timestamp_nanos_opt().is_none()
                || lease_until <= now
                || limit == 0
                || limit > MAX_CLAIM_BATCH_SIZE
            {
                return Err(storage_error("论坛同步领取参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    claim_due_postgres(store, &worker_id, now, lease_until, limit).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn load_snapshot(&self, claim: ForumSyncClaim) -> ApplicationFuture<'_, ForumAuctionSnapshot> {
        Box::pin(async move {
            if !valid_claim(&claim) {
                return Err(storage_error("论坛同步快照 claim 无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => load_snapshot_postgres(store, &claim).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn publish(
        &self,
        claim: ForumSyncClaim,
        rendered: ForumRenderedThread,
        system_log: ForumRenderedPost,
        published: ForumPublishedLayout,
        published_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if !valid_claim(&claim)
                || !valid_rendered(&rendered)
                || !valid_rendered_post(&system_log)
                || !validation::valid_layout(&published)
                || published_at.timestamp_nanos_opt().is_none()
            {
                return Err(storage_error("论坛同步发布确认参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    publish_postgres(
                        store,
                        &claim,
                        &rendered,
                        &system_log,
                        &published,
                        published_at,
                    )
                    .await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn fail(
        &self,
        claim: ForumSyncClaim,
        retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if !valid_claim(&claim)
                || retry_at.timestamp_nanos_opt().is_none()
                || !valid_text(&failure_code, MAX_FAILURE_CODE_BYTES)
                || !valid_text(&safe_detail, MAX_FAILURE_DETAIL_BYTES)
            {
                return Err(storage_error("论坛同步失败记录参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    fail_postgres(store, &claim, retry_at, &failure_code, &safe_detail).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }
}

impl ForumSyncRecoveryPort for DatabaseStore {
    fn list_write_recovery(&self, limit: u16) -> ApplicationFuture<'_, Vec<ForumSyncRecoveryItem>> {
        Box::pin(async move {
            if limit == 0 || limit > MAX_FORUM_SYNC_RECOVERY_ITEMS {
                return Err(storage_error("论坛写入恢复列表上限无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => authorization::list(store, limit).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn authorize_write_retry(
        &self,
        command: ForumWriteRetryAuthorizationCommand,
    ) -> ApplicationFuture<'_, ForumWriteRetryAuthorizationResult> {
        Box::pin(async move {
            if command.command_id.is_nil()
                || command.auction_id.is_nil()
                || !valid_text(&command.actor_id, 256)
                || command.request_hash.as_bytes() == [0; 32]
            {
                return Err(storage_error("论坛写入授权命令无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => authorization::authorize(store, &command).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }
}
