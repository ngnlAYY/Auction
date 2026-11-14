//! 告警台读投影与导航待办计数：只读 signal 水位和操作者事实，不重算阈值。

use auction_application::core::ApplicationError;
use auction_application::operations::{
    AdminPendingCounts, OperationsAlertConsoleEntry, OperationsAlertConsolePage,
    OperationsAlertConsoleQuery, OperationsAlertError, OperationsAlertKind, OperationsAlertSource,
    OperationsAlertWebhookEntry,
};
use sqlx::Row;
use uuid::Uuid;

use crate::database::storage_error_from;

pub(super) async fn list_webhooks_postgres(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    limit: usize,
) -> Result<Vec<OperationsAlertWebhookEntry>, ApplicationError> {
    let rows = sqlx::query(
        "SELECT delivery.id, delivery.status, signal.kind, signal.scope, signal.source,
                delivery.attempt_count, delivery.next_attempt_at, delivery.sending_at,
                delivery.completed_at, delivery.failure_code, delivery.created_at
         FROM public.operations_alert_webhook_deliveries AS delivery
         JOIN public.operations_alert_signals AS signal
           ON signal.fingerprint = delivery.fingerprint
         ORDER BY delivery.created_at DESC, delivery.id DESC
         LIMIT $1",
    )
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .fetch_all(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 webhook 投递运营视图失败"))?;
    rows.iter()
        .map(|row| {
            let status: String = row
                .try_get("status")
                .map_err(|error| storage_error_from(error, "读取 webhook 状态失败"))?;
            if !matches!(
                status.as_str(),
                "pending" | "sending" | "succeeded" | "failed" | "manual_reconcile"
            ) {
                return Err(ApplicationError::OperationsAlert(
                    OperationsAlertError::StoredResultCorrupt,
                ));
            }
            let kind: String = row
                .try_get("kind")
                .map_err(|error| storage_error_from(error, "读取 webhook 种类失败"))?;
            let kind = OperationsAlertKind::parse(&kind).ok_or(
                ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt),
            )?;
            let source: String = row
                .try_get("source")
                .map_err(|error| storage_error_from(error, "读取 webhook 来源失败"))?;
            let source = OperationsAlertSource::parse(&source).ok_or(
                ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt),
            )?;
            let attempt_count: i32 = row
                .try_get("attempt_count")
                .map_err(|error| storage_error_from(error, "读取 webhook 尝试次数失败"))?;
            if attempt_count < 0 {
                return Err(ApplicationError::OperationsAlert(
                    OperationsAlertError::StoredResultCorrupt,
                ));
            }
            let attempt_count = attempt_count as u32;
            Ok(OperationsAlertWebhookEntry {
                delivery_id: row
                    .try_get::<Uuid, _>("id")
                    .map_err(|error| storage_error_from(error, "读取 webhook 标识失败"))?,
                status,
                kind,
                scope: row
                    .try_get("scope")
                    .map_err(|error| storage_error_from(error, "读取 webhook 范围失败"))?,
                source,
                attempt_count,
                next_attempt_at: row
                    .try_get("next_attempt_at")
                    .map_err(|error| storage_error_from(error, "读取 webhook 下次尝试失败"))?,
                sending_at: row
                    .try_get("sending_at")
                    .map_err(|error| storage_error_from(error, "读取 webhook 发送时间失败"))?,
                completed_at: row
                    .try_get("completed_at")
                    .map_err(|error| storage_error_from(error, "读取 webhook 完成时间失败"))?,
                failure_code: row
                    .try_get("failure_code")
                    .map_err(|error| storage_error_from(error, "读取 webhook 失败码失败"))?,
                created_at: row
                    .try_get("created_at")
                    .map_err(|error| storage_error_from(error, "读取 webhook 创建时间失败"))?,
            })
        })
        .collect()
}

pub(super) async fn list_postgres(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
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

pub(super) fn entry_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<OperationsAlertConsoleEntry, ApplicationError> {
    let fingerprint: Vec<u8> = row
        .try_get("fingerprint")
        .map_err(|error| storage_error_from(error, "读取告警标识失败"))?;
    let fingerprint: [u8; 32] = fingerprint.try_into().map_err(|_: Vec<u8>| {
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

/// 导航待办计数：投递人工对账、待确认恢复与活动告警。
pub(super) async fn pending_counts_postgres(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
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
pub(super) async fn recovery_confirmation_count(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
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
