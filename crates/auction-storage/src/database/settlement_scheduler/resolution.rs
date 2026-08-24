//! PostgreSQL 结算 workflow 的完成与失败收口。

use auction_application::SettlementWorkClaim;
use chrono::{DateTime, Utc};
use sqlx::Postgres;

use super::{map_storage_error, repository_error_owned};
use crate::database::TrackedPool;

pub(super) async fn complete_postgres(
    pool: &TrackedPool<Postgres>,
    claim: &SettlementWorkClaim,
    completed_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let result = sqlx::query(
        r#"UPDATE public.auctions
           SET settlement_complete_command_id = $3,
               settlement_next_attempt_at = NULL,
               settlement_last_error_code = NULL,
               updated_at = $4
           WHERE id = $1 AND status = 'settled' AND settlement_workflow_id = $2
             AND settlement_next_attempt_at = $5
             AND (settlement_complete_command_id IS NULL
                 OR settlement_complete_command_id = $3)"#,
    )
    .bind(claim.auction_id.as_uuid())
    .bind(claim.workflow_id)
    .bind(claim.settlement_command_id.as_uuid())
    .bind(completed_at)
    .bind(claim.lease_until)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned("结算 workflow 完成 fence 已失效"));
    }
    Ok(())
}

pub(super) async fn fail_postgres(
    pool: &TrackedPool<Postgres>,
    claim: &SettlementWorkClaim,
    retry_at: DateTime<Utc>,
    failure_code: &str,
) -> Result<(), auction_application::RepositoryError> {
    let result = sqlx::query(
        r#"UPDATE public.auctions
           SET settlement_next_attempt_at = $3,
               settlement_last_error_code = $4,
               state_version = state_version + 1,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $1 AND status = 'settling' AND settlement_workflow_id = $2
             AND settlement_next_attempt_at = $5
             AND settlement_complete_command_id IS NULL"#,
    )
    .bind(claim.auction_id.as_uuid())
    .bind(claim.workflow_id)
    .bind(retry_at)
    .bind(failure_code)
    .bind(claim.lease_until)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned("结算 workflow 失败 fence 已失效"));
    }
    Ok(())
}
