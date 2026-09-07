//! Operations alert 新批次的 due claim 事务。

use super::super::*;
use super::mapping::signal_from_row;
use super::reclaim::reclaim_batch;
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn claim_postgres(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    request: &OperationsAlertClaimRequest,
) -> Result<Option<OperationsAlertBatchClaim>, ApplicationError> {
    let mut transaction =
        pool.begin().await.map_err(|_| storage_error("开启 operations alert 领取事务失败"))?;

    let expired_rows = sqlx::query(
        "SELECT id, delivery_id
         FROM public.operations_alert_batches
         WHERE status = 'claimed' AND claim_expires_at <= $1 AND attempt_count >= 3
         ORDER BY claim_expires_at, id
         LIMIT 100
         FOR UPDATE SKIP LOCKED",
    )
    .bind(request.now)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 operations alert 过期重试批次失败"))?;
    for row in expired_rows {
        let batch_id: Uuid = row
            .try_get("id")
            .map_err(|_| storage_error("读取 operations alert 过期批次 ID 失败"))?;
        let delivery_id: Uuid = row
            .try_get("delivery_id")
            .map_err(|_| storage_error("读取 operations alert 过期 delivery ID 失败"))?;
        sqlx::query(
            "UPDATE public.operations_alert_batches
             SET status = 'failed', claim_token = NULL, claimed_at = NULL,
                 claim_expires_at = NULL, next_attempt_at = NULL,
                 delivery_error_code = 'retry_exhausted', updated_at = $2, completed_at = $2
             WHERE id = $1 AND status = 'claimed' AND attempt_count >= 3",
        )
        .bind(batch_id)
        .bind(request.now)
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("收口 operations alert 过期重试批次失败"))?;
        terminalize_prepared_delivery(
            &mut transaction,
            delivery_id,
            request.now,
            "retry_exhausted",
        )
        .await?;
    }

    let orphan_rows = sqlx::query(
        "SELECT batch.delivery_id, batch.delivery_error_code
         FROM public.operations_alert_batches AS batch
         INNER JOIN public.deliveries AS delivery ON delivery.id = batch.delivery_id
         WHERE batch.status = 'failed' AND delivery.origin_kind = 'operations_alert'
           AND delivery.status = 'prepared'
           AND batch.delivery_error_code IN ('dependency_mismatch', 'retry_exhausted')
           AND delivery.fencing_version = 0 AND delivery.attempt_count = 0
           AND delivery.lease_token IS NULL AND delivery.lease_expires_at IS NULL
           AND delivery.claimed_at IS NULL AND delivery.sending_at IS NULL
           AND delivery.completed_at IS NULL AND delivery.external_reference IS NULL
           AND delivery.failure_code IS NULL AND delivery.activation_command_id IS NULL
           AND delivery.activation_request_digest IS NULL AND delivery.activated_at IS NULL
         ORDER BY batch.updated_at, batch.id
         LIMIT 100
         FOR UPDATE OF batch SKIP LOCKED",
    )
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 operations alert prepared 孤儿失败"))?;
    for row in orphan_rows {
        let delivery_id: Uuid = row
            .try_get("delivery_id")
            .map_err(|_| storage_error("读取 operations alert 孤儿 delivery ID 失败"))?;
        let failure_code: String = row
            .try_get("delivery_error_code")
            .map_err(|_| storage_error("读取 operations alert 孤儿批次错误码失败"))?;
        terminalize_prepared_delivery(&mut transaction, delivery_id, request.now, &failure_code)
            .await?;
    }

    let reclaim_row = sqlx::query(
        "SELECT id, command_id, delivery_id, request_digest, body_digest,
                claim_fencing, attempt_count, status
         FROM public.operations_alert_batches
         WHERE (status = 'pending' AND next_attempt_at IS NOT NULL AND next_attempt_at <= $1)
            OR (status = 'claimed' AND claim_expires_at IS NOT NULL
                AND claim_expires_at <= $1 AND attempt_count < 3)
         ORDER BY created_at, id
         LIMIT 1
         FOR UPDATE SKIP LOCKED",
    )
    .bind(request.now)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 operations alert 重试批次失败"))?;
    if let Some(row) = reclaim_row
        && let Some(claim) = reclaim_batch(&mut transaction, &row, request).await?
    {
        transaction
            .commit()
            .await
            .map_err(|_| storage_error("提交 operations alert 重试领取事务失败"))?;
        return Ok(Some(claim));
    }

    let rows = sqlx::query(
        "SELECT signal.fingerprint, signal.kind, signal.scope, signal.source,
                signal.count_value, signal.age_seconds, signal.state_version
         FROM public.operations_alert_signals AS signal
         WHERE signal.active
           AND signal.next_notify_at <= $1
           AND NOT EXISTS (
               SELECT 1
               FROM public.operations_alert_batch_items AS item
               INNER JOIN public.operations_alert_batches AS batch
                 ON batch.id = item.batch_id
               WHERE item.fingerprint = signal.fingerprint
                 AND batch.status IN ('pending', 'claimed')
           )
         ORDER BY CASE signal.kind
             WHEN 'source_unavailable' THEN 1
             WHEN 'service_missing' THEN 2
             WHEN 'delivery_returned_mail' THEN 3
             WHEN 'delivery_manual_reconcile' THEN 4
             WHEN 'fulfillment_manual_reconcile' THEN 5
             WHEN 'recovery_manual_reconcile' THEN 6
             WHEN 'recovery_failed' THEN 7
             WHEN 'fulfillment_failed' THEN 8
             WHEN 'audit_failure' THEN 9
             ELSE 10
         END, signal.kind, signal.scope, signal.fingerprint
         LIMIT $2
         FOR UPDATE OF signal SKIP LOCKED",
    )
    .bind(request.now)
    .bind(i64::try_from(request.max_items).map_err(|_| OperationsAlertError::Invalid)?)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 operations alert due signal 失败"))?;
    if rows.is_empty() {
        transaction
            .commit()
            .await
            .map_err(|_| storage_error("提交 operations alert 空领取事务失败"))?;
        return Ok(None);
    }

    let mut items = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let signal = signal_from_row(row)?;
        items.push(OperationsAlertBatchItem {
            fingerprint: signal.fingerprint,
            signal_version: row
                .try_get("state_version")
                .map_err(|_| storage_error("读取 operations alert signal 版本失败"))?,
            ordinal: i32::try_from(index)
                .ok()
                .and_then(|index| index.checked_add(1))
                .ok_or_else(|| storage_error("operations alert 批次序号溢出"))?,
            summary: signal.render_summary().map_err(ApplicationError::from)?,
        });
    }
    let summary = loop {
        match canonical_summary(&items, request.max_body_bytes) {
            Ok(value) => break value,
            Err(error) if items.len() > 1 => {
                items.pop();
                let _ = error;
            }
            Err(error) => return Err(error.into()),
        }
    };
    let batch_id = Uuid::new_v4();
    let command_id = Uuid::new_v4();
    let delivery_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/mail-delivery-v1/{command_id}").as_bytes(),
    );
    let request_hash = request_hash(batch_id, delivery_id, command_id, &request.target, &summary)?;
    let claim_token = Uuid::new_v4();
    let body_digest = body_digest(&summary);
    let external_mail_id = format!("operations-alert:{batch_id}");
    let origin_id = format!("operations_alert:{batch_id}");
    let payload = json!({
        "schema_version": 1,
        "operation": "send",
        "world": request.target.world_id,
        "external_mail_id": external_mail_id,
        "template": "operations_alert",
        "result_code": "operations_alert",
        "user_key_user_id": null,
        "credits": 0,
        "cod": 0,
        "attachment_ids": [],
        "body": summary,
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient,
             payload, status, created_at, updated_at)
         VALUES ($1, $2, $3, 'operations_alert', $4, $5, $6, 'prepared', $7, $7)",
    )
    .bind(delivery_id)
    .bind(command_id)
    .bind(request_hash.as_bytes().to_vec())
    .bind(&origin_id)
    .bind(&request.target.recipient)
    .bind(payload)
    .bind(request.now)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("写入 operations alert delivery 失败"))?;
    sqlx::query(
        "INSERT INTO public.operations_alert_batches
            (id, command_id, delivery_id, request_digest, status, claim_token,
             claim_fencing, claimed_at, claim_expires_at, next_attempt_at,
             attempt_count, item_count, body_digest, delivery_error_code,
             created_at, updated_at, completed_at)
         VALUES ($1, $2, $3, $4, 'claimed', $5, 1, $6, $7, NULL,
                 1, $8, $9, '', $6, $6, NULL)",
    )
    .bind(batch_id)
    .bind(command_id)
    .bind(delivery_id)
    .bind(request_hash.as_bytes().to_vec())
    .bind(claim_token)
    .bind(request.now)
    .bind(request.lease_until)
    .bind(i32::try_from(items.len()).map_err(|_| OperationsAlertError::Invalid)? as i32)
    .bind(body_digest.to_vec())
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("写入 operations alert 批次失败"))?;
    for item in &items {
        sqlx::query(
            "INSERT INTO public.operations_alert_batch_items
                (batch_id, fingerprint, signal_version, ordinal, rendered_summary)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(batch_id)
        .bind(item.fingerprint.to_vec())
        .bind(item.signal_version)
        .bind(item.ordinal)
        .bind(&item.summary)
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("写入 operations alert 批次条目失败"))?;
    }
    let request_id = command_id.to_string();
    append_delivery_audit(
        &mut transaction,
        DeliveryAudit {
            occurred_at: request.now,
            actor_type: "system",
            actor_id: "delivery-service",
            action: PREPARE,
            delivery_id,
            request_id: Some(&request_id),
            source: "system",
            metadata: json!({
                "origin_kind": "operations_alert",
                "status": "prepared",
            }),
        },
    )
    .await
    .map_err(|_| storage_error("追加 operations alert delivery 准备审计失败"))?;
    transaction.commit().await.map_err(|_| storage_error("提交 operations alert 领取事务失败"))?;
    Ok(Some(OperationsAlertBatchClaim {
        batch_id,
        command_id,
        delivery_id,
        request_hash,
        body_digest,
        summary,
        items,
        claim_token,
        claim_fencing: 1,
        lease_until: request.lease_until,
        attempt_count: 1,
    }))
}
