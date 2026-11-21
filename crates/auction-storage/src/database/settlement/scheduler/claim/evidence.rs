//! 结算领取的审计和实时事件证据。

use auction_domain::{CommandId, deterministic_uuid};
use auction_time::Timestamp;
use serde_json::json;
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::operations::audit_health::AuditWriteResult as _;
use crate::database::realtime::SequencedEvent;
use crate::database::realtime::append_sequenced_event;

pub(super) async fn insert_settlement_claim_audit(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
    command_id: CommandId,
    state_version: i64,
    occurred_at: Timestamp,
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
    occurred_at: Timestamp,
) -> Result<(), sqlx::Error> {
    let stream_key = format!("auction:{auction_id}");
    append_sequenced_event(
        transaction,
        SequencedEvent {
            event_id: deterministic_uuid(command_id, "settlement-claim-realtime-event", 1),
            stream_key: &stream_key,
            kind: "auction.settling",
            schema_version: 1,
            payload: json!({ "auction_id": auction_id, "state_version": state_version }),
            state_version,
            origin_command_id: Some(command_id.as_uuid()),
            created_at: Some(occurred_at),
        },
    )
    .await
}
