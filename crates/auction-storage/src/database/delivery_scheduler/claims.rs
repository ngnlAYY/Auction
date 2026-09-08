//! delivery outbox due claim 与 claim payload 映射。

use super::mapping::*;

use super::*;

pub(super) async fn claim_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    sending_grace: std::time::Duration,
    excluded_recipients: &[DeliveryRecipientExclusion],
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
    let excluded_worlds =
        excluded_recipients.iter().map(|excluded| excluded.world.as_str()).collect::<Vec<_>>();
    let excluded_names =
        excluded_recipients.iter().map(|excluded| excluded.recipient.as_str()).collect::<Vec<_>>();
    let rows = sqlx::query(concat!(
        r#"SELECT id, command_id, request_digest, origin_kind, origin_id,
                  recipient, payload, fencing_version, attempt_count
           FROM public.deliveries AS deliveries
           WHERE (
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
                 )
             AND "#,
        delivery_completion_gate!(),
        r#"
             AND NOT EXISTS (
                 SELECT 1
                 FROM unnest($3::text[], $4::text[]) AS excluded(world, recipient)
                 WHERE lower(deliveries.payload ->> 'world') = excluded.world
                   AND lower(deliveries.recipient) = excluded.recipient
             )
           ORDER BY created_at, id
           LIMIT $2
           FOR UPDATE SKIP LOCKED"#
    ))
    .bind(now)
    .bind(i64::from(limit))
    .bind(excluded_worlds)
    .bind(excluded_names)
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
        let updated = sqlx::query(concat!(
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
                 )
                 AND "#,
            delivery_completion_gate!()
        ))
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
pub(super) fn map_claim(
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
