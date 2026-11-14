//! Operations alert 健康安全投影读取；只返回固定计数、时间和错误码，不暴露 signal 正文。

use auction_application::core::ApplicationError;
use auction_application::operations::{OperationsAlertHealth, OperationsAlertRuntimeStatus};
use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::database::operations::alert::storage_error;
use crate::database::storage_error_from;

pub(super) async fn publish_runtime_status_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    status: OperationsAlertRuntimeStatus,
) -> Result<(), ApplicationError> {
    let result = sqlx::query(
        "INSERT INTO public.operations_alert_runtime_state
            (singleton_id, enabled, last_observed_at, updated_at)
         VALUES (1, $1, NULL, $2)
         ON CONFLICT (singleton_id) DO UPDATE SET
            enabled = EXCLUDED.enabled,
            last_observed_at = CASE
                WHEN EXCLUDED.enabled AND operations_alert_runtime_state.enabled
                    THEN operations_alert_runtime_state.last_observed_at
                ELSE NULL
            END,
            updated_at = EXCLUDED.updated_at
         WHERE operations_alert_runtime_state.updated_at <= EXCLUDED.updated_at",
    )
    .bind(status.enabled)
    .bind(status.updated_at)
    .execute(pool)
    .await
    .map_err(|error| storage_error_from(error, "发布 operations alert 运行状态失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("operations alert 运行状态发布时间倒退"));
    }
    Ok(())
}

pub(super) async fn health_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
) -> Result<OperationsAlertHealth, ApplicationError> {
    let runtime_row = sqlx::query(
        "SELECT enabled, last_observed_at
         FROM public.operations_alert_runtime_state
         WHERE singleton_id = 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert 运行状态失败"))?;
    let Some(runtime_row) = runtime_row else {
        return Ok(OperationsAlertHealth::disabled());
    };
    let enabled: bool = runtime_row
        .try_get("enabled")
        .map_err(|error| storage_error_from(error, "读取 operations alert 启用状态失败"))?;
    if !enabled {
        return Ok(OperationsAlertHealth::disabled());
    }
    let last_observed_at: Option<DateTime<Utc>> = runtime_row
        .try_get("last_observed_at")
        .map_err(|error| storage_error_from(error, "读取 operations alert 扫描水位失败"))?;
    let signal_row = sqlx::query(
        "SELECT
            COUNT(*) FILTER (WHERE active) AS active_signal_count,
            COUNT(*) FILTER (WHERE active AND next_notify_at <= CURRENT_TIMESTAMP)
                AS due_signal_count
         FROM public.operations_alert_signals",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert signal 健康失败"))?;
    let batch_row = sqlx::query(
        "SELECT
            COUNT(*) FILTER (WHERE status = 'claimed') AS claimed_batch_count,
            COUNT(*) FILTER (WHERE status = 'succeeded') AS succeeded_batch_count,
            COUNT(*) FILTER (WHERE status = 'failed') AS failed_batch_count,
            COUNT(*) FILTER (WHERE status = 'manual_reconcile') AS manual_reconcile_count,
            MAX(completed_at) FILTER (WHERE status = 'succeeded') AS last_success_at,
            MAX(completed_at) FILTER (WHERE status IN ('failed', 'manual_reconcile'))
                AS last_failure_at
         FROM public.operations_alert_batches",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert 批次健康失败"))?;
    let webhook_row = sqlx::query(
        "SELECT
            COUNT(*) FILTER (WHERE status = 'pending') AS pending_count,
            COUNT(*) FILTER (WHERE status = 'sending') AS sending_count,
            COUNT(*) FILTER (WHERE status = 'failed') AS failed_count,
            COUNT(*) FILTER (WHERE status = 'manual_reconcile') AS manual_reconcile_count
         FROM public.operations_alert_webhook_deliveries",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 webhook 健康失败"))?;
    let last_error_code: Option<String> = sqlx::query_scalar(
        "SELECT delivery_error_code
         FROM public.operations_alert_batches
         WHERE status IN ('failed', 'manual_reconcile')
         ORDER BY completed_at DESC, id DESC
         LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert 最近错误失败"))?;
    let health = OperationsAlertHealth {
        enabled,
        active_signal_count: signal_row
            .try_get("active_signal_count")
            .map_err(|error| storage_error_from(error, "读取 operations alert 活跃计数失败"))?,
        due_signal_count: signal_row
            .try_get("due_signal_count")
            .map_err(|error| storage_error_from(error, "读取 operations alert due 计数失败"))?,
        claimed_batch_count: batch_row
            .try_get("claimed_batch_count")
            .map_err(|error| storage_error_from(error, "读取 operations alert claim 计数失败"))?,
        succeeded_batch_count: batch_row
            .try_get("succeeded_batch_count")
            .map_err(|error| storage_error_from(error, "读取 operations alert 成功计数失败"))?,
        failed_batch_count: batch_row
            .try_get("failed_batch_count")
            .map_err(|error| storage_error_from(error, "读取 operations alert 失败计数失败"))?,
        manual_reconcile_count: batch_row
            .try_get("manual_reconcile_count")
            .map_err(|error| storage_error_from(error, "读取 operations alert 人工对账计数失败"))?,
        webhook_pending_count: webhook_row
            .try_get("pending_count")
            .map_err(|error| storage_error_from(error, "读取 webhook 待重试计数失败"))?,
        webhook_sending_count: webhook_row
            .try_get("sending_count")
            .map_err(|error| storage_error_from(error, "读取 webhook 发送中计数失败"))?,
        webhook_failed_count: webhook_row
            .try_get("failed_count")
            .map_err(|error| storage_error_from(error, "读取 webhook 失败计数失败"))?,
        webhook_manual_reconcile_count: webhook_row
            .try_get("manual_reconcile_count")
            .map_err(|error| storage_error_from(error, "读取 webhook 人工对账计数失败"))?,
        last_observed_at,
        last_success_at: batch_row
            .try_get("last_success_at")
            .map_err(|error| storage_error_from(error, "读取 operations alert 最近成功时间失败"))?,
        last_failure_at: batch_row
            .try_get("last_failure_at")
            .map_err(|error| storage_error_from(error, "读取 operations alert 最近失败时间失败"))?,
        last_error_code: last_error_code.unwrap_or_default(),
    };
    health.validate().map_err(ApplicationError::from).map(|_| health)
}
