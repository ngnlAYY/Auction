//! Admin 装备录入的审计和 realtime 事实追加。

use auction_application::admin::AdminIntakeRecord;
use auction_application::core::CompetitionTransactionError;
use serde_json::json;
use uuid::Uuid;

use crate::database::audit_health::AuditWriteResult as _;
use crate::database::competition::{PostgresCompetitionTransaction, map_sql_error};
use crate::database::realtime::allocate_stream_sequence;

use super::failed;

pub(super) async fn append_admin_intake_record_inner(
    transaction: &mut PostgresCompetitionTransaction,
    record: AdminIntakeRecord,
) -> Result<(), CompetitionTransactionError> {
    record.result.validate().map_err(|_| failed("Admin 装备录入审计结果无效"))?;
    let transaction = transaction.transaction_mut()?;
    let metadata = json!({
        "command_id": record.command_id.as_uuid(),
        "auction_id": record.result.auction_id,
        "world_id": record.world_id,
        "lot_id": record.result.lot_id,
        "lot_number": record.result.lot_number,
        "preview_revision": record.result.preview_revision,
        "state_version": record.result.auction_state_version,
    });
    sqlx::query(
        r#"INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
           VALUES ($1, 'admin', $2, 'auction.admin_intake_item', 'lot', $3,
                   'success', $4, 'admin', $5)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&record.actor_id)
    .bind(record.result.lot_id.to_string())
    .bind(record.request_id.as_deref())
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;

    let stream_key = format!("auction:{}", record.result.auction_id);
    let sequence =
        allocate_stream_sequence(transaction, &stream_key, None).await.map_err(map_sql_error)?;
    let payload = json!({
        "world": record.world_id,
        "auction_id": record.result.auction_id,
        "preview_revision": record.result.preview_revision,
    });
    sqlx::query(
        r#"INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version,
             public_payload, state_version, origin_command_id)
           VALUES ($1, $2, $3, 'preview_catalog_changed', 1, $4, $5, $6)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&stream_key)
    .bind(sequence)
    .bind(payload)
    .bind(record.result.auction_state_version)
    .bind(record.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}
