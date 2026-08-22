//! PostgreSQL delivery outbox 的 claim、发送中 fencing 和终态写入适配器。
//!
//! 传输结果只在 claim 的 token/fencing 仍有效时写回。租约过期后的响应不能覆盖新 Worker 的
//! 结果；如果远端结果已经无法确定，调用方必须把记录置为 `manual_reconcile`。

use auction_application::{
    ApplicationError, DeliverySendResult, DeliveryTransportError, DeliveryWorkClaim,
    DeliveryWorkStore,
};
use chrono::{DateTime, Utc};

#[cfg(feature = "postgres")]
use auction_application::RequestHash;
#[cfg(feature = "postgres")]
use serde::Serialize;
#[cfg(feature = "postgres")]
use uuid::Uuid;

use super::DatabaseStore;

#[cfg(feature = "postgres")]
use sqlx::{PgPool, Row, postgres::PgRow};

impl DeliveryWorkStore for DatabaseStore {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> auction_application::ApplicationFuture<'_, Vec<DeliveryWorkClaim>> {
        Box::pin(async move {
            if worker_id.is_empty()
                || worker_id != worker_id.trim()
                || limit == 0
                || lease_until <= now
            {
                return Err(repository_error("delivery 领取参数无效"));
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
                    "delivery outbox 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn start_sending(
        &self,
        _claim: DeliveryWorkClaim,
        _started_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => start_sending_postgres(store.pool(), &_claim, _started_at)
                    .await
                    .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "delivery outbox 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn succeed(
        &self,
        _claim: DeliveryWorkClaim,
        result: DeliverySendResult,
        _completed_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if result.external_reference.is_empty()
                || result.external_reference != result.external_reference.trim()
                || !result.result_metadata.is_object()
            {
                return Err(repository_error("delivery 成功结果无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    succeed_postgres(store.pool(), &_claim, &result, _completed_at)
                        .await
                        .map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "delivery outbox 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn fail(
        &self,
        _claim: DeliveryWorkClaim,
        error: DeliveryTransportError,
        _completed_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = bounded_text(error.failure_code, 64);
            let safe_detail = bounded_text(error.safe_detail, 512);
            if failure_code.is_empty()
                || failure_code != failure_code.trim()
                || safe_detail.is_empty()
                || safe_detail != safe_detail.trim()
            {
                return Err(repository_error("delivery 失败结果无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => fail_postgres(
                    store.pool(),
                    &_claim,
                    &failure_code,
                    &safe_detail,
                    error.outcome_unknown,
                    _completed_at,
                )
                .await
                .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "delivery outbox 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn claim_postgres(
    pool: &PgPool,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: u16,
) -> Result<Vec<DeliveryWorkClaim>, auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    sqlx::query(
        r#"UPDATE public.deliveries
           SET status = 'manual_reconcile',
               lease_token = NULL,
               lease_expires_at = NULL,
               completed_at = $1,
               external_reference = NULL,
               failure_code = 'lease_expired',
               result_metadata = '{"outcome_unknown":true,"reason":"lease_expired"}'::jsonb,
               updated_at = CURRENT_TIMESTAMP
           WHERE status = 'sending'
             AND lease_expires_at <= $1"#,
    )
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    let rows = sqlx::query(
        r#"SELECT id, command_id, request_digest, origin_kind, origin_id,
                  recipient, payload, fencing_version, attempt_count
           FROM public.deliveries
           WHERE status = 'pending'
              OR (status = 'claimed' AND lease_expires_at <= $1)
           ORDER BY created_at, id
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
        let delivery_id: Uuid = row.try_get("id").map_err(map_storage_error)?;
        let lease_token = Uuid::new_v4();
        let old_fencing: i64 = row.try_get("fencing_version").map_err(map_storage_error)?;
        let fencing_version = old_fencing
            .checked_add(1)
            .ok_or_else(|| repository_error_owned("delivery fencing 版本溢出"))?;
        let old_attempt_count: i32 = row.try_get("attempt_count").map_err(map_storage_error)?;
        let attempt_count = old_attempt_count
            .checked_add(1)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| repository_error_owned("delivery 尝试次数溢出"))?;
        let attempt_count_db = i32::try_from(attempt_count)
            .map_err(|_| repository_error_owned("delivery 尝试次数超出数据库范围"))?;
        let updated = sqlx::query(
            r#"UPDATE public.deliveries
               SET status = 'claimed',
                   lease_token = $2,
                   lease_expires_at = $3,
                   fencing_version = $4,
                   attempt_count = $5,
                   claimed_at = $6,
                   sending_at = NULL,
                   completed_at = NULL,
                   external_reference = NULL,
                   failure_code = NULL,
                   result_metadata = '{}'::jsonb,
                   updated_at = CURRENT_TIMESTAMP
               WHERE id = $1
                 AND (status = 'pending' OR (status = 'claimed' AND lease_expires_at <= $6))"#,
        )
        .bind(delivery_id)
        .bind(lease_token)
        .bind(lease_until)
        .bind(fencing_version)
        .bind(attempt_count_db)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;
        if updated.rows_affected() != 1 {
            return Err(repository_error_owned("delivery 领取时状态已变化"));
        }
        claims.push(map_claim(
            &row,
            worker_id,
            lease_token,
            lease_until,
            fencing_version,
            now,
            attempt_count,
        )?);
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(claims)
}

#[cfg(feature = "postgres")]
fn map_claim(
    row: &PgRow,
    worker_id: &str,
    lease_token: Uuid,
    lease_until: DateTime<Utc>,
    fencing_version: i64,
    claimed_at: DateTime<Utc>,
    attempt_count: u32,
) -> Result<DeliveryWorkClaim, auction_application::RepositoryError> {
    let request_digest: Vec<u8> = row.try_get("request_digest").map_err(map_storage_error)?;
    let request_hash = request_hash(request_digest)?;
    let payload: serde_json::Value = row.try_get("payload").map_err(map_storage_error)?;
    if !payload.is_object() {
        return Err(repository_error_owned("delivery payload 不是 JSON 对象"));
    }
    Ok(DeliveryWorkClaim {
        delivery_id: row.try_get("id").map_err(map_storage_error)?,
        command_id: row.try_get("command_id").map_err(map_storage_error)?,
        request_hash,
        origin_kind: row.try_get("origin_kind").map_err(map_storage_error)?,
        origin_id: row.try_get("origin_id").map_err(map_storage_error)?,
        recipient: row.try_get("recipient").map_err(map_storage_error)?,
        payload,
        lease_token,
        lease_until,
        fencing_version,
        attempt_count,
        worker_id: worker_id.to_owned(),
        claimed_at,
    })
}

#[cfg(feature = "postgres")]
async fn start_sending_postgres(
    pool: &PgPool,
    claim: &DeliveryWorkClaim,
    started_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let result = sqlx::query(
        r#"UPDATE public.deliveries
           SET status = 'sending', sending_at = $2, updated_at = CURRENT_TIMESTAMP
           WHERE id = $1
             AND status = 'claimed'
             AND lease_token = $3
             AND fencing_version = $4
             AND lease_expires_at > $2"#,
    )
    .bind(claim.delivery_id)
    .bind(started_at)
    .bind(claim.lease_token)
    .bind(claim.fencing_version)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned("delivery 开始发送时 lease 已失效"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn succeed_postgres(
    pool: &PgPool,
    claim: &DeliveryWorkClaim,
    result: &DeliverySendResult,
    completed_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let updated = sqlx::query(
        r#"UPDATE public.deliveries
           SET status = 'succeeded',
               lease_token = NULL,
               lease_expires_at = NULL,
               completed_at = $2,
               external_reference = $3,
               failure_code = NULL,
               result_metadata = $4,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $1
             AND status = 'sending'
             AND lease_token = $5
             AND fencing_version = $6
             AND lease_expires_at > $2"#,
    )
    .bind(claim.delivery_id)
    .bind(completed_at)
    .bind(&result.external_reference)
    .bind(&result.result_metadata)
    .bind(claim.lease_token)
    .bind(claim.fencing_version)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if updated.rows_affected() != 1 {
        return Err(repository_error_owned("delivery 成功写回时 lease 已失效"));
    }
    if claim.origin_kind == "settlement" {
        let proof = DeliveryProof {
            external_reference: &result.external_reference,
            result_metadata: &result.result_metadata,
        };
        let proof_digest =
            RequestHash::from_request("settlement_delivery_proof", claim.delivery_id, &proof)
                .map_err(|_| repository_error_owned("delivery 成功证明摘要生成失败"))?;
        let updated_fulfillment = sqlx::query(
            r#"UPDATE public.settlement_obligation_fulfillments
               SET execution_state = 'satisfied',
                   claim_token = NULL,
                   claimed_at = NULL,
                   claim_expires_at = NULL,
                   proof_kind = 'delivery_external_reference',
                   proof_reference = $2,
                   proof_digest = $3,
                   proof_observed_at = $4,
                   last_error_code = NULL,
                   completed_at = $4,
                   state_version = state_version + 1,
                   updated_at = CURRENT_TIMESTAMP
               WHERE delivery_id = $1
                 AND execution_state = 'enqueued'"#,
        )
        .bind(claim.delivery_id)
        .bind(&result.external_reference)
        .bind(proof_digest.as_bytes().to_vec())
        .bind(completed_at)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;
        if updated_fulfillment.rows_affected() != 1 {
            return Err(repository_error_owned("delivery 成功但结算履约未处于 enqueued"));
        }
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn fail_postgres(
    pool: &PgPool,
    claim: &DeliveryWorkClaim,
    failure_code: &str,
    safe_detail: &str,
    outcome_unknown: bool,
    completed_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let status = if outcome_unknown { "manual_reconcile" } else { "failed" };
    let metadata = serde_json::json!({
        "safe_detail": safe_detail,
        "outcome_unknown": outcome_unknown,
    });
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let updated = sqlx::query(
        r#"UPDATE public.deliveries
           SET status = $2,
               lease_token = NULL,
               lease_expires_at = NULL,
               completed_at = $3,
               external_reference = NULL,
               failure_code = $4,
               result_metadata = $5,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $1
             AND status = 'sending'
             AND lease_token = $6
             AND fencing_version = $7
             AND lease_expires_at > $3"#,
    )
    .bind(claim.delivery_id)
    .bind(status)
    .bind(completed_at)
    .bind(failure_code)
    .bind(metadata)
    .bind(claim.lease_token)
    .bind(claim.fencing_version)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if updated.rows_affected() != 1 {
        return Err(repository_error_owned("delivery 失败写回时 lease 已失效"));
    }
    if claim.origin_kind == "settlement" {
        let fulfillment_state = if outcome_unknown { "manual_reconcile" } else { "failed" };
        let updated_fulfillment = sqlx::query(
            r#"UPDATE public.settlement_obligation_fulfillments
               SET execution_state = $2,
                   claim_token = NULL,
                   claimed_at = NULL,
                   claim_expires_at = NULL,
                   last_error_code = $3,
                   completed_at = $4,
                   state_version = state_version + 1,
                   updated_at = CURRENT_TIMESTAMP
               WHERE delivery_id = $1
                 AND execution_state = 'enqueued'"#,
        )
        .bind(claim.delivery_id)
        .bind(fulfillment_state)
        .bind(failure_code)
        .bind(completed_at)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;
        if updated_fulfillment.rows_affected() != 1 {
            return Err(repository_error_owned("delivery 失败但结算履约未处于 enqueued"));
        }
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
#[derive(Serialize)]
struct DeliveryProof<'a> {
    external_reference: &'a str,
    result_metadata: &'a serde_json::Value,
}

#[cfg(feature = "postgres")]
fn request_hash(value: Vec<u8>) -> Result<RequestHash, auction_application::RepositoryError> {
    let bytes: [u8; 32] =
        value.try_into().map_err(|_| repository_error_owned("delivery request digest 长度无效"))?;
    if bytes == [0; 32] {
        return Err(repository_error_owned("delivery request digest 不能为零"));
    }
    Ok(RequestHash::new(bytes))
}

fn bounded_text(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn repository_error(message: &str) -> ApplicationError {
    auction_application::RepositoryError::Failed(message.to_owned()).into()
}

#[cfg(feature = "postgres")]
fn repository_error_owned(message: &str) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(message.to_owned())
}

#[cfg(feature = "postgres")]
fn map_storage_error(error: sqlx::Error) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(match error {
        sqlx::Error::RowNotFound => "delivery 记录不存在".to_owned(),
        _ => "delivery 数据库操作失败".to_owned(),
    })
}
