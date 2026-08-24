use super::*;

use serde_json::json;
use uuid::Uuid;

use crate::database::audit_health::AuditWriteResult as _;

pub(super) async fn append_mail_auction_record_inner(
    transaction: &mut PostgresCompetitionTransaction,
    record: MailAuctionIntakeRecord,
) -> Result<(), CompetitionTransactionError> {
    record.result.validate().map_err(|_| failed("MoogleMail 拍卖审计结果无效"))?;
    let transaction = transaction.transaction_mut()?;
    let metadata = json!({
        "command_id": record.command_id.as_uuid(),
        "auction_id": record.result.auction_id,
        "submission_id": record.result.submission_id,
        "preview_revision": record.result.preview_revision,
        "state_version": record.result.auction_state_version,
        "added_count": record.result.added_count,
        "world_id": record.world_id,
        "mailbox_key": record.mailbox_key,
    });
    sqlx::query(
        r#"INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
           VALUES ($1, 'worker', $2, 'auction.mail_intake', 'auction', $3,
                   'success', NULL, 'worker', $4)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&record.mailbox_key)
    .bind(record.result.auction_id.to_string())
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;

    let stream_key = format!("auction:{}", record.result.auction_id);
    sqlx::query(
        "INSERT INTO public.realtime_streams (stream_key) VALUES ($1)
         ON CONFLICT (stream_key) DO NOTHING",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let sequence = sqlx::query_scalar::<_, i64>(
        "SELECT next_sequence FROM public.realtime_streams WHERE stream_key = $1 FOR UPDATE",
    )
    .bind(&stream_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE public.realtime_streams
         SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
         WHERE stream_key = $1",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version,
             public_payload, state_version, origin_command_id)
           VALUES ($1, $2, $3, 'preview_catalog_changed', 1, $4, $5, $6)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&stream_key)
    .bind(sequence)
    .bind(json!({
        "world": record.world_id,
        "auction_id": record.result.auction_id,
        "preview_revision": record.result.preview_revision,
    }))
    .bind(record.result.auction_state_version)
    .bind(record.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}
