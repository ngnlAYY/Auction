//! PostgreSQL 结算 workflow 的完成与失败收口。

use auction_application::settlement::SettlementWorkClaim;
use auction_domain::deterministic_uuid;
use auction_time::Timestamp;
use serde_json::json;
use sqlx::{Postgres, Row};

use super::map_storage_error;
use crate::database::operations::audit_health::AuditWriteResult as _;
use crate::database::{TrackedPool, repository_error};

pub(super) async fn complete_postgres(
    pool: &TrackedPool<Postgres>,
    claim: &SettlementWorkClaim,
    completed_at: Timestamp,
) -> Result<(), auction_application::core::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    // 先锁定 workflow 行；首写必须持有当前未过期租约，同命令的既有标记仍可安全重放。
    let completion_command_id: Option<uuid::Uuid> = sqlx::query_scalar(
        r#"SELECT settlement_complete_command_id
           FROM public.auctions
           WHERE id = $1 AND status = 'settled' AND settlement_workflow_id = $2
           FOR UPDATE"#,
    )
    .bind(claim.auction_id.as_uuid())
    .bind(claim.workflow_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error("结算 workflow 完成 fence 已失效"))?;
    if completion_command_id == Some(claim.settlement_command_id.as_uuid()) {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(());
    }

    // 在同一行锁下由 PostgreSQL 比较同精度的租约截止，避免 Rust 纳秒值与数据库微秒值失配。
    let result = sqlx::query(
        r#"UPDATE public.auctions
           SET settlement_complete_command_id = $3,
               settlement_next_attempt_at = NULL,
               settlement_last_error_code = NULL,
               updated_at = $4
           WHERE id = $1 AND status = 'settled' AND settlement_workflow_id = $2
             AND settlement_complete_command_id IS NULL
             AND settlement_next_attempt_at = $5
             AND settlement_next_attempt_at > clock_timestamp()"#,
    )
    .bind(claim.auction_id.as_uuid())
    .bind(claim.workflow_id)
    .bind(claim.settlement_command_id.as_uuid())
    .bind(completed_at)
    .bind(claim.lease_until)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        // 领取用 SKIP LOCKED，返回围栏拒绝前须等待回滚释放本行。
        transaction.rollback().await.map_err(map_storage_error)?;
        return Err(repository_error("结算 workflow 完成 fence 已失效"));
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(())
}

/// 失败进度、退避时刻与失败审计同事务写入（不变量 5、8）；退避间隔只由数据库按尝试次数
/// 决定，Worker 不提供重试时刻。`safe_detail` 已由调用方截断并校验非空。
pub(super) async fn fail_postgres(
    pool: &TrackedPool<Postgres>,
    claim: &SettlementWorkClaim,
    failure_code: &str,
    safe_detail: &str,
) -> Result<(), auction_application::core::RepositoryError> {
    // 必须从 BEGIN 开始重放整笔失败进度与审计事务，不能单独重试其中一条写入；
    // 冲突后按数据库契约抖动退避。
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::Settlement,
        crate::database::SERIALIZABLE_ATTEMPTS,
        crate::database::observed_serialization_conflict,
        || fail_transaction(pool, claim, failure_code, safe_detail),
    )
    .await
    .map_err(map_storage_error)
}

async fn fail_transaction(
    pool: &TrackedPool<Postgres>,
    claim: &SettlementWorkClaim,
    failure_code: &str,
    safe_detail: &str,
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
             AND (auction.status = 'settling' OR ($6 = 'task_panic_terminal' AND auction.status = 'settled'))
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
    let next_attempt_at: Timestamp = row.try_get("settlement_next_attempt_at")?;
    let state_version: i64 = row.try_get("state_version")?;
    let observed_at: Timestamp = row.try_get("updated_at")?;
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
           VALUES ($1, $2, 'system', 'auction-worker', $5,
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
        "safe_detail": safe_detail,
        "attempt_count": attempt_count,
        "next_attempt_at": next_attempt_at,
        "state_version": state_version,
        "automatic_retry": failure_code != "task_panic_terminal",
        "manual_required": failure_code == "task_panic_terminal",
    }))
    .bind(if failure_code == "task_panic_terminal" {
        "auction.settlement_panic_blocked"
    } else {
        "auction.settlement_retry_scheduled"
    })
    .execute(&mut *transaction)
    .await
    .record_audit_failure()?;
    transaction.commit().await
}
