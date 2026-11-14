//! 读侧口径切换与履约终态的统一处置读法。
//!
//! 统一处置事实 `delivery_dispositions.resolves_alert` 是分类口径的唯一来源；本模块补上
//! 另外两条路径：
//!
//! - `legacy`：只用迁移前各来源的专用排除谓词，等价于迁移前的读侧；
//! - `shadow`：**旧口径继续持有裁决权**，分类口径只产生逐项差异样本，交给
//!   [`auction_application::shadow_run`] 的结构化日志与指标；这是本版缺省模式。
//!
//! 两个口径必须在**同一份数据快照**上读取：每次读取只用一条语句同时返回旧口径与分类口径
//! 的配对计数（投递与通知共用一条，履约一条）。语句级快照保证同一份语句内的所有子查询看
//! 到一致的已提交状态，因此并发投递推进不会制造假差异；先读一个口径、再用连接池补读另一个
//! 口径会把并发变化误判为差异，本模块不做这种读法。
//!
//! 所有 SQL 文本都是编译期常量拼接（只插入本文件的常量，不含运行时业务值），不缓存水位，
//! 也不读取 payload、正文或凭据。

use std::sync::LazyLock;

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::DiagnosticError;
use auction_application::operations::{
    DispositionReadCounts, DispositionReadField, DispositionReadMode, OperationsAlertSnapshot,
    SHADOW_OWNER, select_read_counts,
};
use auction_application::shadow_run::{record_comparison, record_uncomparable};
use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::database::storage_error_from;

/// 分类口径的“未处置”判据，与 `reads.rs` 的插值常量逐字同义：只按当前 fence 与写入时固定
/// 的 `resolves_alert` 过滤。
const CLASSIFIED_UNDISPOSED: &str = r#"NOT EXISTS (
                    SELECT 1 FROM public.delivery_dispositions disposition
                    WHERE disposition.delivery_id = d.id
                      AND disposition.fencing_version = d.fencing_version
                      AND disposition.resolves_alert
                )"#;

/// 迁移前的关注提醒预检退役排除：只有开始发送前失去资格的提醒解除失败计数。
const LEGACY_LOT_WATCH_INVALIDATED: &str = r#"NOT (d.origin_kind = 'lot_watch'
                    AND d.failure_code = 'lot_watch_invalidated'
                    AND d.sending_at IS NULL
                    AND d.result_metadata @>
                        '{"outcome_unknown":false,"preflight_blocked":true,"reason_code":"lot_watch_invalidated"}'::jsonb)"#;

/// 迁移前的 Admin 判失败排除：命令账本中产出当前 fence 的失败命令。
const LEGACY_ADMIN_RECONCILE: &str = r#"NOT EXISTS (
                    SELECT 1 FROM public.delivery_admin_commands admin
                    WHERE admin.delivery_id = d.id AND admin.command_kind = 'reconcile'
                      AND admin.result_payload->>'Status' = 'failed'
                      AND admin.result_payload->'FencingVersion' = to_jsonb(d.fencing_version)
                )"#;

/// 迁移前的装备损失排除：损失事实退役的原投递。
const LEGACY_EQUIPMENT_LOSS: &str = r#"NOT EXISTS (
                    SELECT 1 FROM public.settlement_equipment_losses loss
                    WHERE loss.delivery_id = d.id AND d.failure_code = 'equipment_destroyed'
                )"#;

/// 迁移前的邮资不足判定，与通知来源阈值共用同一失败码与原因码。
const LEGACY_POSTAGE: &str = "d.failure_code IN ('send_rejected', 'send_rejected_after_stage')
                    AND d.result_metadata->>'reason_code' = 'insufficient_postage'";

/// 通知来源的固定范围；结算类投递由其它来源负责。
const NOTIFICATION_SCOPE: &str =
    "d.origin_kind NOT IN ('settlement', 'settlement_recovery_action', 'operations_alert')";

/// 投递与通知来源的配对口径；一条语句让两个口径共享同一语句级快照。
static PAIRED_DELIVERY_COUNTS: LazyLock<String> = LazyLock::new(|| {
    let legacy_delivery = format!(
        r#"d.origin_kind <> 'operations_alert' AND d.status = 'failed'
             AND NOT EXISTS (
                 SELECT 1 FROM public.settlement_legacy_self_reconciliations r
                 WHERE {}
             )
             AND {LEGACY_LOT_WATCH_INVALIDATED}
             AND {LEGACY_ADMIN_RECONCILE}
             AND {LEGACY_EQUIPMENT_LOSS}"#,
        crate::database::delivery::legacy_self_proof::TRUSTED_DELIVERY_PREDICATE,
    );
    let classified_delivery = format!(
        "d.origin_kind <> 'operations_alert' AND d.status = 'failed' AND {CLASSIFIED_UNDISPOSED}"
    );
    let legacy_notification = format!(
        "{NOTIFICATION_SCOPE} AND d.status = 'failed'
             AND {LEGACY_LOT_WATCH_INVALIDATED} AND {LEGACY_ADMIN_RECONCILE}"
    );
    let classified_notification =
        format!("{NOTIFICATION_SCOPE} AND d.status = 'failed' AND {CLASSIFIED_UNDISPOSED}");
    format!(
        r#"SELECT
               COUNT(*) FILTER (WHERE {legacy_delivery})::bigint AS legacy_delivery_failed,
               COUNT(*) FILTER (WHERE {classified_delivery})::bigint
                   AS classified_delivery_failed,
               COUNT(*) FILTER (WHERE {legacy_notification})::bigint
                   AS legacy_notification_failed,
               COALESCE(EXTRACT(EPOCH FROM
                   ($1::timestamptz - MIN(d.created_at) FILTER (WHERE {legacy_notification}))
               )::bigint, 0) AS legacy_oldest_failed_age,
               COUNT(*) FILTER (WHERE {legacy_notification} AND {LEGACY_POSTAGE})::bigint
                   AS legacy_postage,
               COALESCE(EXTRACT(EPOCH FROM
                   ($1::timestamptz - MIN(d.created_at)
                       FILTER (WHERE {legacy_notification} AND {LEGACY_POSTAGE}))
               )::bigint, 0) AS legacy_oldest_postage_age,
               COUNT(*) FILTER (WHERE {classified_notification})::bigint
                   AS classified_notification_failed,
               COALESCE(EXTRACT(EPOCH FROM
                   ($1::timestamptz - MIN(d.created_at) FILTER (WHERE {classified_notification}))
               )::bigint, 0) AS classified_oldest_failed_age,
               COUNT(*) FILTER (WHERE {classified_notification} AND {LEGACY_POSTAGE})::bigint
                   AS classified_postage,
               COALESCE(EXTRACT(EPOCH FROM
                   ($1::timestamptz - MIN(d.created_at)
                       FILTER (WHERE {classified_notification} AND {LEGACY_POSTAGE}))
               )::bigint, 0) AS classified_oldest_postage_age
           FROM public.deliveries d"#
    )
});

/// 履约终态的统一处置判据：装备损失后的线下补偿以处置事实为证。
///
/// 退款义务履约的 `failed` 仍要与不可变损失事实配对，但“已处置”的证明改为统一处置表：
/// `equipment_loss` 处置与损失事实在同一事务写入，`resolves_alert` 在写入时固定。
/// 处置缺失时该失败继续计入告警（宁可多报，也不能因为事实缺失而静默吞掉未处置终态）。
pub(super) const UNIFIED_EQUIPMENT_LOSS_TERMINAL: &str = r#"NOT (fulfillment.last_error_code = 'operator_external_buyer_compensation'
                    AND EXISTS (SELECT 1 FROM public.settlement_equipment_losses loss
                        JOIN public.delivery_dispositions disposition
                          ON disposition.equipment_loss_id = loss.command_id
                         AND disposition.kind = 'equipment_loss'
                         AND disposition.resolves_alert
                        WHERE loss.refund_obligation_id = obligation.id))"#;

/// COD 三次退回以原 COD 投递上的 `cod_returned` 处置收口。
/// 两个失败义务都属于同一恢复记录；处置缺失时继续计入失败，不凭损失/恢复行猜测。
const UNIFIED_COD_RETURN_TERMINAL: &str = r#"NOT (
                    fulfillment.last_error_code IN (
                        'cod_returned_three_times', 'equipment_returned_to_seller'
                    )
                    AND EXISTS (
                        SELECT 1
                        FROM public.settlement_recoveries recovery
                        JOIN public.delivery_dispositions disposition
                          ON disposition.settlement_recovery_id = recovery.id
                         AND disposition.kind = 'cod_returned'
                         AND disposition.resolves_alert
                        WHERE fulfillment.obligation_id IN (
                            recovery.payment_obligation_id,
                            recovery.seller_payout_obligation_id,
                            recovery.buyer_refund_obligation_id
                        )
                    )
                )"#;

/// 迁移前的履约终态排除：只看不可变损失事实。
const LEGACY_EQUIPMENT_LOSS_TERMINAL: &str = r#"NOT (fulfillment.last_error_code = 'operator_external_buyer_compensation'
                    AND EXISTS (SELECT 1 FROM public.settlement_equipment_losses loss
                        WHERE loss.refund_obligation_id = obligation.id))"#;

/// 履约来源的配对口径；同样只用一条语句读取两个口径。
static PAIRED_FULFILLMENT_COUNTS: LazyLock<String> = LazyLock::new(|| {
    format!(
        r#"SELECT
               COUNT(*) FILTER (WHERE fulfillment.execution_state = 'failed'
                   AND {LEGACY_EQUIPMENT_LOSS_TERMINAL})::bigint AS legacy_failed,
               COUNT(*) FILTER (WHERE fulfillment.execution_state = 'failed'
                   AND {UNIFIED_EQUIPMENT_LOSS_TERMINAL}
                   AND {UNIFIED_COD_RETURN_TERMINAL})::bigint AS classified_failed
           FROM public.settlement_obligations AS obligation
           LEFT JOIN public.settlement_obligation_fulfillments AS fulfillment
             ON fulfillment.obligation_id = obligation.id"#
    )
});

/// 同一语句快照下的两个口径读数。
struct PairedCounts {
    legacy: DispositionReadCounts,
    classified: DispositionReadCounts,
}

/// 读取投递、通知与履约的配对口径；每条语句内部保证一致，两条语句分属不同 owner。
async fn paired_counts(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
) -> Result<PairedCounts, ApplicationError> {
    let delivery = sqlx::query(sqlx::AssertSqlSafe(PAIRED_DELIVERY_COUNTS.as_str()))
        .bind(now)
        .fetch_one(pool)
        .await
        .map_err(|error| storage_error_from(error, "读取处置读侧配对口径失败"))?;
    let fulfillment = sqlx::query(sqlx::AssertSqlSafe(PAIRED_FULFILLMENT_COUNTS.as_str()))
        .fetch_one(pool)
        .await
        .map_err(|error| storage_error_from(error, "读取履约终态配对口径失败"))?;
    let column = |row: &sqlx::postgres::PgRow, name: &str| -> Result<i64, ApplicationError> {
        row.try_get(name).map_err(|error| storage_error_from(error, "读取处置读侧配对计数失败"))
    };
    Ok(PairedCounts {
        legacy: DispositionReadCounts {
            delivery_failed: column(&delivery, "legacy_delivery_failed")?,
            notification_failed: column(&delivery, "legacy_notification_failed")?,
            notification_oldest_failed_age_seconds: column(&delivery, "legacy_oldest_failed_age")?,
            notification_postage: column(&delivery, "legacy_postage")?,
            notification_oldest_postage_age_seconds: column(
                &delivery,
                "legacy_oldest_postage_age",
            )?,
            fulfillment_failed: column(&fulfillment, "legacy_failed")?,
        },
        classified: DispositionReadCounts {
            delivery_failed: column(&delivery, "classified_delivery_failed")?,
            notification_failed: column(&delivery, "classified_notification_failed")?,
            notification_oldest_failed_age_seconds: column(
                &delivery,
                "classified_oldest_failed_age",
            )?,
            notification_postage: column(&delivery, "classified_postage")?,
            notification_oldest_postage_age_seconds: column(
                &delivery,
                "classified_oldest_postage_age",
            )?,
            fulfillment_failed: column(&fulfillment, "classified_failed")?,
        },
    })
}

/// 按运行时开关选择告警读侧口径。
///
/// - `classified`：统一处置事实（`resolves_alert`，履约终态为装备损失处置）；
/// - `legacy`：只用迁移前谓词，不产生影子样本；
/// - `shadow`：显示迁移前口径的裁决值，并把分类口径的逐项差异记入影子日志与指标。
///
/// 读取失败时按模式收口，绝不把分类口径冒充旧口径的裁决值：
/// - `legacy`/`shadow` 把投递、通知与履约三个受影响来源标为不可用（观察者看到“来源不可读”，
///   而不是未经批准的切换结果），并把影子样本记为不可比较；
/// - `classified` 保留 `snapshot_postgres` 已得到的分类读值并记录告警，因为它本来就是分类口径，
///   不构成向新口径的静默切换。
pub(crate) async fn apply_read_mode(
    store: &super::super::DatabaseStore,
    now: DateTime<Utc>,
    mut snapshot: OperationsAlertSnapshot,
) -> Result<OperationsAlertSnapshot, ApplicationError> {
    let mode = store.runtime_switches().disposition_read_mode();
    match paired_counts(store.pool(), now).await {
        Ok(paired) => {
            if mode == DispositionReadMode::Shadow {
                record_shadow_samples(store, &snapshot, &paired);
            }
            let selection = select_read_counts(mode, paired.classified, paired.legacy);
            apply_counts(&mut snapshot, selection.counts);
            Ok(snapshot)
        }
        Err(error) => {
            tracing::warn!(
                target: "auction::storage::disposition_reads",
                mode = mode.label(),
                phase = "read_paired_counts",
                safe_kind = error.diagnostic_kind().label(),
                "处置读侧的配对口径不可读"
            );
            if mode == DispositionReadMode::Classified {
                // 分类口径的可见值本来就来自同一分类谓词；保留它不改变裁决来源。
                return Ok(snapshot);
            }
            // 旧口径是裁决值：读不到就必须报告不可用，不能退回分类口径。
            snapshot.delivery = None;
            snapshot.notification = None;
            snapshot.fulfillment = None;
            if mode == DispositionReadMode::Shadow {
                // 不可比较分支必须可见；否则“没有差异样本”会被误读成“差异为零”。
                let recorder = store.shadow_recorder();
                for field in DispositionReadField::ALL {
                    record_uncomparable(
                        recorder,
                        SHADOW_OWNER,
                        field.label(),
                        "paired_query_error",
                    );
                }
            }
            Ok(snapshot)
        }
    }
}

/// 把选择后的读数写回快照；只覆盖真实存在的来源，缺失来源绝不补造。
fn apply_counts(snapshot: &mut OperationsAlertSnapshot, counts: DispositionReadCounts) {
    if let Some(delivery) = snapshot.delivery.as_mut() {
        delivery.failed = counts.delivery_failed;
    }
    if let Some(notification) = snapshot.notification.as_mut() {
        notification.failed_count = counts.notification_failed;
        notification.oldest_failed_age_seconds = counts.notification_oldest_failed_age_seconds;
        notification.postage_insufficient_count = counts.notification_postage;
        notification.oldest_postage_age_seconds = counts.notification_oldest_postage_age_seconds;
    }
    if let Some(fulfillment) = snapshot.fulfillment.as_mut() {
        fulfillment.failed = counts.fulfillment_failed;
    }
}

/// 影子模式逐项记录样本；来源缺失时记为不可比较，不产生假的“一致”。
fn record_shadow_samples(
    store: &super::super::DatabaseStore,
    snapshot: &OperationsAlertSnapshot,
    paired: &PairedCounts,
) {
    let recorder = store.shadow_recorder();
    let compare = |field: DispositionReadField, present: bool| {
        if present {
            record_comparison(
                recorder,
                SHADOW_OWNER,
                field.label(),
                &paired.classified.get(field),
                &paired.legacy.get(field),
            );
        } else {
            record_uncomparable(recorder, SHADOW_OWNER, field.label(), "classified_query_error");
        }
    };
    compare(DispositionReadField::DeliveryFailed, snapshot.delivery.is_some());
    let notification = snapshot.notification.is_some();
    compare(DispositionReadField::NotificationFailed, notification);
    compare(DispositionReadField::NotificationOldestFailedAge, notification);
    compare(DispositionReadField::NotificationPostage, notification);
    compare(DispositionReadField::NotificationOldestPostageAge, notification);
    compare(DispositionReadField::FulfillmentFailed, snapshot.fulfillment.is_some());
}

#[cfg(test)]
mod tests {
    use auction_application::operations::{
        OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot,
        OperationsAlertNotificationSnapshot,
    };

    use super::*;

    fn delivery(failed: i64) -> OperationsAlertDeliverySnapshot {
        OperationsAlertDeliverySnapshot {
            pending: 0,
            failed,
            manual_reconcile: 0,
            returned_mail_manual_reconcile: 0,
            oldest_pending_age_seconds: 0,
            oldest_manual_reconcile_age_seconds: 0,
            oldest_returned_mail_age_seconds: 0,
            claimed: 0,
            oldest_claimed_age_seconds: 0,
            last_update_at: None,
        }
    }

    fn notification(failed: i64, postage: i64) -> OperationsAlertNotificationSnapshot {
        OperationsAlertNotificationSnapshot {
            failed_count: failed,
            oldest_failed_age_seconds: failed,
            postage_insufficient_count: postage,
            oldest_postage_age_seconds: postage,
        }
    }

    fn fulfillment(failed: i64) -> OperationsAlertFulfillmentSnapshot {
        OperationsAlertFulfillmentSnapshot {
            pending: 0,
            blocked: 0,
            failed,
            manual_reconcile: 0,
            waiting_payment: 0,
            waiting_prerequisite: 0,
            oldest_backlog_age_seconds: 0,
            oldest_prerequisite_wait_age_seconds: 0,
        }
    }

    fn snapshot(
        delivery_failed: Option<i64>,
        notification_values: Option<(i64, i64)>,
        fulfillment_failed: Option<i64>,
    ) -> OperationsAlertSnapshot {
        OperationsAlertSnapshot {
            process: None,
            delivery: delivery_failed.map(delivery),
            fulfillment: fulfillment_failed.map(fulfillment),
            recovery: None,
            mail: None,
            credential: None,
            lifecycle: None,
            forum: None,
            notification: notification_values
                .map(|(failed, postage)| notification(failed, postage)),
            reconciliation: None,
        }
    }

    fn counts(delivery_failed: i64, fulfillment_failed: i64) -> DispositionReadCounts {
        DispositionReadCounts {
            delivery_failed,
            notification_failed: 1,
            notification_oldest_failed_age_seconds: 1,
            notification_postage: 1,
            notification_oldest_postage_age_seconds: 1,
            fulfillment_failed,
        }
    }

    #[test]
    fn counts_are_only_applied_to_present_sources() {
        let mut value = snapshot(Some(2), None, None);
        apply_counts(&mut value, counts(7, 3));
        assert_eq!(value.delivery.as_ref().map(|value| value.failed), Some(7));
        assert!(value.notification.is_none(), "缺失来源不能被补造");
        assert!(value.fulfillment.is_none(), "缺失来源不能被补造");
    }

    #[test]
    fn every_present_source_takes_the_selected_counts() {
        let mut value = snapshot(Some(2), Some((3, 4)), Some(5));
        apply_counts(&mut value, counts(7, 8));
        assert_eq!(value.delivery.as_ref().map(|value| value.failed), Some(7));
        assert_eq!(value.notification.as_ref().map(|value| value.failed_count), Some(1));
        assert_eq!(value.fulfillment.as_ref().map(|value| value.failed), Some(8));
    }

    #[test]
    fn paired_orders_select_legacy_for_shadow_and_classified_for_classified() {
        let legacy = counts(9, 3);
        let classified = counts(1, 0);
        assert_eq!(
            select_read_counts(DispositionReadMode::Legacy, classified, legacy).counts,
            legacy
        );
        assert_eq!(
            select_read_counts(DispositionReadMode::Shadow, classified, legacy).counts,
            legacy
        );
        assert_eq!(
            select_read_counts(DispositionReadMode::Classified, classified, legacy).counts,
            classified
        );
    }
}
