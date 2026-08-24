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
use super::delivery_audit::{
    CLAIM, DeliveryAudit, FAIL, OUTCOME_UNKNOWN, RECONCILE_EXPIRED_SENDING, RELEASE, START_SENDING,
    SUCCEED, append as append_delivery_audit, worker_state_metadata,
};

#[cfg(feature = "postgres")]
use sqlx::{Postgres, Row, postgres::PgRow};

#[cfg(feature = "postgres")]
const EXPIRED_SENDING_RECONCILE_LIMIT: i64 = 100;

impl DeliveryWorkStore for DatabaseStore {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        sending_grace: std::time::Duration,
        limit: u16,
    ) -> auction_application::ApplicationFuture<'_, Vec<DeliveryWorkClaim>> {
        Box::pin(async move {
            if worker_id.is_empty()
                || worker_id != worker_id.trim()
                || limit == 0
                || lease_until <= now
                || sending_grace.is_zero()
                || sending_grace > std::time::Duration::from_secs(86_400)
            {
                return Err(repository_error("delivery 领取参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    claim_postgres(store.pool(), &worker_id, now, lease_until, sending_grace, limit)
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

    fn release(
        &self,
        _claim: DeliveryWorkClaim,
        _released_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => release_postgres(store.pool(), &_claim, _released_at)
                    .await
                    .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "delivery outbox 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn renew(
        &self,
        _claim: DeliveryWorkClaim,
        _renewed_at: DateTime<Utc>,
        _lease_until: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if _lease_until <= _renewed_at {
                return Err(repository_error("delivery 续租时间无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    renew_postgres(store.pool(), &_claim, _renewed_at, _lease_until)
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
            let external_reference = error.external_reference.map(|value| bounded_text(value, 512));
            let mut result_metadata = error.result_metadata;
            if failure_code.is_empty()
                || failure_code != failure_code.trim()
                || safe_detail.is_empty()
                || safe_detail != safe_detail.trim()
                || external_reference.as_ref().is_some_and(|value| {
                    value.is_empty() || value != value.trim() || value.chars().any(char::is_control)
                })
                || !result_metadata.is_object()
            {
                return Err(repository_error("delivery 失败结果无效"));
            }
            let Some(metadata) = result_metadata.as_object_mut() else {
                return Err(repository_error("delivery 失败 metadata 必须是 object"));
            };
            metadata.insert("safe_detail".to_owned(), safe_detail.clone().into());
            metadata.insert("outcome_unknown".to_owned(), error.outcome_unknown.into());
            if serde_json::to_vec(&result_metadata)
                .map_err(|_| repository_error("delivery 失败 metadata 无法序列化"))?
                .len()
                > 16 * 1024
            {
                return Err(repository_error("delivery 失败 metadata 超出边界"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => fail_postgres(
                    store.pool(),
                    &_claim,
                    &failure_code,
                    error.outcome_unknown,
                    external_reference.as_deref(),
                    &result_metadata,
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
    pool: &super::TrackedPool<Postgres>,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    sending_grace: std::time::Duration,
    limit: u16,
) -> Result<Vec<DeliveryWorkClaim>, auction_application::RepositoryError> {
    let sending_grace_micros = i64::try_from(sending_grace.as_micros())
        .map_err(|_| repository_error_owned("delivery sending grace 超出数据库范围"))?;
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let expired_rows = sqlx::query(
        r#"WITH expired_sending AS (
               SELECT id
               FROM public.deliveries
               WHERE status = 'sending'
                 AND lease_expires_at + ($2::bigint * interval '1 microsecond') <= $1
               ORDER BY lease_expires_at, id
               LIMIT $3
               FOR UPDATE SKIP LOCKED
           )
           UPDATE public.deliveries AS delivery
           SET status = 'manual_reconcile',
               lease_token = NULL,
               lease_expires_at = NULL,
               completed_at = $1,
               external_reference = NULL,
               failure_code = 'lease_expired_outcome_unknown',
               result_metadata = '{"outcome_unknown":true,"reason":"sending_lease_expired"}'::jsonb,
               fencing_version = delivery.fencing_version + 1,
               updated_at = CURRENT_TIMESTAMP
           FROM expired_sending
           WHERE delivery.id = expired_sending.id
           RETURNING delivery.id, delivery.command_id, delivery.fencing_version,
                     delivery.attempt_count"#,
    )
    .bind(now)
    .bind(sending_grace_micros)
    .bind(EXPIRED_SENDING_RECONCILE_LIMIT)
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    for row in expired_rows {
        let delivery_id: Uuid = row.try_get("id").map_err(map_storage_error)?;
        let command_id: Uuid = row.try_get("command_id").map_err(map_storage_error)?;
        let fencing_version: i64 = row.try_get("fencing_version").map_err(map_storage_error)?;
        let attempt_count: i32 = row.try_get("attempt_count").map_err(map_storage_error)?;
        let request_id = command_id.to_string();
        append_delivery_audit(
            &mut transaction,
            DeliveryAudit {
                occurred_at: now,
                actor_type: "worker",
                actor_id: "delivery-worker",
                action: RECONCILE_EXPIRED_SENDING,
                delivery_id,
                request_id: Some(&request_id),
                source: "worker",
                metadata: serde_json::json!({
                    "status": "manual_reconcile",
                    "failure_code": "lease_expired_outcome_unknown",
                    "fencing_version": fencing_version,
                    "attempt_count": attempt_count,
                }),
            },
        )
        .await
        .map_err(map_storage_error)?;
    }
    let rows = sqlx::query(
        r#"SELECT id, command_id, request_digest, origin_kind, origin_id,
                  recipient, payload, fencing_version, attempt_count
           FROM public.deliveries AS deliveries
           WHERE (
                   status = 'pending'
                   AND (
                       origin_kind <> 'operations_alert'
                       OR EXISTS (
                           SELECT 1
                           FROM public.operations_alert_batches AS alert_batch
                           WHERE alert_batch.delivery_id = deliveries.id
                             AND alert_batch.status = 'enqueued'
                       )
                   )
                 )
              OR (
                   status = 'claimed'
                   AND lease_expires_at <= $1
                   AND (
                       origin_kind <> 'operations_alert'
                       OR EXISTS (
                           SELECT 1
                           FROM public.operations_alert_batches AS alert_batch
                           WHERE alert_batch.delivery_id = deliveries.id
                             AND alert_batch.status = 'enqueued'
                       )
                   )
                 )
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
            r#"UPDATE public.deliveries AS deliveries
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
                 AND (
                       (
                           status = 'pending'
                           AND (
                               origin_kind <> 'operations_alert'
                               OR EXISTS (
                                   SELECT 1
                                   FROM public.operations_alert_batches AS alert_batch
                                   WHERE alert_batch.delivery_id = deliveries.id
                                     AND alert_batch.status = 'enqueued'
                               )
                           )
                       )
                       OR (
                           status = 'claimed'
                           AND lease_expires_at <= $6
                           AND (
                               origin_kind <> 'operations_alert'
                               OR EXISTS (
                                   SELECT 1
                                   FROM public.operations_alert_batches AS alert_batch
                                   WHERE alert_batch.delivery_id = deliveries.id
                                     AND alert_batch.status = 'enqueued'
                               )
                           )
                       )
                 )"#,
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
        let command_id: Uuid = row.try_get("command_id").map_err(map_storage_error)?;
        let request_id = command_id.to_string();
        append_delivery_audit(
            &mut transaction,
            DeliveryAudit {
                occurred_at: now,
                actor_type: "worker",
                actor_id: "delivery-worker",
                action: CLAIM,
                delivery_id,
                request_id: Some(&request_id),
                source: "worker",
                metadata: worker_state_metadata("claimed", fencing_version, attempt_count, None),
            },
        )
        .await
        .map_err(map_storage_error)?;
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
    pool: &super::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    started_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
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
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned("delivery 开始发送时 lease 已失效"));
    }
    let request_id = claim.command_id.to_string();
    append_delivery_audit(
        &mut transaction,
        DeliveryAudit {
            occurred_at: started_at,
            actor_type: "worker",
            actor_id: "delivery-worker",
            action: START_SENDING,
            delivery_id: claim.delivery_id,
            request_id: Some(&request_id),
            source: "worker",
            metadata: worker_state_metadata(
                "sending",
                claim.fencing_version,
                claim.attempt_count,
                None,
            ),
        },
    )
    .await
    .map_err(map_storage_error)?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn release_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    released_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let result = sqlx::query(
        r#"UPDATE public.deliveries
           SET status = 'pending',
               lease_token = NULL,
               lease_expires_at = NULL,
               claimed_at = NULL,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $1
             AND status = 'claimed'
             AND lease_token = $3
             AND fencing_version = $4
             AND lease_expires_at > $2"#,
    )
    .bind(claim.delivery_id)
    .bind(released_at)
    .bind(claim.lease_token)
    .bind(claim.fencing_version)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned("delivery 释放时 lease 已失效"));
    }
    let request_id = claim.command_id.to_string();
    append_delivery_audit(
        &mut transaction,
        DeliveryAudit {
            occurred_at: released_at,
            actor_type: "worker",
            actor_id: "delivery-worker",
            action: RELEASE,
            delivery_id: claim.delivery_id,
            request_id: Some(&request_id),
            source: "worker",
            metadata: worker_state_metadata(
                "pending",
                claim.fencing_version,
                claim.attempt_count,
                None,
            ),
        },
    )
    .await
    .map_err(map_storage_error)?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn renew_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    renewed_at: DateTime<Utc>,
    lease_until: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let result = sqlx::query(
        r#"UPDATE public.deliveries
           SET lease_expires_at = $3,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $1
             AND status = 'sending'
             AND lease_token = $4
             AND fencing_version = $5
             AND lease_expires_at > $2"#,
    )
    .bind(claim.delivery_id)
    .bind(renewed_at)
    .bind(lease_until)
    .bind(claim.lease_token)
    .bind(claim.fencing_version)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned("delivery 续租时 lease 已失效"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn succeed_postgres(
    pool: &super::TrackedPool<Postgres>,
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
        let operation = result
            .result_metadata
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| repository_error_owned("结算 delivery 成功结果缺少 operation"))?;
        match operation {
            // return 的页面动作已经由 parser.prove_return_completed 证明，
            // 可以在同一事务中把对应义务收口为 satisfied。
            "return" => {
                let proof = DeliveryProof {
                    external_reference: &result.external_reference,
                    result_metadata: &result.result_metadata,
                };
                let proof_digest = RequestHash::from_request(
                    "settlement_delivery_proof",
                    claim.delivery_id,
                    &proof,
                )
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
            // send 的 POST 响应最多只能证明请求已被提交。旧版还会读取 Sent
            // 明细和余额来核对远端事实；在该证据接入前必须保持 enqueued，
            // 不能因为 delivery 成功就伪造 satisfied。
            "send" => {
                if result
                    .result_metadata
                    .get("external_reference_proven")
                    .and_then(serde_json::Value::as_bool)
                    .is_none()
                {
                    return Err(repository_error_owned(
                        "结算 send 成功结果缺少 external_reference_proven",
                    ));
                }
            }
            _ => return Err(repository_error_owned("结算 delivery operation 无效")),
        }
    }
    let request_id = claim.command_id.to_string();
    append_delivery_audit(
        &mut transaction,
        DeliveryAudit {
            occurred_at: completed_at,
            actor_type: "worker",
            actor_id: "delivery-worker",
            action: SUCCEED,
            delivery_id: claim.delivery_id,
            request_id: Some(&request_id),
            source: "worker",
            metadata: worker_state_metadata(
                "succeeded",
                claim.fencing_version,
                claim.attempt_count,
                None,
            ),
        },
    )
    .await
    .map_err(map_storage_error)?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn fail_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    failure_code: &str,
    outcome_unknown: bool,
    external_reference: Option<&str>,
    result_metadata: &serde_json::Value,
    completed_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let status = if outcome_unknown { "manual_reconcile" } else { "failed" };
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let updated = sqlx::query(
        r#"UPDATE public.deliveries
           SET status = $2,
               lease_token = NULL,
               lease_expires_at = NULL,
               completed_at = $3,
               external_reference = $5,
               failure_code = $4,
               result_metadata = $6,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $1
             AND status = 'sending'
             AND lease_token = $7
             AND fencing_version = $8
             AND lease_expires_at > $3"#,
    )
    .bind(claim.delivery_id)
    .bind(status)
    .bind(completed_at)
    .bind(failure_code)
    .bind(external_reference)
    .bind(result_metadata)
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
    let request_id = claim.command_id.to_string();
    append_delivery_audit(
        &mut transaction,
        DeliveryAudit {
            occurred_at: completed_at,
            actor_type: "worker",
            actor_id: "delivery-worker",
            action: if outcome_unknown { OUTCOME_UNKNOWN } else { FAIL },
            delivery_id: claim.delivery_id,
            request_id: Some(&request_id),
            source: "worker",
            metadata: worker_state_metadata(
                status,
                claim.fencing_version,
                claim.attempt_count,
                Some(failure_code),
            ),
        },
    )
    .await
    .map_err(map_storage_error)?;
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
