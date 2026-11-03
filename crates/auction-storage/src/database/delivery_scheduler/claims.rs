//! delivery outbox due claim 与 claim payload 映射。

use super::mapping::*;

use super::*;

pub(super) async fn claim_postgres(
    store: &super::super::PostgresStore,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    sending_grace: std::time::Duration,
    exclusions: &DeliveryClaimExclusions,
    limit: u16,
) -> Result<Vec<DeliveryWorkClaim>, auction_application::RepositoryError> {
    store.pool().observe_transaction(
        auction_observability::DatabaseTransactionOwner::DeliveryClaim,
        async {
    let pool = store.pool();
    let intervals = serde_json::to_value(store.delivery_send_intervals)
        .map_err(|_| repository_error_owned("delivery 发送间隔无法序列化"))?;
    let sending_grace_micros = i64::try_from(sending_grace.as_micros())
        .map_err(|_| repository_error_owned("delivery sending grace 超出数据库范围"))?;
    // 过期结果单独短事务：资金scope/account必须在delivery前，不能批量先锁delivery。
    super::expired::reconcile(pool, now, sending_grace_micros).await?;
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    super::quarantine::quarantine_mail_dispositions(&mut transaction, now).await?;
    super::watch::retire_invalidated(&mut transaction, now).await?;
    let excluded_worlds =
        exclusions.recipients.iter().map(|excluded| excluded.world.as_str()).collect::<Vec<_>>();
    let excluded_names = exclusions
        .recipients
        .iter()
        .map(|excluded| excluded.recipient.as_str())
        .collect::<Vec<_>>();
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
             AND NOT EXISTS (
                 SELECT 1
                 FROM unnest($5::text[]) AS excluded(world)
                 WHERE lower(deliveries.payload ->> 'world') = excluded.world
             )
             AND NOT EXISTS (
                 SELECT 1
                 FROM unnest($6::text[]) AS excluded(world)
                 WHERE deliveries.payload ->> 'operation' = 'send'
                   AND lower(deliveries.payload ->> 'world') = excluded.world
             )
             AND NOT EXISTS (
                 SELECT 1 FROM public.delivery_send_cadence AS cadence
                 WHERE cadence.category = public.delivery_send_category(deliveries.origin_kind, deliveries.payload)
                   AND cadence.last_started_at + (($7 ->> (cadence.category || '_seconds'))::bigint * interval '1 second') > $1
             )
           ORDER BY CASE WHEN origin_kind = 'lot_watch' THEN 0 ELSE 1 END, created_at, id
           LIMIT $2
           FOR UPDATE SKIP LOCKED"#
    ))
    .bind(now)
    .bind(i64::from(limit))
    .bind(excluded_worlds)
    .bind(excluded_names)
    .bind(&exclusions.unavailable_worlds)
    .bind(&exclusions.unavailable_send_worlds)
    .bind(intervals)
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
        // 自动领取只清理上次结果证明；维护重排次数属于跨尝试预算，不能随领取归零。
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
                   result_metadata = CASE WHEN result_metadata ? 'sweep_retries'
                       THEN jsonb_build_object('sweep_retries', result_metadata -> 'sweep_retries')
                       ELSE '{}'::jsonb END,
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

        },
    ).await
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
