//! 收口已失去资格且从未开始发送的提醒；不覆盖 sending 或未知外部结果。
//! 资格失效包括取消/重新领先与 lot 有效截止已过（决策 14）；两者共用同一预检退役语义。

use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::mapping::map_storage_error;
use crate::database::delivery::audit::{DeliveryAudit, FAIL, append as append_delivery_audit};

/// 预检退役 UPDATE 的固定尾巴：状态、租约、完成时间、围栏与 RETURNING 列完全一致，
/// 只有 `failure_code` 与 `reason_code` 由调用点区分。用 `concat!` 展开，保持编译期常量
/// SQL 与 `$1` 参数顺序不变。
macro_rules! retire_tail {
    ($failure_code:literal, $reason_code:literal) => {
        concat!(
            "UPDATE public.deliveries AS delivery
        SET status = 'failed', lease_token = NULL, lease_expires_at = NULL,
            completed_at = GREATEST($1, delivery.created_at, delivery.claimed_at),
            external_reference = NULL, failure_code = '",
            $failure_code,
            "',
            result_metadata = '{\"outcome_unknown\":false,\"preflight_blocked\":true,\"reason_code\":\"",
            $reason_code,
            "\"}'::jsonb,
            fencing_version = delivery.fencing_version + 1,
            updated_at = GREATEST(CURRENT_TIMESTAMP, $1, delivery.created_at, delivery.claimed_at)
        FROM retired WHERE delivery.id = retired.id
        RETURNING delivery.id, delivery.command_id, delivery.fencing_version, delivery.attempt_count,
                  delivery.payload, delivery.origin_kind"
        )
    };
}

pub(super) async fn retire_expired(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    now: DateTime<Utc>,
    notifications: &dyn auction_application::notification_metrics::NotificationRecorder,
) -> Result<(), auction_application::core::RepositoryError> {
    let rows = sqlx::query(concat!(
        r#"WITH retired AS (
            SELECT deliveries.id FROM public.deliveries AS deliveries
            WHERE deliveries.status IN ('pending', 'claimed')
              AND deliveries.sending_at IS NULL AND "#,
        crate::database::lot_watch::delivery_watch_expired!(),
        r#"
            ORDER BY deliveries.created_at, deliveries.id
            LIMIT 100 FOR UPDATE OF deliveries SKIP LOCKED
        )"#,
        retire_tail!("lot_watch_invalidated", "lot_watch_invalidated")
    ))
    .bind(now)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    record_retirement(transaction, rows, now, notifications, "lot_watch_invalidated").await?;
    retire_pending(transaction, now, notifications).await
}

/// 玩家改名后冻结收件人已失效：在围栏前退役尚未发送的提醒并记录原因，
/// 不把它当成发送失败或人工对账，也不改写原冻结载荷。
pub(super) async fn retire_renamed(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    now: DateTime<Utc>,
    notifications: &dyn auction_application::notification_metrics::NotificationRecorder,
) -> Result<(), auction_application::core::RepositoryError> {
    let rows = sqlx::query(concat!(
        r#"WITH retired AS (
            SELECT deliveries.id FROM public.deliveries AS deliveries
            WHERE deliveries.status IN ('pending', 'claimed')
              AND deliveries.sending_at IS NULL AND "#,
        crate::database::lot_watch::delivery_watch_renamed!(),
        r#"
            ORDER BY deliveries.created_at, deliveries.id
            LIMIT 100 FOR UPDATE OF deliveries SKIP LOCKED
        )"#,
        retire_tail!("recipient_renamed", "recipient_renamed")
    ))
    .bind(now)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    record_retirement(transaction, rows, now, notifications, "recipient_renamed").await
}

/// 投稿接收回执在 sending 围栏前复核原场次；已开拍或撤场的场次不再补寄回执。
/// 冻结载荷与投稿事实都不改写。
pub(super) async fn retire_receipts(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    now: DateTime<Utc>,
    notifications: &dyn auction_application::notification_metrics::NotificationRecorder,
) -> Result<(), auction_application::core::RepositoryError> {
    let rows = sqlx::query(concat!(
        r#"WITH retired AS (
            SELECT deliveries.id FROM public.deliveries AS deliveries
            WHERE deliveries.status IN ('pending', 'claimed')
              AND deliveries.sending_at IS NULL
              AND deliveries.origin_kind = 'mail_intake'
              AND deliveries.payload_operation = 'send'
              AND deliveries.payload_template = 'auction_submission_receipt'
              AND EXISTS (
                  SELECT 1
                  FROM public.auction_submissions AS submission
                  INNER JOIN public.auctions AS auction ON auction.id = submission.auction_id
                  WHERE submission.source_kind = 'mail'
                    AND submission.external_mail_id = deliveries.payload ->> 'external_mail_id'
                    AND auction.world_id = deliveries.payload_world
                    AND auction.status NOT IN ('draft', 'preview')
              )
            ORDER BY deliveries.created_at, deliveries.id
            LIMIT 100 FOR UPDATE OF deliveries SKIP LOCKED
        )"#,
        retire_tail!("auction_notice_invalidated", "auction_opened")
    ))
    .bind(now)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    record_retirement(transaction, rows, now, notifications, "auction_notice_invalidated").await
}

/// 卖方汇总与换季提醒在 sending 围栏前复核冻结收件人与冻结赛季边界。
///
/// 卖方汇总只按收件人改名退役；换季提醒在边界已过、义务全部完成或收件人改名时退役。
/// 冻结载荷、结算结果与义务状态都不改写。
pub(super) async fn retire_player_notifications(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    now: DateTime<Utc>,
    notifications: &dyn auction_application::notification_metrics::NotificationRecorder,
) -> Result<(), auction_application::core::RepositoryError> {
    // 冻结收件人已改名：该名称不再是任何当前用户，且存在未撤回的改名事实。
    let rows = sqlx::query(concat!(
        r#"WITH retired AS (
            SELECT deliveries.id FROM public.deliveries AS deliveries
            WHERE deliveries.status IN ('pending', 'claimed')
              AND deliveries.sending_at IS NULL
              AND deliveries.origin_kind IN ('seller_summary', 'season_reminder')
              AND NOT EXISTS (
                  SELECT 1 FROM public.users AS current
                  WHERE current.username = deliveries.recipient
              )
              AND EXISTS (
                  SELECT 1 FROM public.user_renames AS rename
                  WHERE rename.previous_username = deliveries.recipient
                    AND rename.reverted_at IS NULL
              )
            ORDER BY deliveries.created_at, deliveries.id
            LIMIT 100 FOR UPDATE OF deliveries SKIP LOCKED
        )"#,
        retire_tail!("recipient_renamed", "recipient_renamed")
    ))
    .bind(now)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    record_retirement(transaction, rows, now, notifications, "recipient_renamed").await?;

    // 冻结赛季边界已过：提醒不再可行动，按预检退役，不进入发送或人工对账。
    let rows = sqlx::query(concat!(
        r#"WITH retired AS (
            SELECT deliveries.id FROM public.deliveries AS deliveries
            WHERE deliveries.status IN ('pending', 'claimed')
              AND deliveries.sending_at IS NULL
              AND deliveries.origin_kind = 'season_reminder'
              AND deliveries.payload ->> 'external_mail_id' ~ '^season-end:'
              AND (substring(deliveries.payload ->> 'external_mail_id' from 12))::timestamptz <= $1
            ORDER BY deliveries.created_at, deliveries.id
            LIMIT 100 FOR UPDATE OF deliveries SKIP LOCKED
        )"#,
        retire_tail!("season_notice_invalidated", "season_boundary_passed")
    ))
    .bind(now)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    record_retirement(transaction, rows, now, notifications, "season_notice_invalidated").await?;

    // 边界尚未到达但义务已全部完成：提醒已无行动价值，退役并记录原因。
    // 义务口径与扫描、发送围栏一致：未激活的 contingent 退款不算未完成义务。
    let rows = sqlx::query(concat!(
        r#"WITH retired AS (
            SELECT deliveries.id FROM public.deliveries AS deliveries
            WHERE deliveries.status IN ('pending', 'claimed')
              AND deliveries.sending_at IS NULL
              AND deliveries.origin_kind = 'season_reminder'
              AND NOT "#,
        season_reminder_outstanding_gate!(),
        r#"
            ORDER BY deliveries.created_at, deliveries.id
            LIMIT 100 FOR UPDATE OF deliveries SKIP LOCKED
        )"#,
        retire_tail!("season_notice_invalidated", "season_reminder_resolved")
    ))
    .bind(now)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    record_retirement(transaction, rows, now, notifications, "season_notice_invalidated").await
}

/// 每条退役行追加一条确定未写入的失败审计与处置事实，并按通知种类记一次 retired 指标；
/// 不改变 failed 状态语义。
async fn record_retirement(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    rows: Vec<sqlx::postgres::PgRow>,
    now: DateTime<Utc>,
    notifications: &dyn auction_application::notification_metrics::NotificationRecorder,
    failure_code: &str,
) -> Result<(), auction_application::core::RepositoryError> {
    for row in rows {
        let delivery_id: Uuid = row.try_get("id").map_err(map_storage_error)?;
        let payload: serde_json::Value = row.try_get("payload").map_err(map_storage_error)?;
        let origin_kind: String = row.try_get("origin_kind").map_err(map_storage_error)?;
        if let Some(kind) =
            auction_application::notification_metrics::NotificationKind::from_delivery(
                &origin_kind,
                payload.get("operation").and_then(serde_json::Value::as_str),
                payload.get("template").and_then(serde_json::Value::as_str),
            )
        {
            notifications.observe_stage(
                kind,
                auction_application::notification_metrics::NotificationStage::Retired,
            );
        }
        let command_id: Uuid = row.try_get("command_id").map_err(map_storage_error)?;
        let fencing_version: i64 = row.try_get("fencing_version").map_err(map_storage_error)?;
        let attempt_count: i32 = row.try_get("attempt_count").map_err(map_storage_error)?;
        let request_id = command_id.to_string();
        append_delivery_audit(
            transaction,
            DeliveryAudit {
                occurred_at: now,
                actor_type: "worker",
                actor_id: "delivery-worker",
                action: FAIL,
                delivery_id,
                request_id: Some(&request_id),
                source: "worker",
                metadata: serde_json::json!({"status": "failed", "failure_code": failure_code,
                "fencing_version": fencing_version, "attempt_count": attempt_count}),
            },
        )
        .await
        .map_err(map_storage_error)?;
        // 同一事务追加统一处置事实：预检退役以投递行自身（id、fence、失败码）为证，
        // 没有外部引用。是否解除失败计数由失败码闭集在领域层决定。
        crate::database::delivery::disposition::append(
            transaction,
            crate::database::delivery::disposition::DeliveryDispositionFact {
                delivery_id,
                fencing_version,
                kind: auction_domain::DeliveryDispositionKind::PreflightRetired,
                source: auction_domain::DeliveryDispositionSource::default(),
                actor_source: auction_domain::DeliveryDispositionActorSource::Worker,
                actor_id: "delivery-worker",
                reason: None,
                reason_code: Some(failure_code),
                detail: None,
                command_id: None,
                request_digest: None,
            },
        )
        .await
        .map_err(map_storage_error)?;
    }
    Ok(())
}

/// lot 已结束或错过有效截止的待定提醒按决策 14 退役；
/// 仍可能有可用慢速代理反击的待定行保持 pending，等竞价事务收口。
async fn retire_pending(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    now: DateTime<Utc>,
    notifications: &dyn auction_application::notification_metrics::NotificationRecorder,
) -> Result<(), auction_application::core::RepositoryError> {
    let rows = sqlx::query(
        "UPDATE public.lot_watch_pending_notifications AS pending
         SET status = 'retired', resolution_reason = 'lot_closed', resolved_at = $1
         FROM public.lots AS lot, public.auctions AS auction
         WHERE pending.lot_id = lot.id AND auction.id = lot.auction_id
           AND pending.status = 'pending'
           AND (
               auction.status <> 'live'
               OR lot.effective_end_at IS NULL
               OR lot.effective_end_at <= $1
           )
         RETURNING pending.user_id, pending.lot_id, pending.leadership_cycle",
    )
    .bind(now)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    for row in rows {
        let user_id: Uuid = row.try_get("user_id").map_err(map_storage_error)?;
        let lot_id: Uuid = row.try_get("lot_id").map_err(map_storage_error)?;
        let cycle: i64 = row.try_get("leadership_cycle").map_err(map_storage_error)?;
        notifications.observe_stage(
            auction_application::notification_metrics::NotificationKind::LotWatchMail,
            auction_application::notification_metrics::NotificationStage::Retired,
        );
        sqlx::query(
            "INSERT INTO public.audit_records
             (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
              result, source, metadata)
             VALUES ($1, $2, 'worker', 'delivery-worker', 'lot_watch.pending_retired',
                     'lot', $3, 'success', 'worker', $4)",
        )
        .bind(Uuid::new_v4())
        .bind(now)
        .bind(lot_id.to_string())
        .bind(serde_json::json!({
            "status": "retired",
            "reason": "lot_closed",
            "user_id": user_id,
            "cycle": cycle,
        }))
        .execute(&mut **transaction)
        .await
        .map_err(map_storage_error)?;
    }
    Ok(())
}
