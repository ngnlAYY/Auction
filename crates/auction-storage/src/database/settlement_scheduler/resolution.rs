//! PostgreSQL 结算 workflow 的完成与失败收口。

use auction_application::SettlementWorkClaim;
use std::time::Duration;

use auction_domain::deterministic_uuid;
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Postgres, Row};

use super::{map_storage_error, repository_error_owned};
use crate::database::TrackedPool;
use crate::database::audit_health::AuditWriteResult as _;

const FAILURE_TRANSACTION_ATTEMPTS: usize = 3;
const FAILURE_RETRY_BASE_DELAY: Duration = Duration::from_millis(10);

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
               updated_at = CASE
                   WHEN settlement_complete_command_id IS NULL THEN $4
                   ELSE updated_at
               END
           WHERE id = $1 AND status = 'settled' AND settlement_workflow_id = $2
             AND (settlement_next_attempt_at IS NULL OR settlement_next_attempt_at = $5)
             AND (settlement_complete_command_id IS NULL OR settlement_complete_command_id = $3)"#,
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
    _retry_at: DateTime<Utc>,
    failure_code: &str,
) -> Result<(), auction_application::RepositoryError> {
    pool.observe_transaction(auction_observability::DatabaseTransactionOwner::Settlement, async {
        for attempt in 1..=FAILURE_TRANSACTION_ATTEMPTS {
            match fail_transaction(pool, claim, failure_code).await {
                Ok(()) => return Ok(()),
                Err(error)
                    if crate::database::observed_serialization_conflict(&error)
                        && attempt < FAILURE_TRANSACTION_ATTEMPTS =>
                {
                    // 必须从 BEGIN 开始重放整笔失败进度与审计事务，不能单独重试其中一条写入。
                    tokio::time::sleep(FAILURE_RETRY_BASE_DELAY.saturating_mul(attempt as u32))
                        .await;
                }
                Err(error) => {
                    crate::database::observe_retry_exhaustion(
                        attempt,
                        FAILURE_TRANSACTION_ATTEMPTS,
                        crate::database::observed_serialization_conflict(&error),
                    );
                    return Err(map_storage_error(error));
                }
            }
        }
        // 循环的最后一次尝试总会返回；这里只为未来改动留下失败关闭的出口，不以 panic 终止 Worker。
        Err(repository_error_owned("结算失败事务重试次数耗尽"))
    })
    .await
}

async fn fail_transaction(
    pool: &TrackedPool<Postgres>,
    claim: &SettlementWorkClaim,
    failure_code: &str,
) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
    // 先取得原 UPDATE 的目标锁，再读取退避起点；仅加锁的竞争者不能耗尽新一轮退避。
    sqlx::query("SELECT id FROM public.auctions WHERE id = $1 FOR UPDATE")
        .bind(claim.auction_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await?;
    let row = sqlx::query(
        r#"WITH retry_clock AS (
               SELECT clock_timestamp() AS observed_at
           )
           UPDATE public.auctions AS auction
           SET settlement_attempt_count = auction.settlement_attempt_count + 1,
               settlement_next_attempt_at = retry_clock.observed_at + CASE
                   WHEN auction.settlement_attempt_count <= 1 THEN interval '5 seconds'
                   WHEN auction.settlement_attempt_count = 2 THEN interval '15 seconds'
                   WHEN auction.settlement_attempt_count = 3 THEN interval '30 seconds'
                   WHEN auction.settlement_attempt_count = 4 THEN interval '1 minute'
                   WHEN auction.settlement_attempt_count = 5 THEN interval '2 minutes'
                   ELSE interval '5 minutes'
               END,
               settlement_last_error_code = $6,
               state_version = auction.state_version + 1,
               updated_at = retry_clock.observed_at
           FROM retry_clock
           WHERE auction.id = $1
             AND auction.status = 'settling'
             AND auction.settlement_workflow_id = $2
             AND auction.settlement_claim_command_id = $3
             AND auction.settlement_complete_command_id IS NULL
             AND auction.settlement_due_at = $4
             AND auction.settlement_next_attempt_at = $5
           RETURNING auction.settlement_attempt_count,
                     auction.settlement_next_attempt_at,
                     auction.state_version,
                     auction.updated_at"#,
    )
    .bind(claim.auction_id.as_uuid())
    .bind(claim.workflow_id)
    .bind(claim.claim_command_id.as_uuid())
    .bind(claim.due_at)
    .bind(claim.lease_until)
    .bind(failure_code)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(sqlx::Error::RowNotFound)?;
    let attempt_count: i32 = row.try_get("settlement_attempt_count")?;
    let next_attempt_at: DateTime<Utc> = row.try_get("settlement_next_attempt_at")?;
    let state_version: i64 = row.try_get("state_version")?;
    let observed_at: DateTime<Utc> = row.try_get("updated_at")?;
    let expected_attempt = claim
        .attempt_count
        .checked_add(1)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| sqlx::Error::Protocol("结算失败尝试次数溢出".to_owned()))?;
    if attempt_count != expected_attempt || state_version <= 0 || next_attempt_at <= observed_at {
        return Err(sqlx::Error::Protocol("结算失败进度结果无效".to_owned()));
    }

    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
                request_id, source, metadata)
           VALUES ($1, $2, 'system', 'auction-worker', 'auction.settlement_retry_scheduled',
                   'auction', $3, 'failure', NULL, 'worker', $4)"#,
    )
    .bind(deterministic_uuid(
        claim.settlement_command_id,
        &format!("settlement-retry-audit:{attempt_count}"),
        1,
    ))
    .bind(observed_at)
    .bind(claim.auction_id.to_string())
    .bind(json!({
        "auction_id": claim.auction_id.to_string(),
        "workflow_id": claim.workflow_id.to_string(),
        "error_code": failure_code,
        "attempt_count": attempt_count,
        "next_attempt_at": next_attempt_at,
        "state_version": state_version,
    }))
    .execute(&mut *transaction)
    .await
    .record_audit_failure()?;
    transaction.commit().await
}
