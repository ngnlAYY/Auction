//! 结算领取的审计和实时事件证据。

use crate::database::TrackedTransaction as Transaction;
use auction_domain::{CommandId, deterministic_uuid};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::operations::audit_health::AuditWriteResult as _;

pub(super) async fn insert_settlement_claim_audit(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
    command_id: CommandId,
    state_version: i64,
    occurred_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
                request_id, source, metadata)
           VALUES ($1, $2, 'system', 'auction-worker', 'auction.settlement_claim', 'auction',
                   $3, 'success', NULL, 'worker', $4)"#,
    )
    .bind(deterministic_uuid(command_id, "settlement-claim-audit", 1))
    .bind(occurred_at)
    .bind(auction_id.to_string())
    .bind(json!({ "auction_id": auction_id.to_string(), "state_version": state_version }))
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
    occurred_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let stream_key = format!("auction:{auction_id}");
    let sequence = crate::database::realtime::allocate_stream_sequence(
        transaction,
        &stream_key,
        Some(occurred_at),
    )
    .await?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
               (event_id, stream_key, stream_sequence, kind, schema_version,
                public_payload, state_version, origin_command_id, created_at)
           VALUES ($1, $2, $3, 'auction.settling', 1, $4, $5, $6, $7)"#,
    )
    .bind(deterministic_uuid(command_id, "settlement-claim-realtime-event", 1))
    .bind(&stream_key)
    .bind(sequence)
    .bind(json!({ "auction_id": auction_id, "state_version": state_version }))
    .bind(state_version)
    .bind(command_id.as_uuid())
    .bind(occurred_at)
    .execute(&mut **transaction)
    .await
    .map(|_| ())
}
