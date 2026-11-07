//! 告警台读投影、确认/静默只追加事实与导航待办计数（W5.8）。
//!
//! 读侧只读取 signal 水位和操作者事实，不重算阈值；写侧只追加一行
//! `operations_alert_acknowledgements` 并（静默时）推进 `next_notify_at`。确认与静默都不
//! 改写 kind/scope/source/count/age/active，也不跳过来源评估，因此下一次观察仍会恢复真实告警。

#[cfg(feature = "postgres")]
use crate::database::{lock_command_id, storage_error_from};
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use sqlx::Row;

use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::operations::{
    AdminPendingCounts, OperationsAlertAcknowledgeCommand, OperationsAlertCommandResult,
    OperationsAlertConsole, OperationsAlertConsoleEntry, OperationsAlertConsolePage,
    OperationsAlertConsoleQuery, OperationsAlertError, OperationsAlertSilenceCommand,
    OperationsAlertSource,
};

use super::DatabaseStore;

impl OperationsAlertConsole for DatabaseStore {
    fn list_alerts(
        &self,
        query: OperationsAlertConsoleQuery,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, OperationsAlertConsolePage> {
        Box::pin(async move {
            query.validate().map_err(ApplicationError::from)?;
            if now.timestamp_nanos_opt().is_none() {
                return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => list_postgres(store.pool(), &query).await,
            }
        })
    }

    fn acknowledge_alert(
        &self,
        command: OperationsAlertAcknowledgeCommand,
    ) -> ApplicationFuture<'_, OperationsAlertCommandResult> {
        Box::pin(async move {
            command.validate().map_err(ApplicationError::from)?;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    write_acknowledgement_postgres(store.pool(), &command, None).await
                }
            }
        })
    }

    fn silence_alert(
        &self,
        command: OperationsAlertSilenceCommand,
    ) -> ApplicationFuture<'_, OperationsAlertCommandResult> {
        Box::pin(async move {
            command.validate().map_err(ApplicationError::from)?;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => write_silence_postgres(store.pool(), &command).await,
            }
        })
    }

    fn pending_counts(&self) -> ApplicationFuture<'_, AdminPendingCounts> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => pending_counts_postgres(store.pool()).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    query: &OperationsAlertConsoleQuery,
) -> Result<OperationsAlertConsolePage, ApplicationError> {
    let offset = query.offset().map_err(ApplicationError::from)?;
    let active_filter = query.filter.active();
    let source_filter = query.source.as_ref().map(|source| source.as_str());
    let mut transaction =
        pool.begin().await.map_err(|error| storage_error_from(error, "开启告警台读取事务失败"))?;
    let counts = sqlx::query(
        "SELECT
            COUNT(*) FILTER (WHERE active) AS active_total,
            COUNT(*) FILTER (WHERE NOT active) AS resolved_total
         FROM public.operations_alert_signals
         WHERE ($1::text IS NULL OR source = $1)",
    )
    .bind(source_filter)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取告警台计数失败"))?;
    let active_total: i64 = counts
        .try_get("active_total")
        .map_err(|error| storage_error_from(error, "读取活动告警计数失败"))?;
    let resolved_total: i64 = counts
        .try_get("resolved_total")
        .map_err(|error| storage_error_from(error, "读取历史告警计数失败"))?;
    let total = match query.filter.active() {
        Some(true) => active_total,
        Some(false) => resolved_total,
        None => active_total
            .checked_add(resolved_total)
            .ok_or(ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt))?,
    };
    let rows = sqlx::query(
        "SELECT fingerprint, kind, scope, source, active, first_seen_at, last_seen_at,
                resolved_at, last_notified_at, next_notify_at, count_value, age_seconds,
                acknowledged_at, acknowledged_by, silenced_until
         FROM public.operations_alert_signals
         WHERE ($1::bool IS NULL OR active = $1)
           AND ($2::text IS NULL OR source = $2)
         ORDER BY active DESC, COALESCE(resolved_at, last_seen_at) DESC, fingerprint
         LIMIT $3 OFFSET $4",
    )
    .bind(active_filter)
    .bind(source_filter)
    .bind(query.page_size)
    .bind(offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取告警台条目失败"))?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交告警台读取事务失败"))?;

    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        entries.push(entry_from_row(&row)?);
    }
    let page = OperationsAlertConsolePage {
        entries,
        page: query.page,
        page_size: query.page_size,
        total,
        active_total,
        resolved_total,
    };
    page.validate(query).map_err(ApplicationError::from)?;
    Ok(page)
}

#[cfg(feature = "postgres")]
fn entry_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<OperationsAlertConsoleEntry, ApplicationError> {
    let fingerprint: Vec<u8> = row
        .try_get("fingerprint")
        .map_err(|error| storage_error_from(error, "读取告警标识失败"))?;
    let fingerprint: [u8; 32] = fingerprint.try_into().map_err(|error| {
        let _ = error;
        ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt)
    })?;
    let kind: String =
        row.try_get("kind").map_err(|error| storage_error_from(error, "读取告警种类失败"))?;
    let kind = auction_application::operations::OperationsAlertKind::parse(&kind)
        .ok_or(ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt))?;
    let scope: String =
        row.try_get("scope").map_err(|error| storage_error_from(error, "读取告警范围失败"))?;
    let source: String =
        row.try_get("source").map_err(|error| storage_error_from(error, "读取告警来源失败"))?;
    let source = OperationsAlertSource::parse(&source)
        .ok_or(ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt))?;
    let entry = OperationsAlertConsoleEntry {
        fingerprint,
        kind,
        scope,
        source,
        active: row
            .try_get("active")
            .map_err(|error| storage_error_from(error, "读取告警活动状态失败"))?,
        count: row
            .try_get("count_value")
            .map_err(|error| storage_error_from(error, "读取告警计数失败"))?,
        age_seconds: row
            .try_get("age_seconds")
            .map_err(|error| storage_error_from(error, "读取告警年龄失败"))?,
        first_seen_at: row
            .try_get("first_seen_at")
            .map_err(|error| storage_error_from(error, "读取告警首次时间失败"))?,
        last_seen_at: row
            .try_get("last_seen_at")
            .map_err(|error| storage_error_from(error, "读取告警最近时间失败"))?,
        resolved_at: row
            .try_get("resolved_at")
            .map_err(|error| storage_error_from(error, "读取告警恢复时间失败"))?,
        last_notified_at: row
            .try_get("last_notified_at")
            .map_err(|error| storage_error_from(error, "读取告警通知时间失败"))?,
        next_notify_at: row
            .try_get("next_notify_at")
            .map_err(|error| storage_error_from(error, "读取告警下次通知时间失败"))?,
        acknowledged_at: row
            .try_get("acknowledged_at")
            .map_err(|error| storage_error_from(error, "读取告警确认时间失败"))?,
        acknowledged_by: row
            .try_get("acknowledged_by")
            .map_err(|error| storage_error_from(error, "读取告警确认人失败"))?,
        silenced_until: row
            .try_get("silenced_until")
            .map_err(|error| storage_error_from(error, "读取告警静默到期时间失败"))?,
    };
    entry.validate().map_err(ApplicationError::from)?;
    Ok(entry)
}

#[cfg(feature = "postgres")]
async fn write_acknowledgement_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    command: &OperationsAlertAcknowledgeCommand,
    silenced_until: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<OperationsAlertCommandResult, ApplicationError> {
    let action = if silenced_until.is_some() { "silence" } else { "acknowledge" };
    let mut transaction =
        pool.begin().await.map_err(|error| storage_error_from(error, "开启告警确认事务失败"))?;
    lock_command_id(&mut transaction, command.command_id)
        .await
        .map_err(|error| storage_error_from(error, "锁定告警确认命令失败"))?;
    if let Some(result) = replay_postgres(
        &mut transaction,
        command.command_id,
        command.fingerprint,
        command.request_hash.as_bytes(),
        action,
    )
    .await?
    {
        transaction
            .commit()
            .await
            .map_err(|error| storage_error_from(error, "提交告警确认重放事务失败"))?;
        return Ok(result);
    }
    let signal = sqlx::query(
        "SELECT active FROM public.operations_alert_signals
         WHERE fingerprint = $1
         FOR UPDATE",
    )
    .bind(command.fingerprint.as_slice())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取告警水位失败"))?
    .ok_or(ApplicationError::OperationsAlert(OperationsAlertError::Invalid))?;
    let active: bool = signal
        .try_get("active")
        .map_err(|error| storage_error_from(error, "读取告警活动状态失败"))?;
    if !active {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
    }
    let occurred_at = Utc::now();
    insert_fact(
        &mut transaction,
        command.command_id,
        command.fingerprint,
        action,
        command.request_hash.as_bytes(),
        &command.actor_id,
        &command.reason,
        None,
        None,
        occurred_at,
    )
    .await?;
    sqlx::query(
        "UPDATE public.operations_alert_signals
         SET acknowledged_at = $2, acknowledged_by = $3
         WHERE fingerprint = $1",
    )
    .bind(command.fingerprint.as_slice())
    .bind(occurred_at)
    .bind(&command.actor_id)
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入告警确认投影失败"))?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交告警确认事务失败"))?;
    Ok(OperationsAlertCommandResult {
        command_id: command.command_id,
        fingerprint: command.fingerprint,
        action: action.to_owned(),
        silenced_until: None,
        replayed: false,
    })
}

#[cfg(feature = "postgres")]
async fn write_silence_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    command: &OperationsAlertSilenceCommand,
) -> Result<OperationsAlertCommandResult, ApplicationError> {
    let mut transaction =
        pool.begin().await.map_err(|error| storage_error_from(error, "开启告警静默事务失败"))?;
    lock_command_id(&mut transaction, command.command_id)
        .await
        .map_err(|error| storage_error_from(error, "锁定告警静默命令失败"))?;
    let action = "silence";
    if let Some(result) = replay_postgres(
        &mut transaction,
        command.command_id,
        command.fingerprint,
        command.request_hash.as_bytes(),
        action,
    )
    .await?
    {
        transaction
            .commit()
            .await
            .map_err(|error| storage_error_from(error, "提交告警静默重放事务失败"))?;
        return Ok(result);
    }
    let signal = sqlx::query(
        "SELECT active FROM public.operations_alert_signals
         WHERE fingerprint = $1
         FOR UPDATE",
    )
    .bind(command.fingerprint.as_slice())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取告警水位失败"))?
    .ok_or(ApplicationError::OperationsAlert(OperationsAlertError::Invalid))?;
    let active: bool = signal
        .try_get("active")
        .map_err(|error| storage_error_from(error, "读取告警活动状态失败"))?;
    if !active {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
    }
    let occurred_at = Utc::now();
    let silenced_until = occurred_at
        .checked_add_signed(chrono::Duration::seconds(command.silence_seconds))
        .ok_or(ApplicationError::OperationsAlert(OperationsAlertError::Invalid))?;
    insert_fact(
        &mut transaction,
        command.command_id,
        command.fingerprint,
        action,
        command.request_hash.as_bytes(),
        &command.actor_id,
        &command.reason,
        Some(command.silence_seconds),
        Some(silenced_until),
        occurred_at,
    )
    .await?;
    // 只推进重复通知时间；active/kind/scope/count/age/state_version 保持不变，
    // 到期后下一次扫描仍按原阈值继续评估。
    sqlx::query(
        "UPDATE public.operations_alert_signals
         SET silenced_until = GREATEST(COALESCE(silenced_until, $2), $2),
             next_notify_at = GREATEST(next_notify_at, $2)
         WHERE fingerprint = $1 AND active",
    )
    .bind(command.fingerprint.as_slice())
    .bind(silenced_until)
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入告警静默投影失败"))?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交告警静默事务失败"))?;
    Ok(OperationsAlertCommandResult {
        command_id: command.command_id,
        fingerprint: command.fingerprint,
        action: action.to_owned(),
        silenced_until: Some(silenced_until),
        replayed: false,
    })
}

/// 同命令 ID 重放：摘要与目标一致返回原结果，否则失败关闭。
#[cfg(feature = "postgres")]
async fn replay_postgres(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command_id: uuid::Uuid,
    fingerprint: [u8; 32],
    request_digest: [u8; 32],
    action: &str,
) -> Result<Option<OperationsAlertCommandResult>, ApplicationError> {
    let row = sqlx::query(
        "SELECT fingerprint, action, request_digest, silenced_until
         FROM public.operations_alert_acknowledgements
         WHERE command_id = $1",
    )
    .bind(command_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取告警确认重放失败"))?;
    let Some(row) = row else { return Ok(None) };
    let stored_fingerprint: Vec<u8> = row
        .try_get("fingerprint")
        .map_err(|error| storage_error_from(error, "读取告警重放标识失败"))?;
    let stored_digest: Vec<u8> = row
        .try_get("request_digest")
        .map_err(|error| storage_error_from(error, "读取告警重放摘要失败"))?;
    let stored_action: String =
        row.try_get("action").map_err(|error| storage_error_from(error, "读取告警重放动作失败"))?;
    let silenced_until: Option<DateTime<Utc>> = row
        .try_get("silenced_until")
        .map_err(|error| storage_error_from(error, "读取告警重放静默时间失败"))?;
    if stored_fingerprint.as_slice() != fingerprint
        || stored_digest.as_slice() != request_digest
        || stored_action != action
    {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::IdentityConflict));
    }
    Ok(Some(OperationsAlertCommandResult {
        command_id,
        fingerprint,
        action: stored_action,
        silenced_until,
        replayed: true,
    }))
}

#[cfg(feature = "postgres")]
#[allow(clippy::too_many_arguments)]
async fn insert_fact(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command_id: uuid::Uuid,
    fingerprint: [u8; 32],
    action: &str,
    request_digest: [u8; 32],
    actor_id: &str,
    reason: &str,
    silence_seconds: Option<i64>,
    silenced_until: Option<DateTime<Utc>>,
    occurred_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO public.operations_alert_acknowledgements
            (fingerprint, action, command_id, request_digest, actor_id, reason,
             silence_seconds, silenced_until, occurred_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(fingerprint.as_slice())
    .bind(action)
    .bind(command_id)
    .bind(request_digest.as_slice())
    .bind(actor_id)
    .bind(reason)
    .bind(silence_seconds)
    .bind(silenced_until)
    .bind(occurred_at)
    .execute(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入告警确认事实失败"))?;
    Ok(())
}

/// 导航待办计数：投递人工对账、待确认恢复与活动告警。
#[cfg(feature = "postgres")]
async fn pending_counts_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
) -> Result<AdminPendingCounts, ApplicationError> {
    let row = sqlx::query(
        "SELECT
            (SELECT COUNT(*) FROM public.deliveries
             WHERE origin_kind <> 'operations_alert' AND status = 'manual_reconcile')
                AS delivery_manual_reconcile,
            (SELECT COUNT(*) FROM public.operations_alert_signals WHERE active)
                AS active_alerts",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取导航待办计数失败"))?;
    let delivery_manual_reconcile: i64 = row
        .try_get("delivery_manual_reconcile")
        .map_err(|error| storage_error_from(error, "读取投递人工对账计数失败"))?;
    let active_alerts: i64 = row
        .try_get("active_alerts")
        .map_err(|error| storage_error_from(error, "读取活动告警计数失败"))?;
    let recovery_confirmation = recovery_confirmation_count(pool).await?;
    let counts =
        AdminPendingCounts { delivery_manual_reconcile, recovery_confirmation, active_alerts };
    counts.validate().map_err(ApplicationError::from)?;
    Ok(counts)
}

/// 与 `allowed_recovery_commands` 同一准入的计数：reclaiming、证据齐全、恰好一个候选结果、
/// 需要黑名单的动作已满足。只改变展示计数，不构成确认授权。
#[cfg(feature = "postgres")]
async fn recovery_confirmation_count(
    pool: &super::TrackedPool<sqlx::Postgres>,
) -> Result<i64, ApplicationError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM public.settlement_recoveries AS recovery
         WHERE recovery.stage = 'reclaiming'
           AND recovery.blocking_code IS NULL
           AND recovery.custody_proof_kind IS NOT NULL
           AND recovery.custody_proof_observed_at IS NOT NULL
           AND EXISTS (
               SELECT 1 FROM public.settlement_recovery_actions AS action
               WHERE action.recovery_id = recovery.id
                 AND action.execution_state = 'satisfied'
                 AND (
                     (recovery.world_id = 'main' AND action.kind = 'reclaim_equipment_take')
                     OR (recovery.world_id = 'isekai' AND action.kind IN
                         ('cancel_carrier_take', 'recover_equipment_take'))
                 )
           )
           AND (
               SELECT COUNT(DISTINCT CASE
                   WHEN recovery.world_id = 'main' AND action.kind = 'reclaim_equipment_take'
                       THEN 'reclaimed'
                   WHEN recovery.world_id = 'isekai' AND action.kind = 'cancel_carrier_take'
                       THEN 'cancelled'
                   WHEN recovery.world_id = 'isekai' AND action.kind = 'recover_equipment_take'
                       THEN 'equipment_recovered'
               END)
               FROM public.settlement_recovery_actions AS action
               WHERE action.recovery_id = recovery.id
                 AND action.execution_state = 'satisfied'
           ) = 1
           AND (
               NOT EXISTS (
                   SELECT 1 FROM public.settlement_recovery_actions AS action
                   WHERE action.recovery_id = recovery.id
                     AND action.execution_state = 'satisfied'
                     AND action.kind IN ('reclaim_equipment_take', 'cancel_carrier_take')
               )
               OR EXISTS (
                   SELECT 1 FROM public.settlement_recovery_actions AS action
                   WHERE action.recovery_id = recovery.id
                     AND action.execution_state = 'satisfied'
                     AND action.kind = 'buyer_blacklist'
               )
           )",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取待确认恢复计数失败"))?;
    Ok(count)
}
