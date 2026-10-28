//! 以当前标的的最终邮件入库事实闭合旧动作；只读取历史，不把已完成退回当作当前持有证明。

use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use serde::Deserialize;

use super::{
    AssetConflictTarget, AttachmentPayload, FundingError, Postgres, Row, Transaction, Uuid,
};
use super::{sql_error, valid_attachment_origin};

/// 调用方已持有业务行锁并验证同会话新鲜正向库存；仅返回本标的可精确闭合的历史投递。
pub(super) async fn closed_source_history(
    tx: &mut Transaction<'_, Postgres>,
    target: &AssetConflictTarget<'_>,
) -> Result<Vec<Uuid>, FundingError> {
    // 仅成交自留及无成交返还库存动作共享例外；普通内部路径已由 validation 验证精确装备证明，
    // 专用修复路径也已在写事务核对同一新鲜快照。卖家留款等无库存动作保持原门。
    if !matches!(
        target.kind,
        auction_domain::SettlementObligationKind::WinnerEquipmentCod
            | auction_domain::SettlementObligationKind::UnsoldReturn
            | auction_domain::SettlementObligationKind::WithdrawnReturn
    ) {
        return Ok(Vec::new());
    }
    let row = sqlx::query(
        "SELECT d.id,d.command_id,d.origin_id,d.recipient,d.payload,d.result_metadata,
            d.external_reference,d.completed_at,s.external_mail_id,s.seller,
            im.decision_metadata->'attachment_count' AS attachment_count
         FROM public.settlement_obligations o
         JOIN public.lots l ON l.id=o.lot_id AND l.auction_id=o.auction_id
            AND l.equipment_id=o.equipment_id AND l.submission_id=o.submission_id
         JOIN public.equipment e ON e.id=l.equipment_id
         JOIN public.auction_submissions s ON s.id=l.submission_id AND s.auction_id=l.auction_id
         JOIN public.inbound_mail im ON im.mailbox_key=s.mailbox_key
            AND im.external_mail_id=s.external_mail_id AND im.world_id=e.world_id
         JOIN public.deliveries d ON im.decision_metadata->'delivery_ids'=jsonb_build_array(d.id::text)
         WHERE o.id=$1 AND o.lot_id=$2 AND e.external_equipment_id=$3 AND e.world_id=$4
           AND o.obligation_kind IN ('winner_equipment_cod','unsold_return','withdrawn_return')
           AND s.source_kind='mail' AND s.mailbox_key='mooglemail:'||e.world_id
           AND im.status='accepted' AND im.decision_code='auction_accepted'
           AND im.sender=s.seller AND im.sender=d.recipient
           AND im.decision_metadata->>'submission_id'=s.id::text
           AND im.decision_metadata->>'auction_id'=l.auction_id::text
           AND d.origin_kind='mail_intake' AND d.status='succeeded' AND d.failure_code IS NULL
           AND d.lease_token IS NULL AND d.lease_expires_at IS NULL
           AND d.sending_at IS NOT NULL AND d.completed_at>=d.sending_at
           AND d.completed_at>=im.processed_at",
    )
    .bind(target.obligation_id.as_uuid())
    .bind(target.lot_id.as_uuid())
    .bind(target.external_equipment_id)
    .bind(target.equipment_world.as_str())
    .fetch_optional(&mut **tx)
    .await
    .map_err(sql_error)?;
    let Some(row) = row else {
        return Ok(Vec::new());
    };
    let id: Uuid = row.try_get("id").map_err(sql_error)?;
    let command_id: Uuid = row.try_get("command_id").map_err(sql_error)?;
    let origin_id: String = row.try_get("origin_id").map_err(sql_error)?;
    let mid: String = row.try_get("external_mail_id").map_err(sql_error)?;
    let payload: serde_json::Value = row.try_get("payload").map_err(sql_error)?;
    let metadata: serde_json::Value = row.try_get("result_metadata").map_err(sql_error)?;
    let recipient: String = row.try_get("recipient").map_err(sql_error)?;
    let seller: String = row.try_get("seller").map_err(sql_error)?;
    let reference: Option<String> = row.try_get("external_reference").map_err(sql_error)?;
    let count: serde_json::Value = row.try_get("attachment_count").map_err(sql_error)?;
    let completed_at: DateTime<Utc> = row.try_get("completed_at").map_err(sql_error)?;
    let Some(payload) = source_payload(&payload, &origin_id, command_id, target, "take", &mid)
    else {
        return Ok(Vec::new());
    };
    let proven = match metadata["proof"].as_str() {
        Some("action_removed_or_success_page") => {
            metadata["external_mail_id"] == mid
                && metadata["expected_credits"] == 0
                && metadata["expected_cod"] == 0
                && metadata["expected_attachment_ids"] == serde_json::json!(payload.attachment_ids)
                && metadata.get("outcome_unknown").is_none_or(|value| value == false)
        }
        // 历史 unknown 的最终只读恢复证明仍保留原错误字段，精确恢复 MID/引用才闭合。
        Some("action_recovered_inbox_v1") => {
            metadata["external_reference_proven"] == true
                && metadata["recovered_mid"] == mid
                && matches!(
                    metadata["reason"].as_str(),
                    Some("inbox_mail_absent" | "attachments_absent")
                )
                && metadata["attachment_count"]
                    .as_u64()
                    .and_then(|count| usize::try_from(count).ok())
                    == Some(payload.attachment_ids.len())
        }
        _ => false,
    };
    if !proven
        || metadata["operation"] != "take"
        || recipient != seller
        || count.as_u64().and_then(|count| usize::try_from(count).ok())
            != Some(payload.attachment_ids.len())
        || reference.as_deref()
            != Some(format!("mooglemail:{}:take:{mid}", target.equipment_world.as_str()).as_str())
    {
        return Ok(Vec::new());
    }

    let mut closed = vec![id];
    // 历史退回数量不设截断；逐行验证完整证明，只累计可闭合的 UUID。
    let mut returns = sqlx::query(
        "SELECT d.id,d.command_id,d.origin_id,d.payload,d.result_metadata,d.external_reference,
            im.external_mail_id,im.decision_metadata->'attachment_count' AS attachment_count
         FROM public.deliveries d
         JOIN public.inbound_mail im ON im.external_mail_id=d.payload->>'external_mail_id'
            AND im.world_id=$2 AND im.mailbox_key='mooglemail:'||im.world_id
         WHERE d.origin_kind='mail_intake' AND d.payload->>'world'=$2
           AND d.payload->>'operation'='return'
           AND d.status='succeeded' AND d.failure_code IS NULL
           AND d.lease_token IS NULL AND d.lease_expires_at IS NULL
           AND d.sending_at IS NOT NULL AND d.completed_at>=d.sending_at
           AND d.completed_at<$3 AND d.recipient=$4
           AND im.status='rejected' AND im.decision_code='auction_equipment_rejected'
           AND im.sender=d.recipient
           AND im.decision_metadata->'delivery_ids' @> jsonb_build_array(d.id::text)
           AND im.decision_metadata->>'submission_id' IS NULL
           AND NOT EXISTS(SELECT 1 FROM public.auction_submissions other
               WHERE other.mailbox_key=im.mailbox_key AND other.external_mail_id=im.external_mail_id)
           AND EXISTS(SELECT 1 FROM jsonb_array_elements_text(CASE
               WHEN jsonb_typeof(d.payload->'attachment_ids')='array'
               THEN d.payload->'attachment_ids' ELSE '[]'::jsonb END) attachment
               WHERE attachment=$1)",
    )
    .bind(target.external_equipment_id)
    .bind(target.equipment_world.as_str())
    .bind(completed_at)
    .bind(&recipient)
    .fetch(&mut **tx);
    while let Some(row) = returns.try_next().await.map_err(sql_error)? {
        let id: Uuid = row.try_get("id").map_err(sql_error)?;
        let command_id: Uuid = row.try_get("command_id").map_err(sql_error)?;
        let origin_id: String = row.try_get("origin_id").map_err(sql_error)?;
        let mid: String = row.try_get("external_mail_id").map_err(sql_error)?;
        let payload: serde_json::Value = row.try_get("payload").map_err(sql_error)?;
        let metadata: serde_json::Value = row.try_get("result_metadata").map_err(sql_error)?;
        let count: serde_json::Value = row.try_get("attachment_count").map_err(sql_error)?;
        let reference: Option<String> = row.try_get("external_reference").map_err(sql_error)?;
        if source_payload(&payload, &origin_id, command_id, target, "return", &mid).is_some_and(
            |payload| {
                count.as_u64().and_then(|count| usize::try_from(count).ok())
                    == Some(payload.attachment_ids.len())
            },
        ) && metadata["proof"] == "action_removed_or_success_page"
            && metadata["operation"] == "return"
            && metadata["external_mail_id"] == mid
            && metadata.get("outcome_unknown").is_none_or(|value| value == false)
            && reference.as_deref()
                == Some(
                    format!("mooglemail:{}:return:{mid}", target.equipment_world.as_str()).as_str(),
                )
        {
            closed.push(id);
        }
    }
    Ok(closed)
}

/// 复用普通 outbox 的拒绝未知字段解码器；只接受无资金、无正文的同世界精确装备动作。
fn source_payload(
    value: &serde_json::Value,
    origin_id: &str,
    command_id: Uuid,
    target: &AssetConflictTarget<'_>,
    operation: &str,
    mid: &str,
) -> Option<AttachmentPayload> {
    let payload = AttachmentPayload::deserialize(value).ok()?;
    (payload.schema_version == 1
        && payload.world == target.equipment_world.as_str()
        && payload.operation == operation
        && payload.external_mail_id == mid
        && auction_application::valid_external_positive_id(mid)
        && !command_id.is_nil()
        && payload.credits == 0
        && payload.cod == 0
        && payload.template.is_none()
        && payload.result_code.is_none()
        && payload.body.is_none()
        && payload.user_key_user_id.is_none()
        && !payload.attachment_ids.is_empty()
        && payload.attachment_ids.len() <= 1000
        && payload.attachment_ids.iter().enumerate().all(|(index, eid)| {
            eid.len() <= 32
                && auction_application::valid_external_positive_id(eid)
                && !payload.attachment_ids[..index].contains(eid)
        })
        && payload.attachment_ids.iter().any(|eid| eid == target.external_equipment_id)
        && valid_attachment_origin(&payload, "mail_intake", origin_id, command_id, false))
    .then_some(payload)
}
