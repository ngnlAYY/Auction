//! Operations alert signal 的 due claim、租约回收和批次映射。
//!
//! claim 使用数据库锁与 fencing identity，过期租约按有限重试次数收口，不能由内存状态替代。

use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn claim_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    request: &OperationsAlertClaimRequest,
) -> Result<Option<OperationsAlertBatchClaim>, ApplicationError> {
    let mut transaction =
        pool.begin().await.map_err(|_| storage_error("开启 operations alert 领取事务失败"))?;

    sqlx::query(
        "UPDATE public.operations_alert_batches
         SET status = 'failed', claim_token = NULL, claimed_at = NULL,
             claim_expires_at = NULL, next_attempt_at = NULL,
             delivery_error_code = 'retry_exhausted', updated_at = $1, completed_at = $1
         WHERE status = 'claimed' AND claim_expires_at <= $1 AND attempt_count >= 3",
    )
    .bind(request.now)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("收口 operations alert 过期重试批次失败"))?;

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
             WHEN 'delivery_manual_reconcile' THEN 3
             WHEN 'fulfillment_manual_reconcile' THEN 4
             WHEN 'recovery_manual_reconcile' THEN 5
             WHEN 'recovery_failed' THEN 6
             WHEN 'fulfillment_failed' THEN 7
             WHEN 'audit_failure' THEN 8
             ELSE 9
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
            ordinal: i32::try_from(index + 1)
                .map_err(|_| storage_error("operations alert 批次序号溢出"))?,
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
async fn reclaim_batch(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &sqlx::postgres::PgRow,
    request: &OperationsAlertClaimRequest,
) -> Result<Option<OperationsAlertBatchClaim>, ApplicationError> {
    let batch_id: Uuid =
        row.try_get("id").map_err(|_| storage_error("读取 operations alert 批次 ID 失败"))?;
    let attempt_count: i32 = row
        .try_get("attempt_count")
        .map_err(|_| storage_error("读取 operations alert 尝试次数失败"))?;
    if attempt_count < 1 {
        return Err(storage_error("operations alert 尝试次数无效"));
    }
    if u32::try_from(attempt_count).ok().is_none_or(|value| value >= MAX_ATTEMPTS) {
        sqlx::query(
            "UPDATE public.operations_alert_batches
             SET status = 'failed', completed_at = $2, updated_at = $2,
                 claim_token = NULL, claimed_at = NULL, claim_expires_at = NULL,
                 next_attempt_at = NULL, delivery_error_code = 'retry_exhausted'
             WHERE id = $1 AND status IN ('pending', 'claimed')",
        )
        .bind(batch_id)
        .bind(request.now)
        .execute(&mut **transaction)
        .await
        .map_err(|_| storage_error("写入 operations alert 重试耗尽状态失败"))?;
        return Ok(None);
    }
    let command_id: Uuid = row
        .try_get("command_id")
        .map_err(|_| storage_error("读取 operations alert 命令 ID 失败"))?;
    let delivery_id: Uuid = row
        .try_get("delivery_id")
        .map_err(|_| storage_error("读取 operations alert delivery ID 失败"))?;
    let request_digest: Vec<u8> = row
        .try_get("request_digest")
        .map_err(|_| storage_error("读取 operations alert 请求摘要失败"))?;
    let body_digest_value: Vec<u8> = row
        .try_get("body_digest")
        .map_err(|_| storage_error("读取 operations alert 正文摘要失败"))?;
    let claim_fencing: i64 = row
        .try_get("claim_fencing")
        .map_err(|_| storage_error("读取 operations alert fencing 失败"))?;
    let delivery_target: Option<(String, serde_json::Value)> =
        sqlx::query_as("SELECT recipient, payload FROM public.deliveries WHERE id = $1")
            .bind(delivery_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(|_| storage_error("读取 operations alert delivery 目标失败"))?;
    let (recipient, payload) =
        delivery_target.ok_or_else(|| storage_error("operations alert delivery 不存在"))?;
    let payload_world = payload.get("world").and_then(serde_json::Value::as_str);
    if recipient != request.target.recipient
        || payload_world != Some(request.target.world_id.as_str())
    {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::IdentityConflict));
    }
    let item_rows = sqlx::query(
        "SELECT fingerprint, signal_version, ordinal, rendered_summary
         FROM public.operations_alert_batch_items
         WHERE batch_id = $1 ORDER BY ordinal",
    )
    .bind(batch_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取 operations alert 批次条目失败"))?;
    let items = map_batch_items(item_rows)?;
    let summary = canonical_summary(&items, auction_application::MAX_OPERATIONS_ALERT_BODY_BYTES)
        .map_err(ApplicationError::from)?;
    let request_hash = request_hash_from_bytes(request_digest)?;
    let stored_body_digest = digest_from_bytes(body_digest_value)?;
    if body_digest(&summary) != stored_body_digest {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt));
    }
    let next_attempt = u32::try_from(attempt_count)
        .map_err(|_| storage_error("operations alert 尝试次数无效"))?
        .checked_add(1)
        .ok_or_else(|| storage_error("operations alert 尝试次数溢出"))?;
    let claim_fencing = claim_fencing
        .checked_add(1)
        .ok_or_else(|| storage_error("operations alert fencing 溢出"))?;
    let claim_token = Uuid::new_v4();
    let updated = sqlx::query(
        "UPDATE public.operations_alert_batches
         SET status = 'claimed', claim_token = $2, claim_fencing = $3,
             claimed_at = $4, claim_expires_at = $5, next_attempt_at = NULL,
             attempt_count = $6, delivery_error_code = '', updated_at = $4
         WHERE id = $1 AND status IN ('pending', 'claimed')",
    )
    .bind(batch_id)
    .bind(claim_token)
    .bind(claim_fencing)
    .bind(request.now)
    .bind(request.lease_until)
    .bind(i32::try_from(next_attempt).map_err(|_| OperationsAlertError::Invalid)?)
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("更新 operations alert 重试租约失败"))?;
    if updated.rows_affected() != 1 {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StaleClaim));
    }
    let claim = OperationsAlertBatchClaim {
        batch_id,
        command_id,
        delivery_id,
        request_hash,
        body_digest: stored_body_digest,
        summary,
        items,
        claim_token,
        claim_fencing,
        lease_until: request.lease_until,
        attempt_count: next_attempt,
    };
    claim
        .validate(auction_application::MAX_OPERATIONS_ALERT_BODY_BYTES)
        .map_err(ApplicationError::from)?;
    Ok(Some(claim))
}

#[cfg(feature = "postgres")]
fn signal_from_row(row: &sqlx::postgres::PgRow) -> Result<OperationsAlertSignal, ApplicationError> {
    let fingerprint: Vec<u8> = row
        .try_get("fingerprint")
        .map_err(|_| storage_error("读取 operations alert signal fingerprint 失败"))?;
    let kind: String =
        row.try_get("kind").map_err(|_| storage_error("读取 operations alert signal 类型失败"))?;
    let scope: String = row
        .try_get("scope")
        .map_err(|_| storage_error("读取 operations alert signal 作用域失败"))?;
    let source: String = row
        .try_get("source")
        .map_err(|_| storage_error("读取 operations alert signal 来源失败"))?;
    let count: i64 = row
        .try_get("count_value")
        .map_err(|_| storage_error("读取 operations alert signal 计数失败"))?;
    let age_seconds: i64 = row
        .try_get("age_seconds")
        .map_err(|_| storage_error("读取 operations alert signal 年龄失败"))?;
    let kind =
        parse_kind(&kind).ok_or_else(|| storage_error("operations alert signal 类型未知"))?;
    let source =
        parse_source(&source).ok_or_else(|| storage_error("operations alert signal 来源未知"))?;
    let signal = OperationsAlertSignal::new(kind, scope, source, count, age_seconds)
        .map_err(ApplicationError::from)?;
    if signal.fingerprint.as_slice() != fingerprint.as_slice() {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt));
    }
    Ok(signal)
}

#[cfg(feature = "postgres")]
fn map_batch_items(
    rows: Vec<sqlx::postgres::PgRow>,
) -> Result<Vec<OperationsAlertBatchItem>, ApplicationError> {
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let fingerprint = digest_from_bytes(
            row.try_get("fingerprint")
                .map_err(|_| storage_error("读取 operations alert 条目 fingerprint 失败"))?,
        )?;
        result.push(OperationsAlertBatchItem {
            fingerprint,
            signal_version: row
                .try_get("signal_version")
                .map_err(|_| storage_error("读取 operations alert 条目版本失败"))?,
            ordinal: row
                .try_get("ordinal")
                .map_err(|_| storage_error("读取 operations alert 条目序号失败"))?,
            summary: row
                .try_get("rendered_summary")
                .map_err(|_| storage_error("读取 operations alert 条目摘要失败"))?,
        });
    }
    if result.is_empty() {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt));
    }
    Ok(result)
}

#[cfg(feature = "postgres")]
fn digest_from_bytes(value: Vec<u8>) -> Result<[u8; 32], ApplicationError> {
    value
        .try_into()
        .map_err(|_| ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt))
}

#[cfg(feature = "postgres")]
fn request_hash_from_bytes(value: Vec<u8>) -> Result<RequestHash, ApplicationError> {
    Ok(RequestHash::new(digest_from_bytes(value)?))
}

#[cfg(feature = "postgres")]
fn parse_kind(value: &str) -> Option<OperationsAlertKind> {
    Some(match value {
        "service_missing" => OperationsAlertKind::ServiceMissing,
        "service_degraded" => OperationsAlertKind::ServiceDegraded,
        "audit_failure" => OperationsAlertKind::AuditFailure,
        "source_unavailable" => OperationsAlertKind::SourceUnavailable,
        "delivery_pending" => OperationsAlertKind::DeliveryPending,
        "delivery_manual_reconcile" => OperationsAlertKind::DeliveryManualReconcile,
        "fulfillment_backlog" => OperationsAlertKind::FulfillmentBacklog,
        "fulfillment_failed" => OperationsAlertKind::FulfillmentFailed,
        "fulfillment_manual_reconcile" => OperationsAlertKind::FulfillmentManualReconcile,
        "recovery_payment_overdue" => OperationsAlertKind::RecoveryPaymentOverdue,
        "recovery_relist_pending" => OperationsAlertKind::RecoveryRelistPending,
        "recovery_failed" => OperationsAlertKind::RecoveryFailed,
        "recovery_manual_reconcile" => OperationsAlertKind::RecoveryManualReconcile,
        "balance_low" => OperationsAlertKind::BalanceLow,
        "evidence_stale" => OperationsAlertKind::EvidenceStale,
        _ => return None,
    })
}

#[cfg(feature = "postgres")]
fn parse_source(value: &str) -> Option<OperationsAlertSource> {
    Some(match value {
        "process" => OperationsAlertSource::Process,
        "delivery" => OperationsAlertSource::Delivery,
        "fulfillment" => OperationsAlertSource::Fulfillment,
        "recovery" => OperationsAlertSource::Recovery,
        _ => return None,
    })
}
