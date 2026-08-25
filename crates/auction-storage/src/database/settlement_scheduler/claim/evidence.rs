//! 结算领取的审计和实时事件证据。

use auction_domain::{CommandId, deterministic_uuid};
use serde_json::json;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::database::audit_health::AuditWriteResult as _;

pub(super) async fn insert_settlement_claim_audit(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
    command_id: CommandId,
    worker_id: &str,
    state_version: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, actor_type, actor_id, action, entity_type, entity_id, result,
                request_id, source, metadata)
           VALUES ($1, 'worker', $2, 'settlement.claimed', 'auction', $3, 'success', $4, 'worker', $5)"#,
    )
    .bind(Uuid::new_v4())
    .bind(worker_id)
    .bind(auction_id.to_string())
    .bind(command_id.to_string())
    .bind(json!({ "command_id": command_id, "state_version": state_version }))
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map(|_| ())
}

pub(super) async fn insert_settlement_claim_event(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
    command_id: CommandId,
    state_version: i64,
) -> Result<(), sqlx::Error> {
    let stream_key = format!("auction:{auction_id}");
    sqlx::query(
        "INSERT INTO public.realtime_streams (stream_key) VALUES ($1)
         ON CONFLICT (stream_key) DO NOTHING",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await?;
    let row = sqlx::query(
        r#"UPDATE public.realtime_streams
           SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
           WHERE stream_key = $1
           RETURNING next_sequence - 1 AS stream_sequence"#,
    )
    .bind(&stream_key)
    .fetch_one(&mut **transaction)
    .await?;
    let sequence: i64 = row.try_get("stream_sequence")?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
               (event_id, stream_key, stream_sequence, kind, schema_version,
                public_payload, state_version, origin_command_id)
           VALUES ($1, $2, $3, 'auction.settling', 1, $4, $5, $6)"#,
    )
    .bind(deterministic_uuid(command_id, "settlement-claim-realtime-event", 1))
    .bind(&stream_key)
    .bind(sequence)
    .bind(json!({ "auction_id": auction_id, "state_version": state_version }))
    .bind(state_version)
    .bind(command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map(|_| ())
}
