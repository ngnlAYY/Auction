//! 草稿的本地形状校验与闭集请求组装。
//!
//! 这里只阻止明显不完整的草稿；真正的准入判定仍由服务端预览与事务完成。

use auction_time::SecondsFormat;

use super::evidence::{SELECTABLE_EVIDENCE_KINDS, decode_hex_digest, latest_send_attempt};
use super::state::{SentProofDraft, SentProofIssue};
use crate::api::{
    DeliveryAttempt, DeliveryDetail, DeliveryEvidence, DeliverySentEvidence, LocalTimeError,
    MAX_JSON_SAFE_INTEGER,
};

/// 把 `datetime-local` 输入换算为带 `Z` 的整分钟；秒或小数非零时不猜测。
pub(super) fn sent_minute_from_input(value: &str) -> Result<String, LocalTimeError> {
    let parse = crate::api::parse_local_datetime;
    #[cfg(test)]
    let parse = super::tests::local_time_parser().unwrap_or(parse);
    let parsed = parse(value)?;
    if parsed.to_utc_datetime().second() != 0 || parsed.timestamp_subsec_nanos() != 0 {
        return Err(LocalTimeError::Invalid);
    }
    Ok(sent_minute_text(parsed))
}

/// 未编辑时直接提交证据已记录的 UTC 分钟，保留服务端文本。
pub(super) fn sent_minute_from_timestamp(value: &str) -> Option<String> {
    let parsed = auction_time::Timestamp::parse_from_rfc3339(value).ok()?;
    (parsed.to_utc_datetime().second() == 0 && parsed.timestamp_subsec_nanos() == 0)
        .then(|| value.to_owned())
}

/// 发送分钟统一使用带 `Z` 的 UTC 整分钟文本（`YYYY-MM-DDTHH:MM:00Z`）；入参的秒与小数已为零。
fn sent_minute_text(minute: auction_time::Timestamp) -> String {
    minute.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn valid_sender_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 128 || trimmed.chars().any(char::is_control) {
        return None;
    }
    Some(trimmed.to_owned())
}

fn valid_sender_uid(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if !(1..=64).contains(&trimmed.len())
        || !trimmed.bytes().all(|byte| byte.is_ascii_digit())
        || trimmed.starts_with('0')
    {
        return None;
    }
    Some(trimmed.to_owned())
}

fn parse_amount(value: &str) -> Option<i64> {
    let parsed = value.trim().parse::<i64>().ok()?;
    (0..=MAX_JSON_SAFE_INTEGER).contains(&parsed).then_some(parsed)
}

/// 只有已领取的装备可走缺少旧清单的回执路径。
pub(super) fn missing_observed_ids(
    evidence: &DeliveryEvidence,
    attempt: &DeliveryAttempt,
    collected: bool,
) -> bool {
    evidence.remaining_equipment_ids().is_none()
        && (!collected || evidence.collected == Some(false) || attempt.equipment_ids.is_empty())
}

/// 组合冻结尝试与已采集证据；任一必需字段缺失都返回本地问题码，不提交半成品。
pub(super) fn build_evidence(
    detail: &DeliveryDetail,
    draft: &SentProofDraft,
) -> Result<DeliverySentEvidence, SentProofIssue> {
    let reference = draft.evidence_reference.trim();
    let evidence = detail
        .evidence
        .iter()
        .find(|item| {
            item.reference == reference && SELECTABLE_EVIDENCE_KINDS.contains(&item.kind.as_str())
        })
        .ok_or(SentProofIssue::Evidence)?;
    let attempt = latest_send_attempt(detail).ok_or(SentProofIssue::Attempt)?;
    if missing_observed_ids(evidence, attempt, draft.collected) {
        return Err(SentProofIssue::MissingObservedEquipmentIds);
    }
    let external_mail_id = evidence.external_mail_id.clone().ok_or(SentProofIssue::Evidence)?;
    let observed_at = evidence.observed_at.clone().ok_or(SentProofIssue::Evidence)?;
    let observed_body_digest = evidence
        .body_digest
        .as_deref()
        .and_then(decode_hex_digest)
        .ok_or(SentProofIssue::Digest)?;
    let original_body_digest =
        attempt.body_digest.as_deref().and_then(decode_hex_digest).ok_or(SentProofIssue::Digest)?;
    let recipient = evidence
        .recipient
        .clone()
        .or_else(|| attempt.recipient.clone())
        .ok_or(SentProofIssue::Recipient)?;
    let subject = evidence
        .subject
        .clone()
        .or_else(|| attempt.subject.clone())
        .ok_or(SentProofIssue::Subject)?;
    let sent_minute = if draft.sent_minute_edited {
        sent_minute_from_input(draft.sent_minute.trim())
    } else {
        evidence
            .sent_at
            .as_deref()
            .and_then(sent_minute_from_timestamp)
            .ok_or(LocalTimeError::Invalid)
    }
    .map_err(SentProofIssue::SentMinute)?;
    let sender_name = valid_sender_name(&draft.sender_name).ok_or(SentProofIssue::SenderName)?;
    let sender_uid = valid_sender_uid(&draft.sender_uid).ok_or(SentProofIssue::SenderUid)?;
    let remaining_credits =
        parse_amount(&draft.remaining_credits).ok_or(SentProofIssue::RemainingCredits)?;
    let cod_pending = parse_amount(&draft.cod_pending).ok_or(SentProofIssue::CodPending)?;
    let receipt = if draft.collected {
        if draft.receipt.len() > 2048 {
            return Err(SentProofIssue::Receipt);
        }
        draft.receipt.trim().to_owned()
    } else {
        String::new()
    };
    let equipment_ids = if draft.collected {
        Vec::new()
    } else {
        evidence
            .remaining_equipment_ids()
            .ok_or(SentProofIssue::MissingObservedEquipmentIds)?
            .to_vec()
    };
    Ok(DeliverySentEvidence {
        schema_version: 1,
        world_id: attempt.world_id.clone(),
        external_mail_id,
        sender_uid,
        sender_name,
        recipient,
        subject,
        sent_minute,
        observed_at,
        original_body_digest,
        observed_body_digest,
        remaining_credits,
        cod_pending,
        equipment_ids,
        collected: draft.collected,
        receipt,
    })
}
