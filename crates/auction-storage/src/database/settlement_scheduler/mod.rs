//! PostgreSQL 结算 workflow 的持久化领取适配器。
//!
//! 生命周期 Worker 只负责把拍卖推进到 `settling`，结算 Worker 再使用稳定的 execute command
//! 调用 `SettlementCommandPort`。租约复用 `settlement_next_attempt_at` 作为数据库 fence；旧
//! Worker 在租约过期后不能覆盖新 Worker 的失败或完成记录。

use auction_application::{
    ApplicationError, ApplicationFuture, SettlementWorkClaim, SettlementWorkStore,
};
use chrono::{DateTime, Utc};

#[cfg(feature = "postgres")]
use auction_application::RequestHash;
#[cfg(feature = "postgres")]
use auction_domain::{AuctionId, CommandId, deterministic_uuid};
#[cfg(feature = "postgres")]
use serde::Serialize;
#[cfg(feature = "postgres")]
use serde_json::json;
#[cfg(feature = "postgres")]
use uuid::Uuid;

use super::DatabaseStore;

#[cfg(feature = "postgres")]
use sqlx::{PgPool, Row};

impl SettlementWorkStore for DatabaseStore {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<SettlementWorkClaim>> {
        Box::pin(async move {
            if worker_id.is_empty() || worker_id != worker_id.trim() || limit == 0 {
                return Err(repository_error("结算 Worker 领取参数无效"));
            }
            if lease_until <= now {
                return Err(repository_error("结算 Worker 租约必须晚于当前时间"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    claim_postgres(store.pool(), &worker_id, now, lease_until, limit)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "结算 workflow 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn complete(
        &self,
        _claim: SettlementWorkClaim,
        _completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => complete_postgres(store.pool(), &_claim, _completed_at)
                    .await
                    .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "结算 workflow 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn fail(
        &self,
        _claim: SettlementWorkClaim,
        _retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = bounded_text(failure_code, 128);
            let safe_detail = bounded_text(safe_detail, 512);
            if failure_code.is_empty() || safe_detail.is_empty() {
                return Err(repository_error("结算 Worker 失败记录不能为空"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    fail_postgres(store.pool(), &_claim, _retry_at, &failure_code)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "结算 workflow 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

fn repository_error(message: &str) -> ApplicationError {
    auction_application::RepositoryError::Failed(message.to_owned()).into()
}

fn bounded_text(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(feature = "postgres")]
#[derive(Debug, Serialize)]
struct SettlementClaimRequest {
    workflow_id: Uuid,
}

#[cfg(feature = "postgres")]
fn workflow_id(auction_id: Uuid) -> Uuid {
    deterministic_uuid(CommandId::from_uuid(auction_id), "settlement-workflow", 1)
}

#[cfg(feature = "postgres")]
fn claim_command_id(auction_id: Uuid) -> CommandId {
    CommandId::from_uuid(deterministic_uuid(
        CommandId::from_uuid(auction_id),
        "settlement-claim",
        1,
    ))
}

#[cfg(feature = "postgres")]
fn settlement_command_id(claim_command_id: CommandId) -> CommandId {
    CommandId::from_uuid(deterministic_uuid(claim_command_id, "settlement-execute", 1))
}

#[cfg(feature = "postgres")]
async fn claim_postgres(
    pool: &PgPool,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: u16,
) -> Result<Vec<SettlementWorkClaim>, auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let rows = sqlx::query(
        r#"SELECT id, status, state_version, effective_end_at,
                  settlement_workflow_id, settlement_claim_command_id,
                  settlement_attempt_count, settlement_next_attempt_at
           FROM public.auctions
           WHERE (
               status = 'final'
               AND effective_end_at IS NOT NULL
               AND effective_end_at <= $1
               AND settlement_workflow_id IS NULL
           )
           OR (
               status = 'settling'
               AND settlement_workflow_id IS NOT NULL
               AND settlement_claim_command_id IS NOT NULL
               AND settlement_complete_command_id IS NULL
               AND settlement_next_attempt_at IS NOT NULL
               AND settlement_next_attempt_at <= $1
           )
           OR (
               status = 'settled'
               AND settlement_workflow_id IS NOT NULL
               AND settlement_claim_command_id IS NOT NULL
               AND settlement_complete_command_id IS NULL
               AND settlement_next_attempt_at IS NOT NULL
               AND settlement_next_attempt_at <= $1
           )
           ORDER BY COALESCE(effective_end_at, settlement_next_attempt_at), id
           LIMIT $2
           FOR UPDATE SKIP LOCKED"#,
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
        let previous_state_version: i64 =
            row.try_get("state_version").map_err(map_storage_error)?;
        if previous_state_version <= 0 {
            return Err(repository_error_owned("结算拍卖状态版本无效"));
        }
        let (workflow, claim_command, attempt_count, is_initial, completion_only) =
            if status == "final" {
                let workflow = workflow_id(auction_uuid);
                let claim_command = claim_command_id(auction_uuid);
                let request_hash = RequestHash::from_request(
                    "claim_settlement",
                    auction_uuid,
                    &SettlementClaimRequest { workflow_id: workflow },
                )
                .map_err(|_| repository_error_owned("无法生成结算领取命令摘要"))?;
                let expected_hash = request_hash.as_bytes().to_vec();
                let command_row = sqlx::query(
                    r#"INSERT INTO public.auction_commands
                       (command_id, command_kind, aggregate_id, request_hash,
                        result_code, result_payload)
                   VALUES ($1, 'claim_settlement', $2, $3, 'accepted', $4)
                   ON CONFLICT (command_id) DO UPDATE
                       SET command_id = EXCLUDED.command_id
                   RETURNING command_kind, aggregate_id, request_hash, result_code"#,
                )
                .bind(claim_command.as_uuid())
                .bind(auction_uuid)
                .bind(&expected_hash)
                .bind(json!({ "auction_id": auction_uuid, "workflow_id": workflow }))
                .fetch_one(&mut *transaction)
                .await
                .map_err(map_storage_error)?;
                let existing_kind: String =
                    command_row.try_get("command_kind").map_err(map_storage_error)?;
                let existing_aggregate: Uuid =
                    command_row.try_get("aggregate_id").map_err(map_storage_error)?;
                let existing_hash: Vec<u8> =
                    command_row.try_get("request_hash").map_err(map_storage_error)?;
                let existing_result: String =
                    command_row.try_get("result_code").map_err(map_storage_error)?;
                if existing_kind != "claim_settlement"
                    || existing_aggregate != auction_uuid
                    || existing_hash != expected_hash
                    || existing_result != "accepted"
                {
                    return Err(repository_error_owned("结算领取命令账本发生冲突"));
                }
                (workflow, claim_command, 1_u32, true, false)
            } else {
                let workflow: Option<Uuid> =
                    row.try_get("settlement_workflow_id").map_err(map_storage_error)?;
                let workflow =
                    workflow.ok_or_else(|| repository_error_owned("结算 workflow 标识缺失"))?;
                let claim_command_uuid: Option<Uuid> =
                    row.try_get("settlement_claim_command_id").map_err(map_storage_error)?;
                let claim_command_uuid = claim_command_uuid
                    .ok_or_else(|| repository_error_owned("结算领取命令标识缺失"))?;
                let old_attempt_count: i32 =
                    row.try_get("settlement_attempt_count").map_err(map_storage_error)?;
                if old_attempt_count <= 0 {
                    return Err(repository_error_owned("结算尝试次数无效"));
                }
                let completion_only = status == "settled";
                let attempt_count = if completion_only {
                    u32::try_from(old_attempt_count)
                        .map_err(|_| repository_error_owned("结算尝试次数超出范围"))?
                } else {
                    old_attempt_count
                        .checked_add(1)
                        .and_then(|value| u32::try_from(value).ok())
                        .ok_or_else(|| repository_error_owned("结算尝试次数溢出"))?
                };
                (
                    workflow,
                    CommandId::from_uuid(claim_command_uuid),
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
                       settlement_due_at = COALESCE(settlement_due_at, effective_end_at, $4),
                       settlement_started_at = $4, settlement_attempt_count = $5,
                       settlement_next_attempt_at = $6, settlement_last_error_code = NULL,
                       state_version = state_version + 1, updated_at = $4
                   WHERE id = $1 AND status = 'final' AND state_version = $7"#,
            )
            .bind(auction_uuid)
            .bind(workflow)
            .bind(claim_command.as_uuid())
            .bind(now)
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
                   SET settlement_attempt_count = $2, settlement_next_attempt_at = $3,
                       settlement_last_error_code = NULL,
                       state_version = state_version + 1, updated_at = $4
                   WHERE id = $1 AND status = 'settling'
                     AND settlement_workflow_id = $5
                     AND settlement_next_attempt_at <= $4
                     AND settlement_complete_command_id IS NULL"#,
            )
            .bind(auction_uuid)
            .bind(attempt_count_i32)
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
        let settlement_command_id = settlement_command_id(claim_command);
        if is_initial {
            let next_state_version = previous_state_version
                .checked_add(1)
                .ok_or_else(|| repository_error_owned("结算拍卖状态版本溢出"))?;
            insert_settlement_claim_audit(
                &mut transaction,
                auction_uuid,
                claim_command,
                worker_id,
                next_state_version,
            )
            .await
            .map_err(map_storage_error)?;
            insert_settlement_claim_event(
                &mut transaction,
                auction_uuid,
                claim_command,
                next_state_version,
            )
            .await
            .map_err(map_storage_error)?;
        }
        claims.push(SettlementWorkClaim {
            auction_id: AuctionId::from_uuid(auction_uuid),
            workflow_id: workflow,
            settlement_command_id,
            completion_only,
            worker_id: worker_id.to_owned(),
            lease_until,
            attempt_count,
        });
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(claims)
}

#[cfg(feature = "postgres")]
async fn insert_settlement_claim_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    auction_id: Uuid,
    command_id: CommandId,
    worker_id: &str,
    state_version: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, actor_type, actor_id, action, entity_type, entity_id, result,
                request_id, source, metadata)
           VALUES ($1, 'worker', $2, 'settlement.claimed', 'auction', $3, 'success', $4, 'worker', $5)"#,
    )
    .bind(Uuid::new_v4())
    .bind(worker_id)
    .bind(auction_id.to_string())
    .bind(command_id.to_string())
    .bind(json!({ "command_id": command_id, "state_version": state_version }))
    .execute(&mut **transaction)
    .await
    .map(|_| ())
}

#[cfg(feature = "postgres")]
async fn insert_settlement_claim_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    auction_id: Uuid,
    command_id: CommandId,
    state_version: i64,
) -> Result<(), sqlx::Error> {
    let stream_key = format!("auction:{auction_id}");
    sqlx::query(
        "INSERT INTO public.realtime_streams (stream_key) VALUES ($1)
         ON CONFLICT (stream_key) DO NOTHING",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await?;
    let row = sqlx::query(
        r#"UPDATE public.realtime_streams
           SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
           WHERE stream_key = $1
           RETURNING next_sequence - 1 AS stream_sequence"#,
    )
    .bind(&stream_key)
    .fetch_one(&mut **transaction)
    .await?;
    let sequence: i64 = row.try_get("stream_sequence")?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
               (event_id, stream_key, stream_sequence, kind, schema_version,
                public_payload, state_version, origin_command_id)
           VALUES ($1, $2, $3, 'auction.settling', 1, $4, $5, $6)"#,
    )
    .bind(deterministic_uuid(command_id, "settlement-claim-realtime-event", 1))
    .bind(&stream_key)
    .bind(sequence)
    .bind(json!({ "auction_id": auction_id, "state_version": state_version }))
    .bind(state_version)
    .bind(command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map(|_| ())
}

#[cfg(feature = "postgres")]
async fn complete_postgres(
    pool: &PgPool,
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

#[cfg(feature = "postgres")]
async fn fail_postgres(
    pool: &PgPool,
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

#[cfg(feature = "postgres")]
fn map_storage_error(error: sqlx::Error) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(match error {
        sqlx::Error::Database(database) if database.code().as_deref() == Some("40001") => {
            "PostgreSQL 结算 workflow 发生序列化冲突".to_owned()
        }
        _ => "PostgreSQL 结算 workflow 持久化失败".to_owned(),
    })
}

#[cfg(feature = "postgres")]
fn repository_error_owned(message: &str) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(message.to_owned())
}
