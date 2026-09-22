//! 将历史邮件资产处置冲突转入可观测人工态，不改写原命令或外部 payload。

use super::*;

pub(super) async fn quarantine_mail_dispositions(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    now: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let rows = sqlx::query(concat!(
        r#"WITH conflicted AS (
            SELECT deliveries.id FROM public.deliveries AS deliveries
            WHERE deliveries.origin_kind = 'mail_intake'
              AND deliveries.payload ->> 'operation' IN ('take', 'return')
              AND deliveries.status IN ('pending', 'claimed')
              AND deliveries.sending_at IS NULL
              AND NOT "#,
        delivery_mail_disposition_gate!(),
        r#"
            ORDER BY deliveries.created_at, deliveries.id
            LIMIT 100 FOR UPDATE OF deliveries SKIP LOCKED
        )
        UPDATE public.deliveries AS delivery
        SET status = 'manual_reconcile', lease_token = NULL, lease_expires_at = NULL,
            completed_at = GREATEST($1, delivery.created_at, delivery.claimed_at),
            external_reference = NULL, failure_code = 'mail_disposition_conflict',
            result_metadata = '{"outcome_unknown":true,"preflight_blocked":true,"reason_code":"mail_disposition_conflict"}'::jsonb,
            fencing_version = delivery.fencing_version + 1,
            updated_at = GREATEST(CURRENT_TIMESTAMP, $1, delivery.created_at, delivery.claimed_at)
        FROM conflicted
        WHERE delivery.id = conflicted.id
        RETURNING delivery.id, delivery.command_id, delivery.fencing_version,
                  delivery.attempt_count"#
    ))
    .bind(now)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    for row in rows {
        let delivery_id: Uuid = row.try_get("id").map_err(map_storage_error)?;
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
                action: super::super::delivery_audit::QUARANTINE_MAIL_DISPOSITION,
                delivery_id,
                request_id: Some(&request_id),
                source: "worker",
                metadata: serde_json::json!({
                    "status": "manual_reconcile",
                    "failure_code": "mail_disposition_conflict",
                    "fencing_version": fencing_version,
                    "attempt_count": attempt_count,
                }),
            },
        )
        .await
        .map_err(map_storage_error)?;
    }
    Ok(())
}
