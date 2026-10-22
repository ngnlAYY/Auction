//! Operations alert signal 观察事务。

use super::super::*;

pub(crate) async fn observe_postgres(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    command: &OperationsAlertObserveCommand,
) -> Result<(), ApplicationError> {
    let batch_window = chrono_duration(command.batch_window)?;
    let next_notify_at = command
        .now
        .checked_add_signed(batch_window)
        .ok_or_else(|| storage_error("operations alert 首次通知时间溢出"))?;
    let mut transaction =
        pool.begin().await.map_err(|_| storage_error("开启 operations alert 观察事务失败"))?;

    for observation in &command.observations {
        let mut fingerprints = Vec::with_capacity(observation.signals.len());
        for signal in &observation.signals {
            signal.validate().map_err(ApplicationError::from)?;
            if fingerprints.contains(&signal.fingerprint) {
                return rollback_operations_alert(
                    transaction,
                    "operations alert 单轮出现重复 signal",
                )
                .await;
            }
            fingerprints.push(signal.fingerprint);
            let state_version: i64 = sqlx::query_scalar(
                "INSERT INTO public.operations_alert_signals
                    (fingerprint, kind, scope, source, active, first_seen_at, last_seen_at,
                     resolved_at, last_notified_at, next_notify_at, count_value, age_seconds,
                     state_version)
                 VALUES ($1, $2, $3, $4, true, $5, $5, NULL, NULL, $6, $7, $8, 1)
                 ON CONFLICT (fingerprint) DO UPDATE SET
                    active = true,
                    last_seen_at = EXCLUDED.last_seen_at,
                    resolved_at = NULL,
                    next_notify_at = COALESCE(
                        operations_alert_signals.next_notify_at, EXCLUDED.next_notify_at
                    ),
                    count_value = EXCLUDED.count_value,
                    age_seconds = EXCLUDED.age_seconds,
                    state_version = operations_alert_signals.state_version + 1
                 WHERE operations_alert_signals.kind = EXCLUDED.kind
                   AND operations_alert_signals.scope = EXCLUDED.scope
                   AND operations_alert_signals.source = EXCLUDED.source
                 RETURNING state_version",
            )
            .bind(signal.fingerprint.to_vec())
            .bind(signal.kind.as_str())
            .bind(&signal.scope)
            .bind(signal.source.as_str())
            .bind(command.now)
            .bind(next_notify_at)
            .bind(signal.count)
            .bind(signal.age_seconds)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| storage_error("写入 operations alert signal 失败"))?;
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
        .map_err(|_| storage_error("读取 operations alert 当前 signal 失败"))?;
        for row in active_rows {
            let fingerprint: Vec<u8> = row
                .try_get("fingerprint")
                .map_err(|_| storage_error("读取 operations alert fingerprint 失败"))?;
            if !fingerprints.iter().any(|value| value.as_slice() == fingerprint.as_slice()) {
                sqlx::query(
                    "UPDATE public.operations_alert_signals
                     SET active = false, resolved_at = $2, next_notify_at = NULL,
                         state_version = state_version + 1
                     WHERE fingerprint = $1 AND active",
                )
                .bind(fingerprint)
                .bind(command.now)
                .execute(&mut *transaction)
                .await
                .map_err(|_| storage_error("关闭已恢复 operations alert signal 失败"))?;
            }
        }
    }

    let runtime_update = sqlx::query(
        "UPDATE public.operations_alert_runtime_state
         SET last_observed_at = GREATEST(COALESCE(last_observed_at, $1), $1)
         WHERE singleton_id = 1 AND enabled",
    )
    .bind(command.now)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("更新 operations alert 扫描水位失败"))?;
    if runtime_update.rows_affected() != 1 {
        return rollback_operations_alert(transaction, "operations alert 运行状态未启用").await;
    }

    transaction.commit().await.map_err(|_| storage_error("提交 operations alert 观察事务失败"))
}
