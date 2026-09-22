//! 从不可变投稿与命令账本恢复邮件接受决定，不依赖可变的远端正文和附件列表。

use auction_application::{
    MailAuctionAcceptance, MailAuctionIntakeLookup, mail_auction_aggregate_id,
};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::*;

pub(super) async fn load_accepted(
    transaction: &mut PostgresCompetitionTransaction,
    lookup: MailAuctionIntakeLookup,
) -> Result<Option<MailAuctionAcceptance>, CompetitionTransactionError> {
    lookup.validate().map_err(|_| failed("MoogleMail 接受决定查询无效"))?;
    let transaction = transaction.transaction_mut()?;
    let row = sqlx::query(
        "SELECT command.command_kind, command.aggregate_id, command.request_hash,
                command.result_code, command.result_payload,
                submission.id AS submission_id, submission.auction_id,
                submission.mailbox_key, submission.external_mail_id,
                submission.seller, submission.received_at, submission.source_kind,
                auction.world_id
         FROM public.auction_commands AS command
         LEFT JOIN public.auction_submissions AS submission
           ON submission.source_command_id = command.command_id
         LEFT JOIN public.auctions AS auction ON auction.id = submission.auction_id
         WHERE command.command_id = $1",
    )
    .bind(lookup.command_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let command_kind: String = row.try_get("command_kind").map_err(map_sql_error)?;
    let aggregate_id: Uuid = row.try_get("aggregate_id").map_err(map_sql_error)?;
    let request_hash: Vec<u8> = row.try_get("request_hash").map_err(map_sql_error)?;
    let result_code: String = row.try_get("result_code").map_err(map_sql_error)?;
    let submission_id: Option<Uuid> = row.try_get("submission_id").map_err(map_sql_error)?;
    let auction_id: Option<Uuid> = row.try_get("auction_id").map_err(map_sql_error)?;
    let mailbox_key: Option<String> = row.try_get("mailbox_key").map_err(map_sql_error)?;
    let external_mail_id: Option<String> =
        row.try_get("external_mail_id").map_err(map_sql_error)?;
    let seller: Option<String> = row.try_get("seller").map_err(map_sql_error)?;
    let received_at: Option<DateTime<Utc>> = row.try_get("received_at").map_err(map_sql_error)?;
    let source_kind: Option<String> = row.try_get("source_kind").map_err(map_sql_error)?;
    let world_id: Option<String> = row.try_get("world_id").map_err(map_sql_error)?;
    if command_kind != "mail_auction_intake"
        || aggregate_id != mail_auction_aggregate_id(&lookup.world_id)
        || request_hash.len() != 32
        || request_hash.iter().all(|byte| *byte == 0)
        || result_code != "accepted"
        || mailbox_key.as_deref() != Some(&lookup.mailbox_key)
        || external_mail_id.as_deref() != Some(&lookup.external_mail_id)
        || seller.as_deref() != Some(&lookup.seller)
        || received_at.map(|value| value.timestamp_micros())
            != Some(lookup.received_at.timestamp_micros())
        || source_kind.as_deref() != Some("mail")
        || world_id.as_deref() != Some(&lookup.world_id)
    {
        return Err(failed("MoogleMail 接受决定身份或账本不一致"));
    }
    let payload: serde_json::Value = row.try_get("result_payload").map_err(map_sql_error)?;
    let result: MailAuctionIntakeResult =
        serde_json::from_value(payload).map_err(|_| failed("MoogleMail 接受决定结果损坏"))?;
    result.validate().map_err(|_| failed("MoogleMail 接受决定结果无效"))?;
    if Some(result.submission_id) != submission_id || Some(result.auction_id) != auction_id {
        return Err(failed("MoogleMail 接受决定与投稿不一致"));
    }

    let rows = sqlx::query(
        "SELECT lot.id, equipment.external_equipment_id
         FROM public.lots AS lot
         INNER JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
         WHERE lot.submission_id = $1 AND lot.auction_id = $2 AND equipment.world_id = $3",
    )
    .bind(result.submission_id)
    .bind(result.auction_id)
    .bind(&lookup.world_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let mut lot_ids = Vec::with_capacity(rows.len());
    let mut attachment_ids = Vec::with_capacity(rows.len());
    for row in rows {
        lot_ids.push(row.try_get::<Uuid, _>("id").map_err(map_sql_error)?);
        attachment_ids
            .push(row.try_get::<String, _>("external_equipment_id").map_err(map_sql_error)?);
    }
    lot_ids.sort_unstable();
    let mut expected_lot_ids = result.lot_ids.clone();
    expected_lot_ids.sort_unstable();
    if lot_ids != expected_lot_ids {
        return Err(failed("MoogleMail 接受决定的原标的集合不完整"));
    }
    attachment_ids.sort_unstable();
    let accepted = MailAuctionAcceptance { result, attachment_ids };
    accepted.validate().map_err(|_| failed("MoogleMail 接受决定的原附件集合无效"))?;
    Ok(Some(accepted))
}
