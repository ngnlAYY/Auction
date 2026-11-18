//! MoogleMail Worker 的安全批次校验与入站事实映射。

use std::collections::HashSet;

use auction_application::mail::{
    MailDisplaySnapshot, MailPollClaim, MailPollIntake, MailProcessingDecision,
};
use auction_time::Timestamp;
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
        && page.next_cursor.as_deref().is_none_or(|value| auction_domain::valid_text(value, 256))
        && if page.messages.is_empty() {
            page.next_cursor.as_deref() == claim.cursor_value.as_deref()
        } else {
            page.next_cursor.as_deref()
                == page.messages.last().map(|message| message.external_mail_id.as_str())
        }
        && page.messages.iter().all(|message| {
            // 任意成功前缀的末封邮件都可能成为 cursor，必须满足持久化游标的长度边界。
            auction_domain::valid_text(&message.external_mail_id, 256)
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

pub(super) fn intake_subject(message: &MailPollFetchedMessage) -> Option<String> {
    let sender = message.sender.as_deref()?;
    let attachments = message.business.as_ref().map_or(&[][..], |mail| mail.attachments.as_slice());
    Some(MailDisplaySnapshot::redact_subject(sender, &message.subject, attachments))
}

pub(super) fn intake_from_message(
    mailbox_key: &str,
    message: MailPollFetchedMessage,
    decision: MailProcessingDecision,
    processed_at: Timestamp,
    subject: Option<String>,
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
        subject,
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

/// 待处理邮件的显示标签；最终决定仍由处理器按发件人与完整主题判定。
fn classify_kind(subject: &str) -> &'static str {
    match subject.trim().to_ascii_lowercase().as_str() {
        "auction" => "equipment",
        "register" => "registration",
        "rebind" => "rebind",
        "rekey" => "rekey",
        // 系统退回的结算邮件以 `[RTS] ` + 原主题送回，标签与原结算邮件一致。
        value if delivery_subject(value.strip_prefix("[rts] ").unwrap_or(value)) => "delivery",
        _ => "unknown",
    }
}

fn delivery_subject(subject: &str) -> bool {
    subject.starts_with("auction equipment delivery")
        || subject.starts_with("auction payment request")
}

#[cfg(test)]
mod tests {
    use auction_time::Timestamp;

    use super::{classify_kind, intake_from_message, intake_subject, pending_decision};
    use crate::worker::mail::poll::MailPollFetchedMessage;

    #[test]
    fn intake_subject_redacts_keys_and_keeps_protocol_labels() {
        let now = Timestamp::from_timestamp(1_800_000_000, 0).unwrap();
        let padded_key_subject = "补充说明：登录 key  中 abcdefghijklmnopqrstuvwxyz!";
        let padded_key = padded_key_subject.strip_prefix("补充说明：登录 key ").unwrap();
        assert_eq!(padded_key.len(), 32);
        assert_eq!(padded_key.trim_start().len(), 31);
        let colon_key_subject = format!("补充说明：登录 key {}:", " ".repeat(31));
        let whitespace_key_subject = format!("补充说明：登录 key {}", " ".repeat(32));
        for (raw, safe, kind) in [
            (colon_key_subject.as_str(), "补充说明：登录 key[REDACTED]", "unknown"),
            (whitespace_key_subject.as_str(), "补充说明：登录 key[REDACTED]", "unknown"),
            ("补充说明：LoGiN KeY =短 值", "补充说明：LoGiN KeY[REDACTED]", "unknown"),
            ("登录 AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "登录 [REDACTED]", "unknown"),
            (
                "https://example.invalid/equip/123/0123abcdef",
                "https://example.invalid/equip/123/[REDACTED]",
                "unknown",
            ),
            (padded_key_subject, "补充说明：登录 key[REDACTED]", "unknown"),
            ("补充说明：LoGiN KeY 短 值", "补充说明：LoGiN KeY[REDACTED]", "unknown"),
            ("补充说明：登录 key", "补充说明：登录 key", "unknown"),
            ("key abc", "key abc", "unknown"),
            ("拍卖问题", "拍卖问题", "unknown"),
            ("rekey", "rekey", "rekey"),
            (
                "[RTS] Auction payment request [5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a]",
                "[RTS] Auction payment request [5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a]",
                "delivery",
            ),
        ] {
            let message = MailPollFetchedMessage {
                external_mail_id: "9001".to_owned(),
                world_id: "main".to_owned(),
                received_at: now,
                sender: Some("Player".to_owned()),
                subject: raw.to_owned(),
                message_display: None,
                content_digest: [1; 32],
                attachment_count: 0,
                credits: 0,
                cod: 0,
                returnable: false,
                claimable: false,
                business: None,
            };
            let decision = pending_decision(&message);
            let safe_subject = intake_subject(&message);
            let intake = intake_from_message("main", message, decision, now, safe_subject);
            assert_eq!(intake.subject.as_deref(), Some(safe));
            assert_eq!(intake.mail_kind, kind);
            assert!(intake.validate());
        }
    }

    #[test]
    fn returned_settlement_mail_keeps_the_delivery_label() {
        let obligation_id = "5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a";
        for subject in [
            format!("Auction payment request [{obligation_id}]"),
            format!("[RTS] Auction payment request [{obligation_id}]"),
            format!("[rts] AUCTION EQUIPMENT DELIVERY [{obligation_id}]"),
        ] {
            assert_eq!(classify_kind(&subject), "delivery", "subject={subject}");
        }
        // 只剥离退信前缀去判结算主题；其他退信与命令主题的标签不变。
        for (subject, kind) in [
            ("[RTS] Auction payment overdue", "unknown"),
            ("[RTS] auction", "unknown"),
            ("[RTS] register", "unknown"),
            ("[RTS][RTS] Auction payment request", "unknown"),
            (" Auction ", "equipment"),
            ("rekey", "rekey"),
        ] {
            assert_eq!(classify_kind(subject), kind, "subject={subject}");
        }
    }
}
