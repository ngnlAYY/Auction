//! MoogleMail Worker 的安全批次校验与入站事实映射。

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
    page.mailbox_key == claim.mailbox_key
        && page.world_id == world_id
        && page.messages.len() <= usize::from(batch_size)
        && page.next_cursor.as_deref().is_none_or(|value| {
            !value.is_empty()
                && value.len() <= 256
                && value == value.trim()
                && !value.chars().any(char::is_control)
        })
        && page.messages.iter().enumerate().all(|(index, message)| {
            !message.external_mail_id.is_empty()
                && message.external_mail_id.len() <= 512
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
                && (index == 0
                    || page.messages[..index]
                        .iter()
                        .all(|previous| previous.external_mail_id != message.external_mail_id))
                && message.business.as_ref().is_none_or(|business| {
                    business.mailbox_key == claim.mailbox_key
                        && business.external_mail_id == message.external_mail_id
                        && business.world_id == message.world_id
                        && business.received_at == message.received_at
                        && business.content_digest == message.content_digest
                        && business.attachments.len()
                            == usize::try_from(message.attachment_count).unwrap_or(usize::MAX)
                        && business.credits == message.credits
                        && business.cod == message.cod
                        && business.returnable == message.returnable
                        && business.claimable == message.claimable
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
    MailPollIntake {
        id,
        external_mail_id: message.external_mail_id,
        world_id: message.world_id,
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
