//! MoogleMail 命令校验、幂等结果、摘要和安全决定辅助函数。

use std::collections::HashSet;

use auction_domain::{CommandId, valid_text};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::core::{StoredCommand, decimal_identity};
use crate::ports::mail::{
    MailAuctionIntakeCommand, MailAuctionIntakeResult, MailDeliveryOperation, MailLoginKey,
    MailMessage, MailNotificationTemplate, MailProcessingDecision, MailProcessingError,
    MailUserCommandKind,
};

/// MoogleMail 系统退信的发件人；与集成层退信回执判定一致，精确比较、不忽略大小写。
const MOOGLEMAIL_SYSTEM_SENDER: &str = "MoogleMail";

/// 系统退信加在原主题前的前缀，已按路由口径转为 ASCII 小写。
const RETURNED_SUBJECT_PREFIX: &str = "[rts] ";

pub(super) fn validate_command(command: &MailAuctionIntakeCommand) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.aggregate_id.is_nil()
        || command.request_hash.as_bytes() == [0; 32]
        || !matches!(command.world_id.as_str(), "main" | "isekai")
        || !valid_text(&command.mailbox_key, 256)
        || !valid_text(&command.external_mail_id, 512)
        || !valid_text(&command.seller, 256)
        || command.received_at.timestamp_nanos_opt().is_none()
        || command.now < command.received_at
        || command.items.is_empty()
        || command.items.len() > 1000
        || command.content_digest == [0; 32]
    {
        return Err(MailProcessingError::InvalidAuctionCommand.into());
    }
    let mut equipment_ids = HashSet::with_capacity(command.items.len());
    for item in &command.items {
        if !decimal_identity(&item.external_equipment_id)
            || !valid_text(&item.name, 512)
            || !valid_text(&item.category, 128)
            || item.level.is_some_and(|level| !(0..=100_000).contains(&level))
            || item.source_fingerprint == [0; 32]
            || !item.attributes.is_object()
            || item.collected_at < command.received_at
            || item.collected_at > command.now
            || item.start_price <= 0
            || !equipment_ids.insert(item.external_equipment_id.as_str())
        {
            return Err(MailProcessingError::InvalidAuctionCommand.into());
        }
    }
    Ok(())
}

pub(super) fn replay_result(
    stored: StoredCommand,
    aggregate_id: Uuid,
    request_hash: crate::ports::core::RequestHash,
) -> Result<MailAuctionIntakeResult, ApplicationError> {
    if !stored.matches_request("mail_auction_intake", aggregate_id, request_hash)
        || stored.result_code != "accepted"
    {
        return Err(MailProcessingError::CommandConflict.into());
    }
    let result: MailAuctionIntakeResult =
        serde_json::from_slice(&stored.result_payload).map_err(|error| {
            record_error(
                DiagnosticLevel::Warn,
                error.diagnostic_kind(),
                "mail_intake",
                "decode_replay_result",
                DiagnosticContext::default(),
            );
            MailProcessingError::StoredResultCorrupt
        })?;
    result.validate()?;
    Ok(result)
}

pub(super) fn decision(
    mail_kind: &str,
    status: &str,
    decision_code: &str,
    metadata: Value,
) -> Result<MailProcessingDecision, ApplicationError> {
    let result = MailProcessingDecision {
        mail_kind: mail_kind.to_owned(),
        status: status.to_owned(),
        decision_code: decision_code.to_owned(),
        metadata,
        equipment_diagnostic: None,
    };
    result.validate()?;
    Ok(result)
}

pub(super) fn has_user_assets(message: &MailMessage) -> bool {
    !message.attachments.is_empty() || message.credits > 0 || message.cod > 0
}

pub(super) fn extract_login_key(body: &str) -> Result<MailLoginKey, &'static str> {
    const MISSING: &str = "missing_key";
    const INVALID: &str = "invalid_key";
    let mut selected = None;
    for token in body.split_whitespace() {
        if token.len() != 43 {
            continue;
        }
        if selected.is_some() {
            return Err(INVALID);
        }
        selected = Some(token);
    }
    let Some(encoded) = selected else {
        return Err(MISSING);
    };
    MailLoginKey::from_encoded(encoded).map_err(|_| INVALID)
}

pub(super) fn stable_command_id(message: &MailMessage, operation: &str) -> CommandId {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!(
            "auction/mooglemail/{operation}/{}/{}/{}",
            message.mailbox_key, message.world_id, message.external_mail_id
        )
        .as_bytes(),
    )
    .into()
}

pub(super) fn auction_request_hash(
    message: &MailMessage,
    aggregate_id: Uuid,
    items: &[crate::ports::mail::MailAuctionItem],
) -> Result<crate::ports::core::RequestHash, ApplicationError> {
    #[derive(Serialize)]
    struct Request<'a> {
        world_id: &'a str,
        mailbox_key: &'a str,
        external_mail_id: &'a str,
        seller: &'a str,
        received_at: DateTime<Utc>,
        content_digest: [u8; 32],
        items: Items<'a>,
    }
    #[derive(Serialize)]
    struct Item<'a> {
        external_equipment_id: &'a str,
        name: &'a str,
        category: &'a str,
        level: Option<i32>,
        attributes: &'a Value,
        source_fingerprint: [u8; 32],
        start_price: i64,
    }
    // 保留原数组顺序和 Item 字段编码，借用逐项投影，不为摘要再分配整封附件列表。
    struct Items<'a>(&'a [crate::ports::mail::MailAuctionItem]);

    impl Serialize for Items<'_> {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            serializer.collect_seq(self.0.iter().map(|item| Item {
                external_equipment_id: &item.external_equipment_id,
                name: &item.name,
                category: &item.category,
                level: item.level,
                attributes: &item.attributes,
                source_fingerprint: item.source_fingerprint,
                start_price: item.start_price,
            }))
        }
    }
    Ok(crate::ports::core::RequestHash::from_request(
        "mail_auction_intake",
        aggregate_id,
        &Request {
            world_id: &message.world_id,
            mailbox_key: &message.mailbox_key,
            external_mail_id: &message.external_mail_id,
            seller: &message.sender,
            received_at: message.received_at,
            content_digest: message.content_digest,
            items: Items(items),
        },
    )?)
}

pub(super) fn delivery_request_hash(
    command_id: CommandId,
    message: &MailMessage,
    operation: MailDeliveryOperation,
    template: Option<MailNotificationTemplate>,
    result_code: Option<&str>,
    user_key_user_id: Option<Uuid>,
    attachment_ids: &[String],
) -> Result<crate::ports::core::RequestHash, ApplicationError> {
    #[derive(Serialize)]
    struct Request<'a> {
        operation: &'static str,
        world_id: &'a str,
        external_mail_id: &'a str,
        recipient: &'a str,
        template: Option<&'static str>,
        result_code: Option<&'a str>,
        user_key_user_id: Option<Uuid>,
        attachment_ids: &'a [String],
    }
    Ok(crate::ports::core::RequestHash::from_request(
        "mail_delivery",
        command_id.into(),
        &Request {
            operation: operation.as_str(),
            world_id: &message.world_id,
            external_mail_id: &message.external_mail_id,
            recipient: &message.sender,
            template: template.map(MailNotificationTemplate::as_str),
            result_code,
            user_key_user_id,
            attachment_ids,
        },
    )?)
}

pub(super) fn user_request_hash(
    message: &MailMessage,
    command_id: CommandId,
    kind: MailUserCommandKind,
) -> Result<crate::ports::core::RequestHash, ApplicationError> {
    #[derive(Serialize)]
    struct Request<'a> {
        operation: &'static str,
        world_id: &'a str,
        mailbox_key: &'a str,
        external_mail_id: &'a str,
        sender: &'a str,
        content_digest: [u8; 32],
    }
    let operation = match kind {
        MailUserCommandKind::Register => "mail-user-register-v1",
        MailUserCommandKind::Rebind => "mail-user-rebind-v1",
        MailUserCommandKind::Rekey => "mail-user-rekey-v1",
    };
    Ok(crate::ports::core::RequestHash::from_request(
        operation,
        command_id.into(),
        &Request {
            operation,
            world_id: &message.world_id,
            mailbox_key: &message.mailbox_key,
            external_mail_id: &message.external_mail_id,
            sender: &message.sender,
            content_digest: message.content_digest,
        },
    )?)
}

pub(super) fn settlement_return_obligation_id(subject: &str) -> Option<Uuid> {
    let (base, encoded) = subject.rsplit_once(" [")?;
    if !subject.ends_with(']')
        || !(base == "auction equipment delivery" || base == "auction payment request")
    {
        return None;
    }
    let encoded = encoded.strip_suffix(']')?;
    let obligation_id = Uuid::parse_str(encoded).ok()?;
    (!obligation_id.is_nil() && obligation_id.to_string() == encoded).then_some(obligation_id)
}

/// 识别买方退回的结算邮件：发件人恰为系统 `MoogleMail`，主题为 `[RTS] ` + 原 canonical 结算主题
/// （`subject` 已按路由口径转为 ASCII 小写）。玩家不能使用系统发件人，自己发来的 `[RTS] …` 主题
/// 不会成为退件证据；证据仍由 recovery Worker 在 Serializable 事务内与义务严格比对。
pub(super) fn system_returned_settlement_obligation_id(
    sender: &str,
    subject: &str,
) -> Option<Uuid> {
    if sender != MOOGLEMAIL_SYSTEM_SENDER {
        return None;
    }
    subject.strip_prefix(RETURNED_SUBJECT_PREFIX).and_then(settlement_return_obligation_id)
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{settlement_return_obligation_id, system_returned_settlement_obligation_id};

    const OBLIGATION_ID: &str = "5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a";

    #[test]
    fn system_returned_settlement_subjects_resolve_their_obligation() {
        let obligation_id = Uuid::parse_str(OBLIGATION_ID).expect("固定义务标识");
        for base in ["auction payment request", "auction equipment delivery"] {
            let returned = format!("[rts] {base} [{OBLIGATION_ID}]");
            assert_eq!(
                system_returned_settlement_obligation_id("MoogleMail", &returned),
                Some(obligation_id)
            );
            // 发件人精确比较：玩家或大小写不同的名字都不是系统退信。
            for sender in ["Player", "mooglemail", "MOOGLEMAIL"] {
                assert_eq!(system_returned_settlement_obligation_id(sender, &returned), None);
            }
            // 带前缀的主题不命中无前缀的兼容路径，无前缀主题也不需要系统发件人。
            assert_eq!(settlement_return_obligation_id(&returned), None);
            let canonical = format!("{base} [{OBLIGATION_ID}]");
            assert_eq!(settlement_return_obligation_id(&canonical), Some(obligation_id));
        }
    }

    #[test]
    fn other_returned_subjects_are_not_settlement_returns() {
        let doubled = format!("[rts] [rts] auction payment request [{OBLIGATION_ID}]");
        let unspaced = format!("[rts]auction payment request [{OBLIGATION_ID}]");
        let seller_payout = format!("[rts] auction seller payout [{OBLIGATION_ID}]");
        for subject in [
            "[rts] auction payment overdue",
            doubled.as_str(),
            unspaced.as_str(),
            seller_payout.as_str(),
            "[rts] auction payment request [00000000-0000-0000-0000-000000000000]",
        ] {
            assert_eq!(
                system_returned_settlement_obligation_id("MoogleMail", subject),
                None,
                "subject={subject}"
            );
        }
    }
}
