//! MoogleMail Worker 的安全批次校验与入站事实映射。

use std::collections::HashSet;

use auction_application::{MailPollClaim, MailPollIntake, MailProcessingDecision};
use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use super::{MailPollFetchedMessage, MailPollPage};

pub(super) fn valid_page(
    page: &MailPollPage,
    claim: &MailPollClaim,
    world_id: &str,
    batch_size: u16,
) -> bool {
    // 只借用 ID，不复制正文或字符串；批次去重不能对每封重新扫描整个已见前缀。
    let mut seen_mail_ids = HashSet::new();
    page.mailbox_key == claim.mailbox_key
        && page.world_id == world_id
        && page.messages.len() <= usize::from(batch_size)
        && page.next_cursor.as_deref().is_none_or(|value| {
            !value.is_empty()
                && value.len() <= 256
                && value == value.trim()
                && !value.chars().any(char::is_control)
        })
        && if page.messages.is_empty() {
            page.next_cursor.as_deref() == claim.cursor_value.as_deref()
        } else {
            page.next_cursor.as_deref()
                == page.messages.last().map(|message| message.external_mail_id.as_str())
        }
        && page.messages.iter().all(|message| {
            !message.external_mail_id.is_empty()
                // 任意成功前缀的末封邮件都可能成为 cursor，必须满足持久化游标的长度边界。
                && message.external_mail_id.len() <= 256
                && message.external_mail_id == message.external_mail_id.trim()
                && !message.external_mail_id.chars().any(char::is_control)
                && message.world_id == world_id
                && message.subject.len() <= 512
                && !message.subject.chars().any(char::is_control)
                && message.received_at.timestamp_nanos_opt().is_some()
                && message.content_digest != [0; 32]
                && message.credits >= 0
                && message.cod >= 0
                && message.attachment_count <= 1000
                && seen_mail_ids.insert(message.external_mail_id.as_str())
                && message.business.as_ref().is_none_or(|business| {
                    business.mailbox_key == claim.mailbox_key
                        && business.external_mail_id == message.external_mail_id
                        && business.world_id == message.world_id
                        && business.received_at == message.received_at
                        && business.content_digest == message.content_digest
                        && usize::try_from(message.attachment_count).is_ok_and(|attachment_count| {
                            business.attachments.len() == attachment_count
                        })
                        && business.credits == message.credits
                        && business.cod == message.cod
                        && business.returnable == message.returnable
                        && business.claimable == message.claimable
                        && message.sender.as_deref().is_some_and(|sender| sender == business.sender)
                        && message.subject == business.subject
                        && message.message_display.as_ref().is_none_or(|display| {
                            message.sender.is_some()
                                && display.validate()
                                && display.credits == business.credits
                                && display.cod == business.cod
                                && display.attachment_ids == business.sorted_attachment_ids()
                        })
                        && business.validate().is_ok()
                })
        })
}

pub(super) fn intake_from_message(
    mailbox_key: &str,
    message: MailPollFetchedMessage,
    decision: MailProcessingDecision,
    processed_at: DateTime<Utc>,
) -> MailPollIntake {
    let mail_kind = decision.mail_kind;
    let id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!(
            "auction/inbound-mail-v1/{mailbox_key}/{}/{}",
            message.world_id, message.external_mail_id
        )
        .as_bytes(),
    );
    let processed_at = processed_at.max(message.received_at);
    let headers_present = message.sender.is_some();
    // 非权威诊断损坏时丢弃旁路投影，不能让日志数据阻断原业务提交或游标推进。
    let equipment_diagnostic = decision.equipment_diagnostic.filter(|diagnostic| {
        let valid =
            mail_kind == "equipment" && decision.status == "rejected" && diagnostic.validate();
        if !valid {
            tracing::warn!(
                world_id = %message.world_id,
                external_mail_id = %message.external_mail_id,
                "MoogleMail equipment diagnostic projection discarded"
            );
        }
        valid
    });
    MailPollIntake {
        id,
        external_mail_id: message.external_mail_id,
        world_id: message.world_id,
        sender: message.sender.filter(|_| headers_present),
        subject: headers_present.then_some(message.subject),
        message_display: headers_present.then_some(message.message_display).flatten(),
        equipment_diagnostic,
        mail_kind,
        status: decision.status,
        content_digest: message.content_digest,
        received_at: message.received_at,
        decision_code: decision.decision_code,
        decision_metadata: decision.metadata,
        processed_at,
    }
}

pub(super) fn pending_decision(message: &MailPollFetchedMessage) -> MailProcessingDecision {
    MailProcessingDecision {
        mail_kind: classify_kind(&message.subject).to_owned(),
        status: "accepted".to_owned(),
        decision_code: "parsed_pending_processing".to_owned(),
        equipment_diagnostic: None,
        metadata: json!({
            "processor_state": "pending",
            "attachment_count": message.attachment_count,
            "credits": message.credits,
            "cod": message.cod,
            "returnable": message.returnable,
            "claimable": message.claimable,
        }),
    }
}

fn classify_kind(subject: &str) -> &'static str {
    match subject.trim().to_ascii_lowercase().as_str() {
        "auction" => "equipment",
        "register" => "registration",
        "rebind" => "rebind",
        value
            if value.starts_with("auction equipment delivery")
                || value.starts_with("auction payment request") =>
        {
            "delivery"
        }
        _ => "unknown",
    }
}
