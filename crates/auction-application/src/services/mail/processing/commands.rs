//! MoogleMail 命令校验、幂等结果、摘要和安全决定辅助函数。

use std::collections::HashSet;

use super::pricing::{decimal_identity, valid_text};
use super::*;

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
    request_hash: crate::RequestHash,
) -> Result<MailAuctionIntakeResult, ApplicationError> {
    if !stored.matches_request("mail_auction_intake", aggregate_id, request_hash)
        || stored.result_code != "accepted"
    {
        return Err(MailProcessingError::CommandConflict.into());
    }
    let result: MailAuctionIntakeResult = serde_json::from_slice(&stored.result_payload)
        .map_err(|_| MailProcessingError::StoredResultCorrupt)?;
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
    items: &[crate::MailAuctionItem],
) -> Result<crate::RequestHash, ApplicationError> {
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
    struct Items<'a>(&'a [crate::MailAuctionItem]);

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
    Ok(crate::RequestHash::from_request(
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
) -> Result<crate::RequestHash, ApplicationError> {
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
    Ok(crate::RequestHash::from_request(
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
) -> Result<crate::RequestHash, ApplicationError> {
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
    };
    Ok(crate::RequestHash::from_request(
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
