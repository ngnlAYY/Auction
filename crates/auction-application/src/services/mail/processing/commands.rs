//! MoogleMail 命令校验、幂等结果、摘要和安全决定辅助函数。

use std::collections::HashSet;

use auction_domain::{CommandId, MAX_BID_AMOUNT, valid_text};
use auction_time::Timestamp;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::core::{RequestHash, StoredCommand, decimal_identity};
use crate::ports::mail::{
    MailAuctionIntakeCommand, MailAuctionIntakeResult, MailDeliveryCommand, MailDeliveryOperation,
    MailDeliveryOrigin, MailLoginKey, MailMessage, MailNotificationTemplate,
    MailProcessingDecision, MailProcessingError, MailUserCommandKind,
};

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
            || !(1..=MAX_BID_AMOUNT).contains(&item.start_price)
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
        received_at: Timestamp,
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
        collected_at: Timestamp,
        start_price: i64,
    }
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
                collected_at: item.collected_at,
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

pub(super) struct DeliveryRequestHashInput<'a> {
    pub command_id: CommandId,
    pub message: &'a MailMessage,
    pub operation: MailDeliveryOperation,
    pub template: Option<MailNotificationTemplate>,
    pub result_code: Option<&'a str>,
    pub user_key_user_id: Option<Uuid>,
    pub attachment_ids: &'a [String],
    pub notification_body: Option<&'a str>,
}

#[derive(Serialize)]
struct DeliveryRequest<'a> {
    operation: &'static str,
    world_id: &'a str,
    external_mail_id: &'a str,
    recipient: &'a str,
    template: Option<&'static str>,
    result_code: Option<&'a str>,
    user_key_user_id: Option<Uuid>,
    attachment_ids: &'a [String],
    // 保留无冻结正文的已发布摘要编码，新增正文只在有值时参与摘要。
    #[serde(skip_serializing_if = "Option::is_none")]
    notification_body: Option<&'a str>,
}

impl DeliveryRequest<'_> {
    fn hash(&self, command_id: CommandId) -> Result<RequestHash, serde_json::Error> {
        RequestHash::from_request("mail_delivery", command_id.into(), self)
    }
}

pub(super) fn delivery_request_hash(
    input: DeliveryRequestHashInput<'_>,
) -> Result<RequestHash, ApplicationError> {
    let request = DeliveryRequest {
        operation: input.operation.as_str(),
        world_id: &input.message.world_id,
        external_mail_id: &input.message.external_mail_id,
        recipient: &input.message.sender,
        template: input.template.map(MailNotificationTemplate::as_str),
        result_code: input.result_code,
        user_key_user_id: input.user_key_user_id,
        attachment_ids: input.attachment_ids,
        notification_body: input.notification_body,
    };
    Ok(request.hash(input.command_id)?)
}

impl MailDeliveryCommand {
    /// 投稿回执原先只在载荷冻结正文。仅在同一投递行的完整载荷已精确匹配后，
    /// 允许带正文的新摘要重放使用旧摘要的回执。
    pub fn matches_legacy_submission_receipt_digest(&self, stored_digest: &[u8]) -> bool {
        if self.origin != MailDeliveryOrigin::MailIntake
            || self.operation != MailDeliveryOperation::Send
            || self.template != Some(MailNotificationTemplate::AuctionSubmissionReceipt)
            || self.notification_body.is_none()
        {
            return false;
        }
        let mut request = DeliveryRequest {
            operation: self.operation.as_str(),
            world_id: &self.world_id,
            external_mail_id: &self.external_mail_id,
            recipient: &self.recipient,
            template: self.template.map(MailNotificationTemplate::as_str),
            result_code: self.result_code.as_deref(),
            user_key_user_id: self.user_key_user_id,
            attachment_ids: &self.attachment_ids,
            notification_body: None,
        };
        if !request.hash(self.command_id).is_ok_and(|legacy| stored_digest == legacy.as_bytes()) {
            return false;
        }
        request.notification_body = self.notification_body.as_deref();
        request.hash(self.command_id).is_ok_and(|current| current == self.request_hash)
    }
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
