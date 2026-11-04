//! 收口已失去资格且从未开始发送的提醒；不覆盖 sending 或未知外部结果。
//! 资格失效包括取消/重新领先与 lot 有效截止已过（决策 14）；两者共用同一预检退役语义。

use super::*;

pub(super) async fn retire_expired(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    now: DateTime<Utc>,
    notifications: &dyn auction_application::NotificationRecorder,
) -> Result<(), auction_application::RepositoryError> {
    let rows = sqlx::query(concat!(
        r#"WITH retired AS (
            SELECT deliveries.id FROM public.deliveries AS deliveries
            WHERE deliveries.status IN ('pending', 'claimed')
              AND deliveries.sending_at IS NULL AND "#,
        crate::database::lot_watch::delivery_watch_expired!(),
        r#"
            ORDER BY deliveries.created_at, deliveries.id
            LIMIT 100 FOR UPDATE OF deliveries SKIP LOCKED
        )
        UPDATE public.deliveries AS delivery
        SET status = 'failed', lease_token = NULL, lease_expires_at = NULL,
            completed_at = GREATEST($1, delivery.created_at, delivery.claimed_at),
            external_reference = NULL, failure_code = 'lot_watch_invalidated',
            result_metadata = '{"outcome_unknown":false,"preflight_blocked":true,"reason_code":"lot_watch_invalidated"}'::jsonb,
            fencing_version = delivery.fencing_version + 1,
            updated_at = GREATEST(CURRENT_TIMESTAMP, $1, delivery.created_at, delivery.claimed_at)
        FROM retired WHERE delivery.id = retired.id
        RETURNING delivery.id, delivery.command_id, delivery.fencing_version, delivery.attempt_count,
                  delivery.payload, delivery.origin_kind"#
    ))
    .bind(now).fetch_all(&mut **transaction).await.map_err(map_storage_error)?;
    for row in rows {
        let delivery_id: Uuid = row.try_get("id").map_err(map_storage_error)?;
        // 提醒在 sending 围栏前失去资格：只记录一次退役，不改变 failed 状态语义。
        let payload: serde_json::Value = row.try_get("payload").map_err(map_storage_error)?;
        let origin_kind: String = row.try_get("origin_kind").map_err(map_storage_error)?;
        if let Some(kind) = auction_application::NotificationKind::from_delivery(
            &origin_kind,
            payload.get("operation").and_then(serde_json::Value::as_str),
            payload.get("template").and_then(serde_json::Value::as_str),
        ) {
            notifications.observe_stage(kind, auction_application::NotificationStage::Retired);
        }
        let command_id: Uuid = row.try_get("command_id").map_err(map_storage_error)?;
        let fencing_version: i64 = row.try_get("fencing_version").map_err(map_storage_error)?;
        let attempt_count: i32 = row.try_get("attempt_count").map_err(map_storage_error)?;
        let request_id = command_id.to_string();
        append_delivery_audit(transaction, DeliveryAudit {
            occurred_at: now, actor_type: "worker", actor_id: "delivery-worker", action: FAIL,
            delivery_id, request_id: Some(&request_id), source: "worker",
            metadata: serde_json::json!({"status": "failed", "failure_code": "lot_watch_invalidated",
                "fencing_version": fencing_version, "attempt_count": attempt_count}),
        }).await.map_err(map_storage_error)?;
    }
    retire_pending(transaction, now, notifications).await
}

/// W14.11：lot 已结束或错过有效截止的待定提醒按决策 14 退役；
/// 仍可能有可用慢速代理反击的待定行保持 pending，等竞价事务收口。
async fn retire_pending(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    now: DateTime<Utc>,
    notifications: &dyn auction_application::NotificationRecorder,
) -> Result<(), auction_application::RepositoryError> {
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
            auction_application::NotificationKind::LotWatchMail,
            auction_application::NotificationStage::Retired,
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
