//! 结算面的投递与恢复水位快照；履约水位与长期未领取清单见 `fulfillment` 模块。

use auction_application::core::ApplicationError;
use auction_application::operations::{
    OperationsAlertDeliverySnapshot, OperationsAlertRecoverySnapshot, OperationsAlertRecoveryWorld,
};
use auction_time::Timestamp;
use sqlx::Row;

use super::{UNDISPOSED, age_seconds};
use crate::database::storage_error;
use crate::database::storage_error_from;

/// 投递水位只统计投递台自己负责的投递：运维告警投递与结算退件 Take 投递各有 owner。
const DELIVERY_OWNED: &str = "origin_kind NOT IN ('operations_alert', 'settlement_return_action')";

pub(super) async fn delivery_snapshot(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
    now: Timestamp,
) -> Result<OperationsAlertDeliverySnapshot, ApplicationError> {
    // 仅完整匹配追加修复事实及原投递身份的 failed 才已解决；不能仅凭 metadata 消除告警。
    // 替换内容只有已审核的编译期 SQL 常量，不插入任何运行时业务值；仅缓存 SQL 文本，不缓存水位。
    // 结算退件 Take 投递（`settlement_return_action`）归所属案件：积压、失败与人工对账都由退件
    // 告警（`settlement_return_pending`/`settlement_return_quarantined`）承担，这里不重复计数。
    static QUERY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        r#"SELECT
            COUNT(*) FILTER (WHERE /* owned */
                AND status IN ('prepared', 'pending', 'claimed', 'sending')) AS pending,
            COUNT(*) FILTER (WHERE /* owned */
                AND status IN ('claimed', 'sending')) AS claimed,
            COUNT(*) FILTER (WHERE /* owned */
                AND status = 'failed'
                -- 已处置终态不再计入，各自以不可变处置事实为证（历史自寄、Admin 判失败、
                -- 装备损失、预检退役、操作者确认）。结算类人工判失败仍由履约水位的 failed 暴露。
                AND /* undisposed */
                ) AS failed,
            COUNT(*) FILTER (WHERE /* owned */
                AND status = 'manual_reconcile') AS manual_reconcile,
            COUNT(*) FILTER (WHERE /* owned */
                AND status = 'manual_reconcile'
                AND failure_code = 'recipient_not_found'
                AND result_metadata @>
                    '{"reason_code":"recipient_not_found","take_required":true}'::jsonb)
                AS returned_mail_manual_reconcile,
            MIN(created_at) FILTER (WHERE /* owned */
                AND status IN ('prepared', 'pending', 'claimed', 'sending')) AS oldest_pending,
            MIN(claimed_at) FILTER (WHERE /* owned */
                AND status IN ('claimed', 'sending')) AS oldest_claimed,
            MIN(created_at) FILTER (WHERE /* owned */
                AND status = 'manual_reconcile') AS oldest_manual,
            MIN(created_at) FILTER (WHERE /* owned */
                AND status = 'manual_reconcile'
                AND failure_code = 'recipient_not_found'
                AND result_metadata @>
                    '{"reason_code":"recipient_not_found","take_required":true}'::jsonb)
                AS oldest_returned_mail,
            MAX(updated_at) FILTER (WHERE origin_kind <> 'operations_alert') AS last_update
         FROM public.deliveries d"#
            .replace("/* undisposed */", UNDISPOSED)
            .replace("/* owned */", DELIVERY_OWNED)
    });
    let row = sqlx::query(sqlx::AssertSqlSafe(QUERY.as_str()))
        .fetch_one(pool)
        .await
        .map_err(|error| storage_error_from(error, "读取 operations alert delivery 水位失败"))?;
    let pending: i64 = row
        .try_get("pending")
        .map_err(|error| storage_error_from(error, "读取 delivery 积压计数失败"))?;
    let failed: i64 = row
        .try_get("failed")
        .map_err(|error| storage_error_from(error, "读取 delivery 失败计数失败"))?;
    let manual_reconcile: i64 = row
        .try_get("manual_reconcile")
        .map_err(|error| storage_error_from(error, "读取 delivery 人工对账计数失败"))?;
    let returned_mail_manual_reconcile: i64 = row
        .try_get("returned_mail_manual_reconcile")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 退信人工对账计数失败"))?;
    Ok(OperationsAlertDeliverySnapshot {
        pending,
        failed,
        manual_reconcile,
        returned_mail_manual_reconcile,
        claimed: row
            .try_get("claimed")
            .map_err(|error| storage_error_from(error, "读取 delivery 已领取计数失败"))?,
        oldest_pending_age_seconds: age_seconds(
            now,
            row.try_get("oldest_pending")
                .map_err(|error| storage_error_from(error, "读取 delivery 积压时间失败"))?,
        ),
        oldest_claimed_age_seconds: age_seconds(
            now,
            row.try_get("oldest_claimed")
                .map_err(|error| storage_error_from(error, "读取 delivery 领取时间失败"))?,
        ),
        oldest_manual_reconcile_age_seconds: age_seconds(
            now,
            row.try_get("oldest_manual")
                .map_err(|error| storage_error_from(error, "读取 delivery 人工对账时间失败"))?,
        ),
        oldest_returned_mail_age_seconds: age_seconds(
            now,
            row.try_get("oldest_returned_mail").map_err(|error| {
                storage_error_from(error, "读取 MoogleMail 退信人工对账时间失败")
            })?,
        ),
        last_update_at: row
            .try_get("last_update")
            .map_err(|error| storage_error_from(error, "读取 delivery 更新时间失败"))?,
    })
}

pub(super) async fn recovery_snapshot(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
    now: Timestamp,
) -> Result<OperationsAlertRecoverySnapshot, ApplicationError> {
    let rows = sqlx::query(
        "SELECT world_id, balance_credits, balance_status, evidence_status,
                balance_observed_at, evidence_last_success_at, updated_at,
                payment_overdue_count, relist_pending_count, failed_action_count,
                manual_reconcile_count, oldest_overdue_age_seconds
         FROM public.settlement_recovery_health
         WHERE world_id IN ('main', 'isekai')
         ORDER BY world_id",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert recovery 水位失败"))?;
    let mut snapshot = OperationsAlertRecoverySnapshot {
        payment_overdue_count: 0,
        payment_mail_unsent_count: 0,
        relist_pending_count: 0,
        failed_action_count: 0,
        manual_reconcile_count: 0,
        oldest_overdue_age_seconds: 0,
        oldest_payment_mail_unsent_age_seconds: 0,
        worlds: Vec::with_capacity(rows.len()),
    };
    for row in rows {
        let world: String = row
            .try_get("world_id")
            .map_err(|error| storage_error_from(error, "读取 recovery 世界失败"))?;
        let payment_overdue_count: i64 = row
            .try_get("payment_overdue_count")
            .map_err(|error| storage_error_from(error, "读取 recovery 逾期计数失败"))?;
        let relist_pending_count: i64 = row
            .try_get("relist_pending_count")
            .map_err(|error| storage_error_from(error, "读取 recovery 重挂计数失败"))?;
        let failed_action_count: i64 = row
            .try_get("failed_action_count")
            .map_err(|error| storage_error_from(error, "读取 recovery 失败计数失败"))?;
        let manual_reconcile_count: i64 = row
            .try_get("manual_reconcile_count")
            .map_err(|error| storage_error_from(error, "读取 recovery 人工对账计数失败"))?;
        snapshot.payment_overdue_count = snapshot
            .payment_overdue_count
            .checked_add(payment_overdue_count)
            .ok_or_else(|| storage_error("recovery 逾期计数溢出"))?;
        snapshot.relist_pending_count = snapshot
            .relist_pending_count
            .checked_add(relist_pending_count)
            .ok_or_else(|| storage_error("recovery 重挂计数溢出"))?;
        snapshot.failed_action_count = snapshot
            .failed_action_count
            .checked_add(failed_action_count)
            .ok_or_else(|| storage_error("recovery 失败计数溢出"))?;
        snapshot.manual_reconcile_count = snapshot
            .manual_reconcile_count
            .checked_add(manual_reconcile_count)
            .ok_or_else(|| storage_error("recovery 人工对账计数溢出"))?;
        let oldest_overdue_age_seconds: i64 = row
            .try_get("oldest_overdue_age_seconds")
            .map_err(|error| storage_error_from(error, "读取 recovery 逾期时间失败"))?;
        snapshot.oldest_overdue_age_seconds =
            snapshot.oldest_overdue_age_seconds.max(oldest_overdue_age_seconds);
        let balance_observed_at: Option<Timestamp> = row
            .try_get("balance_observed_at")
            .map_err(|error| storage_error_from(error, "读取 recovery 余额时间失败"))?;
        let evidence_last_success_at: Option<Timestamp> =
            row.try_get("evidence_last_success_at")
                .map_err(|error| storage_error_from(error, "读取 recovery 证据时间失败"))?;
        let updated_at: Timestamp = row
            .try_get("updated_at")
            .map_err(|error| storage_error_from(error, "读取 recovery 更新时间失败"))?;
        snapshot.worlds.push(OperationsAlertRecoveryWorld {
            world,
            balance_credits: row
                .try_get("balance_credits")
                .map_err(|error| storage_error_from(error, "读取 recovery 余额失败"))?,
            balance_status: row
                .try_get("balance_status")
                .map_err(|error| storage_error_from(error, "读取 recovery 余额状态失败"))?,
            evidence_status: row
                .try_get("evidence_status")
                .map_err(|error| storage_error_from(error, "读取 recovery 证据状态失败"))?,
            evidence_age_seconds: age_seconds(now, evidence_last_success_at.or(Some(updated_at))),
            balance_stall_age_seconds: {
                // 只有“忙/不可用”状态才累计停滞时间；最后一次成功余额观测越久越危险。
                let balance_status: String = row
                    .try_get("balance_status")
                    .map_err(|error| storage_error_from(error, "读取 recovery 余额状态失败"))?;
                if matches!(balance_status.as_str(), "busy" | "unavailable" | "unknown") {
                    age_seconds(now, balance_observed_at.or(Some(updated_at)))
                } else {
                    0
                }
            },
            balance_observed_at,
            evidence_last_success_at,
            updated_at,
        });
    }

    // 付款期限已过但付款投递没有发送证明；该判断复用资金/履约的唯一强谓词。
    let unsent = sqlx::query(
        r#"SELECT
               count(*)::bigint AS payment_mail_unsent_count,
               COALESCE(MAX(GREATEST(
                   0,
                   EXTRACT(EPOCH FROM ($1::timestamptz - recovery.payment_deadline_at))::bigint
               )), 0)::bigint AS oldest_payment_mail_unsent_age_seconds
           FROM public.settlement_recoveries AS recovery
           LEFT JOIN public.settlement_obligation_fulfillments AS payment_f
             ON payment_f.obligation_id = recovery.payment_obligation_id
           LEFT JOIN public.deliveries AS payment_d
             ON payment_d.id = payment_f.delivery_id
           WHERE recovery.stage IN ('awaiting_payment', 'payment_overdue')
             AND recovery.payment_deadline_at <= $1
             AND NOT COALESCE(payment_f.execution_state = 'satisfied', false)
             AND NOT COALESCE(public.delivery_has_frozen_sent_proof(
                 payment_d.id, payment_d.result_metadata, payment_d.external_reference
             ), false)"#,
    )
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取付款邮件未寄出水位失败"))?;
    snapshot.payment_mail_unsent_count = unsent
        .try_get("payment_mail_unsent_count")
        .map_err(|error| storage_error_from(error, "读取付款邮件未寄出计数失败"))?;
    snapshot.oldest_payment_mail_unsent_age_seconds = unsent
        .try_get("oldest_payment_mail_unsent_age_seconds")
        .map_err(|error| storage_error_from(error, "读取付款邮件未寄出时间失败"))?;
    Ok(snapshot)
}
