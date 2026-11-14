//! MoogleMail 拍卖入站的审计记录追加。

use auction_application::core::CompetitionTransactionError;
use auction_application::mail::MailAuctionIntakeRecord;
use serde_json::json;
use uuid::Uuid;

use crate::database::auction::competition::{PostgresCompetitionTransaction, map_sql_error};
use crate::database::mail::intake::failed;
use crate::database::operations::audit_health::AuditWriteResult as _;
use crate::database::realtime::SequencedEvent;
use crate::database::realtime::append_sequenced_event;
use crate::database::realtime::preview_catalog_changed_payload;

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
    append_sequenced_event(
        transaction,
        SequencedEvent {
            event_id: Uuid::new_v4(),
            stream_key: &stream_key,
            kind: "preview_catalog_changed",
            schema_version: 1,
            payload: preview_catalog_changed_payload(
                &record.world_id,
                record.result.auction_id,
                record.result.preview_revision,
            ),
            state_version: record.result.auction_state_version,
            origin_command_id: record.command_id.as_uuid(),
            created_at: None,
        },
    )
    .await
    .map_err(map_sql_error)?;
    Ok(())
}
