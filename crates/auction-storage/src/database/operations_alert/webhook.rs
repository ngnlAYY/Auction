//! Webhook 告警独立持久账本的领取与终态；不读写 MoogleMail delivery/冷却。

use auction_application::core::ApplicationError;
use auction_application::operations::{
    OperationsAlertKind, OperationsAlertWebhookClaimRequest, OperationsAlertWebhookDispatch,
    OperationsAlertWebhookFinish, OperationsAlertWebhookOutcome, render_webhook_payload,
};
use chrono::Utc;
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use super::common::storage_error;
use crate::database::{TrackedPool, storage_error_from};

fn webhook_kind(value: &str) -> Result<OperationsAlertKind, ApplicationError> {
    OperationsAlertKind::parse(value).ok_or_else(|| storage_error("webhook 告警种类无效"))
}

fn payload_for(row: &sqlx::postgres::PgRow) -> Result<serde_json::Value, ApplicationError> {
    row.try_get("payload").map_err(|error| storage_error_from(error, "读取 webhook 冻结载荷失败"))
}

pub(crate) async fn claim_due_webhook_postgres(
    pool: &TrackedPool<sqlx::Postgres>,
    request: &OperationsAlertWebhookClaimRequest,
) -> Result<Option<OperationsAlertWebhookDispatch>, ApplicationError> {
    let now = request.now;
    let lease_until = request.lease_until;
    if lease_until <= now {
        return Err(storage_error("webhook claim lease 无效"));
    }
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "开启 webhook 领取事务失败"))?;

    // 已进入 sending 但租约过期的投递结果未知，必须人工核对，不能自动重发。
    sqlx::query(
        "UPDATE public.operations_alert_webhook_deliveries
         SET status = 'manual_reconcile', claim_token = NULL, claim_expires_at = NULL,
             completed_at = $1, failure_code = 'outcome_unknown',
             updated_at = $1
         WHERE status = 'sending' AND claim_expires_at <= $1",
    )
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "收口过期 webhook 投递失败"))?;

    let pending = sqlx::query(
        "SELECT delivery.id, delivery.command_id, delivery.fingerprint, delivery.payload,
                delivery.claim_fencing, delivery.attempt_count, signal.kind
         FROM public.operations_alert_webhook_deliveries AS delivery
         JOIN public.operations_alert_signals AS signal
           ON signal.fingerprint = delivery.fingerprint
         WHERE delivery.status = 'pending' AND delivery.next_attempt_at <= $1
         ORDER BY delivery.next_attempt_at, delivery.created_at, delivery.id
         LIMIT 1
         FOR UPDATE OF delivery SKIP LOCKED",
    )
    .bind(now)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 webhook 待投递失败"))?;

    let dispatch = if let Some(row) = pending {
        let id: Uuid =
            row.try_get("id").map_err(|error| storage_error_from(error, "webhook id"))?;
        let command_id: Uuid = row
            .try_get("command_id")
            .map_err(|error| storage_error_from(error, "webhook command id"))?;
        let claim_token = Uuid::new_v4();
        let fencing: i64 = row
            .try_get::<i64, _>("claim_fencing")
            .map_err(|error| storage_error_from(error, "webhook fencing"))?
            .checked_add(1)
            .ok_or_else(|| storage_error("webhook fencing 溢出"))?;
        let payload = payload_for(&row)?;
        let previous_attempts: i32 = row
            .try_get("attempt_count")
            .map_err(|error| storage_error_from(error, "webhook attempt_count"))?;
        let attempt_count = u32::try_from(previous_attempts)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| storage_error("webhook attempt_count 溢出"))?;
        let kind_text: String =
            row.try_get("kind").map_err(|error| storage_error_from(error, "webhook kind"))?;
        let kind = webhook_kind(&kind_text)?;
        let updated = sqlx::query(
            "UPDATE public.operations_alert_webhook_deliveries
             SET status = 'sending', claim_token = $2, claim_expires_at = $3,
                 claim_fencing = $4, attempt_count = attempt_count + 1,
                 next_attempt_at = NULL, sending_at = $1, updated_at = $1
             WHERE id = $5 AND status = 'pending'",
        )
        .bind(now)
        .bind(claim_token)
        .bind(lease_until)
        .bind(fencing)
        .bind(id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "领取 webhook 投递失败"))?;
        if updated.rows_affected() != 1 {
            let _ = transaction.rollback().await;
            return Err(storage_error("webhook 投递领取冲突"));
        }
        Some(OperationsAlertWebhookDispatch {
            delivery_id: id,
            command_id,
            claim_token,
            claim_fencing: fencing,
            attempt_count,
            kind,
            payload,
        })
    } else {
        let signal = sqlx::query(
            "SELECT signal.fingerprint, signal.kind, signal.scope, signal.source,
                    signal.count_value, signal.age_seconds, signal.state_version,
                    signal.webhook_generation
             FROM public.operations_alert_signals AS signal
             WHERE signal.active
               AND signal.next_webhook_at IS NOT NULL
               AND signal.next_webhook_at <= $1
               -- 静默只抑制新的重复通知；已存在的 pending 重试仍按自身状态处理。
               AND (signal.silenced_until IS NULL OR signal.silenced_until <= $1)
               AND NOT EXISTS (
                   SELECT 1 FROM public.operations_alert_webhook_deliveries AS delivery
                   WHERE delivery.fingerprint = signal.fingerprint
                     AND delivery.signal_generation = signal.webhook_generation
                     AND delivery.status IN ('pending', 'sending', 'manual_reconcile')
               )
             ORDER BY signal.next_webhook_at, signal.fingerprint
             LIMIT 1
             FOR UPDATE OF signal SKIP LOCKED",
        )
        .bind(now)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "读取 webhook 到期 signal 失败"))?;
        let Some(row) = signal else {
            transaction
                .commit()
                .await
                .map_err(|error| storage_error_from(error, "提交空 webhook 领取事务失败"))?;
            return Ok(None);
        };
        let fingerprint: Vec<u8> = row
            .try_get("fingerprint")
            .map_err(|error| storage_error_from(error, "webhook fingerprint"))?;
        let kind_text: String =
            row.try_get("kind").map_err(|error| storage_error_from(error, "webhook kind"))?;
        let kind = webhook_kind(&kind_text)?;
        let scope: String =
            row.try_get("scope").map_err(|error| storage_error_from(error, "webhook scope"))?;
        let source: String =
            row.try_get("source").map_err(|error| storage_error_from(error, "webhook source"))?;
        let count: i64 = row
            .try_get("count_value")
            .map_err(|error| storage_error_from(error, "webhook count"))?;
        let age: i64 =
            row.try_get("age_seconds").map_err(|error| storage_error_from(error, "webhook age"))?;
        let signal_version: i64 = row
            .try_get("state_version")
            .map_err(|error| storage_error_from(error, "webhook version"))?;
        let signal_generation: i64 = row
            .try_get("webhook_generation")
            .map_err(|error| storage_error_from(error, "webhook generation"))?;
        if count < 0 || age < 0 || signal_version <= 0 || signal_generation <= 0 {
            let _ = transaction.rollback().await;
            return Err(storage_error("webhook signal 投影无效"));
        }
        let payload = render_webhook_payload(&request.template, kind, &scope, &source, count, age)
            .map_err(ApplicationError::from)?;
        let payload_bytes = serde_json::to_vec(&payload).map_err(|error| {
            drop(error);
            storage_error("webhook 载荷序列化失败")
        })?;
        let request_hash = Sha256::digest(&payload_bytes);
        let id = Uuid::new_v4();
        let command_id = Uuid::new_v4();
        let claim_token = Uuid::new_v4();
        let inserted = sqlx::query(
            "INSERT INTO public.operations_alert_webhook_deliveries
                (id, command_id, fingerprint, signal_version, signal_generation,
                 request_hash, payload, status, claim_token, claim_expires_at,
                 claim_fencing, attempt_count, sending_at, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'sending', $8, $9, 1, 1, $10, $10, $10)
             ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(command_id)
        .bind(&fingerprint)
        .bind(signal_version)
        .bind(signal_generation)
        .bind(request_hash.as_slice())
        .bind(&payload)
        .bind(claim_token)
        .bind(lease_until)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "写入 webhook 投递失败"))?;
        if inserted.rows_affected() != 1 {
            let _ = transaction.rollback().await;
            return Err(storage_error("webhook 投递已存在"));
        }
        sqlx::query(
            "UPDATE public.operations_alert_signals
             SET next_webhook_at = NULL, state_version = state_version + 1
             WHERE fingerprint = $1 AND active",
        )
        .bind(&fingerprint)
        .execute(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "消费 webhook 冷却时间失败"))?;
        Some(OperationsAlertWebhookDispatch {
            delivery_id: id,
            command_id,
            claim_token,
            claim_fencing: 1,
            attempt_count: 1,
            kind,
            payload,
        })
    };

    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 webhook 领取事务失败"))?;
    Ok(dispatch)
}

pub(crate) async fn finish_webhook_postgres(
    pool: &TrackedPool<sqlx::Postgres>,
    dispatch: &OperationsAlertWebhookDispatch,
    finish: OperationsAlertWebhookFinish,
) -> Result<(), ApplicationError> {
    finish.validate().map_err(|_: auction_application::operations::OperationsAlertError| {
        storage_error("webhook 终态时间无效")
    })?;
    let (status, next_attempt_at, completed_at, failure_code) = match finish.outcome {
        OperationsAlertWebhookOutcome::Succeeded => ("succeeded", None, Some(Utc::now()), None),
        OperationsAlertWebhookOutcome::Retryable => ("pending", finish.next_webhook_at, None, None),
        OperationsAlertWebhookOutcome::Permanent => {
            ("failed", None, Some(Utc::now()), Some("permanent"))
        }
        OperationsAlertWebhookOutcome::RetryExhausted => {
            ("failed", None, Some(Utc::now()), Some("retry_exhausted"))
        }
        OperationsAlertWebhookOutcome::Unknown => {
            ("manual_reconcile", None, Some(Utc::now()), Some("outcome_unknown"))
        }
    };
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "开启 webhook 终态事务失败"))?;
    let updated = sqlx::query(
        "UPDATE public.operations_alert_webhook_deliveries
         SET status = $2, next_attempt_at = $3, completed_at = $4, failure_code = $5,
             claim_token = NULL, claim_expires_at = NULL, sending_at = NULL, updated_at = $6
         WHERE id = $1 AND command_id = $7 AND claim_token = $8 AND claim_fencing = $9
           AND status = 'sending'",
    )
    .bind(dispatch.delivery_id)
    .bind(status)
    .bind(next_attempt_at)
    .bind(completed_at)
    .bind(failure_code)
    .bind(Utc::now())
    .bind(dispatch.command_id)
    .bind(dispatch.claim_token)
    .bind(dispatch.claim_fencing)
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入 webhook 终态失败"))?;
    if updated.rows_affected() != 1 {
        let _ = transaction.rollback().await;
        return Err(storage_error("webhook 终态租约失效"));
    }
    let signal_next = match finish.outcome {
        OperationsAlertWebhookOutcome::Succeeded | OperationsAlertWebhookOutcome::Retryable => {
            finish.next_webhook_at
        }
        OperationsAlertWebhookOutcome::Permanent
        | OperationsAlertWebhookOutcome::RetryExhausted
        | OperationsAlertWebhookOutcome::Unknown => None,
    };
    sqlx::query(
        "UPDATE public.operations_alert_signals
         SET next_webhook_at = $2, state_version = state_version + 1
         WHERE fingerprint = (SELECT fingerprint FROM public.operations_alert_webhook_deliveries WHERE id=$1)
           AND active
           -- 旧周期投递在新周期重激活后才完成时，不能改写新周期的 next_webhook_at。
           AND webhook_generation = (
               SELECT signal_generation FROM public.operations_alert_webhook_deliveries WHERE id=$1
           )",
    )
    .bind(dispatch.delivery_id)
    .bind(signal_next)
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "更新 webhook signal 冷却失败"))?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 webhook 终态事务失败"))?;
    Ok(())
}
