//! Sent/Take 恢复的纯比对：冻结发送、恢复后状态、通知主题与领取资产是否与原尝试一致。

use crate::ports::delivery::DeliverySendExpectation;
use crate::ports::mail::{
    MoogleMailboxInboxAssets, MoogleMailboxSentEntry, MoogleMailboxSentState,
    MoogleMailboxUnprovenSend, mail_recipient_is_list_projection, sent_list_minute_start,
};

pub(super) fn frozen_entry_matches(
    candidate: &MoogleMailboxUnprovenSend,
    expectation: &DeliverySendExpectation,
    entry: &MoogleMailboxSentEntry,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    candidate.world_id == expectation.world_id
        && mail_recipient_is_list_projection(&entry.recipient, &expectation.recipient)
        && notice_subject_matches(&expectation.subject, &entry.subject)
        // 列表时间仅精确到分钟，发送通常在开始的同一分钟完成，开始围栏按分钟起点比较。
        // 同分钟更早的同内容邮件由详情唯一匹配、MID 绑定唯一和存储侧整分钟歧义检查排除。
        && entry.sent_at >= sent_list_minute_start(candidate.sending_at)
        && entry.sent_at <= candidate.outcome_recorded_at
        && entry.sent_at <= now
}

pub(super) fn frozen_state_matches(
    expectation: &DeliverySendExpectation,
    state: &MoogleMailboxSentState,
) -> bool {
    let (Some(expected), Some(observed)) = (&expectation.credential, &state.observation_credential)
    else {
        return false;
    };
    // 重启可改变当前 owner generation；原凭据 ID/version 不得改变，当前 generation 由存储锁验。
    expected.credential_id == observed.credential_id
        && expected.credential_version == observed.credential_version
        && state.recipient.eq_ignore_ascii_case(&expectation.recipient)
        && state.subject == expectation.subject
        && state.body_digest == expectation.body_digest
        && state.cod_pending == expectation.cod
        && state.remaining_credits == expectation.credits
        && state.remaining_equipment_ids == expectation.equipment_ids
}

pub(super) fn same_send_content(
    left: &DeliverySendExpectation,
    right: &DeliverySendExpectation,
) -> bool {
    let same_credential = match (&left.credential, &right.credential) {
        (Some(left), Some(right)) => {
            left.credential_id == right.credential_id
                && left.credential_version == right.credential_version
        }
        _ => false,
    };
    same_credential
        && left.world_id == right.world_id
        && left.recipient.eq_ignore_ascii_case(&right.recipient)
        && left.subject == right.subject
        && left.body_digest == right.body_digest
        && left.credits == right.credits
        && left.cod == right.cod
        && left.equipment_ids == right.equipment_ids
}

/// 恢复仅接受尚未领取的完整冻结资产矩阵；领取后原 Credits/COD 已不可从剩余值证明。
pub(super) fn recovered_state_matches(
    candidate: &MoogleMailboxUnprovenSend,
    state: &MoogleMailboxSentState,
) -> bool {
    if !state.recipient.eq_ignore_ascii_case(&candidate.recipient)
        || state.collected
        || state.cod_pending != candidate.cod
        || state.remaining_credits != candidate.credits
        || state.remaining_equipment_ids != candidate.attachment_ids
    {
        return false;
    }
    match candidate.template.as_str() {
        "settlement_payment_carrier" => {
            candidate.credits == 1 && candidate.attachment_ids.is_empty()
        }
        "settlement_seller_payout" | "settlement_buyer_refund" => {
            candidate.credits > 0 && candidate.cod == 0 && candidate.attachment_ids.is_empty()
        }
        "settlement_winner_equipment_cod" | "settlement_equipment_handoff" => {
            candidate.credits == 0 && candidate.attachment_ids.len() == 1
        }
        _ => false,
    }
}

/// 通知主题在列表页会按字节截断；多字节截断点会变成 U+FFFD，也可能带省略号。
pub(super) fn notice_subject_matches(expected: &str, list_subject: &str) -> bool {
    if list_subject.is_empty() {
        return false;
    }
    if expected.starts_with(list_subject) {
        return true;
    }
    let without_replacement = list_subject.trim_end_matches('\u{FFFD}');
    if !without_replacement.is_empty() && expected.starts_with(without_replacement) {
        return true;
    }
    without_replacement
        .strip_suffix("\u{2026}")
        .or_else(|| without_replacement.strip_suffix("..."))
        .is_some_and(|prefix| !prefix.is_empty() && expected.starts_with(prefix))
}

pub(super) fn sent_matches(
    candidate: &MoogleMailboxUnprovenSend,
    entry: &MoogleMailboxSentEntry,
) -> bool {
    // Sent 列表标题会被服务端截断到固定长度；只比较可证明的 UUID 前缀，完整标题由详情确认。
    mail_recipient_is_list_projection(&entry.recipient, &candidate.recipient)
        && reference_is_subject_projection(&entry.subject, &candidate.reference)
}

pub(super) fn reference_is_subject_projection(subject: &str, reference: &str) -> bool {
    let Some(start) = subject.find('[') else { return false };
    let tail = &subject[start + 1..];
    let prefix = tail.split(']').next().unwrap_or(tail).trim();
    prefix.len() >= 6 && reference.starts_with(prefix)
}

/// 重排只接受未发生部分领取的原完整资产；集合比较不依赖远端顺序，也不接受重复 ID。
pub(super) fn take_assets_match(
    candidate: &MoogleMailboxUnprovenSend,
    present: &MoogleMailboxInboxAssets,
) -> bool {
    if candidate.attachment_ids.is_empty()
        || candidate.attachment_ids.len() != present.equipment_ids.len()
        || candidate.credits != present.credits
        || candidate.cod != present.cod
    {
        return false;
    }
    let mut expected = candidate.attachment_ids.iter().collect::<std::collections::HashSet<_>>();
    expected.len() == candidate.attachment_ids.len()
        && present.equipment_ids.iter().all(|id| expected.remove(id))
}
