//! Operations alert signal 观察事务；Worker 完整闭集与主机侧 CLI 报告共用同一写入语义。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use std::collections::HashSet;

use crate::database::operations_alert::{
    chrono_duration, rollback_operations_alert, storage_error,
};
use auction_application::core::ApplicationError;
use auction_application::operations::OperationsAlertObserveCommand;
use sqlx::Row;

pub(crate) async fn observe_postgres(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    command: &OperationsAlertObserveCommand,
) -> Result<(), ApplicationError> {
    observe_observations(
        pool,
        &command.observations,
        command.now,
        command.batch_window,
        Watermark::Worker,
    )
    .await
}

pub(crate) async fn observe_host_postgres(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    command: &auction_application::operations::OperationsAlertHostObserveCommand,
) -> Result<(), ApplicationError> {
    observe_observations(
        pool,
        &command.observations,
        command.now,
        command.batch_window,
        Watermark::Host,
    )
    .await
}

/// 扫描水位的更新归属：Worker 每轮前进 `last_observed_at`；主机侧报告只写信号，
/// 不改写 Worker 的扫描水位，避免用 CLI 运行掩盖 Worker 已停止扫描。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Watermark {
    Worker,
    Host,
}

async fn observe_observations(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    observations: &[auction_application::operations::OperationsAlertSourceObservation],
    now: chrono::DateTime<chrono::Utc>,
    batch_window: std::time::Duration,
    watermark: Watermark,
) -> Result<(), ApplicationError> {
    let batch_window = chrono_duration(batch_window)?;
    let next_notify_at = now
        .checked_add_signed(batch_window)
        .ok_or_else(|| storage_error("operations alert 首次通知时间溢出"))?;
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "开启 operations alert 观察事务失败"))?;

    if watermark == Watermark::Host {
        // 主机侧报告没有 Worker 的扫描水位可用，必须在事务内确认告警功能仍然启用；
        // 否则会向已经停用的告警表写入无人通知的活动信号。
        let enabled: Option<bool> = sqlx::query_scalar(
            "SELECT enabled
             FROM public.operations_alert_runtime_state
             WHERE singleton_id = 1",
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "读取 operations alert 运行状态失败"))?;
        if enabled != Some(true) {
            return rollback_operations_alert(transaction, "operations alert 运行状态未启用").await;
        }
    }

    for observation in observations {
        let mut fingerprints = HashSet::with_capacity(observation.signals.len());
        for signal in &observation.signals {
            signal.validate().map_err(ApplicationError::from)?;
            if !fingerprints.insert(signal.fingerprint) {
                return rollback_operations_alert(
                    transaction,
                    "operations alert 单轮出现重复 signal",
                )
                .await;
            }
            let state_version: i64 = sqlx::query_scalar(
                "INSERT INTO public.operations_alert_signals
                    (fingerprint, kind, scope, source, active, first_seen_at, last_seen_at,
                     resolved_at, last_notified_at, next_notify_at, next_webhook_at,
                     count_value, age_seconds, webhook_generation, state_version)
                 VALUES ($1, $2, $3, $4, true, $5, $5, NULL, NULL, $6, $6, $7, $8, 1, 1)
                 ON CONFLICT (fingerprint) DO UPDATE SET
                    active = true,
                    last_seen_at = EXCLUDED.last_seen_at,
                    resolved_at = NULL,
                    next_notify_at = COALESCE(
                        operations_alert_signals.next_notify_at, EXCLUDED.next_notify_at
                    ),
                    -- 活动周期内的 webhook 冷却由独立通道维护；null 表示该通道已终结或暂停，
                    -- 重新观察不能把已人工收口的投递重新变成待发送。
                    next_webhook_at = CASE WHEN operations_alert_signals.active
                        THEN operations_alert_signals.next_webhook_at
                        ELSE EXCLUDED.next_webhook_at END,
                    -- 同一活动周期内版本只用于并发/观察；webhook 去重必须绑定 incident generation，
                    -- 否则上一周期遗留的 manual_reconcile 会永久阻断新事件。
                    webhook_generation = CASE WHEN operations_alert_signals.active
                        THEN operations_alert_signals.webhook_generation
                        ELSE operations_alert_signals.webhook_generation + 1 END,
                    count_value = EXCLUDED.count_value,
                    age_seconds = EXCLUDED.age_seconds,
                    -- 告警台只把确认/静默绑定到当前活动周期：已恢复后再次出现属于新事件，
                    -- 不能让上一周期的操作者上下文继续生效（历史仍保留在追加事实表）。
                    acknowledged_at = CASE WHEN operations_alert_signals.active
                        THEN operations_alert_signals.acknowledged_at END,
                    acknowledged_by = CASE WHEN operations_alert_signals.active
                        THEN operations_alert_signals.acknowledged_by END,
                    silenced_until = CASE WHEN operations_alert_signals.active
                        THEN operations_alert_signals.silenced_until END,
                    resolution_notified_at = CASE WHEN operations_alert_signals.active
                        THEN operations_alert_signals.resolution_notified_at END,
                    state_version = operations_alert_signals.state_version + 1
                 WHERE operations_alert_signals.kind = EXCLUDED.kind
                   AND operations_alert_signals.scope = EXCLUDED.scope
                   AND operations_alert_signals.source = EXCLUDED.source
                 RETURNING state_version",
            )
            .bind(signal.fingerprint.as_slice())
            .bind(signal.kind.as_str())
            .bind(&signal.scope)
            .bind(signal.source.as_str())
            .bind(now)
            .bind(next_notify_at)
            .bind(signal.count)
            .bind(signal.age_seconds)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|error| storage_error_from(error, "写入 operations alert signal 失败"))?;
            if state_version <= 0 {
                return rollback_operations_alert(transaction, "operations alert signal 版本无效")
                    .await;
            }
        }

        // 不完整来源仍可追加已知异常，但未出现不等于已恢复，不能关闭历史告警。
        if !observation.available {
            continue;
        }

        let active_rows = sqlx::query(
            "SELECT fingerprint
             FROM public.operations_alert_signals
             WHERE source = $1 AND active
             FOR UPDATE",
        )
        .bind(observation.source.as_str())
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "读取 operations alert 当前 signal 失败"))?;
        let mut resolved = Vec::new();
        for row in active_rows {
            let fingerprint: Vec<u8> = row.try_get("fingerprint").map_err(|error| {
                storage_error_from(error, "读取 operations alert fingerprint 失败")
            })?;
            if !fingerprints.contains(fingerprint.as_slice()) {
                resolved.push(fingerprint);
            }
        }
        if !resolved.is_empty() {
            // 沿用上方完整来源锁集；批量关闭只减少往返，每条 signal 仍各推进一次版本。
            sqlx::query(
                "UPDATE public.operations_alert_signals
                 SET active = false, resolved_at = $2, next_notify_at = NULL,
                     next_webhook_at = NULL,
                     state_version = state_version + 1
                 WHERE fingerprint = ANY($1::bytea[]) AND active",
            )
            .bind(&resolved)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(|error| {
                storage_error_from(error, "关闭已恢复 operations alert signal 失败")
            })?;
        }
    }

    if watermark == Watermark::Worker {
        let runtime_update = sqlx::query(
            "UPDATE public.operations_alert_runtime_state
             SET last_observed_at = GREATEST(COALESCE(last_observed_at, $1), $1)
             WHERE singleton_id = 1 AND enabled",
        )
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "更新 operations alert 扫描水位失败"))?;
        if runtime_update.rows_affected() != 1 {
            return rollback_operations_alert(transaction, "operations alert 运行状态未启用").await;
        }
    }

    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 operations alert 观察事务失败"))
}
