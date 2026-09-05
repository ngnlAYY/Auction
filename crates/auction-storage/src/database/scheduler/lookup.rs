//! 只读定位已冻结的周轮次；新配置只用于缺失轮次的首次创建。

use auction_application::{RepositoryError, ScheduledRoundSnapshot};
use chrono::{Datelike, NaiveDate};

use crate::database::TrackedPool;

use super::{
    bootstrap,
    mapping::{map_storage_error, repository_error_owned},
};

#[cfg(feature = "postgres")]
pub(super) async fn postgres(
    pool: &TrackedPool<sqlx::Postgres>,
    world_id: &str,
    local_week: NaiveDate,
) -> Result<Option<ScheduledRoundSnapshot>, RepositoryError> {
    validate_key(world_id, local_week)?;
    sqlx::query(
        "SELECT id, world_id, local_week, auction_id, status, preview_due_at, open_due_at,
                scheduled_end_at, create_command_id, preview_completed_at, open_completed_at
         FROM public.scheduled_rounds WHERE world_id = $1 AND local_week = $2",
    )
    .bind(world_id)
    .bind(local_week)
    .fetch_optional(pool)
    .await
    .map_err(map_storage_error)?
    .as_ref()
    .map(bootstrap::snapshot_postgres)
    .transpose()
}

#[cfg(feature = "sqlite")]
pub(super) async fn sqlite(
    pool: &TrackedPool<sqlx::Sqlite>,
    world_id: &str,
    local_week: NaiveDate,
) -> Result<Option<ScheduledRoundSnapshot>, RepositoryError> {
    validate_key(world_id, local_week)?;
    sqlx::query(
        "SELECT id, world_id, local_week, auction_id, status, preview_due_at, open_due_at,
                scheduled_end_at, create_command_id, preview_completed_at, open_completed_at
         FROM scheduled_rounds WHERE world_id = ?1 AND local_week = ?2",
    )
    .bind(world_id)
    .bind(local_week.to_string())
    .fetch_optional(pool)
    .await
    .map_err(map_storage_error)?
    .as_ref()
    .map(bootstrap::snapshot_sqlite)
    .transpose()
}

fn validate_key(world_id: &str, local_week: NaiveDate) -> Result<(), RepositoryError> {
    if !matches!(world_id, "main" | "isekai") || local_week.weekday().num_days_from_monday() != 0 {
        return Err(repository_error_owned("周计划读取身份无效"));
    }
    Ok(())
}
