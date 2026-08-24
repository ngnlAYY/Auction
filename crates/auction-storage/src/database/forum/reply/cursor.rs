//! 论坛回帖扫描游标的 PostgreSQL 更新。

#[cfg(feature = "postgres")]
use auction_application::ApplicationError;
#[cfg(feature = "postgres")]
use auction_application::forum_world_name;
#[cfg(feature = "postgres")]
use auction_domain::SettlementWorld;
#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};

#[cfg(feature = "postgres")]
use super::super::super::PostgresStore;
#[cfg(feature = "postgres")]
use super::validation::storage_error;

#[cfg(feature = "postgres")]
pub(super) async fn mark_scanned_postgres(
    store: &PostgresStore,
    world: SettlementWorld,
    scanned_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let result = sqlx::query(
        "UPDATE public.auction_forum_reply_cursors
         SET last_scan_at = GREATEST(last_scan_at, $2),
             state_version = state_version + 1,
             updated_at = CURRENT_TIMESTAMP
         WHERE scope = $1",
    )
    .bind(forum_world_name(world))
    .bind(scanned_at)
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("更新论坛回帖扫描游标失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛回帖扫描游标不存在"));
    }
    Ok(())
}
