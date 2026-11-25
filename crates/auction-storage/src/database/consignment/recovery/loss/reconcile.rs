//! “无法查清”分支的核对案：按 position 持久的只追加记录，加一条运维告警信号作为提醒（修订裁定）。
//!
//! 保护不变量 4、6、8：核对记录（`consignment_loss_reconciles`）与被拒的确认命令同事务写入，不迁移任何
//! 状态，不决定金额、资格或占用；同一 position 每次无法查清各有一条。告警信号只是提醒，可被 Worker 的
//! 观察收口，核对案不会随之消失。信号写法沿 `operations_alert_signals` 的观察事务：同一 fingerprint 重复出现只累加计数，已恢复后再次出现
//! 是新周期（webhook 代次加一，清空上周期的确认与静默）。signal 只含固定的 kind / scope / source 与计数，
//! 不含任何装备、玩家、命令或证据标识。

use auction_application::consignment::LossReconcileRecord;
use auction_application::operations::{
    OperationsAlertKind, OperationsAlertSignal, OperationsAlertSource,
};
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;

/// 记录一次“无法查清”：核对记录以被拒的确认命令为主键，同命令重放不会走到这里（账本回放原拒绝）。
pub(super) async fn record_reconcile(
    connection: &mut PgConnection,
    position_id: Uuid,
    command_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.consignment_loss_reconciles (command_id, position_id, recorded_at)
         VALUES ($1, $2, $3)",
    )
    .bind(command_id)
    .bind(position_id)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}

/// 某 position 的未结核对记录：该 position 还没有损失终局；按记录时刻升序。
pub(super) async fn open_reconciles(
    store: &DatabaseStore,
    position_id: Uuid,
) -> Result<Vec<LossReconcileRecord>, WriteError> {
    let rows: Vec<(Uuid, Uuid, Timestamp)> = sqlx::query_as(
        "SELECT reconcile.command_id, reconcile.position_id, reconcile.recorded_at
         FROM public.consignment_loss_reconciles AS reconcile
         WHERE reconcile.position_id = $1
           AND NOT EXISTS (SELECT 1 FROM public.consignment_loss_facts AS fact
                           WHERE fact.position_id = reconcile.position_id)
         ORDER BY reconcile.recorded_at, reconcile.command_id",
    )
    .bind(position_id)
    .fetch_all(store.pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(command_id, position_id, recorded_at)| LossReconcileRecord {
            command_id,
            position_id,
            recorded_at,
        })
        .collect())
}

/// 写入（或累加）灭失核对信号。
pub(super) async fn raise_reconcile_signal(
    connection: &mut PgConnection,
    now: Timestamp,
) -> Result<(), WriteError> {
    let signal = OperationsAlertSignal::new(
        OperationsAlertKind::RecoveryManualReconcile,
        "auction",
        OperationsAlertSource::Recovery,
        1,
        0,
    )
    .map_err(|error| {
        tracing::warn!(%error, "灭失核对信号未通过校验");
        WriteError::invariant("consignment_loss_reconcile_signal")
    })?;
    sqlx::query(
        "INSERT INTO public.operations_alert_signals
            (fingerprint, kind, scope, source, active, first_seen_at, last_seen_at,
             resolved_at, last_notified_at, next_notify_at, next_webhook_at,
             count_value, age_seconds, webhook_generation, state_version)
         VALUES ($1, $2, $3, $4, true, $5, $5, NULL, NULL, $5, $5, $6, $7, 1, 1)
         ON CONFLICT (fingerprint) DO UPDATE SET
            count_value = CASE WHEN operations_alert_signals.active
                THEN operations_alert_signals.count_value + EXCLUDED.count_value
                ELSE EXCLUDED.count_value END,
            active = true,
            last_seen_at = EXCLUDED.last_seen_at,
            resolved_at = NULL,
            next_notify_at = COALESCE(
                operations_alert_signals.next_notify_at, EXCLUDED.next_notify_at),
            next_webhook_at = CASE WHEN operations_alert_signals.active
                THEN operations_alert_signals.next_webhook_at
                ELSE EXCLUDED.next_webhook_at END,
            webhook_generation = CASE WHEN operations_alert_signals.active
                THEN operations_alert_signals.webhook_generation
                ELSE operations_alert_signals.webhook_generation + 1 END,
            age_seconds = EXCLUDED.age_seconds,
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
           AND operations_alert_signals.source = EXCLUDED.source",
    )
    .bind(signal.fingerprint.as_slice())
    .bind(signal.kind.as_str())
    .bind(&signal.scope)
    .bind(signal.source.as_str())
    .bind(now)
    .bind(signal.count)
    .bind(signal.age_seconds)
    .execute(connection)
    .await?;
    Ok(())
}
