//! PostgreSQL 结算 workflow 的 due claim 与租约写入。

use auction_application::SettlementWorkClaim;
use auction_domain::AuctionId;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{Postgres, Row};
use std::time::Duration;
use uuid::Uuid;

use super::{
    map_storage_error as map_repository_error, repository_error_owned as repository_failure,
};
use crate::database::TrackedPool;

mod compatibility;
mod evidence;
mod ids;
mod locking;

use evidence::{insert_settlement_claim_audit, insert_settlement_claim_event};
use ids::{claim_command_id, request_hash, settlement_command_id, workflow_id};
use locking::lock_initial_lots;

const CLAIM_TRANSACTION_ATTEMPTS: usize = 8;
const CLAIM_RETRY_BASE_DELAY: Duration = Duration::from_millis(10);

/// 保留数据库原始错误，直到确认是否属于可安全重放的 SQLSTATE。
///
/// 只有 `40001`（serialization_failure）和 `40P01`（deadlock_detected）允许重跑整笔
/// 领取事务；业务失败和无法证明提交结果的错误都不能通过字符串内容猜测后重试。
enum ClaimError {
    Sqlx(sqlx::Error),
    Repository(auction_application::RepositoryError),
}

impl ClaimError {
    fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Sqlx(sqlx::Error::Database(database))
                if matches!(database.code().as_deref(), Some("40001" | "40P01"))
        )
    }

    fn into_repository(self) -> auction_application::RepositoryError {
        match self {
            Self::Sqlx(error) => map_repository_error(error),
            Self::Repository(error) => error,
        }
    }
}

fn map_storage_error(error: sqlx::Error) -> ClaimError {
    ClaimError::Sqlx(error)
}

fn repository_error_owned(message: &str) -> ClaimError {
    ClaimError::Repository(repository_failure(message))
}

/// 原子领取到的结算工作项只携带稳定 workflow、due 世代和租约，不把 SQL 行泄漏到应用层。
pub(super) async fn claim_postgres(
    pool: &TrackedPool<Postgres>,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: u16,
) -> Result<Vec<SettlementWorkClaim>, auction_application::RepositoryError> {
    for attempt in 1..=CLAIM_TRANSACTION_ATTEMPTS {
        match claim_transaction(pool, worker_id, now, lease_until, limit).await {
            Ok(claims) => return Ok(claims),
            Err(error) if error.is_retryable() && attempt < CLAIM_TRANSACTION_ATTEMPTS => {
                // 与旧版 RunSerializable 一致使用有界线性退避；下一次必须从开启事务开始重放。
                sqlx::__rt::sleep(CLAIM_RETRY_BASE_DELAY.saturating_mul(attempt as u32)).await;
            }
            Err(error) => return Err(error.into_repository()),
        }
    }
    unreachable!("结算领取事务重试循环已由上界保证结束")
}

async fn claim_transaction(
    pool: &TrackedPool<Postgres>,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: u16,
) -> Result<Vec<SettlementWorkClaim>, ClaimError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    // 旧版把 final→settling 领取本身视为 Serializable 业务事务，不能只依赖行锁和
    // SKIP LOCKED；隔离级别必须在首条业务查询前确定。
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;
    let rows = sqlx::query(
        r#"SELECT a.id, a.status, a.state_version,
                  CASE
                      WHEN a.status = 'final' THEN lot_schedule.due_at
                      ELSE a.settlement_due_at
                  END AS due_at,
                  a.settlement_workflow_id, a.settlement_claim_command_id,
                  a.settlement_attempt_count, a.settlement_next_attempt_at
           FROM public.auctions AS a
           LEFT JOIN LATERAL (
               SELECT
                   MAX(l.effective_end_at) AS due_at,
                   COUNT(*)::bigint AS lot_count,
                   COUNT(*) FILTER (
                       WHERE l.status = 'live' AND l.effective_end_at IS NOT NULL
                   )::bigint AS live_lot_count
               FROM public.lots AS l
               WHERE l.auction_id = a.id
           ) AS lot_schedule ON TRUE
           WHERE (
               a.status = 'final'
               AND lot_schedule.lot_count > 0
               AND lot_schedule.live_lot_count = lot_schedule.lot_count
               AND lot_schedule.due_at <= $1
               AND a.settlement_workflow_id IS NULL
               AND a.settlement_claim_command_id IS NULL
               AND a.settlement_complete_command_id IS NULL
           )
           OR (
               a.status = 'settling'
               AND a.settlement_workflow_id IS NOT NULL
               AND a.settlement_claim_command_id IS NOT NULL
               AND a.settlement_complete_command_id IS NULL
               AND a.settlement_due_at IS NOT NULL
               AND a.settlement_due_at <= $1
               AND (
                   a.settlement_next_attempt_at IS NULL
                   OR a.settlement_next_attempt_at <= $1
               )
               AND lot_schedule.lot_count > 0
               AND lot_schedule.live_lot_count = lot_schedule.lot_count
               AND lot_schedule.due_at <= $1
               AND a.settlement_due_at = lot_schedule.due_at
           )
           OR (
               a.status = 'settled'
               AND a.settlement_workflow_id IS NOT NULL
               AND a.settlement_claim_command_id IS NOT NULL
               AND a.settlement_complete_command_id IS NULL
               AND a.settlement_due_at IS NOT NULL
               AND a.settlement_due_at <= $1
               AND a.settlement_next_attempt_at IS NOT NULL
               AND a.settlement_next_attempt_at <= $1
           )
           ORDER BY COALESCE(
               a.settlement_next_attempt_at,
               a.settlement_due_at,
               lot_schedule.due_at
           ), a.id
           LIMIT $2
           FOR UPDATE OF a SKIP LOCKED"#,
    )
    .bind(now)
    .bind(i64::from(limit))
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let auction_uuid: Uuid = row.try_get("id").map_err(map_storage_error)?;
        let status: String = row.try_get("status").map_err(map_storage_error)?;
        let due_at: DateTime<Utc> = row.try_get("due_at").map_err(map_storage_error)?;
        let previous_state_version: i64 =
            row.try_get("state_version").map_err(map_storage_error)?;
        if previous_state_version <= 0 {
            return Err(repository_error_owned("结算拍卖状态版本无效"));
        }
        if status == "final"
            && lock_initial_lots(&mut transaction, auction_uuid, now, due_at)
                .await
                .map_err(map_storage_error)?
                .is_none()
        {
            // 等待 lot 锁期间可能刚好提交了软截止延期；本轮跳过并让下一次扫描读取新世代。
            continue;
        }
        let (workflow, claim_command, attempt_count, is_initial, completion_only) =
            if status == "final" {
                let workflow = workflow_id(auction_uuid, due_at)
                    .ok_or_else(|| repository_error_owned("结算 workflow 时间世代无效"))?;
                let claim_command = claim_command_id(auction_uuid, due_at)
                    .ok_or_else(|| repository_error_owned("结算领取命令时间世代无效"))?;
                let previous_attempt_count: i32 =
                    row.try_get("settlement_attempt_count").map_err(map_storage_error)?;
                if previous_attempt_count < 0 {
                    return Err(repository_error_owned("结算尝试次数无效"));
                }
                let attempt_count = previous_attempt_count
                    .checked_add(1)
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(|| repository_error_owned("结算尝试次数超出安全范围"))?;
                let expected_hash = request_hash(auction_uuid, workflow, due_at)
                    .map_err(|_| repository_error_owned("无法生成结算领取命令摘要"))?;
                let command_row = sqlx::query(
                    r#"INSERT INTO public.auction_commands
                       (command_id, command_kind, aggregate_id, request_hash,
                        result_code, result_payload, created_at)
                   VALUES ($1, 'claim_settlement', $2, $3, 'accepted', $4, $5)
                   ON CONFLICT (command_id) DO UPDATE
                       SET command_id = EXCLUDED.command_id
                   RETURNING command_kind, aggregate_id, request_hash, result_code,
                             result_payload"#,
                )
                .bind(claim_command.as_uuid())
                .bind(auction_uuid)
                .bind(&expected_hash)
                .bind(json!({
                    "auction_id": auction_uuid,
                    "workflow_id": workflow,
                    "due_at": due_at,
                }))
                .bind(due_at)
                .fetch_one(&mut *transaction)
                .await
                .map_err(map_storage_error)?;
                validate_claim_ledger(
                    claim_ledger_record(&command_row)?,
                    auction_uuid,
                    workflow,
                    claim_command,
                    due_at,
                    false,
                )
                .map_err(map_claim_ledger_error)?;
                (workflow, claim_command, attempt_count, true, false)
            } else {
                let workflow: Option<Uuid> =
                    row.try_get("settlement_workflow_id").map_err(map_storage_error)?;
                let workflow =
                    workflow.ok_or_else(|| repository_error_owned("结算 workflow 标识缺失"))?;
                let claim_command_uuid: Option<Uuid> =
                    row.try_get("settlement_claim_command_id").map_err(map_storage_error)?;
                let claim_command_uuid = claim_command_uuid
                    .ok_or_else(|| repository_error_owned("结算领取命令标识缺失"))?;
                let expected_workflow = workflow_id(auction_uuid, due_at)
                    .ok_or_else(|| repository_error_owned("结算 workflow 时间世代无效"))?;
                let expected_claim_command = claim_command_id(auction_uuid, due_at)
                    .ok_or_else(|| repository_error_owned("结算领取命令时间世代无效"))?;
                if workflow != expected_workflow
                    || claim_command_uuid != expected_claim_command.as_uuid()
                {
                    return Err(repository_error_owned("结算 workflow 命令世代不一致"));
                }
                let command_row = sqlx::query(
                    r#"SELECT command_kind, aggregate_id, request_hash, result_code,
                              result_payload
                       FROM public.auction_commands
                       WHERE command_id = $1
                       FOR UPDATE"#,
                )
                .bind(claim_command_uuid)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(map_storage_error)?
                .ok_or_else(|| repository_error_owned("结算领取命令账本缺失"))?;
                validate_claim_ledger(
                    claim_ledger_record(&command_row)?,
                    auction_uuid,
                    workflow,
                    expected_claim_command,
                    due_at,
                    true,
                )
                .map_err(map_claim_ledger_error)?;
                let old_attempt_count: i32 =
                    row.try_get("settlement_attempt_count").map_err(map_storage_error)?;
                if old_attempt_count <= 0 {
                    return Err(repository_error_owned("结算尝试次数无效"));
                }
                let completion_only = status == "settled";
                let attempt_count = u32::try_from(old_attempt_count)
                    .map_err(|_| repository_error_owned("结算尝试次数超出范围"))?;
                (
                    workflow,
                    auction_domain::CommandId::from_uuid(claim_command_uuid),
                    attempt_count,
                    false,
                    completion_only,
                )
            };
        let attempt_count_i32 = i32::try_from(attempt_count)
            .map_err(|_| repository_error_owned("结算尝试次数超出数据库范围"))?;
        let updated = if is_initial {
            sqlx::query(
                r#"UPDATE public.auctions
                   SET status = 'settling', settlement_workflow_id = $2,
                       settlement_claim_command_id = $3,
                       settlement_due_at = $4,
                       settlement_started_at = $4, settlement_attempt_count = $6,
                       settlement_next_attempt_at = $7, settlement_last_error_code = NULL,
                       state_version = state_version + 1, updated_at = $5
                   WHERE id = $1 AND status = 'final' AND state_version = $8"#,
            )
            .bind(auction_uuid)
            .bind(workflow)
            .bind(claim_command.as_uuid())
            .bind(due_at)
            // 旧版 Worker 将冻结的 DueAt 作为领取事实时间；now 仅用于 due/租约判断。
            .bind(due_at)
            .bind(attempt_count_i32)
            .bind(lease_until)
            .bind(previous_state_version)
            .execute(&mut *transaction)
            .await
        } else if completion_only {
            sqlx::query(
                r#"UPDATE public.auctions
                   SET settlement_next_attempt_at = $2, updated_at = $3
                   WHERE id = $1 AND status = 'settled'
                     AND settlement_workflow_id = $4
                     AND settlement_next_attempt_at <= $3
                     AND settlement_complete_command_id IS NULL"#,
            )
            .bind(auction_uuid)
            .bind(lease_until)
            .bind(now)
            .bind(workflow)
            .execute(&mut *transaction)
            .await
        } else {
            sqlx::query(
                r#"UPDATE public.auctions
                   SET settlement_next_attempt_at = $2,
                       settlement_last_error_code = NULL,
                       updated_at = $3
                   WHERE id = $1 AND status = 'settling'
                     AND settlement_workflow_id = $4
                     AND (
                         settlement_next_attempt_at IS NULL
                         OR settlement_next_attempt_at <= $3
                     )
                     AND settlement_complete_command_id IS NULL"#,
            )
            .bind(auction_uuid)
            .bind(lease_until)
            .bind(now)
            .bind(workflow)
            .execute(&mut *transaction)
            .await
        }
        .map_err(map_storage_error)?;
        if updated.rows_affected() != 1 {
            return Err(repository_error_owned("结算 workflow 领取时状态已变化"));
        }
        let settlement_command_id = settlement_command_id(auction_uuid, due_at)
            .ok_or_else(|| repository_error_owned("结算完成命令时间世代无效"))?;
        if is_initial {
            let next_state_version = previous_state_version
                .checked_add(1)
                .ok_or_else(|| repository_error_owned("结算拍卖状态版本溢出"))?;
            insert_settlement_claim_audit(
                &mut transaction,
                auction_uuid,
                claim_command,
                next_state_version,
                due_at,
            )
            .await
            .map_err(map_storage_error)?;
            insert_settlement_claim_event(
                &mut transaction,
                auction_uuid,
                claim_command,
                next_state_version,
                due_at,
            )
            .await
            .map_err(map_storage_error)?;
        }
        claims.push(SettlementWorkClaim {
            auction_id: AuctionId::from_uuid(auction_uuid),
            workflow_id: workflow,
            claim_command_id: claim_command,
            settlement_command_id,
            due_at,
            completion_only,
            worker_id: worker_id.to_owned(),
            lease_until,
            attempt_count,
        });
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(claims)
}

fn claim_ledger_record(row: &sqlx::postgres::PgRow) -> Result<ClaimLedgerRecord, ClaimError> {
    Ok(ClaimLedgerRecord {
        command_kind: row.try_get("command_kind").map_err(map_storage_error)?,
        aggregate_id: row.try_get("aggregate_id").map_err(map_storage_error)?,
        request_hash: row.try_get("request_hash").map_err(map_storage_error)?,
        result_code: row.try_get("result_code").map_err(map_storage_error)?,
        result_payload: row.try_get::<Value, _>("result_payload").map_err(map_storage_error)?,
    })
}

fn map_claim_ledger_error(error: ClaimLedgerValidationError) -> ClaimError {
    match error {
        ClaimLedgerValidationError::Conflict => repository_error_owned("结算领取命令账本发生冲突"),
        ClaimLedgerValidationError::Corrupt => repository_error_owned("结算领取命令账本结果损坏"),
    }
}
use compatibility::{ClaimLedgerRecord, ClaimLedgerValidationError, validate_claim_ledger};
