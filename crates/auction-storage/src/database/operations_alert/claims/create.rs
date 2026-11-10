//! Operations alert 新批次的 due claim 事务。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use super::mapping::signal_from_row;
use super::reclaim::reclaim_batch;
use crate::database::delivery_audit::{DeliveryAudit, PREPARE, append as append_delivery_audit};
use crate::database::operations_alert::delivery::terminalize_prepared_delivery;
use crate::database::operations_alert::storage_error;
use auction_application::core::ApplicationError;
use auction_application::operations::{
    OperationsAlertBatchClaim, OperationsAlertBatchItem, OperationsAlertClaimRequest,
    OperationsAlertError, OperationsAlertKind, OperationsAlertSeverity, body_digest,
    canonical_summary_prefix, request_hash,
};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn claim_postgres(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    notifications: &dyn auction_application::notification_metrics::NotificationRecorder,
    request: &OperationsAlertClaimRequest,
) -> Result<Option<OperationsAlertBatchClaim>, ApplicationError> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "开启 operations alert 领取事务失败"))?;

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
    .map_err(|error| storage_error_from(error, "读取 operations alert 过期重试批次失败"))?;
    for row in expired_rows {
        let batch_id: Uuid = row
            .try_get("id")
            .map_err(|error| storage_error_from(error, "读取 operations alert 过期批次 ID 失败"))?;
        let delivery_id: Uuid = row.try_get("delivery_id").map_err(|error| {
            storage_error_from(error, "读取 operations alert 过期 delivery ID 失败")
        })?;
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
        .map_err(|error| storage_error_from(error, "收口 operations alert 过期重试批次失败"))?;
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
    .map_err(|error| storage_error_from(error, "读取 operations alert prepared 孤儿失败"))?;
    for row in orphan_rows {
        let delivery_id: Uuid = row.try_get("delivery_id").map_err(|error| {
            storage_error_from(error, "读取 operations alert 孤儿 delivery ID 失败")
        })?;
        let failure_code: String = row.try_get("delivery_error_code").map_err(|error| {
            storage_error_from(error, "读取 operations alert 孤儿批次错误码失败")
        })?;
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
    .map_err(|error| storage_error_from(error, "读取 operations alert 重试批次失败"))?;
    if let Some(row) = reclaim_row
        && let Some(claim) = reclaim_batch(&mut transaction, &row, request).await?
    {
        transaction
            .commit()
            .await
            .map_err(|error| storage_error_from(error, "提交 operations alert 重试领取事务失败"))?;
        return Ok(Some(claim));
    }

    // 决策 66：紧急信号沿用冷却重复通知；每日摘要类信号只在 24 小时窗口内合并为一批。
    // 摘要批次不再区分单条信号节奏，避免同一故障按不同到期时间各发一封。
    let digest_kinds: Vec<&str> = OperationsAlertKind::ALL
        .into_iter()
        .filter(|kind| kind.severity() == OperationsAlertSeverity::Digest)
        .map(OperationsAlertKind::as_str)
        .collect();
    let max_items = i64::try_from(request.max_items).map_err(|error| {
        let _ = error;
        OperationsAlertError::Invalid
    })?;
    let urgent_rows = sqlx::query(
        "SELECT signal.fingerprint, signal.kind, signal.scope, signal.source,
                signal.count_value, signal.age_seconds, signal.state_version
         FROM public.operations_alert_signals AS signal
         WHERE signal.active
           AND signal.next_notify_at <= $1
           AND signal.kind <> ALL($3::text[])
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
    .bind(max_items)
    .bind(&digest_kinds)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert due signal 失败"))?;

    let mut resolved_rows = Vec::new();
    if urgent_rows.is_empty() {
        // 摘要窗口：最近 24 小时内已有摘要批次入队/成功时，本轮不再生成第二封摘要。
        let digest_due = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                 SELECT 1
                 FROM public.operations_alert_batches AS batch
                 INNER JOIN public.operations_alert_batch_items AS item
                   ON item.batch_id = batch.id
                 INNER JOIN public.operations_alert_signals AS signal
                   ON signal.fingerprint = item.fingerprint
                 WHERE batch.status IN ('enqueued', 'succeeded')
                   AND batch.created_at > $1 - interval '24 hours'
                   AND signal.kind = ANY($2::text[])
             )",
        )
        .bind(request.now)
        .bind(&digest_kinds)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "读取 operations alert 摘要窗口失败"))?;
        if !digest_due {
            let digest_rows = sqlx::query(
                "SELECT signal.fingerprint, signal.kind, signal.scope, signal.source,
                        signal.count_value, signal.age_seconds, signal.state_version
                 FROM public.operations_alert_signals AS signal
                 WHERE signal.active
                   AND signal.next_notify_at <= $1
                   AND signal.kind = ANY($3::text[])
                   AND NOT EXISTS (
                       SELECT 1
                       FROM public.operations_alert_batch_items AS item
                       INNER JOIN public.operations_alert_batches AS batch
                         ON batch.id = item.batch_id
                       WHERE item.fingerprint = signal.fingerprint
                         AND batch.status IN ('pending', 'claimed')
                   )
                 ORDER BY signal.kind, signal.scope, signal.fingerprint
                 LIMIT $2
                 FOR UPDATE OF signal SKIP LOCKED",
            )
            .bind(request.now)
            .bind(max_items)
            .bind(&digest_kinds)
            .fetch_all(&mut *transaction)
            .await
            .map_err(|error| storage_error_from(error, "读取 operations alert 摘要 signal 失败"))?;
            let remaining = request.max_items.saturating_sub(digest_rows.len());
            if remaining > 0 {
                // 已恢复项只在生成摘要批次时附带一次；领取即标记，避免批次失败后同一
                // 恢复项在后续摘要重复出现（恢复本身仍完整保留在告警台历史）。
                resolved_rows = sqlx::query(
                    "SELECT fingerprint, kind, scope, source, state_version, resolved_at
                     FROM public.operations_alert_signals
                     WHERE NOT active
                       AND resolved_at IS NOT NULL
                       AND resolution_notified_at IS NULL
                       AND kind = ANY($2::text[])
                       AND resolved_at > $1 - interval '24 hours'
                     ORDER BY resolved_at, fingerprint
                     LIMIT $3
                     FOR UPDATE SKIP LOCKED",
                )
                .bind(request.now)
                .bind(&digest_kinds)
                .bind(i64::try_from(remaining).map_err(|error| {
                    let _ = error;
                    OperationsAlertError::Invalid
                })?)
                .fetch_all(&mut *transaction)
                .await
                .map_err(|error| {
                    storage_error_from(error, "读取 operations alert 已恢复 signal 失败")
                })?;
            }
            let mut lines = digest_rows;
            let digest_row_start = lines.len();
            lines.append(&mut resolved_rows);
            let rows = lines;
            if rows.is_empty() {
                transaction.commit().await.map_err(|error| {
                    storage_error_from(error, "提交 operations alert 空领取事务失败")
                })?;
                return Ok(None);
            }
            let claim = create_batch(&mut transaction, request, rows, digest_row_start).await?;
            transaction.commit().await.map_err(|error| {
                storage_error_from(error, "提交 operations alert 摘要领取事务失败")
            })?;
            observe_created(notifications, claim.as_ref());
            return Ok(claim);
        }
    }
    if urgent_rows.is_empty() {
        transaction
            .commit()
            .await
            .map_err(|error| storage_error_from(error, "提交 operations alert 空领取事务失败"))?;
        return Ok(None);
    }
    // 紧急批次全部是活动信号：已恢复项的起始下标必须等于行数，否则每行都会落进
    // “已恢复：…”分支，把仍在发生的故障误报为恢复。
    let resolved_from = urgent_rows.len();
    let claim = create_batch(&mut transaction, request, urgent_rows, resolved_from).await?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 operations alert 领取事务失败"))?;
    observe_created(notifications, claim.as_ref());
    Ok(claim)
}

/// 新批次在提交后才计一次 `operations_alert` 创建；重新领取已有批次不新建投递，不计。
fn observe_created(
    notifications: &dyn auction_application::notification_metrics::NotificationRecorder,
    claim: Option<&OperationsAlertBatchClaim>,
) {
    if claim.is_some() {
        notifications.observe_stage(
            auction_application::notification_metrics::NotificationKind::OperationsAlert,
            auction_application::notification_metrics::NotificationStage::Created,
        );
    }
}

/// 从已锁定的 signal 行创建批次；`resolved_from` 之后的行是已恢复摘要项。
async fn create_batch(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    request: &OperationsAlertClaimRequest,
    rows: Vec<sqlx::postgres::PgRow>,
    resolved_from: usize,
) -> Result<Option<OperationsAlertBatchClaim>, ApplicationError> {
    let mut items = Vec::with_capacity(rows.len());
    let mut resolved_fingerprints = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let signal_version: i64 = row
            .try_get("state_version")
            .map_err(|error| storage_error_from(error, "读取 operations alert signal 版本失败"))?;
        let fingerprint: Vec<u8> = row.try_get("fingerprint").map_err(|error| {
            storage_error_from(error, "读取 operations alert signal fingerprint 失败")
        })?;
        let summary = if index < resolved_from {
            signal_from_row(row)?.render_summary().map_err(ApplicationError::from)?
        } else {
            // 已恢复项只出现在下一封摘要，正文明确标注恢复，不触发新的阈值评估。
            let kind: String = row
                .try_get("kind")
                .map_err(|error| storage_error_from(error, "读取已恢复告警种类失败"))?;
            let scope: String = row
                .try_get("scope")
                .map_err(|error| storage_error_from(error, "读取已恢复告警范围失败"))?;
            let source: String = row
                .try_get("source")
                .map_err(|error| storage_error_from(error, "读取已恢复告警来源失败"))?;
            let anchor = auction_application::operations::OperationsAlertSource::parse(&source)
                .ok_or(ApplicationError::OperationsAlert(
                    OperationsAlertError::StoredResultCorrupt,
                ))?
                .runbook_anchor();
            let summary = format!("已恢复：{kind} {scope}；处置手册：{anchor}");
            if !auction_domain::valid_text(&summary, 512) {
                return Err(ApplicationError::OperationsAlert(
                    OperationsAlertError::StoredResultCorrupt,
                ));
            }
            resolved_fingerprints.push(fingerprint.clone());
            summary
        };
        items.push(OperationsAlertBatchItem {
            fingerprint: fingerprint.try_into().map_err(|_| {
                ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt)
            })?,
            signal_version,
            ordinal: i32::try_from(index)
                .ok()
                .and_then(|index| index.checked_add(1))
                .ok_or_else(|| storage_error("operations alert 批次序号溢出"))?,
            summary,
        });
    }
    let (item_count, summary) =
        canonical_summary_prefix(&items, request.max_body_bytes).map_err(ApplicationError::from)?;
    items.truncate(item_count);
    // 正文前缀截断可能丢弃尾部（通常是已恢复项）；只为真正进入批次的行标记已通知，
    // 其余保留到下一封摘要，避免恢复提示丢失。
    resolved_fingerprints.retain(|fingerprint| {
        items.iter().any(|item| item.fingerprint.as_slice() == fingerprint.as_slice())
    });
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
    .bind(request_hash.as_bytes().as_slice())
    .bind(&origin_id)
    .bind(&request.target.recipient)
    .bind(payload)
    .bind(request.now)
    .execute(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入 operations alert delivery 失败"))?;
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
    .bind(request_hash.as_bytes().as_slice())
    .bind(claim_token)
    .bind(request.now)
    .bind(request.lease_until)
    .bind(i32::try_from(items.len()).map_err(|error| {
        let _ = error;
        OperationsAlertError::Invalid
    })?)
    .bind(body_digest.as_slice())
    .execute(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入 operations alert 批次失败"))?;
    // 正文前缀已保证 1–64 项，最多 320 个参数；与 batch/delivery 同事务追加。
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
        "INSERT INTO public.operations_alert_batch_items
            (batch_id, fingerprint, signal_version, ordinal, rendered_summary) ",
    );
    query.push_values(&items, |mut values, item| {
        values
            .push_bind(batch_id)
            .push_bind(item.fingerprint.as_slice())
            .push_bind(item.signal_version)
            .push_bind(item.ordinal)
            .push_bind(&item.summary);
    });
    query
        .build()
        .persistent(false)
        .execute(&mut **transaction)
        .await
        .map_err(|error| storage_error_from(error, "写入 operations alert 批次条目失败"))?;
    if !resolved_fingerprints.is_empty() {
        sqlx::query(
            "UPDATE public.operations_alert_signals
             SET resolution_notified_at = $2
             WHERE fingerprint = ANY($1::bytea[]) AND NOT active
               AND resolution_notified_at IS NULL",
        )
        .bind(&resolved_fingerprints)
        .bind(request.now)
        .execute(&mut **transaction)
        .await
        .map_err(|error| storage_error_from(error, "标记 operations alert 已恢复项进入摘要失败"))?;
    }
    let request_id = command_id.to_string();
    append_delivery_audit(
        &mut *transaction,
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
    .map_err(|error| storage_error_from(error, "追加 operations alert delivery 准备审计失败"))?;
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
